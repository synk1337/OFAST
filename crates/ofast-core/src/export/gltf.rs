//! glTF 2.0 writer for CS2 content: geometry, materials, textures, node placements and lights.
//!
//! Used by [`export_model`] (one model) and by [`super::map::export_map`] (a whole map). Output folder layout:
//!
//! ```text
//! <out>/<name>.gltf        scene description (JSON)
//! <out>/<name>.bin         vertex + index data
//! <out>/textures/*.png     decoded textures
//! ```
//!
//! # Conversions applied
//!
//! - **Axes and units**: see [`crate::coords`]. A Source position `(x, y, z)` in inches is written as
//!   `0.0254 × (x, z, −y)` metres, so that Unreal's importer places it at `(x, −y, z) × 2.54` cm — exactly where
//!   it is in CS2. Placements (node matrices) are converted the same way.
//! - **Materials**: each CS2 material (× tint colour) becomes a glTF PBR material. The CS2 shader name and texture
//!   parameters are kept in `extras.ofast` so the Unreal plugin can later build matching master materials.
//!   Translucent and alpha-tested CS2 materials map to glTF `BLEND` / `MASK`.
//! - **Textures**: see [`TextureRole`]. Textures are decoded and written as PNG at the end, in parallel; a texture
//!   that fails to decode is replaced by a 1×1 neutral image and reported as a warning.

use std::collections::BTreeMap;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use serde_json::{Value as Json, json};

use super::ExportError;
use super::environment;
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
    /// Texture files written.
    pub textures: usize,
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
    let mut writer = GltfWriter::new(content, out_dir)?;
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
}

/// glTF texture indices of one `csgo_environment(_blend)` layer.
struct EnvironmentLayer {
    /// Adjusted colour (sRGB).
    color: Option<usize>,
    /// Normal map.
    normal: Option<usize>,
    /// Occlusion (R), roughness (G), metalness (B).
    surface: Option<usize>,
    /// The height texture as stored (height in R).
    height: Option<usize>,
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

    fn flag(&self, name: &str) -> bool {
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
}

/// Accumulates glTF arrays and the binary buffer while exporting.
pub(crate) struct GltfWriter<'c> {
    content: &'c Content,
    out_dir: PathBuf,
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
    images: Vec<Json>,
    textures: Vec<Json>,
    /// texture job → glTF texture index.
    texture_ids: BTreeMap<TextureJob, usize>,
    /// Textures to decode and write in `finish`: (job, output file name).
    texture_jobs: Vec<(TextureJob, String)>,
    /// glTF material name → CS2 material path, to keep names unique (Unreal matches material slots by name).
    material_names: BTreeMap<String, String>,
    /// Relative PNG path of each glTF texture (same index as `textures`).
    texture_files: Vec<String>,
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
    pub(crate) fn new(content: &'c Content, out_dir: &Path) -> Result<Self, ExportError> {
        fs::create_dir_all(out_dir.join("textures"))?;
        Ok(Self {
            content,
            out_dir: out_dir.to_path_buf(),
            bin: Vec::new(),
            buffer_views: Vec::new(),
            accessors: Vec::new(),
            meshes: Vec::new(),
            nodes: Vec::new(),
            lights: Vec::new(),
            materials: Vec::new(),
            material_ids: BTreeMap::new(),
            material_infos: BTreeMap::new(),
            images: Vec::new(),
            textures: Vec::new(),
            texture_ids: BTreeMap::new(),
            texture_jobs: Vec::new(),
            material_names: BTreeMap::new(),
            texture_files: Vec::new(),
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

    /// Writes a glTF mesh made of the given draw calls of `mesh`, all tinted with `tint`.
    /// Returns `None` when no draw call has any triangle.
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
                primitive["material"] = json!(self.material(&draw.material, tint));
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
        // Tinted variants get their own name so Unreal's material slots can be told apart.
        let name = if tint_key == [255; 4] {
            base
        } else {
            format!(
                "{base}_tint{:02x}{:02x}{:02x}",
                tint_key[0], tint_key[1], tint_key[2]
            )
        };
        let mut material = json!({
            "name": name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
        });
        // csgo_environment(_blend) tint only their tint-masked area, keeping its luminance: baked into the colour.
        let baked_tint = info
            .as_ref()
            .is_some_and(|info| environment::is_environment(&info.shader));
        if tint_key != [255; 4] && !baked_tint {
            // Tints are authored in sRGB; glTF factors are linear.
            let linear = [
                srgb_to_linear(tint[0]),
                srgb_to_linear(tint[1]),
                srgb_to_linear(tint[2]),
                tint[3],
            ];
            material["pbrMetallicRoughness"]["baseColorFactor"] = json!(linear.to_vec());
        }
        if let Some(info) = info {
            self.fill_material(
                &mut material,
                &info,
                [tint_key[0], tint_key[1], tint_key[2]],
            );
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

    /// Fills a glTF material from a CS2 material. `tint` is the model tint (sRGB bytes, white = none), which only
    /// the environment shaders bake in (the others get it as `baseColorFactor` from [`Self::material`]).
    fn fill_material(&mut self, material: &mut Json, info: &MaterialInfo, tint: [u8; 3]) {
        material["extras"] = json!({
            "ofast": {
                "shader": info.shader,
                "textures": info.textures,
                "floats": info.floats,
                "ints": info.ints,
                "vectors": info.vectors,
            }
        });
        let translucent = info.flag("F_TRANSLUCENT")
            || info.flag("F_BLEND_MODE")
            || info.flag("F_OVERLAY")
            || info.shader == "csgo_static_overlay.vfx";
        let alpha_test = info.flag("F_ALPHA_TEST");
        if translucent {
            material["alphaMode"] = json!("BLEND");
        } else if alpha_test {
            material["alphaMode"] = json!("MASK");
            material["alphaCutoff"] = json!(0.5);
        }
        if info.flag("F_RENDER_BACKFACES") || info.flag("F_DOUBLE_SIDED") {
            material["doubleSided"] = json!(true);
        }
        if environment::is_environment(&info.shader) {
            self.fill_environment(material, info, tint, translucent || alpha_test);
            return;
        }

        if let Some((_, path)) = info.first_texture(COLOR_PARAMS) {
            let role = TextureRole::Color {
                keep_alpha: translucent || alpha_test,
            };
            let id = self.texture(path, role);
            material["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": id });
        }

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

    /// `csgo_environment(_blend)` (see [`environment`]): layer 1 becomes the glTF material, with its colour
    /// adjustments and the model tint baked into the colour, and its occlusion, roughness and metalness into one
    /// texture. A blend material also lists both layers' files and its height-blend settings in
    /// `extras.ofast.height_blend`, for Unreal's height-blend material.
    fn fill_environment(
        &mut self,
        material: &mut Json,
        info: &MaterialInfo,
        tint: [u8; 3],
        keep_alpha: bool,
    ) {
        let alpha_test = info.flag("F_ALPHA_TEST");
        let layer1 = self.environment_layer(info, 1, tint, keep_alpha, alpha_test);
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
        if info.shader == environment::ENVIRONMENT_BLEND {
            let layer2 = self.environment_layer(info, 2, tint, keep_alpha, alpha_test);
            let blend = environment::HeightBlend::read(info);
            material["extras"]["ofast"]["height_blend"] = json!({
                "layer1": self.layer_files(&layer1),
                "layer2": self.layer_files(&layer2),
                "softness": blend.softness,
                "zero_point": blend.zero_point,
                "scale": blend.scale,
                "mask_with_height": blend.mask_with_height,
            });
        }
    }

    /// Registers the textures of an environment material's layer `n` (1 or 2).
    fn environment_layer(
        &mut self,
        info: &MaterialInfo,
        n: u8,
        tint: [u8; 3],
        keep_alpha: bool,
        alpha_test: bool,
    ) -> EnvironmentLayer {
        let path = |kind: &str| info.textures.get(&format!("g_t{kind}{n}")).cloned();
        let (color, normal, height) = (path("Color"), path("Normal"), path("Height"));
        let layer = environment::Layer::read(info, n);
        let stem = |p: &str| {
            Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("texture")
                .to_owned()
        };
        let color_id = color.as_ref().map(|c| {
            let job = environment::ColorJob {
                color: c.clone(),
                height: height.clone(),
                layer,
                tint,
                keep_alpha,
            };
            self.register(TextureJob::EnvColor(job), format!("{}_cc.png", stem(c)))
        });
        let normal_id = normal
            .as_ref()
            .map(|p| self.texture(p, TextureRole::Normal { hemi_oct: true }));
        let surface_id = normal.as_ref().or(color.as_ref()).cloned().map(|from| {
            let job = environment::SurfaceJob {
                color: color.clone(),
                normal: normal.clone(),
                height: height.clone(),
                layer,
                alpha_test,
            };
            self.register(
                TextureJob::EnvSurface(job),
                format!("{}_surface.png", stem(&from)),
            )
        });
        let height_id = height.as_ref().map(|p| self.texture(p, TextureRole::Raw));
        EnvironmentLayer {
            color: color_id,
            normal: normal_id,
            surface: surface_id,
            height: height_id,
        }
    }

    /// The PNG files of an environment layer, for `extras.ofast.height_blend`.
    fn layer_files(&self, layer: &EnvironmentLayer) -> Json {
        let file = |id: Option<usize>| id.map(|id| self.texture_files[id].clone());
        json!({
            "color": file(layer.color),
            "normal": file(layer.normal),
            "surface": file(layer.surface),
            "height": file(layer.height),
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
            Some(self.texture_files[id].clone())
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
            Some(self.texture_files[id].clone())
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
        let stem = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("texture");
        let file_name = format!("{stem}{}.png", role.suffix());
        self.register(TextureJob::Single(path.to_owned(), role), file_name)
    }

    /// Registers a metallic-roughness texture assembled from channels of other textures.
    fn orm_texture(&mut self, source: OrmSource) -> usize {
        let from = source
            .roughness
            .as_ref()
            .or(source.metalness.as_ref())
            .map(|(p, _)| p.as_str());
        let stem = from
            .and_then(|p| Path::new(p).file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("texture");
        let file_name = match (&source.roughness, &source.metalness) {
            (Some(_), Some(_)) => format!("{stem}_orm.png"),
            (Some(_), None) => format!("{stem}_rough.png"),
            _ => format!("{stem}_metal.png"),
        };
        self.register(TextureJob::Orm(source), file_name)
    }

    fn register(&mut self, job: TextureJob, file_name: String) -> usize {
        if let Some(&id) = self.texture_ids.get(&job) {
            return id;
        }
        // Two different jobs can derive the same file name (e.g. one roughness map combined with two metalness
        // maps): number the later ones.
        let mut file_name = file_name;
        let base = file_name.trim_end_matches(".png").to_owned();
        let mut n = 2;
        while self
            .texture_files
            .iter()
            .any(|f| f.ends_with(&format!("/{file_name}")))
        {
            file_name = format!("{base}_{n}.png");
            n += 1;
        }
        let stem = file_name.trim_end_matches(".png").to_owned();
        self.texture_jobs.push((job.clone(), file_name.clone()));
        self.texture_files.push(format!("textures/{file_name}"));
        self.images
            .push(json!({ "uri": format!("textures/{file_name}"), "name": stem }));
        self.textures
            .push(json!({ "source": self.images.len() - 1, "sampler": 0 }));
        let id = self.textures.len() - 1;
        self.texture_ids.insert(job, id);
        id
    }

    /// Decodes and writes every registered texture (in parallel), then the `.bin` and `.gltf` files.
    pub(crate) fn finish(
        mut self,
        name: &str,
        roots: Vec<usize>,
    ) -> Result<ExportReport, ExportError> {
        let warnings = Mutex::new(Vec::new());
        let textures_dir = self.out_dir.join("textures");
        let content = self.content;
        self.texture_jobs.par_iter().for_each(|(job, file_name)| {
            let image = match job {
                TextureJob::Single(path, role) => decode_texture(content, path, *role),
                TextureJob::Orm(source) => assemble_orm(content, source),
                TextureJob::EnvColor(job) => environment::bake_color(content, job),
                TextureJob::EnvSurface(job) => environment::bake_surface(content, job),
            }
            .unwrap_or_else(|e| {
                warnings
                    .lock()
                    .expect("lock")
                    .push(format!("texture {file_name}: {e}"));
                Image::new(1, 1, vec![128, 128, 255, 255]) // neutral stand-in
            });
            if let Err(e) = write_png(&textures_dir.join(file_name), &image) {
                warnings
                    .lock()
                    .expect("lock")
                    .push(format!("texture {file_name}: {e}"));
            }
        });
        self.report
            .warnings
            .extend(warnings.into_inner().expect("lock"));
        self.report.textures = self.texture_jobs.len();

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
        if !self.textures.is_empty() {
            document["textures"] = json!(self.textures);
            document["images"] = json!(self.images);
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
        Ok(self.report)
    }
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

/// Writes an RGBA8 image as PNG (fast compression: these are intermediate files for Unreal).
pub fn write_png(path: &Path, image: &Image) -> Result<(), ExportError> {
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        image.width as u32,
        image.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| ExportError::Png(e.to_string()))?;
    writer
        .write_image_data(&image.rgba)
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
