//! Exports a whole CS2 map to glTF (geometry, props, materials, textures, lights) plus an OFAST manifest.
//!
//! What goes in the glTF:
//!
//! | Map content | glTF |
//! |---|---|
//! | world-node scene objects (baked world, overlays/decals) | one mesh per model, one node per placement |
//! | aggregate fragments in world space | one mesh per aggregate, one node |
//! | aggregate fragments with their own transform (instanced props) | one mesh per (model, draw call), one node each |
//! | dynamic props, brush entities (`prop_dynamic`, `prop_physics_multiplayer`, `func_brush`, …) | one mesh per model, one node each |
//! | `light_omni2`, `light_rect`, `light_barn`, `light_spot` | `KHR_lights_punctual` point / spot lights |
//!
//! Skipped: shadow-only "block light" geometry, triggers and clips, the 3D skybox (separate world, M2), baked
//! lightmaps (Unreal relights the map with Lumen).
//!
//! The sun (`light_environment`), sky name and player spawns go into `<map>.ofast.json` (the manifest) with
//! Unreal-space positions and rotations, because the Unreal side sets them up directly (sun + sky actor, cameras).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value as Json, json};

use super::ExportError;
use super::gltf::{ExportReport, GltfWriter, load_model, srgb_to_linear};
use crate::content::Content;
use crate::coords;
use crate::map::{Entity, IDENTITY, Map, Transform, entity_transform, object_flags};
use crate::model::{DrawCall, Model};

/// Entity classes whose `model` is drawn.
const MODEL_ENTITY_CLASSES: &[&str] = &[
    "prop_dynamic",
    "prop_dynamic_override",
    "prop_physics",
    "prop_physics_multiplayer",
    "prop_physics_override",
    "func_brush",
    "func_breakable",
    "func_door",
    "func_door_rotating",
    "prop_door_rotating",
];

/// How far overlays (decals) are pushed off their surface, in inches (0.1 in ≈ 2.5 mm): enough to win the depth
/// test in Unreal, too little to see.
const OVERLAY_OFFSET: f32 = 0.1;

/// Exports `map` (loaded from `content`) to `out_dir/<map>.gltf` and `out_dir/<map>.ofast.json`.
pub fn export_map(
    content: &Content,
    map: &Map,
    out_dir: &Path,
) -> Result<ExportReport, ExportError> {
    let mut exporter = MapExporter {
        writer: GltfWriter::new(content, out_dir)?,
        content,
        models: BTreeMap::new(),
        model_meshes: BTreeMap::new(),
        draw_meshes: BTreeMap::new(),
        roots: Vec::new(),
    };

    for node in &map.nodes {
        for object in &node.scene_objects {
            if object.flags & object_flags::BLOCK_LIGHT != 0 {
                continue;
            }
            let transform = (object.transform != IDENTITY).then_some(&object.transform);
            let offset = if object.flags & object_flags::OVERLAY != 0 {
                OVERLAY_OFFSET
            } else {
                0.0
            };
            exporter.place_model(&object.model, transform, object.tint, offset)?;
        }
        for aggregate in &node.aggregates {
            exporter.place_aggregate(&aggregate.model, aggregate)?;
        }
    }

    for entity in &map.entities {
        let Some(class) = entity.class_name() else {
            continue;
        };
        if MODEL_ENTITY_CLASSES.contains(&class)
            && let Some(model) = entity.string("model").filter(|m| !m.is_empty())
        {
            let transform = entity_transform(entity.origin(), entity.angles(), entity.scales());
            let tint = entity
                .vector3("rendercolor")
                .map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, 1.0])
                .unwrap_or([1.0; 4]);
            exporter.place_model(model, Some(&transform), tint, 0.0)?;
        }
        if let Some(light) = point_light(entity) {
            let transform = entity_transform(entity.origin(), entity.angles(), [1.0; 3]);
            let forward = [transform[0][0], transform[1][0], transform[2][0]];
            let node = exporter
                .writer
                .light(class, light, entity.origin(), forward);
            exporter.roots.push(node);
        }
    }

    let manifest = manifest(map);
    fs::write(
        out_dir.join(format!("{}.ofast.json", map.name)),
        serde_json::to_string_pretty(&manifest).expect("JSON serializes"),
    )?;
    let MapExporter { writer, roots, .. } = exporter;
    writer.finish(&map.name, roots)
}

/// Keeps track of models and meshes already written so each is stored once and placed many times.
struct MapExporter<'c> {
    writer: GltfWriter<'c>,
    content: &'c Content,
    /// Decoded models by path (`None` when the model could not be read). `Rc` so a model can be used while the
    /// writer is borrowed mutably, without copying its buffers.
    models: BTreeMap<String, Option<Rc<Model>>>,
    /// (model path, tint, overlay offset in 1/1000 inch) → glTF meshes of the model's default meshes.
    model_meshes: BTreeMap<(String, [u8; 4], i32), Vec<usize>>,
    /// (model path, draw call, tint) → glTF mesh of that single draw call.
    draw_meshes: BTreeMap<(String, usize, [u8; 4]), Option<usize>>,
    roots: Vec<usize>,
}

fn tint_key(tint: [f32; 4]) -> [u8; 4] {
    tint.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

impl MapExporter<'_> {
    fn model(&mut self, path: &str) -> Option<Rc<Model>> {
        if !self.models.contains_key(path) {
            let model = match load_model(self.content, path) {
                Ok(model) => Some(Rc::new(model)),
                Err(e) => {
                    self.writer
                        .report
                        .warnings
                        .push(format!("model {path}: {e}"));
                    None
                }
            };
            self.models.insert(path.to_owned(), model);
        }
        self.models.get(path).cloned().flatten()
    }

    /// Places a whole model (its default meshes) with an optional Source-space transform. `normal_offset`
    /// (inches) lifts overlays off the surface they decorate.
    fn place_model(
        &mut self,
        path: &str,
        transform: Option<&Transform>,
        tint: [f32; 4],
        normal_offset: f32,
    ) -> Result<(), ExportError> {
        let key = (
            path.to_owned(),
            tint_key(tint),
            (normal_offset * 1000.0) as i32,
        );
        let meshes = match self.model_meshes.get(&key) {
            Some(meshes) => meshes.clone(),
            None => {
                let meshes = self.build_model_meshes(path, tint, normal_offset)?;
                self.model_meshes.insert(key, meshes.clone());
                meshes
            }
        };
        // Every default mesh of the model gets a node with the same placement.
        for mesh in meshes {
            let node = self.writer.node(&short_name(path), Some(mesh), transform);
            self.roots.push(node);
        }
        Ok(())
    }

    /// Writes one glTF mesh per default mesh of a model (most models have exactly one).
    fn build_model_meshes(
        &mut self,
        path: &str,
        tint: [f32; 4],
        normal_offset: f32,
    ) -> Result<Vec<usize>, ExportError> {
        let Some(model) = self.model(path) else {
            return Ok(Vec::new());
        };
        let mut written = Vec::new();
        for mesh in model.default_meshes() {
            let draws: Vec<&DrawCall> = mesh.draw_calls.iter().collect();
            if let Some(gltf_mesh) =
                self.writer
                    .mesh(&short_name(path), mesh, &draws, tint, normal_offset)?
            {
                written.push(gltf_mesh);
            }
        }
        Ok(written)
    }

    /// Places an aggregate: world-space fragments merged into one mesh, transformed fragments individually.
    fn place_aggregate(
        &mut self,
        path: &str,
        aggregate: &crate::map::Aggregate,
    ) -> Result<(), ExportError> {
        let Some(model) = self.model(path) else {
            return Ok(());
        };
        let Some(mesh) = model.meshes.first() else {
            return Ok(());
        };

        // World-space fragments, grouped by tint (a tint is a material variant).
        let mut by_tint: BTreeMap<[u8; 4], (Vec<&DrawCall>, [f32; 4])> = BTreeMap::new();
        for fragment in aggregate.fragments.iter().filter(|f| f.transform.is_none()) {
            if let Some(draw) = mesh.draw_calls.get(fragment.draw_call) {
                let entry = by_tint
                    .entry(tint_key(fragment.tint))
                    .or_insert((Vec::new(), fragment.tint));
                entry.0.push(draw);
            }
        }
        for (draws, tint) in by_tint.values() {
            if let Some(gltf_mesh) = self
                .writer
                .mesh(&short_name(path), mesh, draws, *tint, 0.0)?
            {
                let node = self.writer.node(&short_name(path), Some(gltf_mesh), None);
                self.roots.push(node);
            }
        }

        // Instanced fragments: one mesh per (draw call, tint), one node per placement.
        for fragment in &aggregate.fragments {
            let Some(transform) = &fragment.transform else {
                continue;
            };
            let key = (path.to_owned(), fragment.draw_call, tint_key(fragment.tint));
            let gltf_mesh = match self.draw_meshes.get(&key) {
                Some(&m) => m,
                None => {
                    let m = match mesh.draw_calls.get(fragment.draw_call) {
                        Some(draw) => self.writer.mesh(
                            &short_name(path),
                            mesh,
                            &[draw],
                            fragment.tint,
                            0.0,
                        )?,
                        None => None,
                    };
                    self.draw_meshes.insert(key, m);
                    m
                }
            };
            if let Some(gltf_mesh) = gltf_mesh {
                let node = self
                    .writer
                    .node(&short_name(path), Some(gltf_mesh), Some(transform));
                self.roots.push(node);
            }
        }
        Ok(())
    }
}

/// File name without folders or extension, for readable node names in Unreal.
fn short_name(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.split('.').next().unwrap_or(file).to_owned()
}

/// Linear RGB of an entity's `color` key (sRGB 0–255), white when absent.
fn light_color(entity: &Entity) -> [f32; 3] {
    entity
        .vector3("color")
        .map(|c| c.map(|v| srgb_to_linear(v / 255.0)))
        .unwrap_or([1.0; 3])
}

/// Turns a CS2 local light into a `KHR_lights_punctual` light (point or spot), or `None` for other entities.
///
/// Intensities are first approximations to be calibrated against CS2 screenshots (M2): CS2 `brightness` is an
/// exposure-like value (light = 2^brightness × brightnessscale).
fn point_light(entity: &Entity) -> Option<Json> {
    let class = entity.class_name()?;
    let kind = match class {
        "light_omni2" | "light_omni" => "point",
        "light_spot" | "light_barn" | "light_rect" => "spot",
        _ => return None,
    };
    let brightness = entity.number("brightness").unwrap_or(1.0);
    let scale = entity.number("brightnessscale").unwrap_or(1.0);
    let candela = 2f32.powf(brightness) * scale * 100.0;
    let mut light = json!({
        "name": class,
        "type": kind,
        "color": light_color(entity).to_vec(),
        "intensity": candela,
        "extras": { "ofast": { "class": class, "brightness": brightness, "brightnessscale": scale } },
    });
    if let Some(range) = entity.number("range").filter(|r| *r > 0.0) {
        light["range"] = json!(range * coords::INCH_TO_METRE);
    }
    if kind == "spot" {
        // Rect and barn lights are area lights in CS2; a wide spot is the closest punctual light.
        let outer = entity
            .number("outerconeangle")
            .unwrap_or(if class == "light_spot" { 45.0 } else { 80.0 });
        let inner = entity.number("innerconeangle").unwrap_or(outer * 0.6);
        light["spot"] = json!({
            "innerConeAngle": inner.to_radians().min(outer.to_radians() - 0.01).max(0.0),
            "outerConeAngle": outer.to_radians().min(std::f32::consts::FRAC_PI_2),
        });
    }
    Some(light)
}

/// Unreal rotator (pitch, yaw, roll in degrees) for a Source (pitch, yaw, roll): the Y axis flip negates pitch and
/// yaw (Source pitch looks down when positive, Unreal pitch looks up).
fn unreal_rotator(angles: [f32; 3]) -> [f32; 3] {
    [-angles[0], -angles[1], angles[2]]
}

/// The OFAST manifest: everything Unreal sets up directly rather than through glTF.
fn manifest(map: &Map) -> Json {
    let sun = map.entities_of("light_environment").next().map(|e| {
        json!({
            "rotation": unreal_rotator(e.angles()).to_vec(),
            "color": light_color(e).to_vec(),
            "brightness": e.number("brightness").unwrap_or(1.0),
            "brightnessscale": e.number("brightnessscale").unwrap_or(1.0),
            "skycolor": e.vector3("skycolor").map(|c| c.to_vec()),
            "skyintensity": e.number("skyintensity"),
        })
    });
    let sky = map
        .entities_of("env_sky")
        .next()
        .and_then(|e| e.string("skyname"))
        .map(str::to_owned);
    let spawns = |class: &str| -> Vec<Json> {
        map.entities_of(class)
            .map(|e| {
                let o = e.origin();
                // Standing eye height is 64 units above the player origin.
                json!({
                    "origin_source": o.to_vec(),
                    "eye_unreal": coords::source_point_to_unreal([o[0], o[1], o[2] + 64.0]).to_vec(),
                    "rotation_unreal": unreal_rotator(e.angles()).to_vec(),
                })
            })
            .collect()
    };
    json!({
        "map": map.name,
        "coordinates": "Unreal centimetres; Source (x, y, z) inches -> Unreal (x, -y, z) * 2.54",
        "sun": sun,
        "sky_material": sky,
        "spawns": { "ct": spawns("info_player_counterterrorist"), "t": spawns("info_player_terrorist") },
    })
}
