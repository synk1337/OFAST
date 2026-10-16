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

use crate::bytes::{ByteReader, UnexpectedEof};
use crate::kv3::Value;
use crate::meshopt::{self, MeshoptError};
use crate::resource::{BlockKind, Resource, ResourceError};

pub use attributes::{Geometry, half_to_f32};

/// DXGI format numbers used by CS2 vertex layouts.
#[allow(missing_docs)]
pub mod dxgi {
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32_FLOAT: u32 = 6;
    pub const R16G16B16A16_FLOAT: u32 = 10;
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Meshes shown by default: in the default mesh groups and in the most detailed LOD.
    pub fn default_meshes(&self) -> impl Iterator<Item = &Mesh> {
        let lowest_lod = self
            .lod_masks
            .iter()
            .fold(0u64, |acc, m| acc | m)
            .trailing_zeros();
        self.meshes.iter().filter(move |mesh| {
            let in_group = self
                .mesh_group_masks
                .get(mesh.mesh_index)
                .is_none_or(|mask| mask & self.default_mesh_group_mask != 0);
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
        name,
        mesh_index,
        vertex_buffers,
        index_buffers,
    })
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
            });
        }
    }
    calls
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
