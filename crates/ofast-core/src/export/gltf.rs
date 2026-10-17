//! glTF 2.0 writer for CS2 content: geometry, materials, textures, node placements and lights.
//!
//! Used by [`export_model`] (one model) and by [`super::map::export_map`] (a whole map). Output folder layout:
//!
//! ```text
//! <out>/<name>.gltf        scene description (JSON)
//! <out>/<name>.bin         vertex + index data
//! <out>/textures/*.png     decoded textures (the folder belongs to the exporter: see "Texture files" below)
//! ```
//!
//! # Conversions applied
//!
//! - **Axes and units**: see [`crate::coords`]. A Source position `(x, y, z)` in inches is written as
//!   `0.0254 × (x, z, −y)` metres, so that Unreal's importer places it at `(x, −y, z) × 2.54` cm — exactly where
//!   it is in CS2. Placements (node matrices) are converted the same way.
//! - **Materials**: each CS2 material (× tint colour) becomes a glTF PBR material. The CS2 shader name and texture
//!   parameters are kept in `extras.ofast` so the Unreal plugin can later build matching master materials.
//!   Translucent and alpha-tested CS2 materials map to glTF `BLEND` / `MASK` (cut at the material's
//!   `g_flAlphaTestReference`); `csgo_static_overlay` by its blend mode ([`OverlayBlend`]).
//! - **Textures**: see [`TextureRole`]. Textures are decoded and written as PNG at the end, in parallel; a texture
//!   that fails to decode is replaced by a 1×1 neutral image and reported as a warning.
//!
//! # Texture files
//!
//! The last step of every export (`GltfWriter::finish`) keeps the `textures` folder to exactly what the glTF files
//! in `<out>` use:
//!
//! - **One file per distinct image.** Every texture is hashed (BLAKE3 of its size and pixels) once decoded or baked;
//!   textures with equal pixels share one file, image and glTF texture, whatever produced them (two CS2 textures
//!   with the same content, two bakes whose settings differ only where it changes nothing).
//! - **Names.** A CS2 texture keeps its file name plus the role's suffix (`_normal`, `_raw`, …; `_2`, `_3` when
//!   two differ). A texture OFAST computes for an environment material (`environment`) is named
//!   `<texture>_cc_<hash>.png` or `<texture>_surface_<hash>.png` (a prop's colour with its masked tint baked in,
//!   `tint`: `<texture>_tint_<hash>.png`), `<hash>` being the first 8 hex digits of its pixel
//!   hash: the same name always holds the same pixels, so a tool that imports each file name once (`import_map.py`'s
//!   `import_texture`) never keeps a stale bake after a re-export.
//! - **No leftovers.** After writing, PNGs that no `.gltf` in `<out>` references are deleted from `<out>/textures`
//!   (several exports may share the folder, a model next to a map), and so are `.part` files of an interrupted
//!   export. Re-exports used to pile up: de_inferno's folder held 325 unused files (1 GB) from older runs.
//! - **Encoding.** Lossless PNG, 8 bits per channel like the BC-compressed sources: RGB when every alpha is 255,
//!   RGBA otherwise, zlib's default level with adaptive filters ([`write_png`]). Textures keep their full size: CS2
//!   draws them at that size, and a size cap is better chosen in Unreal (per texture group) than baked into the files.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use serde_json::{Value as Json, json};

use super::{ExportError, ExportOptions};
use super::{environment, tint};
use crate::content::Content;
use crate::coords;
use crate::kv3::Value;
use crate::map::Transform;
use crate::model::{DrawCall, Mesh, Model};
use crate::resource::{BlockKind, Resource};
use crate::texture::{self, Format, Image, Texture};

/// What an export produced.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    /// Path of the written `.gltf` file.
    pub gltf: PathBuf,
    /// glTF meshes written.
    pub meshes: usize,
    /// glTF nodes (placements) written.
    pub nodes: usize,
    /// Triangles written (each mesh counted once, however often it is placed).
    pub triangles: usize,
    /// Materials written.
    pub materials: usize,
    /// Texture files written (one per distinct image).
    pub textures: usize,
    /// Textures whose pixels equalled an earlier one's: they share its file instead of writing their own.
    pub shared_textures: usize,
    /// Files of earlier exports removed from the `textures` folder (no glTF there used them any more).
    pub removed_textures: usize,
    /// Lights written.
    pub lights: usize,
    /// Zero-area triangles removed (see `GltfWriter::mesh`).
    pub degenerate_triangles: usize,
    /// Non-fatal problems (a texture that could not be decoded, a missing model, …).
    pub warnings: Vec<String>,
}

/// Exports the model at `model_path` (e.g. `weapons/models/ak47/weapon_rif_ak47.vmdl_c`) with its default meshes.
pub fn export_model(
    content: &Content,
    model_path: &str,
    out_dir: &Path,
) -> Result<ExportReport, ExportError> {
    let model = load_model(content, model_path)?;
    let name = Path::new(model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_owned();
    // A model on its own has no placement tints: the options change nothing here.
    let mut writer = GltfWriter::new(content, out_dir, ExportOptions::default())?;
    let mut roots = Vec::new();
    for mesh in model.default_meshes() {
        let draws: Vec<&DrawCall> = mesh.draw_calls.iter().collect();
        if let Some(gltf_mesh) = writer.mesh(&mesh.name, mesh, &draws, [1.0; 4], 0.0)? {
            roots.push(writer.node(&mesh.name, Some(gltf_mesh), None));
        }
    }
    writer.finish(&name, roots)
}

/// Reads and decodes a model (`path` with or without `_c`).
pub fn load_model(content: &Content, path: &str) -> Result<Model, ExportError> {
    let data = content.read_resource(path)?;
    let resource = Resource::parse(&data)?;
    Ok(Model::parse(&resource)?)
}

/// How a texture is used, which decides the conversions applied before writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TextureRole {
    /// Base colour. Alpha kept only for translucent / alpha-tested materials (CS2 often stores masks there).
    Color {
        /// Keep the alpha channel.
        keep_alpha: bool,
    },
    /// Normal map. `hemi_oct`: CS2 (`csgo_*`) shaders always store a hemi-octahedral normal in RG (with roughness
    /// in B); other shaders store X/Y and Z is rebuilt.
    Normal {
        /// Decode RG as a hemi-octahedral normal.
        hemi_oct: bool,
    },
    /// Ambient occlusion (grey).
    Occlusion,
    /// Channels kept exactly as decoded (masks such as the blend modulation texture).
    Raw,
}

impl TextureRole {
    fn suffix(self) -> &'static str {
        match self {
            TextureRole::Color { keep_alpha: false } => "",
            TextureRole::Color { keep_alpha: true } => "_rgba",
            TextureRole::Normal { .. } => "_normal",
            TextureRole::Occlusion => "_ao",
            TextureRole::Raw => "_raw",
        }
    }
}

/// Where the channels of a glTF metallic-roughness texture (G = roughness, B = metalness) come from.
///
/// CS2 never stores that combination: roughness is the blue channel of the normal map (`csgo_*` shaders) or the red
/// channel of `g_tMetalness` (`csgo_weapon`), and metalness is the green channel of `g_tMetalness`. The two may
/// have different sizes; the output takes the larger one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OrmSource {
    /// Texture path and channel (0 = R … 3 = A) holding roughness.
    roughness: Option<(String, usize)>,
    /// Texture path and channel holding metalness.
    metalness: Option<(String, usize)>,
}

/// A texture to decode and write when the export finishes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TextureJob {
    /// One CS2 texture, converted for a role.
    Single(String, TextureRole),
    /// A metallic-roughness texture assembled from channels of other textures.
    Orm(OrmSource),
    /// A `csgo_environment(_blend)` layer's colour with its adjustments baked in.
    EnvColor(environment::ColorJob),
    /// A `csgo_environment(_blend)` layer's occlusion / roughness / metalness texture.
    EnvSurface(environment::SurfaceJob),
    /// A prop shader's colour with its masked model tint baked in (`ExportOptions::bake_model_tints`).
    TintColor(tint::TintJob),
}

impl TextureJob {
    /// Textures OFAST computes rather than converts: named after their pixel hash (see "Texture files").
    fn is_baked(&self) -> bool {
        matches!(
            self,
            TextureJob::EnvColor(_) | TextureJob::EnvSurface(_) | TextureJob::TintColor(_)
        )
    }
}

/// A registered texture (glTF texture index = position in [`GltfWriter::texture_jobs`]).
struct RegisteredTexture {
    job: TextureJob,
    /// File name without `.png` given at registration, unique among the export's textures: the final name of a
    /// converted texture, a placeholder (in `extras.ofast`) for a baked one until [`GltfWriter::finish`] names it.
    name: String,
    /// Name the file of a baked texture starts with (`<texture>_cc`, `<texture>_surface`).
    base: String,
}

/// glTF texture indices and settings of one `csgo_environment(_blend)` layer.
struct EnvironmentLayer {
    /// Adjusted colour (sRGB), without the model tint unless [`ExportOptions::bake_model_tints`].
    color: Option<usize>,
    /// Normal map.
    normal: Option<usize>,
    /// Occlusion (R), roughness (G), metalness (B).
    surface: Option<usize>,
    /// The height texture as stored (height in R, tint mask in G).
    height: Option<usize>,
    /// The layer's settings (model-tint strength and tint-mask contrast / brightness go into `extras.ofast`).
    settings: environment::Layer,
}

/// Material information read from a `.vmat_c`.
#[derive(Debug, Clone, Default)]
pub(crate) struct MaterialInfo {
    pub(crate) shader: String,
    pub(crate) textures: BTreeMap<String, String>,
    pub(crate) ints: BTreeMap<String, i64>,
    pub(crate) floats: BTreeMap<String, f64>,
    /// `g_v*` parameters (colours, UV scales…), always four components.
    pub(crate) vectors: BTreeMap<String, [f64; 4]>,
}

impl MaterialInfo {
    fn first_texture<'a>(&'a self, names: &[&'a str]) -> Option<(&'a str, &'a str)> {
        names
            .iter()
            .find_map(|n| self.textures.get(*n).map(|path| (*n, path.as_str())))
    }

    /// An integer parameter (`F_*` feature, `g_b*` switch) that is set and not 0.
    pub(crate) fn flag(&self, name: &str) -> bool {
        self.ints.get(name).is_some_and(|v| *v != 0)
    }

    /// A two-layer material whose second layer is painted per vertex: a [`VERTEX_BLEND_SHADERS`] shader with a
    /// `g_tLayer2Color`. `csgo_simple_3layer_parallax` (de_inferno's fake room windows) also has a
    /// `g_tLayer2Color`, but its layers are depths behind the glass, not painted blends.
    fn is_vertex_blend(&self) -> bool {
        VERTEX_BLEND_SHADERS.contains(&self.shader.as_str())
            && self.textures.contains_key("g_tLayer2Color")
    }

    /// The material reads per-vertex blend weights (`TEXCOORD 4`): the two-layer blends above and
    /// `csgo_environment_blend`.
    fn reads_blend_weights(&self) -> bool {
        self.is_vertex_blend() || self.shader == environment::ENVIRONMENT_BLEND
    }

    /// The alpha below which an alpha-tested material is cut away: its `g_flAlphaTestReference`, or the shaders'
    /// default 0.5 when the material keeps it. The six exported maps use 0.05 – 0.9 (`csgo_foliage` 0.2 – 0.9,
    /// `csgo_static_overlay` 0.18 – 0.63), so a fixed 0.5 cut foliage cards and alpha-tested overlays wrongly.
    fn alpha_test_reference(&self) -> f64 {
        self.floats
            .get("g_flAlphaTestReference")
            .copied()
            .unwrap_or(0.5)
    }
}

/// The shader of Hammer's overlays (decals placed on a surface: grime, signs, floor markings).
pub(crate) const STATIC_OVERLAY: &str = "csgo_static_overlay.vfx";

/// How a `csgo_static_overlay` material is blended with what is under it: its `F_BLEND_MODE` feature, numbered as in
/// the shader's feature list (decompiled in GameTracking-CS2, `csgo_static_overlay.slang`): `0="Opaque",
/// 1="Translucent", 2="Alpha Test", 3="Mod2x", 4="Additive", 5="Multiply", 6="ModThenAdd"`. Only modes 0–2 can be lit
/// (`F_LIT`); every mode is drawn with depth bias −64 and without depth writes, the colour blend being
/// `DstBlend (F_BLEND_MODE==3) ? One : InvSrcAlpha` (the shader's render state).
///
/// | Mode | How CS2 draws it | glTF |
/// |---|---|---|
/// | `Opaque` | a lit surface; the colour's alpha is unused | `OPAQUE` |
/// | `Translucent` | alpha blend over the surface under it | `BLEND` |
/// | `AlphaTest` | cut where the colour's alpha is below `g_flAlphaTestReference` | `MASK` at that reference |
/// | `Mod2x` | unlit: multiplies what is drawn under it by `lerp(1, 2 × colour × tint, alpha × tint alpha)`, the colour read without sRGB decoding (`SrgbRead(false)`), so 128 grey changes nothing, darker darkens, brighter brightens up to 2× | `BLEND` (glTF has no multiply blend; `import_map.py` gives Unreal a Modulate material) |
/// | `Additive`, `Multiply`, `ModThenAdd` | not on the six exported maps; not checked | `BLEND` |
///
/// Other shaders with an `F_BLEND_MODE` (`csgo_unlitgeneric`: de_dust2 and de_mirage have one each) may number
/// their modes differently and keep the plain `BLEND` of every translucent material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayBlend {
    /// 0: drawn like any surface (de_nuke's red "A" / "B" bombsite signs).
    Opaque,
    /// 1: alpha blend (de_nuke's plant logos and painted stripes).
    Translucent,
    /// 2: alpha-tested (de_nuke 1, de_inferno 6, de_anubis 7, de_vertigo 1 materials).
    AlphaTest,
    /// 3: unlit multiply by 2 × colour (grime, scuffs, floor markings, caustics).
    Mod2x,
    /// 4.
    Additive,
    /// 5.
    Multiply,
    /// 6.
    ModThenAdd,
}

impl OverlayBlend {
    /// The blend mode of a `csgo_static_overlay` material (`F_BLEND_MODE`, 0 when not set); `None` for the other
    /// shaders. A value outside 0–6 is read as `Translucent`, the plain `BLEND` every overlay had before.
    pub(crate) fn of(info: &MaterialInfo) -> Option<Self> {
        if info.shader != STATIC_OVERLAY {
            return None;
        }
        Some(match info.ints.get("F_BLEND_MODE").copied().unwrap_or(0) {
            0 => Self::Opaque,
            2 => Self::AlphaTest,
            3 => Self::Mod2x,
            4 => Self::Additive,
            5 => Self::Multiply,
            6 => Self::ModThenAdd,
            _ => Self::Translucent,
        })
    }
}

/// Accumulates glTF arrays and the binary buffer while exporting.
pub(crate) struct GltfWriter<'c> {
    content: &'c Content,
    out_dir: PathBuf,
    options: ExportOptions,
    bin: Vec<u8>,
    buffer_views: Vec<Json>,
    accessors: Vec<Json>,
    meshes: Vec<Json>,
    nodes: Vec<Json>,
    lights: Vec<Json>,
    materials: Vec<Json>,
    /// (material path, tint in 0–255) → material index.
    material_ids: BTreeMap<(String, [u8; 4]), usize>,
    /// material path → parsed info (or None when unreadable).
    material_infos: BTreeMap<String, Option<MaterialInfo>>,
    /// texture job → glTF texture index (before `finish` merges textures with equal pixels).
    texture_ids: BTreeMap<TextureJob, usize>,
    /// Textures to decode and write in `finish`, by glTF texture index.
    texture_jobs: Vec<RegisteredTexture>,
    /// Registered file names (without `.png`), to keep them unique.
    texture_names: BTreeSet<String>,
    /// glTF material name → CS2 material path, to keep names unique (Unreal matches material slots by name).
    material_names: BTreeMap<String, String>,
    pub(crate) report: ExportReport,
}

/// glTF component type and target constants.
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// Shaders whose geometry is not exported: `csgo_effects` draws additive dust / light-shaft cards that would show
/// up as opaque white sheets in a plain PBR material (they come back as Unreal effects later).
const SKIPPED_SHADERS: &[&str] = &["csgo_effects.vfx"];

/// Texture parameter names, in priority order, for each glTF slot.
const COLOR_PARAMS: &[&str] = &["g_tColor", "g_tColor1", "g_tLayer1Color", "g_tColorA"];
const NORMAL_PARAMS: &[&str] = &[
    "g_tNormal",
    "g_tLayer1NormalRoughness",
    "g_tNormalRoughness",
    "g_tNormal1",
    "g_tNormalA",
];
const AO_PARAMS: &[&str] = &["g_tAmbientOcclusion", "g_tLayer1AmbientOcclusion"];
/// Shaders whose `g_tLayer2Color` is a second layer painted per vertex (Hammer's blend brushes), as found on
/// de_mirage, de_dust2 and de_nuke. `csgo_environment_blend` (de_inferno) names its layers `g_tColor1`/`g_tColor2`
/// and blends by height: not handled yet.
const VERTEX_BLEND_SHADERS: &[&str] = &["csgo_lightmappedgeneric.vfx"];

impl<'c> GltfWriter<'c> {
    pub(crate) fn new(
        content: &'c Content,
        out_dir: &Path,
        options: ExportOptions,
    ) -> Result<Self, ExportError> {
        fs::create_dir_all(out_dir.join("textures"))?;
        Ok(Self {
            content,
            out_dir: out_dir.to_path_buf(),
            options,
            bin: Vec::new(),
            buffer_views: Vec::new(),
            accessors: Vec::new(),
            meshes: Vec::new(),
            nodes: Vec::new(),
            lights: Vec::new(),
            materials: Vec::new(),
            material_ids: BTreeMap::new(),
            material_infos: BTreeMap::new(),
            texture_ids: BTreeMap::new(),
            texture_jobs: Vec::new(),
            texture_names: BTreeSet::new(),
            material_names: BTreeMap::new(),
            report: ExportReport::default(),
        })
    }

    /// Appends raw bytes as a buffer view (4-byte aligned) and returns its index.
    fn view(&mut self, bytes: &[u8], target: u32) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
            "target": target,
        }));
        self.bin.extend_from_slice(bytes);
        self.buffer_views.len() - 1
    }

    /// Writes `values` (N floats per element) as an accessor.
    fn float_accessor<const N: usize>(
        &mut self,
        values: &[[f32; N]],
        kind: &str,
        with_bounds: bool,
    ) -> usize {
        let bytes: Vec<u8> = values
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let view = self.view(&bytes, ARRAY_BUFFER);
        let mut accessor = json!({
            "bufferView": view,
            "componentType": FLOAT,
            "count": values.len(),
            "type": kind,
        });
        if with_bounds {
            // glTF requires min/max on POSITION.
            let mut min = [f32::MAX; N];
            let mut max = [f32::MIN; N];
            for v in values {
                for k in 0..N {
                    min[k] = min[k].min(v[k]);
                    max[k] = max[k].max(v[k]);
                }
            }
            accessor["min"] = json!(min.to_vec());
            accessor["max"] = json!(max.to_vec());
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// Writes a glTF mesh made of the given draw calls of `mesh`, all placed with the tint `tint` (sRGB RGBA 0–1).
    /// Each draw call's own tint and alpha multiply it ([`DrawCall::placement_tint`]): Hammer paints mesh faces and
    /// overlays that way (de_nuke's yellow road paint is a white material). Returns `None` when no draw call has
    /// any triangle.
    ///
    /// `normal_offset` (inches) pushes every vertex along its normal: used for overlays/decals so they sit just in
    /// front of the surface they decorate instead of fighting with it for depth (CS2 uses a depth bias instead).
    pub(crate) fn mesh(
        &mut self,
        name: &str,
        mesh: &Mesh,
        draws: &[&DrawCall],
        tint: [f32; 4],
        normal_offset: f32,
    ) -> Result<Option<usize>, ExportError> {
        let mut primitives = Vec::new();
        for draw in draws {
            if !draw.material.is_empty()
                && let Some(info) = self.material_info(&draw.material)
                && SKIPPED_SHADERS.contains(&info.shader.as_str())
            {
                continue;
            }
            let mut g = mesh.geometry(draw)?;
            // CS2 meshes contain zero-area triangles (repeated or collinear vertices) that the game ignores, but
            // Unreal's glTF importer drops a whole primitive when it finds one — leaving holes in walls.
            let before = g.indices.len();
            g.indices = without_degenerate_triangles(&g.positions, &g.indices);
            self.report.degenerate_triangles += (before - g.indices.len()) / 3;
            if g.indices.is_empty() {
                continue;
            }
            self.report.triangles += g.indices.len() / 3;

            let to_gltf = coords::source_dir_to_gltf;
            let positions: Vec<[f32; 3]> = g
                .positions
                .iter()
                .enumerate()
                .map(|(i, &p)| {
                    let n = g.normals.get(i).copied().unwrap_or([0.0; 3]);
                    coords::source_point_to_gltf([0, 1, 2].map(|k| p[k] + n[k] * normal_offset))
                })
                .collect();
            let mut attributes = serde_json::Map::new();
            attributes.insert(
                "POSITION".into(),
                json!(self.float_accessor(&positions, "VEC3", true)),
            );
            if g.normals.len() == g.positions.len() {
                let normals: Vec<[f32; 3]> = g.normals.iter().map(|&n| to_gltf(n)).collect();
                attributes.insert(
                    "NORMAL".into(),
                    json!(self.float_accessor(&normals, "VEC3", false)),
                );
                if g.tangents.len() == g.positions.len() {
                    let tangents: Vec<[f32; 4]> = g
                        .tangents
                        .iter()
                        .map(|t| {
                            let v = to_gltf([t[0], t[1], t[2]]);
                            [v[0], v[1], v[2], t[3]]
                        })
                        .collect();
                    attributes.insert(
                        "TANGENT".into(),
                        json!(self.float_accessor(&tangents, "VEC4", false)),
                    );
                }
            }
            for (set, uvs) in g.uvs.iter().enumerate() {
                if uvs.len() == g.positions.len() {
                    attributes.insert(
                        format!("TEXCOORD_{set}"),
                        json!(self.float_accessor(uvs, "VEC2", false)),
                    );
                }
            }
            // Only layered materials get the weights: glTF multiplies base colour by COLOR_0, and ordinary draw
            // calls can share a vertex buffer that carries blend weights.
            let layered = !draw.material.is_empty()
                && self
                    .material_info(&draw.material)
                    .is_some_and(|info| info.reads_blend_weights());
            if layered {
                // Layer blend weights travel as vertex colour; Unreal's blend materials read them back. A draw
                // without painted weights (14 on de_mirage + de_inferno) gets zeros, i.e. layer 1 like the glTF
                // stand-in: without COLOR_0 Unreal would read white, layer 2.
                let unpainted;
                let weights: &[[f32; 4]] = if g.blend.len() == g.positions.len() {
                    &g.blend
                } else {
                    unpainted = vec![[0.0, 0.0, 0.0, 1.0]; g.positions.len()];
                    &unpainted
                };
                attributes.insert(
                    "COLOR_0".into(),
                    json!(self.float_accessor(weights, "VEC4", false)),
                );
            }
            let index_bytes: Vec<u8> = g.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let view = self.view(&index_bytes, ELEMENT_ARRAY_BUFFER);
            self.accessors.push(json!({
                "bufferView": view,
                "componentType": UNSIGNED_INT,
                "count": g.indices.len(),
                "type": "SCALAR",
            }));
            let mut primitive =
                json!({ "attributes": attributes, "indices": self.accessors.len() - 1, "mode": 4 });
            if !draw.material.is_empty() {
                primitive["material"] =
                    json!(self.material(&draw.material, draw.placement_tint(tint)));
            }
            primitives.push(primitive);
        }
        if primitives.is_empty() {
            return Ok(None);
        }
        self.meshes
            .push(json!({ "name": name, "primitives": primitives }));
        self.report.meshes += 1;
        Ok(Some(self.meshes.len() - 1))
    }

    /// Adds a node (placement) and returns its index. `transform` is in Source space.
    pub(crate) fn node(
        &mut self,
        name: &str,
        mesh: Option<usize>,
        transform: Option<&Transform>,
    ) -> usize {
        let mut node = json!({ "name": name });
        if let Some(mesh) = mesh {
            node["mesh"] = json!(mesh);
        }
        if let Some(t) = transform {
            node["matrix"] = json!(gltf_matrix(t).to_vec());
        }
        self.nodes.push(node);
        self.report.nodes += 1;
        self.nodes.len() - 1
    }

    /// Adds a `KHR_lights_punctual` light on a new node. `forward` is the Source direction the light shines.
    pub(crate) fn light(
        &mut self,
        name: &str,
        light: Json,
        origin: [f32; 3],
        forward: [f32; 3],
    ) -> usize {
        self.lights.push(light);
        let index = self.lights.len() - 1;
        let node = self.nodes.len();
        let mut matrix = light_matrix(coords::source_dir_to_gltf(forward));
        let p = coords::source_point_to_gltf(origin);
        matrix[12..15].copy_from_slice(&p);
        self.nodes.push(json!({
            "name": name,
            "matrix": matrix.to_vec(),
            "extensions": { "KHR_lights_punctual": { "light": index } },
        }));
        self.report.nodes += 1;
        self.report.lights += 1;
        node
    }

    /// Returns the glTF material for a CS2 material path and tint, creating it on first use.
    fn material(&mut self, path: &str, tint: [f32; 4]) -> usize {
        let tint_key = tint.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let key = (path.to_owned(), tint_key);
        if let Some(&id) = self.material_ids.get(&key) {
            return id;
        }
        let info = self.material_info(path);
        let base = self.unique_material_name(path);
        // Tinted variants get their own name so Unreal's material slots can be told apart; a variant with less
        // than full opacity (a draw call's `m_flAlpha`: de_nuke's scuff decals) adds its alpha byte.
        let name = match tint_key {
            [255, 255, 255, 255] => base,
            [r, g, b, 255] => format!("{base}_tint{r:02x}{g:02x}{b:02x}"),
            [r, g, b, a] => format!("{base}_tint{r:02x}{g:02x}{b:02x}{a:02x}"),
        };
        let mut material = json!({
            "name": name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
        });
        match info {
            // The material decides what the tint does (fill_material).
            Some(info) => self.fill_material(&mut material, &info, tint_key),
            // Unreadable: the tint on the whole surface (sRGB authored, glTF factors linear).
            None if tint_key != [255; 4] => {
                let mut linear = [tint[0], tint[1], tint[2]].map(srgb_to_linear).to_vec();
                linear.push(tint[3]);
                material["pbrMetallicRoughness"]["baseColorFactor"] = json!(linear);
            }
            None => {}
        }
        self.materials.push(material);
        let id = self.materials.len() - 1;
        self.material_ids.insert(key, id);
        self.report.materials += 1;
        id
    }

    /// The file name of a material, made unique: two CS2 materials can share a file name in different folders
    /// (e.g. `materials/de_dust/sitebwall13a.vmat` and `.../brush_shapes/sitebwall13a.vmat`), and Unreal would mix
    /// them up. The second one gets its folder name appended.
    fn unique_material_name(&mut self, path: &str) -> String {
        let p = Path::new(path);
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_owned();
        let folder = p
            .parent()
            .and_then(|f| f.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("x");
        for candidate in [stem.clone(), format!("{stem}_{folder}")]
            .into_iter()
            .chain((2..).map(|n| format!("{stem}_{folder}_{n}")))
        {
            match self.material_names.get(&candidate) {
                Some(owner) if owner != path => continue,
                _ => {
                    self.material_names
                        .insert(candidate.clone(), path.to_owned());
                    return candidate;
                }
            }
        }
        unreachable!("the candidate list is endless")
    }

    fn material_info(&mut self, path: &str) -> Option<MaterialInfo> {
        if let Some(cached) = self.material_infos.get(path) {
            return cached.clone();
        }
        let info = match read_material(self.content, path) {
            Ok(info) => Some(info),
            Err(e) => {
                // Hammer's tool materials (light blockers, clips…) only exist in the editor, never in the game files.
                if !path.replace('\\', "/").starts_with("materials/tools/") {
                    self.report.warnings.push(format!("material {path}: {e}"));
                }
                None
            }
        };
        self.material_infos.insert(path.to_owned(), info.clone());
        info
    }

    /// Fills a glTF material from a CS2 material. `tint` is the placement's model tint (sRGB RGBA bytes, white =
    /// none): the environment shaders describe theirs in `extras.ofast.environment`, the others get it from
    /// [`Self::fill_model_tint`].
    fn fill_material(&mut self, material: &mut Json, info: &MaterialInfo, tint: [u8; 4]) {
        let rgb = [tint[0], tint[1], tint[2]];
        material["extras"] = json!({
            "ofast": {
                "shader": info.shader,
                "textures": info.textures,
                "floats": info.floats,
                "ints": info.ints,
                "vectors": info.vectors,
            }
        });
        // Static overlays by their blend mode (OverlayBlend); the other shaders by their features.
        let (translucent, alpha_test) = match OverlayBlend::of(info) {
            Some(OverlayBlend::Opaque) => (false, false),
            Some(OverlayBlend::AlphaTest) => (false, true),
            // Plain alpha, and the modes glTF has no blend for (Mod2x: Unreal's material reads F_BLEND_MODE).
            Some(_) => (true, false),
            None => (
                info.flag("F_TRANSLUCENT") || info.flag("F_BLEND_MODE") || info.flag("F_OVERLAY"),
                info.flag("F_ALPHA_TEST"),
            ),
        };
        if translucent {
            material["alphaMode"] = json!("BLEND");
        } else if alpha_test {
            material["alphaMode"] = json!("MASK");
            material["alphaCutoff"] = json!(info.alpha_test_reference());
        }
        if info.flag("F_RENDER_BACKFACES") || info.flag("F_DOUBLE_SIDED") {
            material["doubleSided"] = json!(true);
        }
        if environment::is_environment(&info.shader) {
            // They tint only their tint-masked area, keeping its luminance: no factor can say that.
            self.fill_environment(material, info, rgb, translucent || alpha_test);
            return;
        }
        self.fill_model_tint(material, info, tint, translucent || alpha_test);

        // CS2 shaders (csgo_*): hemi-octahedral normal in RG with roughness in B; metalness in g_tMetalness.G,
        // whose R is the roughness for weapons only (see ValveResourceFormat's complex shader). Otherwise a
        // uniform g_flMetalness.
        let csgo = info.shader.starts_with("csgo_");
        let normal = info.first_texture(NORMAL_PARAMS).map(|(_, p)| p.to_owned());
        if let Some(path) = &normal {
            let id = self.texture(path, TextureRole::Normal { hemi_oct: csgo });
            material["normalTexture"] = json!({ "index": id });
        }
        let metal = info.textures.get("g_tMetalness").cloned();
        let roughness = match (&metal, &normal) {
            (Some(m), _) if info.shader == "csgo_weapon.vfx" => Some((m.clone(), 0)),
            (_, Some(n)) if csgo => Some((n.clone(), 2)),
            _ => None,
        };
        let metalness = metal.map(|m| (m, 1));
        let pbr = &mut material["pbrMetallicRoughness"];
        pbr["metallicFactor"] = json!(if metalness.is_some() {
            1.0
        } else {
            info.floats.get("g_flMetalness").copied().unwrap_or(0.0)
        });
        if roughness.is_some() || metalness.is_some() {
            let has_metal_texture = metalness.is_some();
            let id = self.orm_texture(OrmSource {
                roughness,
                metalness,
            });
            let pbr = &mut material["pbrMetallicRoughness"];
            pbr["metallicRoughnessTexture"] = json!({ "index": id });
            if !has_metal_texture {
                // The texture's B (metalness) is 0; the factor carries the uniform value instead.
                pbr["metallicFactor"] =
                    json!(info.floats.get("g_flMetalness").copied().unwrap_or(0.0));
            }
        }
        if let Some((_, path)) = info.first_texture(AO_PARAMS) {
            let id = self.texture(path, TextureRole::Occlusion);
            material["occlusionTexture"] = json!({ "index": id });
        }
        if let Some(layers) = self.layer_textures(info) {
            material["extras"]["ofast"]["layers"] = layers;
        }
    }

    /// The colour texture and model tint of a material that is not an environment one (see [`tint`]). The tint
    /// multiplies the colour: as `baseColorFactor` when it covers the whole surface. With a tint mask on a colour
    /// texture, it goes into `extras.ofast.tint_mask` for Unreal's material, the factor staying white, so that
    /// other glTF readers see the untinted colour; with [`ExportOptions::bake_model_tints`] it is baked into the
    /// colour instead (a mask on the second UV set cannot be baked: Unreal applies those either way).
    ///
    /// ```text
    /// "tint_mask": {
    ///   "tint": [r, g, b],     linear: lerp(1, rendercolor, g_flModelTintAmount) × g_vColorTint
    ///   "texture": "textures/….png" or null when baked (the mask in R)
    ///   "uv": 0 | 1,           UV set of the mask (g_bUseSecondaryUvForTintMask)
    ///   "baked": false         true: already in baseColorTexture, do not apply again
    /// }
    /// ```
    fn fill_model_tint(
        &mut self,
        material: &mut Json,
        info: &MaterialInfo,
        tint: [u8; 4],
        keep_alpha: bool,
    ) {
        let model_tint = tint::ModelTint::read(info, [tint[0], tint[1], tint[2]]);
        let color = info.first_texture(COLOR_PARAMS).map(|(_, p)| p.to_owned());
        // A mask matters only under a tint, and needs a colour texture to mask.
        let mask = model_tint
            .mask
            .clone()
            .filter(|_| !model_tint.is_white() && color.is_some());
        let baked = mask.is_some() && self.options.bake_model_tints && model_tint.mask_uv == 0;
        if let Some(path) = &color {
            let id = match &mask {
                Some(mask) if baked => {
                    let job = tint::TintJob::new(
                        path.clone(),
                        mask.clone(),
                        model_tint.color,
                        keep_alpha,
                    );
                    self.register(TextureJob::TintColor(job), format!("{}_tint", stem(path)))
                }
                _ => self.texture(path, TextureRole::Color { keep_alpha }),
            };
            material["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": id });
        }
        if let Some(mask) = &mask {
            let file = (!baked).then(|| {
                let id = self.texture(mask, TextureRole::Raw);
                self.texture_file(id)
            });
            material["extras"]["ofast"]["tint_mask"] = json!({
                "tint": model_tint.color,
                "texture": file,
                "uv": model_tint.mask_uv,
                "baked": baked,
            });
        }
        let rgb = if mask.is_some() {
            [1.0; 3]
        } else {
            model_tint.color
        };
        let factor = [rgb[0], rgb[1], rgb[2], f32::from(tint[3]) / 255.0];
        if factor.iter().any(|c| (c - 1.0).abs() > 1e-5) {
            material["pbrMetallicRoughness"]["baseColorFactor"] = json!(factor.to_vec());
        }
    }

    /// `csgo_environment(_blend)` (see [`environment`]): layer 1 becomes the glTF material, with its colour
    /// adjustments baked into the colour, and its occlusion, roughness and metalness into one texture.
    ///
    /// `extras.ofast.environment` describes every layer for Unreal's environment and height-blend materials: its
    /// files (colour, normal, surface, height) and model-tint settings, plus the model tint itself (linear RGB).
    /// The tint is not in the colour (`model_tint_baked` false): Unreal's material applies it, so a colour is baked
    /// once for all the tints placed on it, and other glTF readers see the untinted colour. With
    /// [`ExportOptions::bake_model_tints`] it is baked into the colour instead, one texture per tint. A blend
    /// material's height-blend settings go into `extras.ofast.height_blend`.
    fn fill_environment(
        &mut self,
        material: &mut Json,
        info: &MaterialInfo,
        tint: [u8; 3],
        keep_alpha: bool,
    ) {
        let alpha_test = info.flag("F_ALPHA_TEST");
        let blend = info.shader == environment::ENVIRONMENT_BLEND;
        let layers: Vec<EnvironmentLayer> = (1..=if blend { 2 } else { 1 })
            .map(|n| self.environment_layer(info, n, tint, keep_alpha, alpha_test, blend))
            .collect();
        let layer1 = &layers[0];
        if let Some(id) = layer1.color {
            material["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": id });
        }
        if let Some(id) = layer1.normal {
            material["normalTexture"] = json!({ "index": id });
        }
        if let Some(id) = layer1.surface {
            let pbr = &mut material["pbrMetallicRoughness"];
            pbr["metallicRoughnessTexture"] = json!({ "index": id });
            pbr["metallicFactor"] = json!(1.0);
            material["occlusionTexture"] = json!({ "index": id });
        }
        let mut description = json!({
            // Tints are authored in sRGB; Unreal's vector parameters are linear.
            "model_tint": tint.map(|c| srgb_to_linear(f32::from(c) / 255.0)),
            "model_tint_baked": self.options.bake_model_tints,
        });
        for (index, layer) in layers.iter().enumerate() {
            description[format!("layer{}", index + 1)] = self.layer_json(layer);
        }
        material["extras"]["ofast"]["environment"] = description;
        if blend {
            let blend = environment::HeightBlend::read(info);
            material["extras"]["ofast"]["height_blend"] = json!({
                "softness": blend.softness,
                "zero_point": blend.zero_point,
                "scale": blend.scale,
                "mask_with_height": blend.mask_with_height,
            });
        }
    }

    /// Registers the textures of an environment material's layer `n` (1 or 2). The height texture is written when
    /// Unreal needs it: for a `csgo_environment_blend` (`blend`, its height blend) and for a layer whose model tint
    /// Unreal applies (the tint mask is its G); otherwise it is only baked from.
    fn environment_layer(
        &mut self,
        info: &MaterialInfo,
        n: u8,
        tint: [u8; 3],
        keep_alpha: bool,
        alpha_test: bool,
        blend: bool,
    ) -> EnvironmentLayer {
        let path = |kind: &str| info.textures.get(&format!("g_t{kind}{n}")).cloned();
        let (color, normal, height) = (path("Color"), path("Normal"), path("Height"));
        let layer = environment::Layer::read(info, n);
        // The tint baked into the colour: none (white) unless asked, Unreal's material applies it.
        let (baked_tint, tinted_in_material) = if self.options.bake_model_tints {
            (tint, false)
        } else {
            ([255; 3], tint != [255; 3] && layer.model_tint != 0.0)
        };
        let color_id = color.as_ref().map(|c| {
            let job = environment::ColorJob::new(
                c.clone(),
                height.clone(),
                layer,
                baked_tint,
                keep_alpha,
            );
            match job {
                Some(job) => self.register(TextureJob::EnvColor(job), format!("{}_cc", stem(c))),
                // Nothing changes the colour: the plain texture, shared with any other material using it.
                None => self.texture(c, TextureRole::Color { keep_alpha }),
            }
        });
        let normal_id = normal
            .as_ref()
            .map(|p| self.texture(p, TextureRole::Normal { hemi_oct: true }));
        let surface_id = normal.as_ref().or(color.as_ref()).cloned().map(|from| {
            let job = environment::SurfaceJob::new(
                color.clone(),
                normal.clone(),
                height.clone(),
                layer,
                alpha_test,
            );
            self.register(
                TextureJob::EnvSurface(job),
                format!("{}_surface", stem(&from)),
            )
        });
        let height_id = height
            .as_ref()
            .filter(|_| blend || tinted_in_material)
            .map(|p| self.texture(p, TextureRole::Raw));
        EnvironmentLayer {
            color: color_id,
            normal: normal_id,
            surface: surface_id,
            height: height_id,
            settings: layer,
        }
    }

    /// The PNG file of a registered texture, relative to the glTF, as it goes into `extras.ofast` (a placeholder for
    /// a baked texture, replaced in `finish`).
    fn texture_file(&self, id: usize) -> String {
        format!("textures/{}.png", self.texture_jobs[id].name)
    }

    /// An environment layer for `extras.ofast.environment`: its PNG files (null when it has none, or when Unreal
    /// does not need its height texture) and its model-tint settings — `tint_strength` = `g_flModelTintAmount` ×
    /// `g_bModelTintN`, `tint_mask` = contrast and brightness applied to the height texture's G.
    fn layer_json(&self, layer: &EnvironmentLayer) -> Json {
        let file = |id: Option<usize>| id.map(|id| self.texture_file(id));
        json!({
            "color": file(layer.color),
            "normal": file(layer.normal),
            "surface": file(layer.surface),
            "height": file(layer.height),
            "tint_strength": layer.settings.model_tint,
            "tint_mask": layer.settings.tint_mask,
        })
    }

    /// For two-layer blend materials ([`MaterialInfo::is_vertex_blend`]), the PNG files of both layers and the
    /// blend modulation mask, for Unreal's layer-blend material. Layer weights travel per vertex as `COLOR_0`.
    fn layer_textures(&mut self, info: &MaterialInfo) -> Option<Json> {
        if !info.is_vertex_blend() {
            return None;
        }
        let layer2_color = info.textures.get("g_tLayer2Color")?.clone();
        let hemi_oct = info.shader.starts_with("csgo_");
        let mut file = |param_names: &[&str], role: TextureRole| -> Option<String> {
            let (_, path) = info.first_texture(param_names)?;
            let path = path.to_owned();
            let id = self.texture(&path, role);
            Some(self.texture_file(id))
        };
        let layer1_normal = ["g_tLayer1NormalRoughness", "g_tNormal"];
        let layer2_normal = ["g_tLayer2NormalRoughness", "g_tNormal2"];
        let normal_role = TextureRole::Normal { hemi_oct };
        let layer1 = (
            file(COLOR_PARAMS, TextureRole::Color { keep_alpha: false }),
            file(&layer1_normal, normal_role),
        );
        let layer2 = (
            file(
                &["g_tLayer2Color"],
                TextureRole::Color { keep_alpha: false },
            ),
            file(&layer2_normal, normal_role),
        );
        let modulation = file(&["g_tBlendModulation"], TextureRole::Raw);
        // Per-layer roughness: blue channel of each layer's normal map, as a G-channel texture.
        let mut roughness = |param_names: &[&str]| -> Option<String> {
            let (_, path) = info.first_texture(param_names)?;
            let id = self.orm_texture(OrmSource {
                roughness: Some((path.to_owned(), 2)),
                metalness: None,
            });
            Some(self.texture_file(id))
        };
        let layers = json!({
            "layer1": { "color": layer1.0, "normal": layer1.1, "roughness": roughness(&layer1_normal) },
            "layer2": { "color": layer2.0, "normal": layer2.1, "roughness": roughness(&layer2_normal) },
            "modulation": modulation,
            "softness": info.floats.get("g_flBlendSoftness").copied(),
            // How the modulation texture is read (csgo_lightmappedgeneric): 0 none (plain vertex weight),
            // 1 mask = G / softness = R, 2 mask = G / softness = parameter, 3 mask = A / softness = parameter.
            "fancy_blending": info.ints.get("F_FANCY_BLENDING").copied().unwrap_or(0),
            "source_layer2": layer2_color,
        });
        Some(layers)
    }

    /// Registers a texture for a role and returns its glTF texture index; it is decoded and written in `finish`.
    fn texture(&mut self, path: &str, role: TextureRole) -> usize {
        let name = format!("{}{}", stem(path), role.suffix());
        self.register(TextureJob::Single(path.to_owned(), role), name)
    }

    /// Registers a metallic-roughness texture assembled from channels of other textures.
    fn orm_texture(&mut self, source: OrmSource) -> usize {
        let from = source
            .roughness
            .as_ref()
            .or(source.metalness.as_ref())
            .map_or("texture", |(p, _)| stem(p));
        let name = match (&source.roughness, &source.metalness) {
            (Some(_), Some(_)) => format!("{from}_orm"),
            (Some(_), None) => format!("{from}_rough"),
            _ => format!("{from}_metal"),
        };
        self.register(TextureJob::Orm(source), name)
    }

    /// Registers a texture job under file name `name` (without `.png`) and returns its glTF texture index; the same
    /// job registered again gets the same index.
    fn register(&mut self, job: TextureJob, name: String) -> usize {
        if let Some(&id) = self.texture_ids.get(&job) {
            return id;
        }
        // Two different jobs can derive the same file name (e.g. one roughness map combined with two metalness
        // maps): number the later ones.
        let unique = (1..)
            .map(|n| match n {
                1 => name.clone(),
                n => format!("{name}_{n}"),
            })
            .find(|candidate| !self.texture_names.contains(candidate))
            .expect("the candidate list is endless");
        self.texture_names.insert(unique.clone());
        self.texture_jobs.push(RegisteredTexture {
            job: job.clone(),
            name: unique,
            base: name,
        });
        let id = self.texture_jobs.len() - 1;
        self.texture_ids.insert(job, id);
        id
    }

    /// Decodes or bakes every registered texture (in parallel) and writes each distinct image once (see "Texture
    /// files" in the module docs). Returns the file name of each registered texture; textures with equal pixels get
    /// the same one.
    ///
    /// Threads finish in any order, so the first thread to produce an image writes it to a temporary
    /// `<index>.part`; once all are done the file is renamed after the first texture, in registration order, that
    /// has those pixels — the names never depend on thread timing.
    fn write_textures(&mut self) -> Vec<String> {
        let warnings = Mutex::new(Vec::new());
        let textures_dir = self.out_dir.join("textures");
        let part = |index: usize| textures_dir.join(format!("{index}.part"));
        let content = self.content;
        // Pixel hash → the texture whose thread wrote the image.
        let writers: Mutex<HashMap<blake3::Hash, usize>> = Mutex::default();
        let hashes: Vec<blake3::Hash> = self
            .texture_jobs
            .par_iter()
            .enumerate()
            .map(|(index, texture)| {
                let warn = |e: &dyn std::fmt::Display| {
                    warnings
                        .lock()
                        .expect("lock")
                        .push(format!("texture {}: {e}", texture.name));
                };
                let image = match &texture.job {
                    TextureJob::Single(path, role) => decode_texture(content, path, *role),
                    TextureJob::Orm(source) => assemble_orm(content, source),
                    TextureJob::EnvColor(job) => environment::bake_color(content, job),
                    TextureJob::EnvSurface(job) => environment::bake_surface(content, job),
                    TextureJob::TintColor(job) => tint::bake_color(content, job),
                }
                .unwrap_or_else(|e| {
                    warn(&e);
                    Image::new(1, 1, vec![128, 128, 255, 255]) // neutral stand-in
                });
                let hash = pixel_hash(&image);
                let first = match writers.lock().expect("lock").entry(hash) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(index);
                        true
                    }
                    std::collections::hash_map::Entry::Occupied(_) => false,
                };
                if first && let Err(e) = write_png(&part(index), &image) {
                    warn(&e);
                }
                hash
            })
            .collect();
        let writers = writers.into_inner().expect("lock");

        let mut file_of: HashMap<blake3::Hash, String> = HashMap::new();
        let mut taken: HashSet<String> = HashSet::new();
        for (texture, hash) in self.texture_jobs.iter().zip(&hashes) {
            if file_of.contains_key(hash) {
                self.report.shared_textures += 1;
                continue;
            }
            let preferred = if texture.job.is_baked() {
                format!("{}_{}", texture.base, &hash.to_hex()[..8])
            } else {
                texture.name.clone()
            };
            // A clash only happens when a CS2 texture is itself named like another one's bake, or two hashes share
            // their first 8 digits: number the later file.
            let file = (1..)
                .map(|n| match n {
                    1 => format!("{preferred}.png"),
                    n => format!("{preferred}_{n}.png"),
                })
                .find(|candidate| !taken.contains(candidate))
                .expect("the candidate list is endless");
            if let Err(e) = fs::rename(part(writers[hash]), textures_dir.join(&file)) {
                warnings
                    .lock()
                    .expect("lock")
                    .push(format!("texture {file}: {e}"));
            }
            taken.insert(file.clone());
            file_of.insert(*hash, file);
        }
        self.report
            .warnings
            .extend(warnings.into_inner().expect("lock"));
        self.report.textures = file_of.len();
        hashes.iter().map(|hash| file_of[hash].clone()).collect()
    }

    /// Writes every registered texture, then the `.bin` and `.gltf` files, then removes texture files no glTF in the
    /// folder uses any more (see "Texture files" in the module docs).
    pub(crate) fn finish(
        mut self,
        name: &str,
        roots: Vec<usize>,
    ) -> Result<ExportReport, ExportError> {
        let files = self.write_textures();
        // One glTF image and texture per file, in order of first use; materials are pointed at them.
        let mut images: Vec<Json> = Vec::new();
        let mut textures: Vec<Json> = Vec::new();
        let mut texture_of_file: HashMap<&str, usize> = HashMap::new();
        let texture_of: Vec<usize> = files
            .iter()
            .map(|file| {
                *texture_of_file.entry(file).or_insert_with(|| {
                    let uri = format!("textures/{file}");
                    images.push(json!({ "uri": uri, "name": file.trim_end_matches(".png") }));
                    textures.push(json!({ "source": images.len() - 1, "sampler": 0 }));
                    textures.len() - 1
                })
            })
            .collect();
        // Placeholder file names in `extras.ofast` → the files written.
        let renamed: HashMap<String, String> = self
            .texture_jobs
            .iter()
            .zip(&files)
            .map(|(texture, file)| {
                (
                    format!("textures/{}.png", texture.name),
                    format!("textures/{file}"),
                )
            })
            .filter(|(from, to)| from != to)
            .collect();
        for material in &mut self.materials {
            remap_texture_indices(material, &texture_of);
            if let Some(extras) = material.get_mut("extras") {
                rename_files(extras, &renamed);
            }
        }

        let bin_name = format!("{name}.bin");
        fs::write(self.out_dir.join(&bin_name), &self.bin)?;
        let mut document = json!({
            "asset": { "version": "2.0", "generator": "OFAST (https://github.com/synk1337/OFAST)" },
            "scene": 0,
            "scenes": [{ "name": name, "nodes": roots }],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "materials": self.materials,
            "accessors": self.accessors,
            "bufferViews": self.buffer_views,
            "buffers": [{ "uri": bin_name, "byteLength": self.bin.len() }],
        });
        if !textures.is_empty() {
            document["textures"] = json!(textures);
            document["images"] = json!(images);
            // Linear filtering with mipmaps, repeat wrapping.
            document["samplers"] =
                json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
        }
        if !self.lights.is_empty() {
            document["extensionsUsed"] = json!(["KHR_lights_punctual"]);
            document["extensions"] = json!({ "KHR_lights_punctual": { "lights": self.lights } });
        }
        let gltf = self.out_dir.join(format!("{name}.gltf"));
        fs::write(
            &gltf,
            serde_json::to_string(&document).expect("JSON serializes"),
        )?;
        self.report.gltf = gltf;
        self.report.removed_textures =
            remove_stale_textures(&self.out_dir, &mut self.report.warnings);
        Ok(self.report)
    }
}

/// File stem of a CS2 path (`materials/a/b_color_tga_1234.vtex` → `b_color_tga_1234`).
fn stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("texture")
}

/// BLAKE3 of an image's size and pixels: equal hashes mean equal images.
fn pixel_hash(image: &Image) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(image.width as u64).to_le_bytes());
    hasher.update(&(image.height as u64).to_le_bytes());
    hasher.update(&image.rgba);
    hasher.finalize()
}

/// Points a glTF material's texture slots (`baseColorTexture`, `normalTexture`, … and those in
/// `pbrMetallicRoughness`) from registered texture indices to the merged ones: `texture_of[registered]`.
fn remap_texture_indices(material: &mut Json, texture_of: &[usize]) {
    fn remap(object: &mut Json, texture_of: &[usize]) {
        let Some(object) = object.as_object_mut() else {
            return;
        };
        for (key, slot) in object.iter_mut() {
            if key.ends_with("Texture")
                && let Some(index) = slot.get("index").and_then(Json::as_u64)
            {
                slot["index"] = json!(texture_of[index as usize]);
            }
        }
    }
    remap(material, texture_of);
    if let Some(pbr) = material.get_mut("pbrMetallicRoughness") {
        remap(pbr, texture_of);
    }
}

/// Replaces every string in `value` (recursively) that is a key of `renamed` by its value.
fn rename_files(value: &mut Json, renamed: &HashMap<String, String>) {
    match value {
        Json::String(s) => {
            if let Some(new) = renamed.get(s.as_str()) {
                *s = new.clone();
            }
        }
        Json::Array(items) => items.iter_mut().for_each(|v| rename_files(v, renamed)),
        Json::Object(map) => map.values_mut().for_each(|v| rename_files(v, renamed)),
        _ => {}
    }
}

/// Deletes from `<out_dir>/textures` the PNGs that no `.gltf` in `out_dir` references, and the `.part` files of an
/// interrupted export; returns how many files went. Several exports may share the folder (a model next to a map),
/// so every glTF there is read — if one cannot be, nothing is deleted and a warning says why. Only files directly in
/// the folder are touched.
fn remove_stale_textures(out_dir: &Path, warnings: &mut Vec<String>) -> usize {
    let mut used: HashSet<String> = HashSet::new();
    let entries = match fs::read_dir(out_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warnings.push(format!("stale textures not checked: {e}"));
            return 0;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "gltf") {
            continue;
        }
        let document = fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).map_err(|e| e.to_string()));
        match document {
            Ok(document) => {
                for image in document["images"].as_array().into_iter().flatten() {
                    if let Some(file) = image["uri"]
                        .as_str()
                        .and_then(|uri| uri.strip_prefix("textures/"))
                    {
                        used.insert(file.to_owned());
                    }
                }
            }
            Err(e) => {
                warnings.push(format!(
                    "stale textures not removed: {} unreadable ({e})",
                    path.display()
                ));
                return 0;
            }
        }
    }
    let Ok(files) = fs::read_dir(out_dir.join("textures")) else {
        return 0;
    };
    let mut removed = 0;
    for entry in files.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale = match path.extension().and_then(|ext| ext.to_str()) {
            Some("png") => !used.contains(&name),
            Some("part") => true,
            _ => false,
        };
        if stale && entry.file_type().is_ok_and(|t| t.is_file()) {
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(e) => warnings.push(format!("stale texture {name} not removed: {e}")),
            }
        }
    }
    removed
}

pub(crate) fn read_material(content: &Content, path: &str) -> Result<MaterialInfo, ExportError> {
    let data = content.read_resource(path)?;
    let resource = Resource::parse(&data)?;
    let root = resource.kv3(BlockKind::DATA)?.root;
    let mut info = MaterialInfo {
        shader: root
            .get("m_shaderName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        ..MaterialInfo::default()
    };
    for param in root
        .get("m_textureParams")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if let (Some(name), Some(value)) = (
            param.get("m_name").and_then(Value::as_str),
            param.get("m_pValue").and_then(Value::as_str),
        ) {
            info.textures.insert(name.to_owned(), value.to_owned());
        }
    }
    for param in root
        .get("m_intParams")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if let (Some(name), Some(value)) = (
            param.get("m_name").and_then(Value::as_str),
            param.get("m_nValue").and_then(Value::as_i64),
        ) {
            info.ints.insert(name.to_owned(), value);
        }
    }
    for param in root
        .get("m_floatParams")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if let (Some(name), Some(value)) = (
            param.get("m_name").and_then(Value::as_str),
            param.get("m_flValue").and_then(Value::as_f64),
        ) {
            info.floats.insert(name.to_owned(), value);
        }
    }
    for param in root
        .get("m_vectorParams")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if let (Some(name), Some(value)) = (
            param.get("m_name").and_then(Value::as_str),
            param.get("m_value").and_then(Value::as_array),
        ) {
            let mut vector = [0.0; 4];
            for (v, c) in vector.iter_mut().zip(value) {
                *v = c.as_f64().unwrap_or_default();
            }
            info.vectors.insert(name.to_owned(), vector);
        }
    }
    Ok(info)
}

/// Reads and decodes a CS2 texture to RGBA8 without any role-specific conversion.
pub(crate) fn decode_raw(content: &Content, path: &str) -> Result<(Texture, Image), ExportError> {
    let data = content.read_resource(path)?;
    let resource = Resource::parse(&data)?;
    let header = Texture::parse(&resource)?;
    let image = header.decode_rgba8(&data)?;
    Ok((header, image))
}

/// Decodes a CS2 texture for a role (see [`TextureRole`]).
fn decode_texture(content: &Content, path: &str, role: TextureRole) -> Result<Image, ExportError> {
    let (header, mut image) = decode_raw(content, path)?;
    let opaque = |image: &mut Image| {
        image
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|px| px[3] = 255)
    };
    match role {
        TextureRole::Color { keep_alpha } => {
            if !keep_alpha {
                // Opaque CS2 colour textures use alpha as a mask channel, not opacity.
                opaque(&mut image);
            }
        }
        TextureRole::Occlusion | TextureRole::Raw => {}
        TextureRole::Normal { hemi_oct } => {
            if hemi_oct || header.conversions.hemi_oct_normal {
                texture::decode_hemi_oct_normal(&mut image);
            } else if header.format == Format::Ati2n {
                texture::reconstruct_normal_z(&mut image);
            }
            opaque(&mut image);
        }
    }
    Ok(image)
}

/// Builds a glTF metallic-roughness image (R 255, G roughness, B metalness, A 255) from channels of CS2 textures,
/// sampling the smaller source at the larger one's size.
fn assemble_orm(content: &Content, source: &OrmSource) -> Result<Image, ExportError> {
    let load = |s: &Option<(String, usize)>| -> Result<Option<(Image, usize)>, ExportError> {
        match s {
            Some((path, channel)) => Ok(Some((decode_raw(content, path)?.1, *channel))),
            None => Ok(None),
        }
    };
    let roughness = load(&source.roughness)?;
    let metalness = load(&source.metalness)?;
    let (width, height) = [&roughness, &metalness]
        .iter()
        .filter_map(|s| s.as_ref().map(|(img, _)| (img.width, img.height)))
        .max_by_key(|(w, h)| w * h)
        .unwrap_or((1, 1));
    // Nearest sample of `channel` at output pixel (x, y), or `default` without a source.
    let sample = |s: &Option<(Image, usize)>, x: usize, y: usize, default: u8| -> u8 {
        match s {
            Some((img, channel)) => {
                let sx = x * img.width / width;
                let sy = y * img.height / height;
                img.rgba[(sy * img.width + sx) * 4 + channel]
            }
            None => default,
        }
    };
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                255,
                sample(&roughness, x, y, 255),
                sample(&metalness, x, y, 0),
                255,
            ]);
        }
    }
    Ok(Image::new(width, height, rgba))
}

/// Keeps only triangles with three distinct vertices and a non-zero area (in Source units²).
fn without_degenerate_triangles(positions: &[[f32; 3]], indices: &[u32]) -> Vec<u32> {
    indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|&&[a, b, c]| {
            if a == b || b == c || a == c {
                return false;
            }
            let [pa, pb, pc] = [a, b, c].map(|i| positions[i as usize]);
            let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            // Twice the area; 1e-6 square inches is far below anything visible.
            (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() > 1e-6
        })
        .flatten()
        .copied()
        .collect()
}

/// Converts a Source-space 3×4 placement to a column-major glTF 4×4 matrix: `C · M · Cᵀ`, translation `C · t`,
/// where `C` is the Source → glTF axis change (see [`crate::coords`]).
pub(crate) fn gltf_matrix(t: &Transform) -> [f32; 16] {
    // Columns of the 3×3 part in glTF space: image of each glTF axis.
    let axis = |gltf_axis: [f32; 3]| -> [f32; 3] {
        // glTF axis → Source axis (Cᵀ), apply M, back to glTF (C).
        let src = [gltf_axis[0], -gltf_axis[2], gltf_axis[1]];
        let moved = [0, 1, 2].map(|r| t[r][0] * src[0] + t[r][1] * src[1] + t[r][2] * src[2]);
        coords::source_dir_to_gltf(moved)
    };
    let x = axis([1.0, 0.0, 0.0]);
    let y = axis([0.0, 1.0, 0.0]);
    let z = axis([0.0, 0.0, 1.0]);
    let p = coords::source_point_to_gltf([t[0][3], t[1][3], t[2][3]]);
    [
        x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, p[0], p[1], p[2], 1.0,
    ]
}

/// A rotation matrix (column-major 4×4, no translation) whose local −Z points along `dir` (glTF space), as
/// `KHR_lights_punctual` lights shine down their −Z axis.
fn light_matrix(dir: [f32; 3]) -> [f32; 16] {
    let normalize = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
        v.map(|c| c / l)
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let z = normalize(dir.map(|c| -c));
    let up = if z[1].abs() > 0.99 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let x = normalize(cross(up, z));
    let y = cross(z, x);
    [
        x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// sRGB (0–1) to linear.
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Writes an RGBA8 image as PNG, without its alpha channel when every alpha is 255.
///
/// Lossless; zlib's default level with adaptive filters (a filter chosen per row). Measured on de_inferno's
/// textures against the earlier RGBA + fast compression: a 2048² baked colour 12.3 → 9.5 MB (16.1 → 10.2 MB for a
/// noisier one), a 4096² surface texture 15.9 → 5.1 MB, for about 0.3–0.9 s instead of 20–70 ms per texture,
/// spread over all cores. Decoded BC textures are noisy, so a colour texture stays near 60 % of its raw size: the big
/// savings come from writing each image once (see "Texture files" in the module docs).
pub fn write_png(path: &Path, image: &Image) -> Result<(), ExportError> {
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        image.width as u32,
        image.height as u32,
    );
    let opaque = image.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let rgb: Vec<u8>;
    let data = if opaque {
        rgb = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect();
        encoder.set_color(png::ColorType::Rgb);
        &rgb
    } else {
        encoder.set_color(png::ColorType::Rgba);
        &image.rgba
    };
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Default);
    encoder.set_adaptive_filter(png::AdaptiveFilterType::Adaptive);
    let mut writer = encoder
        .write_header()
        .map_err(|e| ExportError::Png(e.to_string()))?;
    writer
        .write_image_data(data)
        .map_err(|e| ExportError::Png(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{IDENTITY, entity_transform};

    #[test]
    fn identity_placement_is_identity_in_gltf() {
        let m = gltf_matrix(&IDENTITY);
        assert_eq!(
            m,
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0
            ]
        );
    }

    #[test]
    fn placement_matches_converting_points() {
        // Transforming a point in Source then converting must equal converting then applying the glTF matrix.
        let t = entity_transform([100.0, -50.0, 10.0], [15.0, 120.0, 5.0], [1.0, 2.0, 0.5]);
        let point = [3.0f32, -7.0, 11.0];
        let moved = [0, 1, 2]
            .map(|r| t[r][0] * point[0] + t[r][1] * point[1] + t[r][2] * point[2] + t[r][3]);
        let expected = coords::source_point_to_gltf(moved);
        let m = gltf_matrix(&t);
        let g = coords::source_point_to_gltf(point);
        let actual = [0, 1, 2].map(|r| m[r] * g[0] + m[4 + r] * g[1] + m[8 + r] * g[2] + m[12 + r]);
        for k in 0..3 {
            assert!(
                (actual[k] - expected[k]).abs() < 1e-4,
                "{actual:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn lights_point_down_their_minus_z() {
        let dir = [0.3f32, -0.8, 0.52];
        let m = light_matrix(dir);
        let len = (dir.iter().map(|c| c * c).sum::<f32>()).sqrt();
        // −Z column equals the normalized direction.
        for k in 0..3 {
            assert!((-m[8 + k] - dir[k] / len).abs() < 1e-5);
        }
    }

    /// A fresh, empty folder under the system temp folder.
    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ofast-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("textures")).expect("temp folder");
        dir
    }

    #[test]
    fn stale_textures_go_unless_a_gltf_in_the_folder_uses_them() {
        let dir = temp_folder("stale");
        let gltf = |images: &[&str]| {
            let images: Vec<Json> = images
                .iter()
                .map(|f| json!({ "uri": format!("textures/{f}") }))
                .collect();
            json!({ "images": images }).to_string()
        };
        // A map and a model exported into the same folder; an older export left old.png and an interrupted one 7.part.
        fs::write(dir.join("map.gltf"), gltf(&["a.png", "shared.png"])).expect("write");
        fs::write(dir.join("model.gltf"), gltf(&["b.png", "shared.png"])).expect("write");
        for file in [
            "a.png",
            "b.png",
            "shared.png",
            "old.png",
            "7.part",
            "notes.txt",
        ] {
            fs::write(dir.join("textures").join(file), b"x").expect("write");
        }
        let mut warnings = Vec::new();
        assert_eq!(remove_stale_textures(&dir, &mut warnings), 2);
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut left: Vec<String> = fs::read_dir(dir.join("textures"))
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["a.png", "b.png", "notes.txt", "shared.png"]);

        // A glTF that cannot be read could be using anything: nothing is removed.
        fs::write(dir.join("broken.gltf"), "{").expect("write");
        fs::write(dir.join("textures").join("old.png"), b"x").expect("write");
        assert_eq!(remove_stale_textures(&dir, &mut warnings), 0);
        assert_eq!(warnings.len(), 1);
        assert!(dir.join("textures").join("old.png").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn opaque_images_are_written_without_alpha() {
        let dir = temp_folder("png");
        let read = |path: &Path| {
            let decoder = png::Decoder::new(fs::File::open(path).expect("open"));
            let mut reader = decoder.read_info().expect("PNG");
            let mut pixels = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut pixels).expect("frame");
            pixels.truncate(info.buffer_size());
            (info.color_type, pixels)
        };
        let opaque = Image::new(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 255]);
        let path = dir.join("opaque.png");
        write_png(&path, &opaque).expect("write");
        assert_eq!(
            read(&path),
            (png::ColorType::Rgb, vec![10, 20, 30, 40, 50, 60])
        );
        let translucent = Image::new(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 128]);
        write_png(&path, &translucent).expect("write");
        assert_eq!(
            read(&path),
            (png::ColorType::Rgba, translucent.rgba.clone())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merged_textures_are_renamed_in_materials() {
        let mut material = json!({
            "pbrMetallicRoughness": { "baseColorTexture": { "index": 2 }, "metallicRoughnessTexture": { "index": 1 } },
            "normalTexture": { "index": 0 },
            "extras": { "ofast": { "environment": { "layer2": { "color": "textures/a_cc.png" } }, "textures": {
                "g_tColor1": "materials/a_cc.png"
            } } },
        });
        remap_texture_indices(&mut material, &[0, 1, 0]);
        assert_eq!(
            material["pbrMetallicRoughness"]["baseColorTexture"]["index"],
            0
        );
        assert_eq!(
            material["pbrMetallicRoughness"]["metallicRoughnessTexture"]["index"],
            1
        );
        let renamed = HashMap::from([(
            "textures/a_cc.png".to_owned(),
            "textures/a_cc_0badcafe.png".to_owned(),
        )]);
        rename_files(&mut material["extras"], &renamed);
        let ofast = &material["extras"]["ofast"];
        assert_eq!(
            ofast["environment"]["layer2"]["color"],
            "textures/a_cc_0badcafe.png"
        );
        // CS2 paths are not file names, even when they look alike.
        assert_eq!(ofast["textures"]["g_tColor1"], "materials/a_cc.png");
    }

    /// A `csgo_environment` material with a colour correction and its layer's model tint on (the shader's default),
    /// filled once per tint (white = none) by a writer with `options`. Returns the materials and the number of
    /// colour bakes registered.
    fn tinted_environment(options: ExportOptions, tints: &[[u8; 3]]) -> (Vec<Json>, usize) {
        let info = MaterialInfo {
            shader: environment::ENVIRONMENT.into(),
            textures: [
                ("g_tColor1", "materials/plaster_color.vtex"),
                ("g_tHeight1", "materials/plaster_height.vtex"),
                ("g_tNormal1", "materials/plaster_normal.vtex"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .into(),
            floats: [
                ("g_fTextureColorBrightness1".to_owned(), 1.25),
                ("g_fTintMaskContrast1".to_owned(), 2.0),
            ]
            .into(),
            ..Default::default()
        };
        let content = Content::new();
        let dir = temp_folder(&format!("tint-{}", options.bake_model_tints));
        let mut writer = GltfWriter::new(&content, &dir, options).expect("writer");
        let materials = tints
            .iter()
            .map(|&[r, g, b]| {
                let mut material = json!({});
                writer.fill_material(&mut material, &info, [r, g, b, 255]);
                material
            })
            .collect();
        let bakes = writer
            .texture_jobs
            .iter()
            .filter(|t| matches!(t.job, TextureJob::EnvColor(_)))
            .count();
        let _ = fs::remove_dir_all(&dir);
        (materials, bakes)
    }

    #[test]
    fn model_tints_are_left_to_the_material_unless_baking_is_asked() {
        let tints = [[255; 3], [230, 120, 130], [120, 160, 230]];
        let (materials, bakes) = tinted_environment(ExportOptions::default(), &tints);
        // One colour bake (the brightness) for all three tints.
        assert_eq!(bakes, 1);
        let pink = &materials[1]["extras"]["ofast"]["environment"];
        assert_eq!(pink["model_tint_baked"], false);
        let linear = tints[1].map(|c| f64::from(srgb_to_linear(f32::from(c) / 255.0)));
        assert_eq!(pink["model_tint"], json!(linear));
        assert_eq!(pink["layer1"]["tint_strength"], 1.0);
        assert_eq!(pink["layer1"]["tint_mask"], json!([2.0, 1.0]));
        // The tint mask is the height texture's G: written where Unreal tints, not for the untinted variant.
        assert!(pink["layer1"]["height"].is_string());
        let white = &materials[0]["extras"]["ofast"]["environment"];
        assert!(white["layer1"]["height"].is_null());
        let channels = white["model_tint"].as_array().expect("RGB");
        assert!(
            channels
                .iter()
                .all(|c| (c.as_f64().expect("number") - 1.0).abs() < 1e-6)
        );
        // Every variant shows the same (untinted) colour to other glTF readers.
        let color = |m: &Json| m["pbrMetallicRoughness"]["baseColorTexture"]["index"].clone();
        assert!(materials.iter().all(|m| color(m) == color(&materials[0])));

        let baking = ExportOptions {
            bake_model_tints: true,
        };
        let (materials, bakes) = tinted_environment(baking, &tints);
        assert_eq!(bakes, 3);
        let pink = &materials[1]["extras"]["ofast"]["environment"];
        assert_eq!(pink["model_tint_baked"], true);
        assert!(pink["layer1"]["height"].is_null(), "only baked from");
    }

    /// de_inferno's potted tree material (`csgo_foliage`, `F_TINT_MASK`), filled once per placement tint by a
    /// writer with `options`, the mask on UV set `uv`. Returns the materials and the registered texture jobs.
    fn tinted_foliage(
        options: ExportOptions,
        tints: &[[u8; 3]],
        uv: i64,
    ) -> (Vec<Json>, Vec<TextureJob>) {
        let info = MaterialInfo {
            shader: "csgo_foliage.vfx".into(),
            textures: [
                ("g_tColor", "models/cress_plant_kit_01_color.vtex"),
                ("g_tTintMask", "models/cress_plant_kit_01_tintmask.vtex"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .into(),
            ints: [
                ("F_TINT_MASK".to_owned(), 1),
                ("F_ALPHA_TEST".to_owned(), 1),
                ("g_bUseSecondaryUvForTintMask".to_owned(), uv),
            ]
            .into(),
            ..Default::default()
        };
        let content = Content::new();
        let dir = temp_folder(&format!("foliage-{}-{uv}", options.bake_model_tints));
        let mut writer = GltfWriter::new(&content, &dir, options).expect("writer");
        let materials = tints
            .iter()
            .map(|&[r, g, b]| {
                let mut material = json!({});
                writer.fill_material(&mut material, &info, [r, g, b, 255]);
                material
            })
            .collect();
        let jobs = writer.texture_jobs.iter().map(|t| t.job.clone()).collect();
        let _ = fs::remove_dir_all(&dir);
        (materials, jobs)
    }

    #[test]
    fn tint_masks_go_to_extras_and_leave_the_colour_untinted() {
        let tints = [[255; 3], [255, 0, 0]];
        let (materials, jobs) = tinted_foliage(ExportOptions::default(), &tints, 0);
        let (white, red) = (&materials[0], &materials[1]);
        // No factor: it would redden the whole plant (CS2 reddens the flowers under the mask only).
        for material in [white, red] {
            assert!(material["pbrMetallicRoughness"]["baseColorFactor"].is_null());
        }
        let color = |m: &Json| m["pbrMetallicRoughness"]["baseColorTexture"]["index"].clone();
        assert_eq!(color(white), color(red));
        let extras = &red["extras"]["ofast"]["tint_mask"];
        assert_eq!(extras["tint"], json!([1.0, 0.0, 0.0]));
        assert_eq!(extras["uv"], 0);
        assert_eq!(extras["baked"], false);
        assert_eq!(
            extras["texture"],
            "textures/cress_plant_kit_01_tintmask_raw.png"
        );
        // Untinted: nothing to apply, no mask written.
        assert!(white["extras"]["ofast"]["tint_mask"].is_null());
        assert!(!jobs.iter().any(TextureJob::is_baked));
        assert_eq!(jobs.len(), 2, "colour (with alpha) and mask: {jobs:?}");

        // Baked for other glTF readers: the red variant gets its own colour, the mask stays out.
        let baking = ExportOptions {
            bake_model_tints: true,
        };
        let (materials, jobs) = tinted_foliage(baking, &tints, 0);
        assert_ne!(color(&materials[0]), color(&materials[1]));
        let extras = &materials[1]["extras"]["ofast"]["tint_mask"];
        assert_eq!(extras["baked"], true);
        assert!(extras["texture"].is_null());
        assert_eq!(
            jobs.iter()
                .filter(|j| matches!(j, TextureJob::TintColor(_)))
                .count(),
            1
        );
        // A mask on the second UV set has another layout than the colour: never baked, Unreal applies it.
        let (materials, _) = tinted_foliage(baking, &tints, 1);
        let extras = &materials[1]["extras"]["ofast"]["tint_mask"];
        assert_eq!(
            (extras["baked"].clone(), extras["uv"].clone()),
            (json!(false), json!(1))
        );
    }

    #[test]
    fn whole_surface_tints_stay_factors() {
        let info = |shader: &str, ints: &[(&str, i64)]| MaterialInfo {
            shader: shader.into(),
            textures: [("g_tColor".to_owned(), "props/door_color.vtex".to_owned())].into(),
            ints: ints.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            floats: [("g_flModelTintAmount".to_owned(), 0.5)].into(),
            ..Default::default()
        };
        let content = Content::new();
        let dir = temp_folder("factors");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let mut factor = |info: &MaterialInfo| {
            let mut material = json!({});
            writer.fill_material(&mut material, info, [0, 0, 0, 255]);
            material["pbrMetallicRoughness"]["baseColorFactor"].clone()
        };
        // Half the amount: halfway to black.
        assert_eq!(
            factor(&info("csgo_complex.vfx", &[])),
            json!([0.5, 0.5, 0.5, 1.0])
        );
        assert!(factor(&info("csgo_vertexlitgeneric.vfx", &[("F_NOTINT", 1)])).is_null());
        // A tint mask without its feature does nothing.
        assert_eq!(
            factor(&info(
                "csgo_complex.vfx",
                &[("g_bUseSecondaryUvForTintMask", 1)]
            )),
            json!([0.5, 0.5, 0.5, 1.0])
        );
        // Shaders not checked keep the placement tint as it is.
        assert_eq!(
            factor(&info("csgo_static_overlay.vfx", &[])),
            json!([0.0, 0.0, 0.0, 1.0])
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Fills an untinted material of `shader` with a colour texture, the integer parameters `ints` and the floats
    /// `floats`. Returns the glTF material and the role its colour texture was registered with.
    fn blend_test_material(
        writer: &mut GltfWriter,
        shader: &str,
        ints: &[(&str, i64)],
        floats: &[(&str, f64)],
    ) -> (Json, Option<TextureRole>) {
        let info = MaterialInfo {
            shader: shader.into(),
            textures: [(
                "g_tColor".to_owned(),
                "materials/overlays/grime_color.vtex".to_owned(),
            )]
            .into(),
            ints: ints.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            floats: floats.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            ..Default::default()
        };
        let mut material = json!({});
        writer.fill_material(&mut material, &info, [255; 4]);
        let index = material["pbrMetallicRoughness"]["baseColorTexture"]["index"].as_u64();
        let role = index.and_then(|i| match &writer.texture_jobs[i as usize].job {
            TextureJob::Single(_, role) => Some(*role),
            _ => None,
        });
        (material, role)
    }

    /// glTF's alpha mode and cutoff of a material (`OPAQUE` when not written).
    fn alpha_mode(material: &Json) -> (String, Option<f64>) {
        (
            material["alphaMode"]
                .as_str()
                .unwrap_or("OPAQUE")
                .to_owned(),
            material["alphaCutoff"].as_f64(),
        )
    }

    #[test]
    fn static_overlays_are_blended_by_their_blend_mode() {
        let content = Content::new();
        let dir = temp_folder("overlay-blend");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let mut fill = |ints: &[(&str, i64)], floats: &[(&str, f64)]| {
            blend_test_material(&mut writer, STATIC_OVERLAY, ints, floats)
        };
        let opaque = (String::from("OPAQUE"), None);
        let blend = (String::from("BLEND"), None);
        let keep_alpha = |keep_alpha| Some(TextureRole::Color { keep_alpha });

        // de_nuke's bombsite signs: opaque and lit, their colour without alpha. No F_BLEND_MODE is mode 0.
        for ints in [&[("F_LIT", 1)][..], &[("F_BLEND_MODE", 0), ("F_LIT", 1)]] {
            let (material, role) = fill(ints, &[]);
            assert_eq!(
                (alpha_mode(&material), role),
                (opaque.clone(), keep_alpha(false))
            );
        }
        let (material, role) = fill(&[("F_BLEND_MODE", 1), ("F_LIT", 1)], &[]);
        assert_eq!(
            (alpha_mode(&material), role),
            (blend.clone(), keep_alpha(true))
        );
        // Alpha-tested: cut at the material's own reference (de_nuke's plant logo: 0.4), 0.5 when it keeps the
        // shader's default.
        let (material, role) = fill(
            &[("F_BLEND_MODE", 2), ("F_LIT", 1)],
            &[("g_flAlphaTestReference", 0.4)],
        );
        assert_eq!(
            (alpha_mode(&material), role),
            (("MASK".into(), Some(0.4)), keep_alpha(true))
        );
        assert_eq!(
            alpha_mode(&fill(&[("F_BLEND_MODE", 2)], &[]).0),
            ("MASK".into(), Some(0.5))
        );
        // Mod2x (de_nuke's grime) and the modes not checked: glTF has no such blend, BLEND with the alpha.
        for mode in 3..=6 {
            let (material, role) = fill(&[("F_BLEND_MODE", mode)], &[]);
            assert_eq!(
                (alpha_mode(&material), role),
                (blend.clone(), keep_alpha(true)),
                "mode {mode}"
            );
            assert_eq!(material["extras"]["ofast"]["ints"]["F_BLEND_MODE"], mode);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_shaders_keep_their_features_and_cut_at_their_reference() {
        let content = Content::new();
        let dir = temp_folder("alpha-test");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let mut mode = |shader: &str, ints: &[(&str, i64)], floats: &[(&str, f64)]| {
            alpha_mode(&blend_test_material(&mut writer, shader, ints, floats).0)
        };
        assert_eq!(
            mode(
                "csgo_foliage.vfx",
                &[("F_ALPHA_TEST", 1)],
                &[("g_flAlphaTestReference", 0.2)]
            ),
            ("MASK".into(), Some(0.2))
        );
        assert_eq!(
            mode("csgo_vertexlitgeneric.vfx", &[("F_ALPHA_TEST", 1)], &[]),
            ("MASK".into(), Some(0.5))
        );
        // A reference without the feature changes nothing.
        assert_eq!(
            mode("csgo_complex.vfx", &[], &[("g_flAlphaTestReference", 0.2)]),
            ("OPAQUE".into(), None)
        );
        // csgo_unlitgeneric numbers its own F_BLEND_MODE (not checked): plain BLEND as before.
        for blend_mode in [1, 2, 4] {
            assert_eq!(
                mode(
                    "csgo_unlitgeneric.vfx",
                    &[("F_BLEND_MODE", blend_mode)],
                    &[]
                ),
                ("BLEND".into(), None)
            );
        }
        assert_eq!(
            mode(
                "csgo_lightmappedgeneric.vfx",
                &[("F_OVERLAY", 1), ("F_TRANSLUCENT", 1)],
                &[]
            ),
            ("BLEND".into(), None)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// de_nuke's road paint: a white `csgo_lightmappedgeneric` overlay (`hr_nuke_parkingstripe_single`, colour =
    /// translucency only) that Hammer paints yellow through its draw call's `m_vTintColor`. One triangle drawn three
    /// times: with that yellow, with a scuff decal's `m_flAlpha`, and untinted.
    #[test]
    fn draw_call_tints_colour_their_material() {
        use crate::model::{Attribute, VertexBuffer, dxgi};

        let path = "materials/de_nuke/hr_nuke_parkingstripe_single.vmat";
        let info = MaterialInfo {
            shader: "csgo_lightmappedgeneric.vfx".into(),
            textures: [(
                "g_tColor".to_owned(),
                "materials/de_nuke/hr_nuke_parkingstripe_single_trans_psd_3aa6a8d1.vtex".to_owned(),
            )]
            .into(),
            ints: [("F_OVERLAY".to_owned(), 1), ("F_TRANSLUCENT".to_owned(), 1)].into(),
            floats: [("g_flModelTintAmount".to_owned(), 1.0)].into(),
            ..Default::default()
        };
        let draw = |tint: [f32; 3], alpha: f32| DrawCall {
            material: path.into(),
            vertex_buffers: vec![0],
            index_buffer: 0,
            base_vertex: 0,
            start_index: 0,
            index_count: 3,
            compressed_normal_tangent: false,
            tint,
            alpha,
        };
        let positions = [[0.0f32; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = Mesh {
            name: "overlay".into(),
            mesh_index: 0,
            vertex_buffers: vec![VertexBuffer {
                count: 3,
                stride: 12,
                attributes: vec![Attribute {
                    semantic: "POSITION".into(),
                    semantic_index: 0,
                    format: dxgi::R32G32B32_FLOAT,
                    offset: 0,
                }],
                data: positions
                    .iter()
                    .flatten()
                    .flat_map(|f| f.to_le_bytes())
                    .collect(),
            }],
            index_buffers: vec![vec![0, 1, 2]],
            draw_calls: vec![
                draw([0.851_251_7, 0.459_079_9, 0.046_148_84], 1.0),
                draw([1.0; 3], 0.25),
                draw([1.0; 3], 1.0),
            ],
        };
        let content = Content::new();
        let dir = temp_folder("draw-tint");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        writer
            .material_infos
            .insert(path.to_owned(), Some(info.clone()));
        let draws: Vec<&DrawCall> = mesh.draw_calls.iter().collect();
        // The scene object places it untinted (m_vTintColor 1, 1, 1, 1).
        let gltf_mesh = writer
            .mesh("overlay", &mesh, &draws, [1.0; 4], 0.0)
            .expect("mesh")
            .expect("triangles");
        let materials: Vec<Json> = writer.meshes[gltf_mesh]["primitives"]
            .as_array()
            .expect("primitives")
            .iter()
            .map(|p| writer.materials[p["material"].as_u64().expect("material") as usize].clone())
            .collect();
        let factor = |m: &Json| -> Vec<f32> {
            m["pbrMetallicRoughness"]["baseColorFactor"]
                .as_array()
                .map(|f| {
                    f.iter()
                        .map(|c| c.as_f64().expect("number") as f32)
                        .collect()
                })
                .unwrap_or_default()
        };

        // Yellow as CS2 draws it: sRGB (237, 179, 63), linear in the glTF factor.
        assert_eq!(
            materials[0]["name"],
            "hr_nuke_parkingstripe_single_tintedb33f"
        );
        let yellow = [237.0, 179.0, 63.0].map(|c| srgb_to_linear(c / 255.0));
        let got = factor(&materials[0]);
        assert_eq!(got.len(), 4);
        for (g, w) in got.iter().zip(yellow.iter().chain([1.0].iter())) {
            assert!((g - w).abs() < 1e-3, "{got:?}");
        }
        // The alpha makes a variant of its own (named with its alpha byte) and goes into the factor's opacity.
        assert_eq!(
            materials[1]["name"],
            "hr_nuke_parkingstripe_single_tintffffff40"
        );
        assert!((factor(&materials[1])[3] - 64.0 / 255.0).abs() < 1e-6);
        // An untinted draw call keeps the plain material.
        assert_eq!(materials[2]["name"], "hr_nuke_parkingstripe_single");
        assert!(factor(&materials[2]).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_painted_two_layer_materials_blend() {
        let material = |shader: &str| MaterialInfo {
            shader: shader.into(),
            textures: [("g_tLayer2Color".to_owned(), "layer2.vtex".to_owned())].into(),
            ..Default::default()
        };
        assert!(material("csgo_lightmappedgeneric.vfx").is_vertex_blend());
        // de_inferno's fake room windows: layers behind the glass, not a painted blend.
        assert!(!material("csgo_simple_3layer_parallax.vfx").is_vertex_blend());
        let mut single = material("csgo_lightmappedgeneric.vfx");
        single.textures.clear();
        assert!(!single.is_vertex_blend());
    }
}
