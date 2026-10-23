//! Compiled models (`.vmdl_c`): meshes, vertex/index buffers and draw calls, decoded to plain geometry.
//!
//! # How a CS2 model is put together
//!
//! ```text
//! .vmdl_c
//!  ├─ DATA  (KV3)  mesh groups ("bodygroups"), LOD masks, skeleton, material groups
//!  ├─ CTRL  (KV3)  embedded_meshes[]: for each mesh, which blocks hold its data and its buffers
//!  ├─ MDAT  (KV3)  one per mesh: draw calls (material, which buffers, index range)
//!  ├─ MVTX         one per vertex buffer: raw or meshopt-compressed vertices
//!  └─ MIDX         one per index buffer: raw or meshopt-compressed triangle indices
//! ```
//!
//! Older files put all buffers of a mesh in one binary `VBIB`/`MBUF` block instead of `MVTX`/`MIDX`; both layouts
//! are read. Block numbers in `CTRL` are plain indices into the resource's block table.
//!
//! Each vertex buffer declares its **input layout**: a list of attributes (`POSITION`, `NORMAL`, `TEXCOORD`, …),
//! each with a DXGI format and a byte offset inside the vertex. [`Geometry`] decodes the ones OFAST needs,
//! including CS2's packed normal + tangent formats.
//!
//! Details: `docs/formats/model.md`. Knowledge from ValveResourceFormat (MIT); independent implementation.

mod attributes;
mod skeleton;

use crate::bytes::{ByteReader, UnexpectedEof};
use crate::kv3::Value;
use crate::meshopt::{self, MeshoptError};
use crate::resource::{BlockKind, Resource, ResourceError};

pub use attributes::{Geometry, half_to_f32};
pub use skeleton::{
    Affine, Bone, Skeleton, affine, compose, invert_rigid, normalize_quat, quat_matrix, quat_mul,
};

/// DXGI format numbers used by CS2 vertex layouts.
#[allow(missing_docs)]
pub mod dxgi {
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32_FLOAT: u32 = 6;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UINT: u32 = 12;
    pub const R32G32_FLOAT: u32 = 16;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UINT: u32 = 30;
    pub const R16G16_FLOAT: u32 = 34;
    pub const R16G16_UNORM: u32 = 35;
    pub const R16G16_SNORM: u32 = 37;
    pub const R32_UINT: u32 = 42;
}

/// Everything that can go wrong while reading a model.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The container or a KV3 block could not be read.
    #[error(transparent)]
    Resource(#[from] ResourceError),
    /// The data ended in the middle of a structure.
    #[error(transparent)]
    UnexpectedEof(#[from] UnexpectedEof),
    /// A buffer failed to decompress.
    #[error("mesh `{mesh}`: {source}")]
    Meshopt {
        /// Mesh name.
        mesh: String,
        /// Decoder error.
        source: MeshoptError,
    },
    /// A ZSTD-compressed buffer failed to decompress.
    #[error("mesh `{mesh}`: ZSTD: {message}")]
    Zstd {
        /// Mesh name.
        mesh: String,
        /// Decoder message.
        message: String,
    },
    /// A required KV3 key is missing or has the wrong type.
    #[error("model is missing `{0}`")]
    MissingKey(&'static str),
    /// A block index points outside the block table.
    #[error("block index {0} does not exist")]
    BadBlockIndex(i64),
    /// A draw call references a buffer that does not exist.
    #[error("mesh `{mesh}` draw call references missing {kind} buffer {index}")]
    BadBufferReference {
        /// Mesh name.
        mesh: String,
        /// "vertex" or "index".
        kind: &'static str,
        /// The index used.
        index: usize,
    },
}

/// One attribute of a vertex layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// Semantic name, upper-case (`POSITION`, `NORMAL`, `TEXCOORD`, `BLENDINDICES`, …).
    pub semantic: String,
    /// Semantic index (e.g. 1 for the second `TEXCOORD`).
    pub semantic_index: u32,
    /// DXGI format number (see [`dxgi`]).
    pub format: u32,
    /// Byte offset inside one vertex.
    pub offset: usize,
}

/// A decoded vertex buffer.
#[derive(Debug, Clone)]
pub struct VertexBuffer {
    /// Number of vertices.
    pub count: usize,
    /// Bytes per vertex.
    pub stride: usize,
    /// The vertex layout.
    pub attributes: Vec<Attribute>,
    /// `count × stride` bytes.
    pub data: Vec<u8>,
}

impl VertexBuffer {
    /// First attribute with this semantic and index.
    pub fn attribute(&self, semantic: &str, index: u32) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|a| a.semantic == semantic && a.semantic_index == index)
    }
}

/// One draw call: a range of triangles drawn with one material.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawCall {
    /// Material path, e.g. `weapons/models/ak47/materials/weapon_rif_ak47.vmat` (no `_c`).
    pub material: String,
    /// Indices into the mesh's vertex buffers: the streams read together (e.g. positions in one, UVs and
    /// normals in another).
    pub vertex_buffers: Vec<usize>,
    /// Index into the mesh's index buffers.
    pub index_buffer: usize,
    /// Added to every index.
    pub base_vertex: u32,
    /// First index used.
    pub start_index: u32,
    /// Number of indices used (3 per triangle).
    pub index_count: u32,
    /// Normals and tangents are stored in a packed format.
    pub compressed_normal_tangent: bool,
    /// The draw call's own tint (`m_vTintColor`), **linear** RGB, white when absent. Hammer stores the colour picked
    /// for a mesh or overlay face here (`VertexPaintUIPickColor` materials), not in the material: de_nuke's yellow
    /// road paint is a white `hr_nuke_parkingstripe_single` drawn with (0.851, 0.459, 0.046) = sRGB (237, 179, 63)
    /// through gamma 2.2. It multiplies the placement's tint ([`DrawCall::placement_tint`]).
    pub tint: [f32; 3],
    /// The draw call's opacity (`m_flAlpha`), 1 when absent.
    pub alpha: f32,
}

impl DrawCall {
    /// True when the draw call recolours nothing: white tint, full opacity.
    pub fn is_untinted(&self) -> bool {
        self.tint == [1.0; 3] && self.alpha == 1.0
    }

    /// The tint this draw call is drawn with when its model is placed with `placement` (sRGB RGBA 0–1: a scene
    /// object's `m_vTintColor`, an aggregate fragment's, an entity's `rendercolor`).
    ///
    /// CS2 hands its shaders one 8-bit **sRGB** tint per instance (decoded to linear in the vertex shader, e.g.
    /// `csgo_complex`'s), so the draw call's linear tint is encoded back to sRGB and the two multiply there;
    /// gamma 2.2 is the encoding the stored values round-trip with (de_nuke's road paint: 237.0, 179.0, 63.0). A
    /// white draw call returns `placement` unchanged.
    pub fn placement_tint(&self, placement: [f32; 4]) -> [f32; 4] {
        if self.is_untinted() {
            return placement;
        }
        let encode = |linear: f32| linear.clamp(0.0, 1.0).powf(1.0 / 2.2);
        [
            placement[0] * encode(self.tint[0]),
            placement[1] * encode(self.tint[1]),
            placement[2] * encode(self.tint[2]),
            placement[3] * self.alpha.clamp(0.0, 1.0),
        ]
    }
}

/// One mesh of a model.
#[derive(Debug, Clone)]
pub struct Mesh {
    /// Mesh name from the compiler (`body_hd`, …).
    pub name: String,
    /// Position in the model's mesh list (used by group and LOD masks).
    pub mesh_index: usize,
    /// Decoded vertex buffers.
    pub vertex_buffers: Vec<VertexBuffer>,
    /// Decoded index buffers.
    pub index_buffers: Vec<Vec<u32>>,
    /// Draw calls.
    pub draw_calls: Vec<DrawCall>,
    /// Bone influences per vertex the compiler kept (`m_skeleton.m_nBoneWeightCount`, 1 – 4 on agents; 0 when the
    /// mesh is not skinned). With 1, CS2 stores no weights: every vertex follows its first bone index fully.
    pub bone_weight_count: u32,
    /// Named points on the skeleton (`m_attachments`): where a player's weapons sit, a weapon's muzzle, …
    pub attachments: Vec<Attachment>,
}

/// A named point that follows bones: `weapon` on an agent's `wpn` bone, `pistol` on its right thigh,
/// `weapon_holster_center` on a weapon, `muzzle_flash`, …
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// Attachment name.
    pub name: String,
    /// The bones it follows, each with its offset from that bone (1 on every CS2 player and weapon attachment).
    pub influences: Vec<Influence>,
    /// Follows the bones' positions only (`m_bIgnoreRotation`).
    pub ignore_rotation: bool,
}

/// One bone an [`Attachment`] follows.
#[derive(Debug, Clone, PartialEq)]
pub struct Influence {
    /// Bone name, as the model's skeleton spells it or in lower case (`hand_r` for `hand_R`).
    pub bone: String,
    /// Translation from the bone, inches, bone frame.
    pub offset: [f32; 3],
    /// Rotation from the bone, `(x, y, z, w)`.
    pub rotation: [f32; 4],
    /// Share of this bone (the weights sum to 1).
    pub weight: f32,
}

impl Attachment {
    /// The attachment's frame relative to its bone and that bone's index in `skeleton`, for the single-bone
    /// attachments CS2 players and weapons use. `None` when it follows several bones, a bone the skeleton lacks, or
    /// only the bone's position ([`Self::ignore_rotation`]: no fixed offset from the bone describes that; none of
    /// the player and weapon attachments OFAST uses does it, `tests/cs2_model.rs` checks).
    pub fn on_bone(&self, skeleton: &Skeleton) -> Option<(usize, Affine)> {
        let [influence] = self.influences.as_slice() else {
            return None;
        };
        if self.ignore_rotation {
            return None;
        }
        Some((
            skeleton.find(&influence.bone)?,
            affine(influence.rotation, influence.offset),
        ))
    }
}

impl Mesh {
    /// Decodes the geometry of one draw call (positions, normals, tangents, UVs, triangle indices).
    pub fn geometry(&self, draw: &DrawCall) -> Result<Geometry, ModelError> {
        // A draw call without explicit streams reads the first vertex buffer.
        let handles: &[usize] = if draw.vertex_buffers.is_empty() {
            &[0]
        } else {
            &draw.vertex_buffers
        };
        let mut streams = Vec::with_capacity(handles.len());
        for &handle in handles {
            streams.push(self.vertex_buffers.get(handle).ok_or_else(|| {
                ModelError::BadBufferReference {
                    mesh: self.name.clone(),
                    kind: "vertex",
                    index: handle,
                }
            })?);
        }
        let ib = self.index_buffers.get(draw.index_buffer).ok_or_else(|| {
            ModelError::BadBufferReference {
                mesh: self.name.clone(),
                kind: "index",
                index: draw.index_buffer,
            }
        })?;
        Ok(Geometry::decode(&streams, ib, draw))
    }
}

/// A model: its meshes plus the information needed to pick the default look.
#[derive(Debug, Clone)]
pub struct Model {
    /// All embedded meshes, ordered by mesh index.
    pub meshes: Vec<Mesh>,
    /// Mesh group (bodygroup choice) names.
    pub mesh_groups: Vec<String>,
    /// For each mesh index: the mesh groups it belongs to (bit per group).
    pub mesh_group_masks: Vec<u64>,
    /// The groups shown by default.
    pub default_mesh_group_mask: u64,
    /// For each mesh index: the LODs it belongs to (bit per LOD).
    pub lod_masks: Vec<u64>,
    /// Paths of external `.vmesh` files (not embedded) — empty strings for embedded meshes.
    pub external_meshes: Vec<String>,
    /// The bones (empty or a single `dummy` bone on unskinned models).
    pub skeleton: Skeleton,
    /// `m_remappingTable`: skeleton bone indices, one slice per mesh (see [`Model::bone_remap`]).
    pub remapping_table: Vec<u32>,
    /// `m_remappingTableStarts`: where each mesh's slice of the remapping table begins, by mesh index.
    pub remapping_starts: Vec<usize>,
    /// `m_modelInfo.m_keyValueText`: the model's keyvalues as KV3 text (`format:vmdlkeys4`), empty when it has
    /// none. Agents keep their procedural-bone constraints there ([`crate::anim::BoneConstraints`]).
    pub key_values_text: String,
}

impl Model {
    /// Reads every embedded mesh of a compiled model.
    pub fn parse(resource: &Resource<'_>) -> Result<Self, ModelError> {
        let data = resource.kv3(BlockKind::DATA)?.root;
        // Models with only external meshes (or none, e.g. physics-only gibs) have no CTRL block.
        let ctrl_kind = BlockKind(*b"CTRL");
        let ctrl = match resource.block(ctrl_kind) {
            Some(_) => resource.kv3(ctrl_kind)?.root,
            None => Value::Null,
        };

        let mut model = Self {
            meshes: Vec::new(),
            mesh_groups: strings(data.get("m_meshGroups")),
            mesh_group_masks: integers(data.get("m_refMeshGroupMasks")),
            default_mesh_group_mask: data
                .get("m_nDefaultMeshGroupMask")
                .and_then(Value::as_i64)
                .unwrap_or(1) as u64,
            lod_masks: integers(data.get("m_refLODGroupMasks")),
            external_meshes: strings(data.get("m_refMeshes")),
            skeleton: Skeleton::parse(&data),
            remapping_table: integers(data.get("m_remappingTable"))
                .into_iter()
                .map(|v| v as u32)
                .collect(),
            remapping_starts: integers(data.get("m_remappingTableStarts"))
                .into_iter()
                .map(|v| v as usize)
                .collect(),
            key_values_text: data
                .get("m_modelInfo")
                .and_then(|info| info.get("m_keyValueText"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        };

        let embedded = ctrl
            .get("embedded_meshes")
            .and_then(Value::as_array)
            .unwrap_or_default();
        for entry in embedded {
            model.meshes.push(read_embedded_mesh(resource, entry)?);
        }
        model.meshes.sort_by_key(|m| m.mesh_index);
        Ok(model)
    }

    /// The remapping slice of mesh `mesh_index`: a vertex's bone index `i` (`BLENDINDICES`) is skeleton bone
    /// `bone_remap(mesh)[i]`. A slice runs from its start to the next mesh's start; it begins with the mesh's own
    /// render bones (`MDAT m_skeleton.m_bones`, same order) and on agents ends with hitbox bones vertices never use.
    pub fn bone_remap(&self, mesh_index: usize) -> &[u32] {
        let Some(&start) = self.remapping_starts.get(mesh_index) else {
            return &[];
        };
        let end = self
            .remapping_starts
            .get(mesh_index + 1)
            .copied()
            .unwrap_or(self.remapping_table.len())
            .min(self.remapping_table.len());
        self.remapping_table.get(start..end).unwrap_or_default()
    }

    /// Meshes shown by default: in the default mesh groups and in the most detailed LOD.
    pub fn default_meshes(&self) -> impl Iterator<Item = &Mesh> {
        self.meshes_in_groups(self.default_mesh_group_mask)
    }

    /// The attachment named `name` (in the first mesh that has it: every LOD repeats them).
    pub fn attachment(&self, name: &str) -> Option<&Attachment> {
        self.meshes
            .iter()
            .flat_map(|m| &m.attachments)
            .find(|a| a.name == name)
    }

    /// Meshes shown with the mesh groups `mask` (bit per [`Self::mesh_groups`] entry, as an entity's
    /// `m_MeshGroupMask` names them), in the most detailed LOD.
    pub fn meshes_in_groups(&self, mask: u64) -> impl Iterator<Item = &Mesh> {
        let lowest_lod = self
            .lod_masks
            .iter()
            .fold(0u64, |acc, m| acc | m)
            .trailing_zeros();
        self.meshes.iter().filter(move |mesh| {
            let in_group = self
                .mesh_group_masks
                .get(mesh.mesh_index)
                .is_none_or(|groups| groups & mask != 0);
            let in_lod = self
                .lod_masks
                .get(mesh.mesh_index)
                .is_none_or(|mask| lowest_lod >= 64 || mask & (1 << lowest_lod) != 0);
            in_group && in_lod
        })
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn integers(value: Option<&Value>) -> Vec<u64> {
    value
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|v| match v.unflagged() {
            Value::UInt(u) => *u,
            other => other.as_i64().unwrap_or(0) as u64,
        })
        .collect()
}

/// Looks up the first of several key spellings (the format renamed keys over time).
fn field<'v>(value: &'v Value, keys: &[&str]) -> Option<&'v Value> {
    keys.iter().find_map(|k| value.get(k))
}

fn int(value: &Value, keys: &[&str]) -> Option<i64> {
    field(value, keys).and_then(Value::as_i64)
}

fn flag(value: &Value, keys: &[&str]) -> bool {
    match field(value, keys).map(Value::unflagged) {
        Some(Value::Bool(b)) => *b,
        Some(other) => other.as_i64().is_some_and(|i| i != 0),
        None => false,
    }
}

fn block_bytes<'a>(resource: &Resource<'a>, index: i64) -> Result<&'a [u8], ModelError> {
    let block = usize::try_from(index)
        .ok()
        .and_then(|i| resource.blocks().get(i))
        .ok_or(ModelError::BadBlockIndex(index))?;
    Ok(resource.block_data(block))
}

/// Reads one `CTRL → embedded_meshes[]` entry and everything it points at.
fn read_embedded_mesh(resource: &Resource<'_>, entry: &Value) -> Result<Mesh, ModelError> {
    let name = field(entry, &["m_Name", "name"])
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mesh_index = int(entry, &["m_nMeshIndex", "mesh_index"])
        .unwrap_or(0)
        .max(0) as usize;
    let data_block =
        int(entry, &["m_nDataBlock", "data_block"]).ok_or(ModelError::MissingKey("data_block"))?;
    let mesh_data = crate::kv3::binary::decode(block_bytes(resource, data_block)?)
        .map_err(|source| ResourceError::Kv3 {
            kind: BlockKind(*b"MDAT"),
            source,
        })?
        .root;

    let (vertex_buffers, index_buffers) = match int(entry, &["vbib_block"]) {
        // Older layout: one binary VBIB/MBUF block with every buffer of the mesh.
        Some(vbib) if vbib >= 0 => read_vbib(block_bytes(resource, vbib)?, &name)?,
        // Current layout: buffers described in KV3, data in separate MVTX / MIDX blocks.
        _ => {
            let mut vbs = Vec::new();
            for desc in field(entry, &["m_vertexBuffers"])
                .and_then(Value::as_array)
                .unwrap_or_default()
            {
                let raw = read_kv3_buffer(resource, desc, &name, BufferKind::Vertex)?;
                vbs.push(VertexBuffer {
                    count: raw.count,
                    stride: raw.stride,
                    attributes: layout_from_kv3(desc),
                    data: raw.data,
                });
            }
            let mut ibs = Vec::new();
            for desc in field(entry, &["m_indexBuffers"])
                .and_then(Value::as_array)
                .unwrap_or_default()
            {
                let raw = read_kv3_buffer(resource, desc, &name, BufferKind::Index)?;
                ibs.push(raw.into_indices());
            }
            (vbs, ibs)
        }
    };

    Ok(Mesh {
        draw_calls: read_draw_calls(&mesh_data),
        attachments: read_attachments(&mesh_data),
        bone_weight_count: mesh_data
            .get("m_skeleton")
            .and_then(|s| s.get("m_nBoneWeightCount"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0) as u32,
        name,
        mesh_index,
        vertex_buffers,
        index_buffers,
    })
}

/// The attachments of a mesh (`MDAT` → `m_attachments`: a list of `{ key, value }`, the value holding up to three
/// influences of which the first `m_nInfluences` count).
fn read_attachments(mesh_data: &Value) -> Vec<Attachment> {
    let numbers = |v: Option<&Value>| -> Vec<f32> {
        let mut out = Vec::new();
        if let Some(v) = v {
            crate::map::numbers(v, &mut out);
        }
        out
    };
    mesh_data
        .get("m_attachments")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let value = entry.get("value")?;
            let name = value.get("m_name").and_then(Value::as_str)?.to_owned();
            let bones = strings(value.get("m_influenceNames"));
            let offsets = numbers(value.get("m_vInfluenceOffsets"));
            let rotations = numbers(value.get("m_vInfluenceRotations"));
            let weights = numbers(value.get("m_influenceWeights"));
            let count = value
                .get("m_nInfluences")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(0, bones.len() as i64) as usize;
            let influences = (0..count)
                .map(|i| Influence {
                    bone: bones[i].clone(),
                    offset: std::array::from_fn(|k| offsets.get(3 * i + k).copied().unwrap_or(0.0)),
                    rotation: normalize_quat(std::array::from_fn(|k| {
                        rotations
                            .get(4 * i + k)
                            .copied()
                            .unwrap_or(if k == 3 { 1.0 } else { 0.0 })
                    })),
                    weight: weights.get(i).copied().unwrap_or(1.0),
                })
                .collect();
            Some(Attachment {
                name,
                influences,
                ignore_rotation: flag(value, &["m_bIgnoreRotation"]),
            })
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BufferKind {
    Vertex,
    Index,
}

/// Buffer bytes after decompression, before interpretation.
struct RawBuffer {
    count: usize,
    stride: usize,
    data: Vec<u8>,
}

impl RawBuffer {
    fn into_indices(self) -> Vec<u32> {
        match self.stride {
            2 => self
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])))
                .collect(),
            _ => self
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        }
    }
}

/// Decompresses buffer bytes: ZSTD first (if flagged), then meshopt (if flagged), else raw.
fn decompress_buffer(
    mut bytes: Vec<u8>,
    count: usize,
    stride: usize,
    meshopt_compressed: bool,
    zstd: bool,
    kind: BufferKind,
    mesh: &str,
) -> Result<Vec<u8>, ModelError> {
    let expected = count * stride;
    if zstd {
        use std::io::Read;
        let mut out = Vec::with_capacity(expected);
        let mut input = bytes.as_slice();
        let mut decoder =
            ruzstd::decoding::StreamingDecoder::new(&mut input).map_err(|e| ModelError::Zstd {
                mesh: mesh.to_owned(),
                message: e.to_string(),
            })?;
        decoder
            .read_to_end(&mut out)
            .map_err(|e| ModelError::Zstd {
                mesh: mesh.to_owned(),
                message: e.to_string(),
            })?;
        bytes = out;
    }
    if meshopt_compressed && bytes.len() != expected {
        let meshopt_error = |source| ModelError::Meshopt {
            mesh: mesh.to_owned(),
            source,
        };
        return match kind {
            BufferKind::Vertex => {
                meshopt::decode_vertex_buffer(count, stride, &bytes).map_err(meshopt_error)
            }
            BufferKind::Index => {
                let indices = meshopt::decode_index_buffer(count, &bytes).map_err(meshopt_error)?;
                Ok(match stride {
                    2 => indices
                        .iter()
                        .flat_map(|&i| (i as u16).to_le_bytes())
                        .collect(),
                    _ => indices.iter().flat_map(|&i| i.to_le_bytes()).collect(),
                })
            }
        };
    }
    if bytes.len() < expected {
        return Err(UnexpectedEof {
            what: "mesh buffer data",
            offset: 0,
            needed: expected,
            available: bytes.len(),
        }
        .into());
    }
    bytes.truncate(expected);
    Ok(bytes)
}

/// Reads a buffer described in KV3 (`m_nBlockIndex`, `m_nElementCount`, …) from its MVTX / MIDX block.
fn read_kv3_buffer(
    resource: &Resource<'_>,
    desc: &Value,
    mesh: &str,
    kind: BufferKind,
) -> Result<RawBuffer, ModelError> {
    let count = int(desc, &["m_nElementCount"]).unwrap_or(0).max(0) as usize;
    let stride = int(desc, &["m_nElementSizeInBytes"]).unwrap_or(0).max(0) as usize;
    let bytes = match int(desc, &["m_nBlockIndex"]) {
        Some(index) if index >= 0 => block_bytes(resource, index)?.to_vec(),
        _ => match field(desc, &["m_pData"]).map(Value::unflagged) {
            Some(Value::Blob(bytes)) => bytes.clone(),
            _ => return Err(ModelError::MissingKey("m_nBlockIndex / m_pData")),
        },
    };
    let data = decompress_buffer(
        bytes,
        count,
        stride,
        flag(desc, &["m_bMeshoptCompressed"]),
        flag(desc, &["m_bCompressedZSTD"]),
        kind,
        mesh,
    )?;
    Ok(RawBuffer {
        count,
        stride,
        data,
    })
}

/// Reads the vertex layout of a KV3 buffer description.
fn layout_from_kv3(desc: &Value) -> Vec<Attribute> {
    field(desc, &["m_inputLayoutFields"])
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let semantic = match field(f, &["m_pSemanticName"]).map(Value::unflagged) {
                Some(Value::String(s)) => s.clone(),
                // Some files store the name as a NUL-padded blob.
                Some(Value::Blob(b)) => {
                    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or_default())
                        .into_owned()
                }
                _ => String::new(),
            };
            Attribute {
                semantic: semantic.to_ascii_uppercase(),
                semantic_index: int(f, &["m_nSemanticIndex"]).unwrap_or(0).max(0) as u32,
                format: int(f, &["m_Format"]).unwrap_or(0).max(0) as u32,
                offset: int(f, &["m_nOffset"]).unwrap_or(0).max(0) as usize,
            }
        })
        .collect()
}

/// Reads the draw calls of a mesh's `MDAT` block.
fn read_draw_calls(mesh_data: &Value) -> Vec<DrawCall> {
    let mut calls = Vec::new();
    for object in mesh_data
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        for draw in object
            .get("m_drawCalls")
            .and_then(Value::as_array)
            .unwrap_or_default()
        {
            // Only triangle lists exist in practice ("RENDER_PRIM_TRIANGLES" or 5).
            let primitive = draw.get("m_nPrimitiveType");
            let triangles = match primitive.map(Value::unflagged) {
                Some(Value::String(s)) => s == "RENDER_PRIM_TRIANGLES",
                Some(other) => other.as_i64() == Some(5),
                None => true,
            };
            if !triangles {
                continue;
            }
            let buffer_handle = |v: Option<&Value>| {
                v.and_then(|b| int(b, &["m_hBuffer"])).unwrap_or(0).max(0) as usize
            };
            let compressed = match draw.get("m_nFlags").map(Value::unflagged) {
                Some(Value::String(s)) => {
                    s.contains("MESH_DRAW_FLAGS_USE_COMPRESSED_NORMAL_TANGENT")
                }
                Some(other) => other.as_i64().is_some_and(|f| f & 2 != 0),
                None => flag(draw, &["m_bUseCompressedNormalTangent"]),
            };
            calls.push(DrawCall {
                material: field(draw, &["m_material", "m_pMaterial"])
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                vertex_buffers: draw
                    .get("m_vertexBuffers")
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .map(|b| buffer_handle(Some(b)))
                    .collect(),
                index_buffer: buffer_handle(draw.get("m_indexBuffer")),
                base_vertex: int(draw, &["m_nBaseVertex"]).unwrap_or(0).max(0) as u32,
                start_index: int(draw, &["m_nStartIndex"]).unwrap_or(0).max(0) as u32,
                index_count: int(draw, &["m_nIndexCount"]).unwrap_or(0).max(0) as u32,
                compressed_normal_tangent: compressed,
                tint: draw_tint(draw),
                alpha: field(draw, &["m_flAlpha"])
                    .and_then(Value::as_f64)
                    .map_or(1.0, |a| a as f32),
            });
        }
    }
    calls
}

/// A draw call's `m_vTintColor` (three floats, linear), white when absent or malformed.
fn draw_tint(draw: &Value) -> [f32; 3] {
    let values: Vec<f32> = field(draw, &["m_vTintColor"])
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();
    match values.as_slice() {
        [r, g, b, ..] => [*r, *g, *b],
        _ => [1.0; 3],
    }
}

/// Parses an older binary `VBIB` / `MBUF` block holding all vertex and index buffers of a mesh.
fn read_vbib(block: &[u8], mesh: &str) -> Result<(Vec<VertexBuffer>, Vec<Vec<u32>>), ModelError> {
    let mut r = ByteReader::new(block);
    // Header: (offset, count) for vertex buffers, then for index buffers; offsets relative to their own field.
    let vb_field = r.position();
    let vb_offset = r.u32("VBIB vertex buffer offset")? as usize;
    let vb_count = r.u32("VBIB vertex buffer count")?;
    let ib_field = r.position();
    let ib_offset = r.u32("VBIB index buffer offset")? as usize;
    let ib_count = r.u32("VBIB index buffer count")?;

    let read_record =
        |start: usize, kind: BufferKind| -> Result<(RawBuffer, Vec<Attribute>), ModelError> {
            let mut rec = ByteReader::new(block);
            rec.seek(start, "VBIB buffer record")?;
            let count = rec.u32("VBIB element count")? as usize;
            let size_field = rec.u32("VBIB element size")?;
            let stride = (size_field & 0x03FF_FFFF) as usize;
            // Bit 26 set means the data is NOT meshopt-compressed; bit 27 means ZSTD.
            let meshopt_compressed = size_field & 0x0400_0000 == 0;
            let zstd = size_field & 0x0800_0000 != 0;
            let attr_field = rec.position();
            let attr_offset = rec.u32("VBIB attribute offset")? as usize;
            let attr_count = rec.u32("VBIB attribute count")?;
            let data_field = rec.position();
            let data_offset = rec.u32("VBIB data offset")? as usize;
            let data_size = rec.u32("VBIB data size")? as usize;

            let mut attributes = Vec::new();
            let mut a = ByteReader::new(block);
            a.seek(attr_field + attr_offset, "VBIB attributes")?;
            for _ in 0..attr_count {
                let name = a.array::<32>("VBIB semantic name")?;
                let semantic =
                    String::from_utf8_lossy(name.split(|&c| c == 0).next().unwrap_or_default())
                        .to_ascii_uppercase();
                let semantic_index = a.i32("VBIB semantic index")?.max(0) as u32;
                let format = a.u32("VBIB attribute format")?;
                let offset = a.u32("VBIB attribute offset")? as usize;
                a.skip(12, "VBIB slot, slot type, step rate")?;
                attributes.push(Attribute {
                    semantic,
                    semantic_index,
                    format,
                    offset,
                });
            }

            let start = data_field + data_offset;
            let bytes = block.get(start..start + data_size).ok_or(UnexpectedEof {
                what: "VBIB buffer data",
                offset: start,
                needed: data_size,
                available: block.len().saturating_sub(start),
            })?;
            let compressed = count * stride > data_size;
            let data = decompress_buffer(
                bytes.to_vec(),
                count,
                stride,
                compressed && meshopt_compressed,
                compressed && zstd,
                kind,
                mesh,
            )?;
            Ok((
                RawBuffer {
                    count,
                    stride,
                    data,
                },
                attributes,
            ))
        };

    let mut vbs = Vec::new();
    for i in 0..vb_count as usize {
        let (raw, attributes) = read_record(vb_field + vb_offset + 24 * i, BufferKind::Vertex)?;
        vbs.push(VertexBuffer {
            count: raw.count,
            stride: raw.stride,
            attributes,
            data: raw.data,
        });
    }
    let mut ibs = Vec::new();
    for i in 0..ib_count as usize {
        let (raw, _) = read_record(ib_field + ib_offset + 24 * i, BufferKind::Index)?;
        ibs.push(raw.into_indices());
    }
    Ok((vbs, ibs))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `MDAT` with one scene object holding the given draw calls.
    fn mesh_data(draws: Vec<Value>) -> Value {
        let object = Value::Object([("m_drawCalls", Value::Array(draws))].into_iter().collect());
        Value::Object(
            [("m_sceneObjects", Value::Array(vec![object]))]
                .into_iter()
                .collect(),
        )
    }

    fn floats(values: &[f64]) -> Value {
        Value::Array(values.iter().map(|&v| Value::Float(v)).collect())
    }

    #[test]
    fn draw_calls_keep_their_tint_and_alpha() {
        // de_nuke's parking stripe overlay (n0_lr0_c202_s_mesh_overlay202) and a draw call with neither key.
        let stripe = Value::Object(
            [
                (
                    "m_material",
                    Value::String("materials/de_nuke/hr_nuke_parkingstripe_single.vmat".into()),
                ),
                ("m_nIndexCount", Value::Int(351)),
                (
                    "m_vTintColor",
                    floats(&[
                        0.851_251_661_777_496_3,
                        0.459_079_921_245_574_95,
                        0.046_148_840_337_991_714,
                    ]),
                ),
                ("m_flAlpha", Value::Float(0.25)),
            ]
            .into_iter()
            .collect(),
        );
        let plain = Value::Object(
            [("m_material", Value::String("materials/plain.vmat".into()))]
                .into_iter()
                .collect(),
        );
        let draws = read_draw_calls(&mesh_data(vec![stripe, plain]));
        assert_eq!(draws.len(), 2);
        assert!((draws[0].tint[0] - 0.851_25).abs() < 1e-5);
        assert_eq!(draws[0].alpha, 0.25);
        assert!(!draws[0].is_untinted());
        assert_eq!((draws[1].tint, draws[1].alpha), ([1.0; 3], 1.0));
        assert!(draws[1].is_untinted());
    }

    #[test]
    fn draw_tints_meet_the_placement_tint_in_srgb() {
        let draw = |tint: [f32; 3], alpha: f32| DrawCall {
            material: String::new(),
            vertex_buffers: Vec::new(),
            index_buffer: 0,
            base_vertex: 0,
            start_index: 0,
            index_count: 0,
            compressed_normal_tangent: false,
            tint,
            alpha,
        };
        // The stored linear floats are Hammer's colour picker bytes through gamma 2.2: (237, 179, 63) exactly.
        let yellow = draw([0.851_251_7, 0.459_079_9, 0.046_148_84], 1.0);
        let bytes = yellow.placement_tint([1.0; 4]).map(|c| c * 255.0);
        for (got, want) in bytes.iter().zip([237.0, 179.0, 63.0, 255.0]) {
            assert!((got - want).abs() < 0.01, "{bytes:?}");
        }
        // It multiplies the placement's own tint and alpha.
        let half = draw([1.0; 3], 0.5).placement_tint([0.5, 1.0, 1.0, 0.8]);
        assert_eq!(half, [0.5, 1.0, 1.0, 0.4]);
        // White leaves the placement as it is.
        let placement = [0.2, 0.4, 0.6, 1.0];
        assert_eq!(draw([1.0; 3], 1.0).placement_tint(placement), placement);
    }
}
