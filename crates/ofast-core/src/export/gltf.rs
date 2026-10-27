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
//!   `g_flAlphaTestReference`); `csgo_static_overlay` by its blend mode (`OverlayBlend`, private). The shaders CS2
//!   draws unlit (`csgo_unlitgeneric`, `csgo_black_unlit`) get `KHR_materials_unlit`; weapons a lower
//!   `KHR_materials_specular`; characters' cloth-shaded materials `KHR_materials_sheen` and a specular texture, CS2's
//!   cloth shading (`assemble_cloth_sheen`, private).
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
//!   `tint`: `<texture>_tint_<hash>.png`; an unlit card's two textures multiplied: `<texture>_two_<hash>.png`; a
//!   cloth-shaded character material's sheen and specular: `<colour>_sheen_<hash>.png`, `<metalness>_specular_<hash>.png`),
//!   `<hash>` being the first 8 hex digits of its pixel
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
use super::{environment, paint, tint};
use crate::anim::{BoneConstraints, Clip, NmSkeleton, Retargeter};
use crate::content::Content;
use crate::coords;
use crate::kv3::Value;
use crate::map::Transform;
use crate::model::{
    Affine, DrawCall, Mesh, Model, Skeleton, affine, compose, invert_rigid, normalize_quat,
    quat_mul,
};
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
///
/// A model with a real skeleton (agents, weapons) is written skinned: its bones as glTF nodes and one skin
/// (`GltfWriter::skin`), every mesh with `JOINTS_0` / `WEIGHTS_0` bound to it, so Unreal imports a skeletal mesh in
/// CS2's bind pose. Mesh nodes whose names a bone already uses get a `_mesh` suffix: Unreal's glTF importer renames
/// every clashing node, which would break bone names.
pub fn export_model(
    content: &Content,
    model_path: &str,
    out_dir: &Path,
) -> Result<ExportReport, ExportError> {
    export_model_with_clips(content, model_path, &[], out_dir)
}

/// [`export_model`], plus CS2 animation clips (`.vnmclip` paths) as glTF animations on the model's skin, retargeted
/// onto its skeleton by bone name ([`crate::anim::Retargeter`]). Each animation is named after its clip's file. An
/// additive clip (deltas over another clip) is skipped with a warning: composing it needs its base pose.
pub fn export_model_with_clips(
    content: &Content,
    model_path: &str,
    clip_paths: &[String],
    out_dir: &Path,
) -> Result<ExportReport, ExportError> {
    export_model_animated(content, model_path, out_dir, None, |model, warnings| {
        clip_animations(content, model, clip_paths, warnings)
    })
}

/// An animation keyed on a model's skin: for every frame, each bone of the model's skeleton relative to its parent
/// (translation in inches and rotation, Source axes, as [`crate::anim::Retargeter::locals`] gives them).
#[derive(Debug, Clone)]
pub struct BoneAnimation {
    /// The glTF animation's name (Unreal names the AnimSequence `<file><name>`).
    pub name: String,
    /// `frames[f][bone]`.
    pub frames: Vec<Vec<([f32; 3], [f32; 4])>>,
    /// Frames per second (30 is one Unreal recognises; it guesses the rate from the key times).
    pub fps: f32,
    /// `scales[f][bone]`: uniform scale, 0 to hide what the bone carries (a weapon not carried). A frame or bone
    /// missing here has scale 1 (empty: no scale keys at all). Written as glTF step keys; Unreal's AnimSequence
    /// still blends between keys, so a bone shrinks or grows over one frame.
    pub scales: Vec<Vec<f32>>,
}

/// [`export_model`] with the animations `animate` makes for the loaded model (it may add warnings), written as
/// `<file_name>.gltf` (default: the model's file name). A model without a skeleton is written without them.
pub fn export_model_animated(
    content: &Content,
    model_path: &str,
    out_dir: &Path,
    file_name: Option<&str>,
    animate: impl FnOnce(&Model, &mut Vec<String>) -> Result<Vec<BoneAnimation>, ExportError>,
) -> Result<ExportReport, ExportError> {
    let model = load_model(content, model_path)?;
    let name = file_name.map(str::to_owned).unwrap_or_else(|| {
        Path::new(model_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_owned()
    });
    // A model on its own has no placement tints: the options change nothing here.
    let mut writer = GltfWriter::new(content, out_dir, ExportOptions::default())?;
    let skin = writer.skin(&model.skeleton);
    let bone_names: HashSet<String> = model
        .skeleton
        .bones
        .iter()
        .map(|b| b.name.to_ascii_lowercase())
        .collect();
    let mut roots = Vec::new();
    for mesh in model.default_meshes() {
        let draws: Vec<&DrawCall> = mesh.draw_calls.iter().collect();
        let remap = skin.as_ref().map(|_| model.bone_remap(mesh.mesh_index));
        if let Some(gltf_mesh) = writer.mesh(&mesh.name, mesh, &draws, [1.0; 4], 0.0, remap)? {
            let node_name = if bone_names.contains(&mesh.name.to_ascii_lowercase()) {
                format!("{}_mesh", mesh.name)
            } else {
                mesh.name.clone()
            };
            let node = writer.node(&node_name, Some(gltf_mesh), None);
            if let Some((_, skin_index)) = &skin
                && !writer.bind_skin(node, *skin_index)
            {
                writer.report.warnings.push(format!(
                    "mesh {}: no skin weights, exported rigid",
                    mesh.name
                ));
            }
            roots.push(node);
        }
    }
    let animations = animate(&model, &mut writer.report.warnings)?;
    match &skin {
        Some((_, skin_index)) => {
            for animation in &animations {
                writer.animation(&model.skeleton, *skin_index, animation);
            }
        }
        None if !animations.is_empty() => writer.report.warnings.push(format!(
            "{model_path} has no skeleton: animations not exported"
        )),
        None => {}
    }
    if let Some((bone_roots, _)) = skin {
        roots.extend(bone_roots);
    }
    writer.finish(&name, roots)
}

/// Writes `model`'s meshes in mesh groups `mesh_groups` rigid, without a skin (each vertex where the bind pose has
/// it), as `<file_name>.gltf`: a static mesh in Unreal, for a model shown without animation (a weapon lying on the
/// ground). Skinned, Unreal also made a physics asset per weapon, and rendering the AWP's thumbnail crashed the GPU.
pub fn export_model_static(
    content: &Content,
    model: &Model,
    mesh_groups: u64,
    out_dir: &Path,
    file_name: &str,
) -> Result<ExportReport, ExportError> {
    let mut writer = GltfWriter::new(content, out_dir, ExportOptions::default())?;
    let mut roots = Vec::new();
    for mesh in model.meshes_in_groups(mesh_groups) {
        let draws: Vec<&DrawCall> = mesh.draw_calls.iter().collect();
        if let Some(gltf_mesh) = writer.mesh(&mesh.name, mesh, &draws, [1.0; 4], 0.0, None)? {
            roots.push(writer.node(&mesh.name, Some(gltf_mesh), None));
        }
    }
    writer.finish(file_name, roots)
}

/// A model merged into a character ([`export_character`]): gloves on an agent, a weapon in its hand.
#[derive(Debug, Clone)]
pub struct MergedModel<'m> {
    /// Prefix of its mesh nodes' names (keeps them apart from the base model's and each other's).
    pub label: String,
    /// The model.
    pub model: &'m Model,
    /// Its mesh groups to write, one bit per group (as an entity's `m_MeshGroupMask`).
    pub mesh_groups: u64,
    /// How its meshes follow the character's skeleton.
    pub binding: Binding,
    /// The skin painted on it (a weapon's, M10), if any.
    pub paint: Option<paint::Paint>,
}

/// How a merged model's meshes follow the character's skeleton.
#[derive(Debug, Clone, Copy)]
pub enum Binding {
    /// Skinned by bone name: each of its bones is the character's bone of the same name, or that bone's nearest
    /// ancestor the character has — how CS2 bone-merges glove models onto an agent.
    ByName,
    /// Every vertex on character bone `bone`, moved by `placement` from the model's space to the character's model
    /// space at its bind pose (Source axes, inches).
    Rigid {
        /// The character bone.
        bone: usize,
        /// The merged model's space → the character's model space at bind pose.
        placement: Affine,
    },
    /// Skinned onto its own copy of its bones: its bone `b` is the character's bone `first + b` (the model's
    /// skeleton appended to the character's, as a viewmodel carries each weapon), its vertices moved by `placement`
    /// from the model's space to the character's model space at bind pose.
    Offset {
        /// The character bone of the model's bone 0.
        first: usize,
        /// The merged model's space → the character's model space at bind pose.
        placement: Affine,
    },
}

/// Writes a character as `<file_name>.gltf`: `skeleton` (the base model's bones, possibly followed by added ones),
/// the base model's meshes in mesh groups `mesh_groups`, the `merged` models' meshes, and `animations` keyed on
/// `skeleton`.
#[allow(clippy::too_many_arguments)]
pub fn export_character(
    content: &Content,
    base: &Model,
    mesh_groups: u64,
    skeleton: &Skeleton,
    merged: &[MergedModel<'_>],
    animations: &[BoneAnimation],
    out_dir: &Path,
    file_name: &str,
) -> Result<ExportReport, ExportError> {
    let mut writer = GltfWriter::new(content, out_dir, ExportOptions::default())?;
    let (bone_roots, skin_index) = writer.skin(skeleton).ok_or_else(|| {
        ExportError::Character(format!("{file_name}: the skeleton has no real bones"))
    })?;
    let bone_names: HashSet<String> = skeleton
        .bones
        .iter()
        .map(|b| b.name.to_ascii_lowercase())
        .collect();
    let mut roots = Vec::new();
    let mut add = |writer: &mut GltfWriter<'_>,
                   node_name: String,
                   mesh: &Mesh,
                   skin: MeshSkin<'_>,
                   placement: Option<&Affine>|
     -> Result<(), ExportError> {
        let draws: Vec<&DrawCall> = mesh.draw_calls.iter().collect();
        if let Some(gltf_mesh) = writer.mesh_with(
            &node_name,
            mesh,
            &draws,
            [1.0; 4],
            0.0,
            Some(skin),
            placement,
        )? {
            let node_name = if bone_names.contains(&node_name.to_ascii_lowercase()) {
                format!("{node_name}_mesh")
            } else {
                node_name
            };
            let node = writer.node(&node_name, Some(gltf_mesh), None);
            if !writer.bind_skin(node, skin_index) {
                writer
                    .report
                    .warnings
                    .push(format!("mesh {node_name}: no skin weights, exported rigid"));
            }
            roots.push(node);
        }
        Ok(())
    };
    for mesh in base.meshes_in_groups(mesh_groups) {
        let remap = base.bone_remap(mesh.mesh_index);
        add(
            &mut writer,
            mesh.name.clone(),
            mesh,
            MeshSkin::Remap(remap),
            None,
        )?;
    }
    for part in merged {
        writer.paint = part.paint.clone();
        for mesh in part.model.meshes_in_groups(part.mesh_groups) {
            let name = format!("{}_{}", part.label, mesh.name);
            match part.binding {
                Binding::ByName => {
                    let remap: Vec<u32> = part
                        .model
                        .bone_remap(mesh.mesh_index)
                        .iter()
                        .map(|&b| merged_bone(&part.model.skeleton, b as usize, skeleton) as u32)
                        .collect();
                    add(&mut writer, name, mesh, MeshSkin::Remap(&remap), None)?;
                }
                Binding::Rigid { bone, placement } => {
                    add(
                        &mut writer,
                        name,
                        mesh,
                        MeshSkin::Bone(bone as u16),
                        Some(&placement),
                    )?;
                }
                Binding::Offset { first, placement } => {
                    let remap: Vec<u32> = part
                        .model
                        .bone_remap(mesh.mesh_index)
                        .iter()
                        .map(|&b| first as u32 + b)
                        .collect();
                    // A mesh without bone indices is rigid on the model's first bone (its root).
                    add(
                        &mut writer,
                        name,
                        mesh,
                        MeshSkin::RemapOr(&remap, first as u16),
                        Some(&placement),
                    )?;
                }
            }
        }
    }
    writer.paint = None;
    for animation in animations {
        writer.animation(skeleton, skin_index, animation);
    }
    roots.extend(bone_roots);
    writer.finish(file_name, roots)
}

/// The bone of `target` that bone `bone` of `source` merges onto: the one of the same name (ignoring case), else
/// that of its nearest ancestor that has one, else the root.
fn merged_bone(source: &Skeleton, bone: usize, target: &Skeleton) -> usize {
    let mut current = Some(bone);
    while let Some(b) = current {
        let Some(found) = source.bones.get(b) else {
            break;
        };
        if let Some(index) = target.find(&found.name) {
            return index;
        }
        current = found.parent;
    }
    0
}

/// How [`GltfWriter::mesh_with`] binds a mesh's vertices to a skin.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MeshSkin<'a> {
    /// The mesh's own bone indices through a remapping slice ([`Model::bone_remap`], or one translated onto another
    /// skeleton).
    Remap(&'a [u32]),
    /// Every vertex fully on one joint.
    Bone(u16),
    /// [`MeshSkin::Remap`] for a mesh with bone indices, else every vertex on the joint (a weapon part drawn
    /// rigid on the model's root).
    RemapOr(&'a [u32], u16),
}

/// CS2 clips as animations on `model`'s skeleton (see [`export_model_with_clips`]).
fn clip_animations(
    content: &Content,
    model: &Model,
    clip_paths: &[String],
    warnings: &mut Vec<String>,
) -> Result<Vec<BoneAnimation>, ExportError> {
    let mut skeletons: HashMap<String, NmSkeleton> = HashMap::new();
    let mut animations = Vec::new();
    // The model's twist bones and shoulder blades, placed after each pose as CS2 does.
    let constraints = BoneConstraints::of(model);
    warnings.extend(
        constraints
            .skipped
            .iter()
            .filter(|s| !s.contains("CBoneConstraintDotToMorph"))
            .map(|s| format!("constraint {s}: not applied")),
    );
    for clip_path in clip_paths {
        let clip_name = Path::new(clip_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip")
            .trim_end_matches(".vnmclip")
            .to_owned();
        let clip =
            Clip::parse(&kv3_data(content, clip_path)?).map_err(|source| ExportError::Clip {
                path: clip_path.clone(),
                source,
            })?;
        if clip.is_additive() {
            warnings.push(format!(
                "clip {clip_path}: additive (deltas over another clip), not exported yet"
            ));
            continue;
        }
        if !skeletons.contains_key(&clip.skeleton) {
            let skeleton = NmSkeleton::parse(&kv3_data(content, &clip.skeleton)?);
            skeletons.insert(clip.skeleton.clone(), skeleton);
        }
        let retargeter = Retargeter::new(&model.skeleton, &skeletons[&clip.skeleton]);
        // Keys on a 30 fps grid rather than the clip's own frames: Unreal's glTF import guesses an animation's
        // frame rate from its key times, and a still clip such as `idle_rifle` stores 2 frames over 0.73 s
        // (1.4 fps). Past the clip's end the last key holds its last frame.
        let keys = (clip.duration * ANIMATION_FPS).round().max(0.0) as usize + 1;
        let frames = (0..keys)
            .map(|k| {
                clip.sample(k as f32 / ANIMATION_FPS, false).map(|pose| {
                    let mut world = retargeter.world(&pose);
                    constraints.apply(&mut world);
                    retargeter.locals_of(&world)
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| ExportError::Clip {
                path: clip_path.clone(),
                source,
            })?;
        animations.push(BoneAnimation {
            name: clip_name,
            frames,
            fps: ANIMATION_FPS,
            scales: Vec::new(),
        });
    }
    Ok(animations)
}

/// The KV3 `DATA` block of a resource (`path` with or without `_c`), for other exporters.
pub(crate) fn resource_data(content: &Content, path: &str) -> Result<Value, ExportError> {
    kv3_data(content, path)
}

/// The KV3 `DATA` block of a resource (`path` with or without `_c`).
fn kv3_data(content: &Content, path: &str) -> Result<Value, ExportError> {
    let bytes = content.read_resource(path)?;
    let resource = Resource::parse(&bytes)?;
    Ok(resource.kv3(BlockKind::DATA)?.root)
}

/// glTF `JOINTS_0` / `WEIGHTS_0` for vertices with CS2 bone indices `joints` (into the mesh's remapping slice
/// `remap`) and weight bytes `weights` (empty when the mesh stores none: the first bone then takes the whole vertex).
///
/// Slots with weight 0 get joint 0 (they may hold leftover indices, and glTF wants unused joints at 0). Weights are
/// rescaled to sum to exactly 255 (they already do on every CS2 agent vertex); an all-zero vertex follows its first
/// bone. Indices outside the slice go to bone 0.
pub fn skin_attributes(
    joints: &[[u16; 4]],
    weights: &[[u8; 4]],
    remap: &[u32],
) -> (Vec<[u16; 4]>, Vec<[u16; 4]>) {
    joints
        .iter()
        .enumerate()
        .map(|(i, local)| {
            let mut w: [u16; 4] = weights.get(i).map_or([255, 0, 0, 0], |w| w.map(u16::from));
            let sum: u16 = w.iter().sum();
            if sum == 0 {
                w = [255, 0, 0, 0];
            } else if sum != 255 {
                let scaled = w.map(|v| (u32::from(v) * 255 + u32::from(sum) / 2) / u32::from(sum));
                w = scaled.map(|v| v as u16);
                let total: i32 = w.iter().map(|&v| i32::from(v)).sum();
                let largest = (0..4).max_by_key(|&k| w[k]).unwrap_or(0);
                w[largest] = (i32::from(w[largest]) + 255 - total) as u16;
            }
            let bones = std::array::from_fn(|k| {
                if w[k] == 0 {
                    0
                } else {
                    remap.get(usize::from(local[k])).copied().unwrap_or(0) as u16
                }
            });
            (bones, w)
        })
        .unzip()
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
/// CS2 never stores that combination: roughness is the blue channel of the normal map (`csgo_*` shaders), the red
/// channel of `g_tMetalness` (`csgo_weapon`) or, for materials with anisotropic roughness, the red and green channels
/// of `g_tAnisoGloss` (the normal map then has no blue channel); metalness is the green channel of `g_tMetalness`.
/// The textures may have different sizes; the output takes the larger one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OrmSource {
    /// Texture path and channel (0 = R … 3 = A, or [`MEAN_RG`]) holding roughness.
    roughness: Option<(String, usize)>,
    /// Texture path and channel holding metalness.
    metalness: Option<(String, usize)>,
}

/// An [`OrmSource`] channel standing for the mean of R and G: `g_tAnisoGloss` holds roughness along two surface
/// directions there, and glTF has one isotropic roughness.
const MEAN_RG: usize = 4;

/// A cloth-shaded character material's sheen ([`TextureJob::ClothSheen`], [`assemble_cloth_sheen`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClothSheen {
    /// The colour texture (`g_tColor`).
    color: String,
    /// `g_tMetalness`, whose B × (1 − G) is the cloth mask.
    metalness: String,
    /// `g_flSheenTintColor` (sRGB → linear) × `g_flSheenScale`, as `f32` bits.
    factor: [u32; 3],
}

/// `g_flSheenScale` when a cloth-shaded material does not set it: the shader's default (Source 2 Viewer).
const SHEEN_SCALE_DEFAULT: f64 = 0.667;

/// The least mean cloth mask (`g_tMetalness` B × (1 − G)) a cloth-shaded material's texture needs for its cloth to
/// be converted ([`GltfWriter::has_cloth_mask`]): the fabrics have about 0.5, the gloves and lenses 0.
const CLOTH_MASK_MIN: f64 = 0.05;

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
    /// `csgo_unlitgeneric` with `F_TWOTEXTURE`: `g_tColor` × `g_tColor2` (see [`bake_two_texture`]), keeping the
    /// alpha or not.
    TwoTexture(String, String, bool),
    /// A cloth-shaded character material's sheen colour and roughness ([`assemble_cloth_sheen`]).
    ClothSheen(ClothSheen),
    /// A cloth-shaded character material's specular, 1 − its cloth mask in A, from its `g_tMetalness`
    /// ([`assemble_cloth_specular`]).
    ClothSpecular(String),
    /// A weapon material's colour painted with a skin ([`paint::bake_color`]).
    PaintColor(paint::PaintJob),
    /// A weapon material's metallic-roughness painted with a skin ([`paint::bake_surface`]).
    PaintSurface(paint::PaintJob),
}

impl TextureJob {
    /// Textures OFAST computes rather than converts: named after their pixel hash (see "Texture files").
    fn is_baked(&self) -> bool {
        matches!(
            self,
            TextureJob::EnvColor(_)
                | TextureJob::EnvSurface(_)
                | TextureJob::TintColor(_)
                | TextureJob::TwoTexture(..)
                | TextureJob::ClothSheen(_)
                | TextureJob::ClothSpecular(_)
                | TextureJob::PaintColor(_)
                | TextureJob::PaintSurface(_)
        )
    }
}

/// The second colour texture of a `csgo_unlitgeneric` material with `F_TWOTEXTURE` ("TwoTexture"), whose colour
/// is then `g_tColor × g_tColor2` (see [`bake_two_texture`]). Only with both textures at their default transforms
/// (`g_vTex2CoordScale` 1, offset 0, no rotation: de_dust2's skybox clouds), as one glTF texture cannot say more.
fn two_texture(info: &MaterialInfo) -> Option<&str> {
    if info.shader != "csgo_unlitgeneric.vfx" || !info.flag("F_TWOTEXTURE") {
        return None;
    }
    let vector =
        |name: &str, default: [f64; 2]| info.vectors.get(name).map_or(default, |v| [v[0], v[1]]);
    let untransformed = vector("g_vTex2CoordScale", [1.0, 1.0]) == [1.0, 1.0]
        && vector("g_vTex2CoordOffset", [0.0, 0.0]) == [0.0, 0.0]
        && info
            .floats
            .get("g_flTex2CoordRotation")
            .is_none_or(|r| *r == 0.0);
    untransformed
        .then(|| info.textures.get("g_tColor2").map(String::as_str))
        .flatten()
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
    /// String attributes (`m_stringAttributes`): a weapon material's `composite_inputs`, the material its skins are
    /// painted with (M10, [`crate::paint`]).
    pub(crate) strings: BTreeMap<String, String>,
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
    skins: Vec<Json>,
    animations: Vec<Json>,
    lights: Vec<Json>,
    materials: Vec<Json>,
    /// (material path, tint in 0–255, the skin's label or "") → material index.
    material_ids: BTreeMap<(String, [u8; 4], String), usize>,
    /// The skin painted on the meshes being added ([`MergedModel::paint`]): their materials with `composite_inputs`
    /// get a painted variant.
    paint: Option<paint::Paint>,
    /// material path → parsed info (or None when unreadable).
    material_infos: BTreeMap<String, Option<MaterialInfo>>,
    /// `g_tMetalness` texture path → whether it holds a cloth mask ([`GltfWriter::has_cloth_mask`]).
    cloth_masks: BTreeMap<String, bool>,
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
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_BYTE: u32 = 5121;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
/// A buffer view that is neither vertex nor index data (inverse bind matrices): glTF wants no `target` there.
const NO_TARGET: u32 = 0;

/// Whether every key of a channel equals the first (within float noise of 1e-6), or there are none.
fn still<const N: usize>(keys: &[[f32; N]]) -> bool {
    keys.iter()
        .all(|k| k.iter().zip(&keys[0]).all(|(a, b)| (a - b).abs() <= 1e-6))
}

/// Key rate of exported animations: CS2's player clips are authored at 30 fps.
const ANIMATION_FPS: f32 = 30.0;

/// The rotation that turns Source axes into glTF axes (`coords::source_dir_to_gltf`): −90° about X. A skeleton's
/// root bones carry it, so their children keep CS2's own local rotations.
const SOURCE_TO_GLTF_ROTATION: [f32; 4] = [
    -std::f32::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f32::consts::FRAC_1_SQRT_2,
];

/// Shaders whose geometry is not exported: `csgo_effects` draws additive dust / light-shaft cards that would show
/// up as opaque white sheets in a plain PBR material (they come back as Unreal effects later).
const SKIPPED_SHADERS: &[&str] = &["csgo_effects.vfx"];

/// Shaders CS2 draws without lighting: their pixel shaders (decompiled in GameTracking-CS2) read no light, only
/// colour × tint and fog. Their materials get glTF's `KHR_materials_unlit`, which Unreal's Interchange imports with
/// its unlit master (`MI_Unlit_Opaque` / `_Mask` / `_Blend`). `csgo_black_unlit` draws plain black.
const UNLIT_SHADERS: &[&str] = &["csgo_unlitgeneric.vfx", "csgo_black_unlit.vfx"];

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
            skins: Vec::new(),
            animations: Vec::new(),
            lights: Vec::new(),
            materials: Vec::new(),
            material_ids: BTreeMap::new(),
            paint: None,
            material_infos: BTreeMap::new(),
            cloth_masks: BTreeMap::new(),
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
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
        });
        if target != NO_TARGET {
            view["target"] = json!(target);
        }
        self.buffer_views.push(view);
        self.bin.extend_from_slice(bytes);
        self.buffer_views.len() - 1
    }

    /// Writes four integers per vertex (`JOINTS_0`, `WEIGHTS_0`) as unsigned bytes, or unsigned shorts when a value
    /// exceeds 255. `normalized`: read as 0 – 1 (weights, which are bytes summing to 255).
    fn vec4_int_accessor(&mut self, values: &[[u16; 4]], normalized: bool) -> usize {
        let wide = values.iter().flatten().any(|&v| v > 255);
        let bytes: Vec<u8> = if wide {
            values
                .iter()
                .flatten()
                .flat_map(|v| v.to_le_bytes())
                .collect()
        } else {
            values.iter().flatten().map(|&v| v as u8).collect()
        };
        let view = self.view(&bytes, ARRAY_BUFFER);
        let mut accessor = json!({
            "bufferView": view,
            "componentType": if wide { UNSIGNED_SHORT } else { UNSIGNED_BYTE },
            "count": values.len(),
            "type": "VEC4",
        });
        if normalized {
            accessor["normalized"] = json!(true);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// Writes a skeleton as glTF nodes plus one skin; returns the root bone nodes and the skin index, or `None` for
    /// a placeholder skeleton ([`Skeleton::is_real`]).
    ///
    /// Bone `i` is skin joint `i`, so the remapped `JOINTS_0` values are skeleton bone indices. Node transforms are
    /// the bind pose in glTF axes: a root bone gets the Source → glTF rotation and a converted translation, every
    /// other bone its CS2 local rotation and its translation in metres (the conversion is a rotation, inherited down
    /// the chain). The inverse bind matrices are the inverses of the same chain, so they match the nodes exactly; Unreal
    /// takes its reference pose from them.
    pub(crate) fn skin(&mut self, skeleton: &Skeleton) -> Option<(Vec<usize>, usize)> {
        if !skeleton.is_real() {
            return None;
        }
        let first = self.nodes.len();
        let mut world: Vec<Affine> = Vec::with_capacity(skeleton.bones.len());
        let mut roots = Vec::new();
        for (i, bone) in skeleton.bones.iter().enumerate() {
            let (translation, rotation) = match bone.parent {
                None => (
                    coords::source_point_to_gltf(bone.position),
                    normalize_quat(quat_mul(SOURCE_TO_GLTF_ROTATION, bone.rotation)),
                ),
                Some(_) => (
                    bone.position.map(|c| c * coords::INCH_TO_METRE),
                    bone.rotation,
                ),
            };
            let local = affine(rotation, translation);
            world.push(match bone.parent {
                Some(p) => compose(&world[p], &local),
                None => {
                    roots.push(first + i);
                    local
                }
            });
            self.nodes.push(json!({
                "name": bone.name,
                "translation": translation.to_vec(),
                "rotation": rotation.to_vec(),
            }));
        }
        for (i, bone) in skeleton.bones.iter().enumerate() {
            if let Some(p) = bone.parent {
                let parent = &mut self.nodes[first + p];
                match parent.get_mut("children").and_then(Json::as_array_mut) {
                    Some(children) => children.push(json!(first + i)),
                    None => parent["children"] = json!([first + i]),
                }
            }
        }
        self.report.nodes += skeleton.bones.len();
        // glTF matrices are column-major.
        let bytes: Vec<u8> = world
            .iter()
            .flat_map(|w| {
                let m = invert_rigid(w);
                let column_major: [f32; 16] = std::array::from_fn(|k| {
                    let (col, row) = (k / 4, k % 4);
                    match row {
                        3 => {
                            if col == 3 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        _ => m[row][col],
                    }
                });
                column_major.into_iter().flat_map(f32::to_le_bytes)
            })
            .collect();
        let view = self.view(&bytes, NO_TARGET);
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": FLOAT,
            "count": skeleton.bones.len(),
            "type": "MAT4",
        }));
        self.skins.push(json!({
            "joints": (first..first + skeleton.bones.len()).collect::<Vec<_>>(),
            "inverseBindMatrices": self.accessors.len() - 1,
            "skeleton": roots.first().copied().unwrap_or(first),
        }));
        Some((roots, self.skins.len() - 1))
    }

    /// Writes one glTF animation keying every bone of `skeleton` on the joints of skin `skin` (made by
    /// [`Self::skin`]): `frames[f][bone]` is the bone's translation (inches) and rotation relative to its parent, in
    /// Source axes, at `fps` frames per second. Root bones get the Source → glTF rotation as in the bind pose, the
    /// others their translation in metres. A bone whose scale ever differs from 1 also gets a `scale` channel.
    pub(crate) fn animation(
        &mut self,
        skeleton: &Skeleton,
        skin: usize,
        animation: &BoneAnimation,
    ) {
        let BoneAnimation {
            name,
            frames,
            fps,
            scales,
        } = animation;
        let fps = *fps;
        if frames.is_empty() {
            return;
        }
        let joints: Vec<usize> = self.skins[skin]["joints"]
            .as_array()
            .map(|j| {
                j.iter()
                    .filter_map(Json::as_u64)
                    .map(|n| n as usize)
                    .collect()
            })
            .unwrap_or_default();
        let times: Vec<[f32; 1]> = (0..frames.len()).map(|f| [f as f32 / fps]).collect();
        let input = self.float_accessor(&times, "SCALAR", true);
        // A channel that never changes (a bone the animation does not move) gets two keys, at the start and the end,
        // instead of one per frame: a round-long player animation would otherwise repeat ~30 still bones per frame.
        let last = times.last().map_or(0.0, |t| t[0]);
        let ends = self.float_accessor(&[[0.0], [last]], "SCALAR", true);
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for (b, bone) in skeleton.bones.iter().enumerate() {
            let Some(&node) = joints.get(b) else { continue };
            let (translations, mut rotations): (Vec<[f32; 3]>, Vec<[f32; 4]>) = frames
                .iter()
                .map(|pose| {
                    let (t, r) = pose
                        .get(b)
                        .copied()
                        .unwrap_or((bone.position, bone.rotation));
                    match bone.parent {
                        None => (
                            coords::source_point_to_gltf(t),
                            normalize_quat(quat_mul(SOURCE_TO_GLTF_ROTATION, r)),
                        ),
                        Some(_) => (t.map(|c| c * coords::INCH_TO_METRE), r),
                    }
                })
                .unzip();
            // q and −q are the same rotation; keep neighbouring keys on the same side so interpolating between
            // them takes the short way.
            for k in 1..rotations.len() {
                let dot: f32 = (0..4).map(|c| rotations[k][c] * rotations[k - 1][c]).sum();
                if dot < 0.0 {
                    rotations[k] = rotations[k].map(|c| -c);
                }
            }
            let translation_still = still(&translations);
            let rotation_still = still(&rotations);
            for (path, output, key_times) in [
                (
                    "translation",
                    if translation_still {
                        self.float_accessor(&[translations[0], translations[0]], "VEC3", false)
                    } else {
                        self.float_accessor(&translations, "VEC3", false)
                    },
                    if translation_still { ends } else { input },
                ),
                (
                    "rotation",
                    if rotation_still {
                        self.float_accessor(&[rotations[0], rotations[0]], "VEC4", false)
                    } else {
                        self.float_accessor(&rotations, "VEC4", false)
                    },
                    if rotation_still { ends } else { input },
                ),
            ] {
                samplers.push(
                    json!({ "input": key_times, "output": output, "interpolation": "LINEAR" }),
                );
                channels.push(json!({
                    "sampler": samplers.len() - 1,
                    "target": { "node": node, "path": path },
                }));
            }
            let bone_scales: Vec<f32> = (0..frames.len())
                .map(|f| scales.get(f).and_then(|s| s.get(b)).copied().unwrap_or(1.0))
                .collect();
            if bone_scales.iter().any(|&s| s != 1.0) {
                let values: Vec<[f32; 3]> = bone_scales.iter().map(|&s| [s; 3]).collect();
                let output = self.float_accessor(&values, "VEC3", false);
                samplers.push(json!({ "input": input, "output": output, "interpolation": "STEP" }));
                channels.push(json!({
                    "sampler": samplers.len() - 1,
                    "target": { "node": node, "path": "scale" },
                }));
            }
        }
        self.animations.push(json!({
            "name": name,
            "samplers": samplers,
            "channels": channels,
        }));
    }

    /// Binds a mesh node to a skin made by [`Self::skin`] when its mesh carries joints and weights (a node with a
    /// skin must; a mesh without them stays an ordinary node).
    pub(crate) fn bind_skin(&mut self, node: usize, skin: usize) -> bool {
        let skinned = self.nodes[node]
            .get("mesh")
            .and_then(Json::as_u64)
            .and_then(|m| self.meshes.get(m as usize))
            .and_then(|m| m.get("primitives"))
            .and_then(Json::as_array)
            .is_some_and(|primitives| {
                primitives
                    .iter()
                    .all(|p| p["attributes"].get("JOINTS_0").is_some())
            });
        if skinned {
            self.nodes[node]["skin"] = json!(skin);
        }
        skinned
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
    ///
    /// `skin`: the mesh's bone remapping slice ([`crate::model::Model::bone_remap`]) when the mesh is written for a
    /// skin made by [`Self::skin`]: its vertices then get `JOINTS_0` / `WEIGHTS_0` ([`skin_attributes`]).
    pub(crate) fn mesh(
        &mut self,
        name: &str,
        mesh: &Mesh,
        draws: &[&DrawCall],
        tint: [f32; 4],
        normal_offset: f32,
        skin: Option<&[u32]>,
    ) -> Result<Option<usize>, ExportError> {
        self.mesh_with(
            name,
            mesh,
            draws,
            tint,
            normal_offset,
            skin.map(MeshSkin::Remap),
            None,
        )
    }

    /// [`Self::mesh`] with more control, for meshes merged into another model's skeleton
    /// ([`export_character`]): `placement` moves every vertex (Source model space → the skeleton's model space at
    /// its bind pose) before writing, and [`MeshSkin::Bone`] binds every vertex fully to one joint.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mesh_with(
        &mut self,
        name: &str,
        mesh: &Mesh,
        draws: &[&DrawCall],
        tint: [f32; 4],
        normal_offset: f32,
        skin: Option<MeshSkin<'_>>,
        placement: Option<&Affine>,
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
            if let Some(m) = placement {
                let rotate = |v: [f32; 3]| -> [f32; 3] {
                    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * v[k]).sum())
                };
                for p in &mut g.positions {
                    let r = rotate(*p);
                    *p = std::array::from_fn(|i| r[i] + m[i][3]);
                }
                for n in &mut g.normals {
                    *n = rotate(*n);
                }
                for t in &mut g.tangents {
                    let r = rotate([t[0], t[1], t[2]]);
                    *t = [r[0], r[1], r[2], t[3]];
                }
            }

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
            let skinning = match skin {
                Some(MeshSkin::Remap(remap) | MeshSkin::RemapOr(remap, _))
                    if g.joints.len() == g.positions.len() =>
                {
                    Some(skin_attributes(&g.joints, &g.weights, remap))
                }
                Some(MeshSkin::Bone(joint) | MeshSkin::RemapOr(_, joint)) => Some((
                    vec![[joint, 0, 0, 0]; g.positions.len()],
                    vec![[255, 0, 0, 0]; g.positions.len()],
                )),
                _ => None,
            };
            if let Some((joints, weights)) = skinning {
                attributes.insert(
                    "JOINTS_0".into(),
                    json!(self.vec4_int_accessor(&joints, false)),
                );
                attributes.insert(
                    "WEIGHTS_0".into(),
                    json!(self.vec4_int_accessor(&weights, true)),
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
        // CS2 paints a skin on the materials that name their composite inputs (the weapon's body, not its sticker
        // gaps or scope glass). Looked up only while a skin is painted: the cache below serves every other call.
        let paint = match self.paint.clone() {
            Some(paint) => self
                .material_info(path)
                .filter(|i| i.strings.contains_key("composite_inputs"))
                .map(|_| paint),
            None => None,
        };
        let key = (
            path.to_owned(),
            tint_key,
            paint.as_ref().map(|p| p.label.clone()).unwrap_or_default(),
        );
        if let Some(&id) = self.material_ids.get(&key) {
            return id;
        }
        let info = self.material_info(path);
        let base = self.unique_material_name(path);
        // Tinted variants get their own name so Unreal's material slots can be told apart; a variant with less
        // than full opacity (a draw call's `m_flAlpha`: de_nuke's scuff decals) adds its alpha byte. Painted ones
        // add the skin's label.
        let mut name = match tint_key {
            [255, 255, 255, 255] => base,
            [r, g, b, 255] => format!("{base}_tint{r:02x}{g:02x}{b:02x}"),
            [r, g, b, a] => format!("{base}_tint{r:02x}{g:02x}{b:02x}{a:02x}"),
        };
        if let Some(paint) = &paint {
            name = format!("{name}_{}", paint.label);
        }
        let mut material = json!({
            "name": name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
        });
        match info {
            // The material decides what the tint does (fill_material).
            Some(info) => {
                self.fill_material(&mut material, &info, tint_key);
                if let Some(paint) = &paint {
                    self.paint_material(&mut material, path, paint);
                }
            }
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

    /// Replaces a weapon material's colour and metallic-roughness with `paint`'s ([`paint::bake_color`],
    /// [`paint::bake_surface`]); the material stays as it is, with a warning, when its skin cannot be read or its
    /// style is not baked yet ([`paint::unpainted_reason`]).
    fn paint_material(&mut self, material: &mut Json, path: &str, paint: &paint::Paint) {
        let params = match crate::paint::weapon_params(
            self.content,
            &paint.recipe,
            path,
            paint.seed,
            paint.wear,
        ) {
            Ok(params) => params,
            Err(e) => {
                self.report
                    .warnings
                    .push(format!("{path}: skin {}: {e}", paint.label));
                return;
            }
        };
        if let Some(why) = paint::unpainted_reason(&params) {
            self.report.warnings.push(format!(
                "{path}: skin {}: {why}, left unpainted",
                paint.label
            ));
            return;
        }
        let job = paint::PaintJob {
            key: format!("{path}|{}", paint.label),
            params: std::sync::Arc::new(params),
        };
        let name = format!("{}_{}", stem(path), paint.label);
        let color = self.register(TextureJob::PaintColor(job.clone()), name.clone());
        let surface = self.register(TextureJob::PaintSurface(job), format!("{name}_orm"));
        let pbr = &mut material["pbrMetallicRoughness"];
        pbr["baseColorTexture"] = json!({ "index": color });
        pbr["metallicRoughnessTexture"] = json!({ "index": surface });
        pbr["metallicFactor"] = json!(1.0);
        pbr["roughnessFactor"] = json!(1.0);
        material["extras"]["ofast"]["paint"] = json!(paint.label);
    }

    /// The file name of a material, made unique: two CS2 materials can share a file name in different folders
    /// (e.g. `materials/de_dust/sitebwall13a.vmat` and `.../brush_shapes/sitebwall13a.vmat`), and Unreal would mix
    /// them up. The second one gets its folder name appended.
    /// Whether the `g_tMetalness` texture at `path` holds a cloth mask: CS2 shades cloth where B × (1 − G) is set
    /// (`docs/formats/material.md`), and a mean of [`CLOTH_MASK_MIN`] or more counts. The fabrics' textures have about
    /// 0.5 (B 117 – 151 of 255, G near 0); the constants CS2 compiles for gloves (all 0) and the SAS lenses (G 1) have
    /// none, though their materials set `F_CLOTH_SHADING` too. Decoded once per texture; an unreadable one has none.
    fn has_cloth_mask(&mut self, path: &str) -> bool {
        if let Some(&known) = self.cloth_masks.get(path) {
            return known;
        }
        let mean = decode_raw(self.content, path).map_or(0.0, |(_, image)| {
            let pixels = image.rgba.len() / 4;
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| f64::from(p[2]) / 255.0 * (1.0 - f64::from(p[1]) / 255.0))
                .sum::<f64>()
                / pixels.max(1) as f64
        });
        let cloth = mean >= CLOTH_MASK_MIN;
        self.cloth_masks.insert(path.to_owned(), cloth);
        cloth
    }

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
        if UNLIT_SHADERS.contains(&info.shader.as_str()) {
            material["extensions"] = json!({ "KHR_materials_unlit": {} });
            if info.shader == "csgo_black_unlit.vfx" {
                // No texture: without a factor glTF readers would take white.
                material["pbrMetallicRoughness"]["baseColorFactor"] = json!([0.0, 0.0, 0.0, 1.0]);
            }
            return;
        }

        // CS2 shaders (csgo_*): hemi-octahedral normal in RG with roughness in B; metalness in g_tMetalness.G,
        // whose R is the roughness for weapons only (see ValveResourceFormat's complex shader). Materials with
        // anisotropic roughness (94 `csgo_character` ones, the SAS body among them) compile their normal map to two
        // channels and keep roughness in g_tAnisoGloss.RG instead (`examples/material_channels.rs`); reading the
        // normal's missing blue channel made those bodies mirror-smooth. Otherwise a uniform g_flMetalness.
        let csgo = info.shader.starts_with("csgo_");
        let normal = info.first_texture(NORMAL_PARAMS).map(|(_, p)| p.to_owned());
        if let Some(path) = &normal {
            let id = self.texture(path, TextureRole::Normal { hemi_oct: csgo });
            material["normalTexture"] = json!({ "index": id });
        }
        let metal = info.textures.get("g_tMetalness").cloned();
        let aniso = info.textures.get("g_tAnisoGloss").cloned();
        let roughness = match (&metal, &aniso, &normal) {
            (Some(m), _, _) if info.shader == "csgo_weapon.vfx" => Some((m.clone(), 0)),
            (_, Some(a), _) if csgo => Some((a.clone(), MEAN_RG)),
            (_, _, Some(n)) if csgo => Some((n.clone(), 2)),
            _ => None,
        };
        let metalness = metal.map(|m| (m, 1));
        if info.shader == "csgo_weapon.vfx" {
            // CS2's weapon shader reflects 2 % at normal incidence off its non-metals, not glTF's 4 %
            // (ValveResourceFormat's complex shader: `F0 = 0.02` for csgo_weapon): half the specular.
            material["extensions"]["KHR_materials_specular"] = json!({ "specularFactor": 0.5 });
        }
        let cloth = match (
            info.first_texture(COLOR_PARAMS),
            info.textures.get("g_tMetalness"),
        ) {
            (Some((_, color)), Some(metal))
                if info.shader == "csgo_character.vfx"
                    && info.ints.get("F_CLOTH_SHADING") == Some(&1)
                    && self.has_cloth_mask(&metal.clone()) =>
            {
                Some((color.to_owned(), metal.clone()))
            }
            _ => None,
        };
        if let Some((color, metal)) = cloth {
            // CS2's cloth shading (knits, uniforms; Source 2 Viewer's `complex.frag`): where its cloth mask is
            // (`g_tMetalness` B × (1 − G); G is metalness, as on the others, whatever the file's name), a sheen of
            // √albedo × `g_flSheenTintColor` × `g_flSheenScale` replaces the ordinary highlight, and the cloth keeps
            // all its diffuse light. Without it the Phoenix balaclava's knit took a glossy highlight, like leather or
            // metal, and the fabrics read darker than CS2's.
            let scale = info
                .floats
                .get("g_flSheenScale")
                .copied()
                .unwrap_or(SHEEN_SCALE_DEFAULT) as f32;
            // An sRGB colour, as `g_vColorTint` (Source 2 Viewer reads it with `SrgbRead`).
            let tint = info
                .vectors
                .get("g_flSheenTintColor")
                .map_or([1.0; 3], |v| {
                    [v[0], v[1], v[2]].map(|c| srgb_to_linear(c as f32))
                });
            let sheen = self.register(
                TextureJob::ClothSheen(ClothSheen {
                    color: color.clone(),
                    metalness: metal.clone(),
                    factor: tint.map(|c| (c * scale).to_bits()),
                }),
                format!("{}_sheen", stem(&color)),
            );
            let specular = self.register(
                TextureJob::ClothSpecular(metal.clone()),
                format!("{}_specular", stem(&metal)),
            );
            material["extensions"]["KHR_materials_sheen"] = json!({
                "sheenColorFactor": [1.0, 1.0, 1.0],
                "sheenColorTexture": { "index": sheen },
                "sheenRoughnessFactor": 1.0,
                "sheenRoughnessTexture": { "index": sheen },
            });
            material["extensions"]["KHR_materials_specular"] = json!({
                "specularFactor": 1.0,
                "specularTexture": { "index": specular },
            });
        }
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
                _ => match two_texture(info) {
                    Some(second) => self.register(
                        TextureJob::TwoTexture(path.clone(), second.to_owned(), keep_alpha),
                        format!("{}_two", stem(path)),
                    ),
                    None => self.texture(path, TextureRole::Color { keep_alpha }),
                },
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
                    TextureJob::TwoTexture(first, second, keep_alpha) => {
                        bake_two_texture(content, first, second, *keep_alpha)
                    }
                    TextureJob::ClothSheen(job) => assemble_cloth_sheen(content, job),
                    TextureJob::PaintColor(job) => paint::bake_color(content, job),
                    TextureJob::PaintSurface(job) => paint::bake_surface(content, job),
                    TextureJob::ClothSpecular(metalness) => {
                        assemble_cloth_specular(content, metalness)
                    }
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
        if !self.skins.is_empty() {
            document["skins"] = json!(self.skins);
        }
        if !self.animations.is_empty() {
            document["animations"] = json!(self.animations);
        }
        if !textures.is_empty() {
            document["textures"] = json!(textures);
            document["images"] = json!(images);
            // Linear filtering with mipmaps, repeat wrapping.
            document["samplers"] =
                json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
        }
        let mut extensions_used = Vec::new();
        if !self.lights.is_empty() {
            extensions_used.push("KHR_lights_punctual");
            document["extensions"] = json!({ "KHR_lights_punctual": { "lights": self.lights } });
        }
        for extension in [
            "KHR_materials_unlit",
            "KHR_materials_specular",
            "KHR_materials_sheen",
        ] {
            if self
                .materials
                .iter()
                .any(|m| m["extensions"].get(extension).is_some())
            {
                extensions_used.push(extension);
            }
        }
        if !extensions_used.is_empty() {
            document["extensionsUsed"] = json!(extensions_used);
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
    // The extensions' texture slots (`KHR_materials_sheen`'s `sheenColorTexture`).
    if let Some(extensions) = material.get_mut("extensions").and_then(Json::as_object_mut) {
        for extension in extensions.values_mut() {
            remap(extension, texture_of);
        }
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
    for param in root
        .get("m_stringAttributes")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if let (Some(name), Some(value)) = (
            param.get("m_name").and_then(Value::as_str),
            param.get("m_value").and_then(Value::as_str),
        ) {
            info.strings.insert(name.to_owned(), value.to_owned());
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

/// `csgo_unlitgeneric`'s two-texture colour (`F_TWOTEXTURE`): `first × second`, the colours multiplied in linear
/// (both are sRGB textures) and the alphas multiplied, at the larger texture's size (nearest sample of the smaller).
/// The product is Source 1's `UnlitTwoTexture` rule, which the feature ports: Valve's decompiled files show only the
/// default variant, without it. `keep_alpha` false: alpha 255, as for any opaque colour texture.
fn bake_two_texture(
    content: &Content,
    first: &str,
    second: &str,
    keep_alpha: bool,
) -> Result<Image, ExportError> {
    let a = decode_raw(content, first)?.1;
    let b = decode_raw(content, second)?.1;
    Ok(multiply_textures(&a, &b, keep_alpha))
}

/// The texel-by-texel product of two sRGB colour textures (see [`bake_two_texture`]).
fn multiply_textures(a: &Image, b: &Image, keep_alpha: bool) -> Image {
    let (width, height) = if a.width * a.height >= b.width * b.height {
        (a.width, a.height)
    } else {
        (b.width, b.height)
    };
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let texel = |image: &Image, c| environment::sample(image, x, y, width, height, c);
            for c in 0..3 {
                let product = environment::SRGB_TO_LINEAR[usize::from(texel(a, c))]
                    * environment::SRGB_TO_LINEAR[usize::from(texel(b, c))];
                rgba.push(environment::linear_to_srgb(product));
            }
            rgba.push(if keep_alpha {
                ((u16::from(texel(a, 3)) * u16::from(texel(b, 3)) + 127) / 255) as u8
            } else {
                255
            });
        }
    }
    Image::new(width, height, rgba)
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
    let (width, height) = largest(&[&roughness, &metalness]);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                255,
                channel_at(&roughness, x, y, (width, height), 255),
                channel_at(&metalness, x, y, (width, height), 0),
                255,
            ]);
        }
    }
    Ok(Image::new(width, height, rgba))
}

/// The size of the largest of `sources` (1 × 1 without any).
fn largest(sources: &[&Option<(Image, usize)>]) -> (usize, usize) {
    sources
        .iter()
        .filter_map(|s| s.as_ref().map(|(img, _)| (img.width, img.height)))
        .max_by_key(|(w, h)| w * h)
        .unwrap_or((1, 1))
}

/// The nearest sample of a source's channel ([`MEAN_RG`]: the mean of R and G) at pixel (x, y) of an output of
/// `size`, or `default` without a source.
fn channel_at(
    source: &Option<(Image, usize)>,
    x: usize,
    y: usize,
    (width, height): (usize, usize),
    default: u8,
) -> u8 {
    match source {
        Some((img, channel)) => {
            let sx = x * img.width / width;
            let sy = y * img.height / height;
            let pixel = (sy * img.width + sx) * 4;
            if *channel == MEAN_RG {
                ((u16::from(img.rgba[pixel]) + u16::from(img.rgba[pixel + 1])) / 2) as u8
            } else {
                img.rgba[pixel + channel]
            }
        }
        None => default,
    }
}

/// CS2's cloth mask at a `g_tMetalness` pixel: B × (1 − G) (Source 2 Viewer's `complex.frag`).
fn cloth_mask(green: u8, blue: u8) -> f32 {
    f32::from(blue) / 255.0 * (1.0 - f32::from(green) / 255.0)
}

/// A cloth-shaded character material's sheen, from CS2's cloth shading (Source 2 Viewer's `complex.frag`,
/// `texturing.slang`): RGB (sRGB) the sheen colour, √(linear albedo) × `factor` (at most 1) where the cloth mask is,
/// black elsewhere, as `KHR_materials_sheen`'s colour; A the cloth mask itself, as its roughness. Not a roughness:
/// Interchange wires a sheen roughness to Unreal's Cloth input, which lerps the ordinary highlight towards the cloth
/// lobe (`ClothBxDF`), the very blend CS2 makes by its cloth mask; the cloth lobe takes Unreal's main roughness, as
/// CS2's takes the material's. Other glTF readers get the broadest sheen on the cloth and none elsewhere (its colour
/// is black there). The textures may differ in size; the output takes the largest.
fn assemble_cloth_sheen(content: &Content, job: &ClothSheen) -> Result<Image, ExportError> {
    let color = Some((decode_raw(content, &job.color)?.1, 0));
    let metalness = Some((decode_raw(content, &job.metalness)?.1, 0));
    let size = largest(&[&color, &metalness]);
    let (Some((color, _)), Some((metalness, _))) = (&color, &metalness) else {
        unreachable!("both decoded above");
    };
    let factor = job.factor.map(f32::from_bits);
    let lut = &*environment::SRGB_TO_LINEAR;
    let mut rgba = Vec::with_capacity(size.0 * size.1 * 4);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let c = pixel_at(color, x, y, size);
            let m = pixel_at(metalness, x, y, size);
            let mask = cloth_mask(m[1], m[2]);
            for (k, f) in factor.iter().enumerate() {
                let albedo = lut[usize::from(c[k])];
                rgba.push(environment::linear_to_srgb(
                    (albedo.sqrt() * f).clamp(0.0, 1.0) * mask,
                ));
            }
            rgba.push((mask * 255.0).round() as u8);
        }
    }
    Ok(Image::new(size.0, size.1, rgba))
}

/// The nearest pixel of `image` to pixel (x, y) of an output of `size`.
fn pixel_at(image: &Image, x: usize, y: usize, (width, height): (usize, usize)) -> [u8; 4] {
    let pixel = ((y * image.height / height) * image.width + x * image.width / width) * 4;
    [
        image.rgba[pixel],
        image.rgba[pixel + 1],
        image.rgba[pixel + 2],
        image.rgba[pixel + 3],
    ]
}

/// A cloth-shaded character material's specular as `KHR_materials_specular` reads it: A = 1 − the cloth mask of its
/// `g_tMetalness` (CS2's cloth replaces the ordinary highlight with its sheen there), RGB white.
fn assemble_cloth_specular(content: &Content, metalness: &str) -> Result<Image, ExportError> {
    let image = decode_raw(content, metalness)?.1;
    let rgba = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let keep = ((1.0 - cloth_mask(p[1], p[2])) * 255.0).round() as u8;
            [255, 255, 255, keep]
        })
        .collect();
    Ok(Image::new(image.width, image.height, rgba))
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
    let opaque = image.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    if opaque {
        let rgb: Vec<u8> = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect();
        encode_png(path, image.width, image.height, png::ColorType::Rgb, &rgb)
    } else {
        encode_png(
            path,
            image.width,
            image.height,
            png::ColorType::Rgba,
            &image.rgba,
        )
    }
}

/// Writes an 8-bit grayscale image (`width × height` bytes, row-major) as PNG, like [`write_png`].
pub fn write_gray_png(
    path: &Path,
    width: usize,
    height: usize,
    texels: &[u8],
) -> Result<(), ExportError> {
    encode_png(path, width, height, png::ColorType::Grayscale, texels)
}

/// The PNG writer of [`write_png`] and [`write_gray_png`]: 8 bits per channel, zlib's default level, adaptive filters.
fn encode_png(
    path: &Path,
    width: usize,
    height: usize,
    color: png::ColorType,
    data: &[u8],
) -> Result<(), ExportError> {
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(color);
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
    use crate::model::Bone;

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
            "extensions": { "KHR_materials_sheen": { "sheenColorTexture": { "index": 2 } } },
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
        assert_eq!(
            material["extensions"]["KHR_materials_sheen"]["sheenColorTexture"]["index"],
            0
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
            bone_weight_count: 0,
            attachments: Vec::new(),
            skin_bones: Vec::new(),
            hitboxes: Vec::new(),
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
            .mesh("overlay", &mesh, &draws, [1.0; 4], 0.0, None)
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
    fn cloth_shaded_characters_get_a_sheen_of_their_own_colour() {
        let content = Content::new();
        let dir = temp_folder("cloth");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let info = |cloth: bool, metalness: &str| MaterialInfo {
            shader: "csgo_character.vfx".into(),
            textures: [
                ("g_tColor", "knit_color.vtex"),
                ("g_tNormal", "knit_normal.vtex"),
                ("g_tMetalness", metalness),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
            ints: BTreeMap::from([("F_CLOTH_SHADING".to_owned(), i64::from(cloth))]),
            floats: BTreeMap::from([("g_flSheenScale".to_owned(), 0.5)]),
            vectors: BTreeMap::from([("g_flSheenTintColor".to_owned(), [1.0, 0.5, 0.25, 0.0])]),
            strings: BTreeMap::new(),
        };
        // A knit with a cloth mask; the same without the flag; a glove with the flag but a constant with no mask.
        writer.cloth_masks.insert("knit_cloth.vtex".into(), true);
        writer.cloth_masks.insert("glove_metal.vtex".into(), false);
        for (path, cloth, metalness) in [
            ("characters/knit.vmat", true, "knit_cloth.vtex"),
            ("characters/vest.vmat", false, "knit_cloth.vtex"),
            ("characters/glove.vmat", true, "glove_metal.vtex"),
        ] {
            writer
                .material_infos
                .insert(path.into(), Some(info(cloth, metalness)));
        }
        let knit = writer.material("characters/knit.vmat", [1.0; 4]);
        let vest = writer.material("characters/vest.vmat", [1.0; 4]);
        let glove = writer.material("characters/glove.vmat", [1.0; 4]);
        for plain in [vest, glove] {
            assert!(
                writer.materials[plain]["extensions"]
                    .get("KHR_materials_sheen")
                    .is_none()
            );
        }
        let knit = writer.materials[knit].clone();
        let job = |index: &Json| {
            let index = index["index"].as_u64().expect("an index") as usize;
            writer.texture_jobs[index].job.clone()
        };
        // The sheen: one texture, √albedo × tint (an sRGB colour) × scale under the cloth mask, and the mask itself.
        let sheen = &knit["extensions"]["KHR_materials_sheen"];
        assert_eq!(sheen["sheenColorTexture"], sheen["sheenRoughnessTexture"]);
        assert_eq!(
            job(&sheen["sheenColorTexture"]),
            TextureJob::ClothSheen(ClothSheen {
                color: "knit_color.vtex".into(),
                metalness: "knit_cloth.vtex".into(),
                factor: [1.0f32, 0.5, 0.25].map(|c| (srgb_to_linear(c) * 0.5).to_bits()),
            })
        );
        // The specular: 1 − the cloth mask.
        assert_eq!(
            job(&knit["extensions"]["KHR_materials_specular"]["specularTexture"]),
            TextureJob::ClothSpecular("knit_cloth.vtex".into())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cloth_mask_is_blue_where_green_is_not() {
        assert_eq!(cloth_mask(0, 255), 1.0);
        assert_eq!(cloth_mask(255, 255), 0.0);
        assert_eq!(cloth_mask(0, 0), 0.0);
        assert!((cloth_mask(51, 255) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn character_and_weapon_roughness_sources() {
        let content = Content::new();
        let dir = temp_folder("character");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let texture = |name: &str, path: &str| (name.to_owned(), path.to_owned());
        let materials = [
            // Anisotropic roughness: a two-channel normal map and g_tAnisoGloss (the SAS body).
            (
                "characters/body.vmat",
                "csgo_character.vfx",
                vec![
                    texture("g_tNormal", "body_normal.vtex"),
                    texture("g_tAnisoGloss", "body_gloss.vtex"),
                    texture("g_tMetalness", "body_metal.vtex"),
                ],
            ),
            // Roughness in the normal map's blue channel (the SAS legs).
            (
                "characters/legs.vmat",
                "csgo_character.vfx",
                vec![
                    texture("g_tNormal", "legs_normal.vtex"),
                    texture("g_tMetalness", "legs_cloth.vtex"),
                ],
            ),
            // A weapon: roughness in g_tMetalness.R, 2 % specular for non-metals.
            (
                "weapons/ak47.vmat",
                "csgo_weapon.vfx",
                vec![
                    texture("g_tNormal", "ak_normal.vtex"),
                    texture("g_tMetalness", "ak_rough.vtex"),
                ],
            ),
        ];
        for (path, shader, textures) in materials {
            let info = MaterialInfo {
                shader: shader.into(),
                textures: textures.into_iter().collect(),
                ..Default::default()
            };
            writer.material_infos.insert(path.to_owned(), Some(info));
            writer.material(path, [1.0; 4]);
        }
        let orm = |roughness: &str| {
            writer.texture_jobs.iter().find_map(|t| match &t.job {
                TextureJob::Orm(source)
                    if source.roughness.as_ref().is_some_and(|r| r.0 == roughness) =>
                {
                    Some(source.clone())
                }
                _ => None,
            })
        };
        let body = orm("body_gloss.vtex").expect("the body's roughness from g_tAnisoGloss");
        assert_eq!(body.roughness.as_ref().map(|r| r.1), Some(MEAN_RG));
        assert_eq!(body.metalness, Some(("body_metal.vtex".into(), 1)));
        assert_eq!(
            orm("legs_normal.vtex")
                .and_then(|s| s.roughness)
                .map(|r| r.1),
            Some(2)
        );
        assert_eq!(
            orm("ak_rough.vtex").and_then(|s| s.roughness).map(|r| r.1),
            Some(0)
        );
        assert!(orm("body_normal.vtex").is_none());
        let specular: Vec<bool> = writer
            .materials
            .iter()
            .map(|m| m["extensions"].get("KHR_materials_specular").is_some())
            .collect();
        assert_eq!(specular, [false, false, true]);
        assert_eq!(
            writer.materials[2]["extensions"]["KHR_materials_specular"]["specularFactor"],
            json!(0.5)
        );
    }

    /// de_mirage's `wrongway_timer` (`csgo_unlitgeneric`, translucent) and `black_simple` (`csgo_black_unlit`, no
    /// texture) next to a lit material: only the unlit ones get `KHR_materials_unlit`, the file declares it, and
    /// `extras.ofast` keeps the CS2 shader.
    #[test]
    fn unlit_shaders_get_khr_materials_unlit() {
        let content = Content::new();
        let dir = temp_folder("unlit");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let infos = [
            (
                "materials/tools/wrongway_timer.vmat",
                "csgo_unlitgeneric.vfx",
                Some("materials/tools/wrongway_timer_color.vtex"),
                true,
            ),
            (
                "materials/dev/black_simple.vmat",
                "csgo_black_unlit.vfx",
                None,
                false,
            ),
            (
                "materials/de_mirage/plaster.vmat",
                "csgo_complex.vfx",
                None,
                false,
            ),
        ];
        for (path, shader, color, translucent) in infos {
            let info = MaterialInfo {
                shader: shader.into(),
                textures: color
                    .map(|c| ("g_tColor".to_owned(), c.to_owned()))
                    .into_iter()
                    .collect(),
                ints: [("F_BLEND_MODE".to_owned(), i64::from(translucent))].into(),
                ..Default::default()
            };
            writer.material_infos.insert(path.to_owned(), Some(info));
            writer.material(path, [1.0; 4]);
        }
        let report = writer.finish("unlit", Vec::new()).expect("finish");
        let document: Json =
            serde_json::from_str(&fs::read_to_string(&report.gltf).expect("read")).expect("JSON");
        assert_eq!(document["extensionsUsed"], json!(["KHR_materials_unlit"]));
        let materials = &document["materials"];
        assert_eq!(
            materials[0]["extensions"],
            json!({ "KHR_materials_unlit": {} })
        );
        assert_eq!(materials[0]["alphaMode"], "BLEND");
        assert_eq!(
            materials[0]["extras"]["ofast"]["shader"],
            "csgo_unlitgeneric.vfx"
        );
        assert_eq!(
            materials[1]["extensions"],
            json!({ "KHR_materials_unlit": {} })
        );
        assert_eq!(
            materials[1]["pbrMetallicRoughness"]["baseColorFactor"],
            json!([0.0, 0.0, 0.0, 1.0])
        );
        assert!(materials[2].get("extensions").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// de_dust2's skybox clouds (`nuke_clouds_002`): `csgo_unlitgeneric` with `F_TWOTEXTURE` draws the product of its
    /// two cloud textures, baked into one colour texture.
    #[test]
    fn two_texture_cards_bake_the_product_of_their_textures() {
        let mut info = MaterialInfo {
            shader: "csgo_unlitgeneric.vfx".into(),
            textures: [
                ("g_tColor", "materials/clouds_003_color.vtex"),
                ("g_tColor2", "materials/clouds_001_color.vtex"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .into(),
            ints: [
                ("F_BLEND_MODE".to_owned(), 4),
                ("F_TWOTEXTURE".to_owned(), 1),
            ]
            .into(),
            vectors: [("g_vTex2CoordScale".to_owned(), [1.0, 1.0, 0.0, 0.0])].into(),
            ..Default::default()
        };
        assert_eq!(two_texture(&info), Some("materials/clouds_001_color.vtex"));
        let content = Content::new();
        let dir = temp_folder("two-texture");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let mut material = json!({});
        writer.fill_material(&mut material, &info, [255; 4]);
        assert_eq!(material["alphaMode"], "BLEND");
        let job = &writer.texture_jobs[material["pbrMetallicRoughness"]["baseColorTexture"]["index"]
            .as_u64()
            .expect("colour texture") as usize];
        assert_eq!(
            job.job,
            TextureJob::TwoTexture(
                "materials/clouds_003_color.vtex".into(),
                "materials/clouds_001_color.vtex".into(),
                true
            )
        );
        // A second texture moved or scaled is not the same UV: no bake, the first texture alone.
        info.vectors
            .insert("g_vTex2CoordScale".into(), [2.0, 2.0, 0.0, 0.0]);
        assert_eq!(two_texture(&info), None);
        info.vectors.clear();
        info.ints.remove("F_TWOTEXTURE");
        assert_eq!(two_texture(&info), None);
        let _ = fs::remove_dir_all(&dir);

        // White clouds: the colour stays white, the alphas multiply; the smaller texture is sampled at the larger size.
        let a = Image::new(2, 1, vec![255, 255, 255, 128, 255, 255, 255, 255]);
        let b = Image::new(1, 1, vec![255, 255, 255, 64]);
        let product = multiply_textures(&a, &b, true);
        assert_eq!((product.width, product.height), (2, 1));
        assert_eq!(product.rgba, vec![255, 255, 255, 32, 255, 255, 255, 64]);
        // Colours multiply in linear: mid grey (sRGB 188 ≈ linear 0.5) times itself is linear 0.25 (sRGB 137).
        let grey = Image::new(1, 1, vec![188, 188, 188, 255]);
        let squared = multiply_textures(&grey, &grey, false);
        assert!(
            squared.rgba[..3].iter().all(|&c| (136..=138).contains(&c)),
            "{:?}",
            squared.rgba
        );
        assert_eq!(squared.rgba[3], 255);
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

    #[test]
    fn skin_attributes_remap_clear_unused_slots_and_sum_to_255() {
        // Remapping slice: local index 0 → bone 5, 1 → bone 7, 2 → bone 9.
        let remap = [5, 7, 9];
        let joints = [[0, 1, 17, 6], [2, 0, 0, 0], [1, 2, 0, 0]];
        let weights = [[200, 55, 0, 0], [255, 0, 0, 0], [100, 100, 0, 0]];
        let (j, w) = skin_attributes(&joints, &weights, &remap);
        // Zero-weight slots holding leftover indices (17, 6) become joint 0.
        assert_eq!(j[0], [5, 7, 0, 0]);
        assert_eq!(w[0], [200, 55, 0, 0]);
        assert_eq!(j[1], [9, 0, 0, 0]);
        // 200 in total is rescaled to 255.
        assert_eq!(w[2].iter().sum::<u16>(), 255);
        assert_eq!(j[2], [7, 9, 0, 0]);
        // No weights stored (one bone per vertex): the first bone takes the whole vertex.
        let (j, w) = skin_attributes(&[[1, 23, 17, 0]], &[], &remap);
        assert_eq!((j[0], w[0]), ([7, 0, 0, 0], [255, 0, 0, 0]));
    }

    /// Glove bones merge onto the agent bone of the same name (any case), or their nearest ancestor's.
    #[test]
    fn merged_bones_follow_names_then_ancestors() {
        let bone = |name: &str, parent: Option<usize>| Bone {
            name: name.into(),
            parent,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            flags: 0,
        };
        let agent = Skeleton {
            bones: vec![
                bone("root_motion", None),
                bone("pelvis", Some(0)),
                bone("hand_R", Some(1)),
            ],
        };
        let glove = Skeleton {
            bones: vec![
                bone("hand_r", None),
                bone("glove_strap", Some(0)),
                bone("loose", None),
            ],
        };
        assert_eq!(merged_bone(&glove, 0, &agent), 2);
        assert_eq!(merged_bone(&glove, 1, &agent), 2);
        assert_eq!(merged_bone(&glove, 2, &agent), 0);
    }

    #[test]
    fn skin_inverse_bind_matrices_undo_the_bone_nodes() {
        let h = 0.5f32;
        let bone =
            |name: &str, parent: Option<usize>, position: [f32; 3], rotation: [f32; 4]| Bone {
                name: name.into(),
                parent,
                position,
                rotation,
                flags: 0,
            };
        let skeleton = Skeleton {
            bones: vec![
                bone("root_motion", None, [10.0, 20.0, 0.0], [h, h, h, h]),
                bone(
                    "pelvis",
                    Some(0),
                    [0.0, 42.8248, -2.8535],
                    [0.0, 0.0, 0.0, 1.0],
                ),
                bone(
                    "spine_0",
                    Some(1),
                    [0.0, 0.0, 5.0],
                    normalize_quat([0.1, 0.2, 0.0, 1.0]),
                ),
            ],
        };
        let content = Content::new();
        let dir = temp_folder("skin");
        let mut writer = GltfWriter::new(&content, &dir, ExportOptions::default()).expect("writer");
        let (roots, skin) = writer.skin(&skeleton).expect("a real skeleton");
        assert_eq!(roots, vec![0]);
        assert_eq!(writer.nodes[1]["children"], json!([2]));
        assert_eq!(writer.skins[skin]["joints"], json!([0, 1, 2]));

        // Rebuild each node's world matrix from the written TRS and multiply by the written inverse bind matrix.
        let mut world: Vec<Affine> = Vec::new();
        for (i, b) in skeleton.bones.iter().enumerate() {
            let node = &writer.nodes[i];
            let t: Vec<f32> = node["translation"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            let r: Vec<f32> = node["rotation"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            let local = affine([r[0], r[1], r[2], r[3]], [t[0], t[1], t[2]]);
            world.push(match b.parent {
                Some(p) => compose(&world[p], &local),
                None => local,
            });
        }
        let accessor =
            &writer.accessors[writer.skins[skin]["inverseBindMatrices"].as_u64().unwrap() as usize];
        let view = &writer.buffer_views[accessor["bufferView"].as_u64().unwrap() as usize];
        assert!(
            view.get("target").is_none(),
            "inverse bind matrices are not vertex data"
        );
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        for (i, w) in world.iter().enumerate() {
            let m: Vec<f32> = writer.bin[offset + i * 64..offset + (i + 1) * 64]
                .chunks(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            // Column-major 4 × 4 → rows of a 3 × 4.
            let ibm: Affine =
                std::array::from_fn(|row| std::array::from_fn(|col| m[col * 4 + row]));
            let id = compose(w, &ibm);
            for (r, row) in id.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    let expected = if r == c { 1.0 } else { 0.0 };
                    assert!((v - expected).abs() < 1e-4, "bone {i}: {id:?}");
                }
            }
        }

        // The root carries the Source → glTF rotation: a bone 42.8 inches up in Source (the pelvis) is 42.8 inches
        // up the glTF Y axis, i.e. 1.0877 m above the root's glTF position.
        let pelvis_y = world[1][1][3] - world[0][1][3];
        assert!((pelvis_y - 42.8248 * 0.0254).abs() < 1e-3, "{pelvis_y}");
    }
}
