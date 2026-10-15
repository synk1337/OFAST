//! glTF 2.0 export of a CS2 model: geometry, materials and textures, ready for Unreal's Interchange importer.
//!
//! Output folder layout:
//!
//! ```text
//! <out>/<model>.gltf        scene description (JSON)
//! <out>/<model>.bin         vertex + index data
//! <out>/textures/*.png      decoded textures
//! ```
//!
//! # Conversions applied
//!
//! - **Axes and units**: glTF is Y-up, metres. A Source position `(x, y, z)` in inches becomes
//!   `0.0254 × (y, z, x)` — a pure rotation (no mirroring), so triangle winding and tangent handedness are kept.
//!   Unreal's glTF importer then converts to its own axes and centimetres (1 Source inch = 2.54 cm).
//! - **Materials**: each CS2 material becomes a glTF PBR material. The CS2 shader name and its texture parameter
//!   names are kept in the material's `extras.ofast` so the Unreal plugin can later build matching master
//!   materials.
//! - **Textures**: colour → base colour; `g_tNormal` → normal map (Z reconstructed for 2-channel maps,
//!   hemi-octahedral maps decoded); CS2's two-channel roughness/metalness texture → glTF's metallic-roughness
//!   (roughness in G, metalness in B); ambient occlusion → occlusion.

use std::collections::BTreeMap;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use super::ExportError;
use crate::kv3::Value;
use crate::model::{Geometry, Model};
use crate::resource::{BlockKind, Resource};
use crate::texture::{self, Format, Image, Texture};
use crate::vpk::Vpk;

/// Inches → metres.
const INCH: f32 = 0.0254;

/// What an export produced.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    /// Path of the written `.gltf` file.
    pub gltf: PathBuf,
    /// Meshes exported (default mesh group and LOD only).
    pub meshes: usize,
    /// Triangles written.
    pub triangles: usize,
    /// Materials written.
    pub materials: usize,
    /// Texture files written.
    pub textures: usize,
    /// Non-fatal problems (a texture that could not be decoded, …).
    pub warnings: Vec<String>,
}

/// Exports the model at `model_path` (inside the VPK, e.g. `weapons/models/ak47/weapon_rif_ak47.vmdl_c`).
pub fn export_model(
    vpk: &Vpk,
    model_path: &str,
    out_dir: &Path,
) -> Result<ExportReport, ExportError> {
    let data = vpk.read_path(model_path)?;
    let resource = Resource::parse(&data)?;
    let model = Model::parse(&resource)?;
    let name = Path::new(model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_owned();

    fs::create_dir_all(out_dir.join("textures"))?;
    let mut writer = GltfWriter::new(vpk, out_dir);
    let mut nodes = Vec::new();
    for mesh in model.default_meshes() {
        let mut primitives = Vec::new();
        for draw in &mesh.draw_calls {
            let geometry = mesh.geometry(draw)?;
            if geometry.indices.is_empty() {
                continue;
            }
            writer.report.triangles += geometry.indices.len() / 3;
            let mut primitive = writer.primitive(&geometry);
            if !draw.material.is_empty() {
                let material = writer.material(&draw.material);
                primitive["material"] = json!(material);
            }
            primitives.push(primitive);
        }
        if primitives.is_empty() {
            continue;
        }
        writer
            .meshes
            .push(json!({ "name": mesh.name, "primitives": primitives }));
        nodes.push(json!({ "name": mesh.name, "mesh": writer.meshes.len() - 1 }));
        writer.report.meshes += 1;
    }
    writer.finish(&name, nodes)
}

/// Accumulates glTF arrays and the binary buffer while exporting.
struct GltfWriter<'v> {
    vpk: &'v Vpk,
    out_dir: PathBuf,
    bin: Vec<u8>,
    buffer_views: Vec<Json>,
    accessors: Vec<Json>,
    meshes: Vec<Json>,
    materials: Vec<Json>,
    material_ids: BTreeMap<String, usize>,
    images: Vec<Json>,
    textures: Vec<Json>,
    texture_ids: BTreeMap<String, usize>,
    report: ExportReport,
}

/// glTF component type and target constants.
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

impl<'v> GltfWriter<'v> {
    fn new(vpk: &'v Vpk, out_dir: &Path) -> Self {
        Self {
            vpk,
            out_dir: out_dir.to_path_buf(),
            bin: Vec::new(),
            buffer_views: Vec::new(),
            accessors: Vec::new(),
            meshes: Vec::new(),
            materials: Vec::new(),
            material_ids: BTreeMap::new(),
            images: Vec::new(),
            textures: Vec::new(),
            texture_ids: BTreeMap::new(),
            report: ExportReport::default(),
        }
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

    /// Writes one draw call's geometry (converted to glTF axes) and returns the primitive JSON.
    fn primitive(&mut self, g: &Geometry) -> Json {
        let to_gltf = |v: [f32; 3]| [v[1], v[2], v[0]];
        let positions: Vec<[f32; 3]> = g
            .positions
            .iter()
            .map(|&p| to_gltf(p).map(|c| c * INCH))
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
        let index_bytes: Vec<u8> = g.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&index_bytes, ELEMENT_ARRAY_BUFFER);
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": UNSIGNED_INT,
            "count": g.indices.len(),
            "type": "SCALAR",
        }));
        json!({ "attributes": attributes, "indices": self.accessors.len() - 1, "mode": 4 })
    }

    /// Returns the glTF material index for a CS2 material path, creating it (and its textures) on first use.
    fn material(&mut self, path: &str) -> usize {
        if let Some(&id) = self.material_ids.get(path) {
            return id;
        }
        let name = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_owned();
        let mut material = json!({
            "name": name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
        });
        match self.read_material(path) {
            Ok(info) => self.fill_material(&mut material, &info),
            Err(e) => self.report.warnings.push(format!("material {path}: {e}")),
        }
        self.materials.push(material);
        let id = self.materials.len() - 1;
        self.material_ids.insert(path.to_owned(), id);
        self.report.materials += 1;
        id
    }

    fn read_material(&self, path: &str) -> Result<MaterialInfo, ExportError> {
        let data = self.vpk.read_path(&format!("{path}_c"))?;
        let resource = Resource::parse(&data)?;
        let root = resource.kv3(BlockKind::DATA)?.root;
        let shader = root
            .get("m_shaderName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut textures = BTreeMap::new();
        for param in root
            .get("m_textureParams")
            .and_then(Value::as_array)
            .unwrap_or_default()
        {
            if let (Some(name), Some(value)) = (
                param.get("m_name").and_then(Value::as_str),
                param.get("m_pValue").and_then(Value::as_str),
            ) {
                textures.insert(name.to_owned(), value.to_owned());
            }
        }
        Ok(MaterialInfo { shader, textures })
    }

    fn fill_material(&mut self, material: &mut Json, info: &MaterialInfo) {
        material["extras"] =
            json!({ "ofast": { "shader": info.shader, "textures": info.textures } });
        let tex = |name: &str| info.textures.get(name).cloned();

        if let Some(path) = tex("g_tColor")
            && let Some(id) = self.texture(&path, TextureRole::Color)
        {
            material["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": id });
        }
        if let Some(path) = tex("g_tNormal")
            && let Some(id) = self.texture(&path, TextureRole::Normal)
        {
            material["normalTexture"] = json!({ "index": id });
        }
        if let Some(path) = tex("g_tMetalness").or_else(|| tex("g_tRoughness"))
            && let Some(id) = self.texture(&path, TextureRole::RoughnessMetalness)
        {
            let pbr = &mut material["pbrMetallicRoughness"];
            pbr["metallicRoughnessTexture"] = json!({ "index": id });
            pbr["metallicFactor"] = json!(1.0);
            pbr["roughnessFactor"] = json!(1.0);
        }
        if let Some(path) = tex("g_tAmbientOcclusion")
            && let Some(id) = self.texture(&path, TextureRole::Occlusion)
        {
            material["occlusionTexture"] = json!({ "index": id });
        }
    }

    /// Decodes a CS2 texture for a role, writes it as PNG and returns the glTF texture index.
    fn texture(&mut self, path: &str, role: TextureRole) -> Option<usize> {
        let key = format!("{path}#{role:?}");
        if let Some(&id) = self.texture_ids.get(&key) {
            return Some(id);
        }
        let stem = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("texture");
        let file_name = format!("{stem}{}.png", role.suffix());
        match self.decode_texture(path, role) {
            Ok(image) => {
                if let Err(e) = write_png(&self.out_dir.join("textures").join(&file_name), &image) {
                    self.report.warnings.push(format!("texture {path}: {e}"));
                    return None;
                }
            }
            Err(e) => {
                self.report.warnings.push(format!("texture {path}: {e}"));
                return None;
            }
        }
        self.images
            .push(json!({ "uri": format!("textures/{file_name}"), "name": stem }));
        self.textures
            .push(json!({ "source": self.images.len() - 1, "sampler": 0 }));
        let id = self.textures.len() - 1;
        self.texture_ids.insert(key, id);
        self.report.textures += 1;
        Some(id)
    }

    fn decode_texture(&self, path: &str, role: TextureRole) -> Result<Image, ExportError> {
        let data = self.vpk.read_path(&format!("{path}_c"))?;
        let resource = Resource::parse(&data)?;
        let header = Texture::parse(&resource)?;
        let mut image = header.decode_rgba8(&data)?;
        match role {
            TextureRole::Color => {
                // CS2 colour textures use alpha as a mask channel (e.g. ~0 over most of the AK-47), not opacity.
                // Viewers treat PNG alpha as transparency, so export opaque colour. Translucent materials will be
                // handled when material translation lands (M2).
                image
                    .rgba
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .for_each(|px| px[3] = 255);
            }
            TextureRole::Occlusion => {}
            TextureRole::Normal => {
                if header.conversions.hemi_oct_normal {
                    texture::decode_hemi_oct_normal(&mut image);
                } else if header.format == Format::Ati2n {
                    texture::reconstruct_normal_z(&mut image);
                }
                // glTF normal maps have no alpha meaning; keep it opaque.
                image
                    .rgba
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .for_each(|px| px[3] = 255);
            }
            TextureRole::RoughnessMetalness => {
                // CS2: roughness in R, metalness in G. glTF: roughness in G, metalness in B.
                for px in image.rgba.as_chunks_mut::<4>().0.iter_mut() {
                    let (roughness, metalness) = (px[0], px[1]);
                    px.copy_from_slice(&[255, roughness, metalness, 255]);
                }
            }
        }
        Ok(image)
    }

    /// Writes the `.bin` and `.gltf` files.
    fn finish(mut self, name: &str, nodes: Vec<Json>) -> Result<ExportReport, ExportError> {
        let bin_name = format!("{name}.bin");
        fs::write(self.out_dir.join(&bin_name), &self.bin)?;
        let node_ids: Vec<usize> = (0..nodes.len()).collect();
        let mut document = json!({
            "asset": { "version": "2.0", "generator": "OFAST (https://github.com/synk1337/OFAST)" },
            "scene": 0,
            "scenes": [{ "name": name, "nodes": node_ids }],
            "nodes": nodes,
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
        let gltf = self.out_dir.join(format!("{name}.gltf"));
        fs::write(
            &gltf,
            serde_json::to_string_pretty(&document).expect("JSON serializes"),
        )?;
        self.report.gltf = gltf;
        Ok(self.report)
    }
}

/// Material information read from a `.vmat_c`.
struct MaterialInfo {
    shader: String,
    textures: BTreeMap<String, String>,
}

/// How a texture is used, which decides the conversions applied before writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextureRole {
    Color,
    Normal,
    RoughnessMetalness,
    Occlusion,
}

impl TextureRole {
    fn suffix(self) -> &'static str {
        match self {
            TextureRole::Color => "",
            TextureRole::Normal => "_normal",
            TextureRole::RoughnessMetalness => "_orm",
            TextureRole::Occlusion => "_ao",
        }
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
