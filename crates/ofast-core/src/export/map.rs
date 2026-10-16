//! Exports a whole CS2 map to glTF (geometry, props, materials, textures, lights) plus an OFAST manifest.
//!
//! What goes in the glTF:
//!
//! | Map content | glTF |
//! |---|---|
//! | world-node scene objects (baked world, overlays/decals) | one mesh per model, one node per placement |
//! | "block light" scene objects (invisible light blockers) | same, nodes named `lightblocker_…` |
//! | aggregate fragments in world space | one mesh per aggregate (per tint and kind), one node |
//! | aggregate fragments with their own transform (instanced props) | one mesh per (model, draw call), one node each |
//! | dynamic props, brush entities (`prop_dynamic`, `prop_physics_multiplayer`, `func_brush`, …) | one mesh per model, one node each |
//! | `light_omni2`, `light_rect`, `light_barn`, `light_spot` | `KHR_lights_punctual` point / spot lights |
//!
//! The 3D skybox (a separate, 1/16-scale world shipped in its own archive) is placed with the same rules, every
//! placement composed with the skybox → world transform, so it shows at full size around the playable map.
//!
//! Light blockers are invisible tool meshes (`toolsblocklight` materials) that seal gaps for CS2's light bake; without
//! them sunlight leaks into interiors. They are exported under [`LIGHT_BLOCKER_NODE_PREFIX`] so tools can hide them
//! while keeping their shadows. World geometry (built in Hammer, not placed from a model) is named with
//! [`WORLD_GEOMETRY_NODE_PREFIX`]: CS2 shades the back of its faces too, which Unreal reproduces with shadow proxies.
//!
//! Skipped: triggers and clips, baked lightmaps (Unreal relights the map with Lumen).
//!
//! Next to it: `<map>_sky.hdr` (the 2D sky, see [`super::sky`]) and `<map>.ofast.json`, the manifest of what the
//! Unreal side sets up directly, in Unreal units: sun, ambient, sky, exposure, fog, the local lights with their exact
//! shapes and ranges, and player spawns (see `docs/formats/lighting.md` for the light units).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Value as Json, json};

use super::gltf::{ExportReport, GltfWriter, load_model, srgb_to_linear};
use super::sky::{SkyExport, export_sky, write_flat_hdr};
use super::{ExportError, ExportOptions};
use crate::content::Content;
use crate::coords;
use crate::map::{Entity, IDENTITY, Map, Transform, compose, entity_transform, object_flags};
use crate::model::{DrawCall, Model};
use crate::resource::{BlockKind, Resource};

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

/// Lux (Unreal light units) per unit of CS2 light. Every exported light value is a CS2 value times this.
///
/// CS2 shades diffuse surfaces as `albedo × light` (no 1/π, Source 2 Viewer's `pbr.slang`) while Unreal does
/// `albedo × lux / π`, so π makes a lit surface come out with the same scene value in both engines; the exposure in
/// the manifest is derived with the same constant. Any other value works too as long as everything uses it — it
/// only shifts Unreal's EV100.
pub const LUX_PER_UNIT: f32 = std::f32::consts::PI;

/// Real candela per unit of linear local-light brightness, as Hammer states it: EV 0 is 1000 lm from a point light,
/// i.e. 1000 / 4π cd (lights2.fgd defaults: 80 cd; `brightness_candelas` on de_mirage's lights = 79.6 × 2^EV).
pub const PHOTOMETRIC_CANDELA_PER_UNIT: f32 = 1000.0 / (4.0 * std::f32::consts::PI);

/// Distance at which CS2 local light brightness is specified: 100 units, in metres.
const LIGHT_REFERENCE_DISTANCE_M: f32 = 100.0 * coords::INCH_TO_METRE;

/// Node-name prefix of 3D-skybox geometry (Unreal actor labels start with it).
pub const SKYBOX_NODE_PREFIX: &str = "sky3d_";

/// Node-name prefix of light blockers ("block light" scene objects): invisible in CS2 but stopping light, so Unreal
/// should hide them and keep their shadows.
pub const LIGHT_BLOCKER_NODE_PREFIX: &str = "lightblocker_";

/// Node-name prefix of world geometry: geometry built in Hammer (walls, floors and roofs) rather than placed from a
/// model — world-node placements without `OBJECT_TYPE_MODEL` that are not overlays, and brush entities (`func_*`).
/// Hammer geometry is often a lone wall with no outer face, so Unreal gives these meshes two-sided shadows
/// (`import_map.py`'s shadow proxies); the casters CS2 needed from behind were all of this kind.
pub const WORLD_GEOMETRY_NODE_PREFIX: &str = "world_";

/// A map's 3D skybox world and the transform that places it in the main map (see [`crate::map::sky_to_world`]).
pub struct Skybox<'a> {
    /// The skybox world, loaded from its own archive.
    pub map: &'a Map,
    /// Skybox → main-world placement (scale ×16 around the sky camera for most maps).
    pub transform: Transform,
}

/// Exports `map` (loaded from `content`), and its 3D skybox when given, to `out_dir/<map>.gltf` and
/// `out_dir/<map>.ofast.json`.
pub fn export_map(
    content: &Content,
    map: &Map,
    skybox: Option<&Skybox<'_>>,
    out_dir: &Path,
    options: ExportOptions,
) -> Result<ExportReport, ExportError> {
    let mut exporter = MapExporter {
        writer: GltfWriter::new(content, out_dir, options)?,
        content,
        models: BTreeMap::new(),
        model_meshes: BTreeMap::new(),
        draw_meshes: BTreeMap::new(),
        roots: Vec::new(),
        node_prefix: "",
        placing_blocker: false,
        placing_world_geometry: false,
    };

    exporter.place_world(map, None)?;
    if let Some(skybox) = skybox {
        // The skybox is drawn at its real size around the map: everything in it goes through the sky transform.
        // Its nodes are prefixed so tools can tell the backdrop from the playable map.
        exporter.node_prefix = SKYBOX_NODE_PREFIX;
        exporter.place_world(skybox.map, Some(&skybox.transform))?;
        exporter.node_prefix = "";
    }

    // Local lights go both into the glTF (punctual approximations any tool can read) and into the manifest (exact
    // shapes and ranges for Unreal, which Interchange's glTF import would lose).
    let lights: Vec<LocalLight> = map
        .entities
        .iter()
        .filter_map(LocalLight::from_entity)
        .collect();
    for light in &lights {
        let node = exporter
            .writer
            .light(light.class, light.gltf(), light.origin, light.forward());
        exporter.roots.push(node);
    }

    // Every env_sky with the transform of the world it lives in: the 3D skybox's first (they turn with the skybox).
    let env_skies: Vec<(&Entity, Transform)> = skybox
        .map(|s| {
            s.map
                .entities_of("env_sky")
                .map(|e| (e, s.transform))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .chain(map.entities_of("env_sky").map(|e| (e, IDENTITY)))
        .collect();
    // The visible 2D sky: the first enabled one, the skybox world's winning (that is the one the engine draws).
    let visible = env_skies
        .iter()
        .find(|(e, _)| e.flag("startdisabled") != Some(true))
        .or(env_skies.first());
    let mut sky = None;
    if let Some((env_sky, frame)) = visible {
        let file = format!("{}_sky.hdr", map.name);
        match export_sky(content, env_sky, frame, &out_dir.join(&file)) {
            Ok(exported) => sky = exported.map(|s| (file, s)),
            Err(e) => exporter.writer.report.warnings.push(format!("sky: {e}")),
        }
    }

    // The sky that lights the map is the env_sky light_environment names in `skytexture` — not always the visible
    // one (de_inferno lights with a disabled "LIGHT_sky" and shows a "MAT_sky").
    let sun = map.entities_of("light_environment").next();
    let lighting_name = sun
        .and_then(|e| e.string("skytexture"))
        .filter(|s| !s.is_empty());
    let lighting = lighting_name.and_then(|name| {
        env_skies
            .iter()
            .find(|(e, _)| e.string("targetname") == Some(name))
    });
    let mut ambient_sky = None;
    if let (Some((env_sky, frame)), Some((visible_sky, _))) = (lighting, visible)
        && !std::ptr::eq(*env_sky, *visible_sky)
    {
        let file = format!("{}_ambient.hdr", map.name);
        match export_sky(content, env_sky, frame, &out_dir.join(&file)) {
            Ok(exported) => ambient_sky = exported.map(|s| (file, s)),
            Err(e) => exporter
                .writer
                .report
                .warnings
                .push(format!("ambient sky: {e}")),
        }
    } else if lighting_name.is_some() && lighting.is_none() {
        exporter.writer.report.warnings.push(format!(
            "light_environment skytexture {:?} names no env_sky; lighting with the visible sky",
            lighting_name.unwrap_or_default()
        ));
    }

    let lighting_export = ambient_sky.as_ref().or(sky.as_ref()).map(|(_, s)| s);
    let mut ambient = ambient_manifest(sun, lighting_export);
    if let Some((file, _)) = &ambient_sky {
        ambient["hdr"] = json!(file);
    }
    if ambient["source"] == "sky_color" {
        // A flat sky colour lights the map: give the Sky Light a uniform cube map of that colour.
        let colour = sun
            .and_then(|e| e.vector3("skycolor"))
            .map(|c| c.map(|v| srgb_to_linear(v / 255.0)))
            .unwrap_or([1.0; 3]);
        let file = format!("{}_ambient.hdr", map.name);
        write_flat_hdr(&out_dir.join(&file), colour)?;
        ambient["hdr"] = json!(file);
    }
    let mut manifest = manifest(
        content,
        map,
        sky,
        ambient,
        &mut exporter.writer.report.warnings,
    );
    manifest["lights"] = lights.iter().map(LocalLight::manifest).collect();
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
    /// Prepended to node names ([`SKYBOX_NODE_PREFIX`] while placing the 3D skybox).
    node_prefix: &'static str,
    /// True while placing light blockers: their node names get [`LIGHT_BLOCKER_NODE_PREFIX`].
    placing_blocker: bool,
    /// True while placing world geometry (not from a model): node names get [`WORLD_GEOMETRY_NODE_PREFIX`].
    placing_world_geometry: bool,
}

/// An aggregate's world-space fragments merged per (tint key, world geometry?): their draw calls and the tint.
type FragmentGroups<'m> = BTreeMap<([u8; 4], bool), (Vec<&'m DrawCall>, [f32; 4])>;

fn tint_key(tint: [f32; 4]) -> [u8; 4] {
    tint.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

impl MapExporter<'_> {
    /// Node name for a placement of `path`: the model's file name, prefixed for 3D-skybox geometry, light blockers
    /// and world geometry (`sky3d_lightblocker_…` for a blocker in the skybox, `world_…` for a wall).
    fn node_name(&self, path: &str) -> String {
        let kind = if self.placing_blocker {
            LIGHT_BLOCKER_NODE_PREFIX
        } else if self.placing_world_geometry {
            WORLD_GEOMETRY_NODE_PREFIX
        } else {
            ""
        };
        format!("{}{}{}", self.node_prefix, kind, short_name(path))
    }

    /// Places a world's geometry (scene objects, aggregates, model entities), all moved by `root` when given.
    fn place_world(&mut self, map: &Map, root: Option<&Transform>) -> Result<(), ExportError> {
        let under_root = |t: &Transform| -> Option<Transform> {
            match (root, *t == IDENTITY) {
                (Some(r), true) => Some(*r),
                (Some(r), false) => Some(compose(r, t)),
                (None, true) => None,
                (None, false) => Some(*t),
            }
        };
        for node in &map.nodes {
            for object in &node.scene_objects {
                let offset = if object.flags & object_flags::OVERLAY != 0 {
                    OVERLAY_OFFSET
                } else {
                    0.0
                };
                let transform = under_root(&object.transform);
                self.placing_blocker = object.flags & object_flags::BLOCK_LIGHT != 0;
                self.placing_world_geometry =
                    object.flags & (object_flags::MODEL | object_flags::OVERLAY) == 0;
                self.place_model(&object.model, transform.as_ref(), object.tint, offset)?;
                self.placing_blocker = false;
                self.placing_world_geometry = false;
            }
            for aggregate in &node.aggregates {
                self.place_aggregate(&aggregate.model, aggregate, root)?;
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
                let transform = under_root(&transform);
                // Brush entities (`func_brush`, doors, breakables) are Hammer geometry too; props are models.
                self.placing_world_geometry = class.starts_with("func_");
                self.place_model(model, transform.as_ref(), tint, 0.0)?;
                self.placing_world_geometry = false;
            }
        }
        Ok(())
    }

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
            let node = self
                .writer
                .node(&self.node_name(path), Some(mesh), transform);
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

    /// Places an aggregate: world-space fragments merged into one mesh, transformed fragments individually. `root`
    /// moves the whole aggregate (the 3D skybox).
    fn place_aggregate(
        &mut self,
        path: &str,
        aggregate: &crate::map::Aggregate,
        root: Option<&Transform>,
    ) -> Result<(), ExportError> {
        let Some(model) = self.model(path) else {
            return Ok(());
        };
        let Some(mesh) = model.meshes.first() else {
            return Ok(());
        };

        // World-space fragments, grouped by tint (a tint is a material variant) and by world geometry vs model pieces
        // (they cast shadows differently, see object_flags::MODEL; de_inferno's aggregates never mix the two).
        let is_world = |f: &crate::map::Fragment| f.flags & object_flags::MODEL == 0;
        let mut groups: FragmentGroups = BTreeMap::new();
        for fragment in aggregate.fragments.iter().filter(|f| f.transform.is_none()) {
            if let Some(draw) = mesh.draw_calls.get(fragment.draw_call) {
                let entry = groups
                    .entry((tint_key(fragment.tint), is_world(fragment)))
                    .or_insert((Vec::new(), fragment.tint));
                entry.0.push(draw);
            }
        }
        for (&(_, world), (draws, tint)) in &groups {
            if let Some(gltf_mesh) = self
                .writer
                .mesh(&short_name(path), mesh, draws, *tint, 0.0)?
            {
                self.placing_world_geometry = world;
                let node = self
                    .writer
                    .node(&self.node_name(path), Some(gltf_mesh), root);
                self.placing_world_geometry = false;
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
                let placed = root.map(|r| compose(r, transform));
                self.placing_world_geometry = is_world(fragment);
                let node = self.writer.node(
                    &self.node_name(path),
                    Some(gltf_mesh),
                    Some(placed.as_ref().unwrap_or(transform)),
                );
                self.placing_world_geometry = false;
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

/// Linear RGB of a light: its `color` key (sRGB 0–255), or its `colortemperature` when `colormode` is 1; white
/// when absent.
fn light_color(entity: &Entity) -> [f32; 3] {
    if entity.number("colormode") == Some(1.0)
        && let Some(kelvin) = entity.number("colortemperature").filter(|k| *k > 0.0)
    {
        return kelvin_to_srgb(kelvin).map(srgb_to_linear);
    }
    entity
        .vector3("color")
        .map(|c| c.map(|v| srgb_to_linear(v / 255.0)))
        .unwrap_or([1.0; 3])
}

/// Approximate sRGB (0–1) colour of a black body at `kelvin` (Tanner Helland's fit of Mitchell Charity's table,
/// good to a few percent between 1000 K and 40000 K).
fn kelvin_to_srgb(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let green = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_16 * (t - 60.0).powf(-0.075_514_85)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [red, green, blue].map(|c| (c / 255.0).clamp(0.0, 1.0))
}

/// The shape of a [`LocalLight`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum LightShape {
    /// Emits all around (`light_omni`, and `light_omni2` with a cone wider than a hemisphere).
    Point,
    /// A cone (`light_spot`, narrow `light_omni2`): inner and outer half-angles in degrees.
    Spot { inner: f32, outer: f32 },
    /// An emitting rectangle facing forward (`light_rect`, `light_barn`): full width and height in inches.
    Rect { width: f32, height: f32 },
}

/// A CS2 local light (`light_omni2`, `light_rect`, `light_barn`, legacy `light_omni` / `light_spot`).
///
/// CS2 light units (Hammer's FGDs and Source 2 Viewer's renderer, see `docs/formats/lighting.md`):
/// - `light_omni2`, `light_rect`, `light_barn` give `brightness` in stops (EV): linear = 2^brightness ×
///   brightnessscale. Older `light_omni` / `light_spot` give it linear: brightness × brightnessscale.
/// - The linear value is the illuminance reached 100 units (2.54 m) in front of the light, so in candela
///   it is linear × [`LUX_PER_UNIT`] × 2.54² — matched to CS2's own sun and sky.
/// - Hammer also states the light in real photometric units: [`PHOTOMETRIC_CANDELA_PER_UNIT`] × linear.
/// - The light emits along the entity's forward (+X) axis; `range` (inches) is where it fades out.
#[derive(Debug, Clone, PartialEq)]
struct LocalLight<'a> {
    class: &'a str,
    shape: LightShape,
    /// Source position (inches) and angles.
    origin: [f32; 3],
    angles: [f32; 3],
    /// Linear brightness (see above).
    linear: f32,
    /// Linear RGB.
    color: [f32; 3],
    /// Fade-out distance in inches.
    range: Option<f32>,
    /// Radius of a point or spot light's emitting sphere, in inches (`size_params[0]` of `light_omni2`).
    radius: f32,
    /// Strength of this light's bounce in CS2's bake.
    bounce: f32,
    brightness: f32,
    brightnessscale: f32,
}

impl<'a> LocalLight<'a> {
    fn from_entity(entity: &'a Entity) -> Option<Self> {
        let class = entity.class_name()?;
        // `size_params` is three numbers, stored as an array or a string depending on the lump's age.
        let size_params = entity.vector3("size_params");
        let size = |i: usize| size_params.map(|s| s[i]).filter(|v| *v > 0.0);
        let shape = match class {
            "light_omni" => LightShape::Point,
            "light_omni2" => {
                // `outer_angle` is a half-angle: 180 lights the whole sphere.
                let outer = entity.number("outer_angle").unwrap_or(180.0);
                if outer >= 90.0 {
                    LightShape::Point
                } else {
                    let inner = entity
                        .number("inner_angle")
                        .unwrap_or(outer * 0.6)
                        .min(outer);
                    LightShape::Spot { inner, outer }
                }
            }
            "light_spot" => {
                let outer = entity.number("outerconeangle").unwrap_or(45.0);
                let inner = entity
                    .number("innerconeangle")
                    .unwrap_or(outer * 0.6)
                    .min(outer);
                LightShape::Spot { inner, outer }
            }
            // A rect light's `size_params` holds half-sizes: Hammer's defaults (16, 16) with 80 cd and 120 nits only
            // agree for a 32 × 32 rectangle (lights2.fgd).
            "light_rect" => LightShape::Rect {
                width: 2.0 * size(0).unwrap_or(16.0),
                height: 2.0 * size(1).unwrap_or(16.0),
            },
            // A barn light's `size_params` is its barn-door aperture; the emitter is `luminaire_size` across (the
            // barn doors themselves are not reproduced yet).
            "light_barn" => {
                let side = entity
                    .number("luminaire_size")
                    .filter(|s| *s > 0.0)
                    .unwrap_or(4.0);
                LightShape::Rect {
                    width: side,
                    height: side,
                }
            }
            _ => return None,
        };
        let brightness = entity.number("brightness").unwrap_or(1.0);
        let brightnessscale = entity.number("brightnessscale").unwrap_or(1.0);
        let linear = match class {
            "light_omni" | "light_spot" => brightness * brightnessscale,
            _ => 2f32.powf(brightness) * brightnessscale,
        };
        Some(Self {
            class,
            shape,
            origin: entity.origin(),
            angles: entity.angles(),
            linear,
            color: light_color(entity),
            range: entity.number("range").filter(|r| *r > 0.0),
            radius: if matches!(shape, LightShape::Rect { .. }) {
                0.0
            } else {
                size(0).unwrap_or(0.0)
            },
            bounce: entity.number("bouncescale").unwrap_or(1.0),
            brightness,
            brightnessscale,
        })
    }

    /// Candela in OFAST's CS2-matched units (see the type's docs).
    fn candela(&self) -> f32 {
        self.linear * LUX_PER_UNIT * LIGHT_REFERENCE_DISTANCE_M * LIGHT_REFERENCE_DISTANCE_M
    }

    /// Source-space direction the light shines in.
    fn forward(&self) -> [f32; 3] {
        let t = entity_transform([0.0; 3], self.angles, [1.0; 3]);
        [t[0][0], t[1][0], t[2][0]]
    }

    /// `KHR_lights_punctual` light: rect lights become wide spots, the closest punctual shape.
    fn gltf(&self) -> Json {
        let mut light = json!({
            "name": self.class,
            "type": if self.shape == LightShape::Point { "point" } else { "spot" },
            "color": self.color.to_vec(),
            "intensity": self.candela(),
            "extras": { "ofast": {
                "class": self.class, "brightness": self.brightness, "brightnessscale": self.brightnessscale,
            } },
        });
        if let Some(range) = self.range {
            light["range"] = json!(range * coords::INCH_TO_METRE);
        }
        let cone = match self.shape {
            LightShape::Point => None,
            LightShape::Spot { inner, outer } => Some((inner, outer)),
            LightShape::Rect { .. } => Some((48.0, 80.0)),
        };
        if let Some((inner, outer)) = cone {
            let outer = outer.to_radians().min(std::f32::consts::FRAC_PI_2);
            light["spot"] = json!({
                "innerConeAngle": inner.to_radians().min(outer - 0.01).max(0.0),
                "outerConeAngle": outer,
            });
        }
        light
    }

    /// Manifest entry, in Unreal units (cm, degrees, candela): the exact shape and range for Unreal's own lights.
    fn manifest(&self) -> Json {
        let cm = |inches: f32| inches * coords::INCH_TO_CM;
        let mut light = json!({
            "class": self.class,
            "location": coords::source_point_to_unreal(self.origin).to_vec(),
            "rotation": unreal_rotator(self.angles).to_vec(),
            "intensity_cd": self.candela(),
            // Real-world candela (Hammer's photometric units), for physically lit scenes.
            "intensity_cd_photometric": self.linear * PHOTOMETRIC_CANDELA_PER_UNIT,
            "color": self.color.to_vec(),
            "attenuation_radius": self.range.map(cm),
            "source_radius": cm(self.radius),
            "indirect_intensity": self.bounce,
        });
        match self.shape {
            LightShape::Point => light["shape"] = json!("point"),
            LightShape::Spot { inner, outer } => {
                light["shape"] = json!("spot");
                light["inner_cone"] = json!(inner);
                light["outer_cone"] = json!(outer);
            }
            LightShape::Rect { width, height } => {
                light["shape"] = json!("rect");
                light["source_width"] = json!(cm(width));
                light["source_height"] = json!(cm(height));
            }
        }
        light
    }
}

/// Unreal rotator (pitch, yaw, roll in degrees) for a Source (pitch, yaw, roll): the Y axis flip negates pitch and
/// yaw (Source pitch looks down when positive, Unreal pitch looks up).
fn unreal_rotator(angles: [f32; 3]) -> [f32; 3] {
    [-angles[0], -angles[1], angles[2]]
}

/// Sun (`light_environment`) settings for the manifest. Its `brightness` is a plain linear multiplier (unlike local
/// lights), so the Unreal illuminance is brightness × brightnessscale × [`LUX_PER_UNIT`].
fn sun_manifest(sun: &Entity) -> Json {
    let brightness = sun.number("brightness").unwrap_or(1.0);
    let scale = sun.number("brightnessscale").unwrap_or(1.0);
    json!({
        "rotation": unreal_rotator(sun.angles()).to_vec(),
        "color": light_color(sun).to_vec(),
        "brightness": brightness,
        "brightnessscale": scale,
        "illuminance_lux": brightness * scale * LUX_PER_UNIT,
        // Hammer's "Sun Spread Angle" (degrees, shadow softness) ≈ Unreal's Source Angle.
        "source_angle": sun.number("angulardiameter").unwrap_or(1.0),
        // How much the sun's bounce light was scaled in CS2's bake ≈ Unreal's Indirect Lighting Intensity.
        "indirect_intensity": sun.number("bouncescale").unwrap_or(1.0),
    })
}

/// Ambient (sky) light for the manifest. When `light_environment` names a sky texture (the usual case), the bake
/// lights the map from the `env_sky` HDR and `skycolor` is ignored (Hammer's `lights_base.fgd`); otherwise the
/// ambient is the flat `skycolor × skyintensity` (the exporter then writes `<map>_ambient.hdr`). CS2 light values are
/// irradiance / π, hence the 1/π for Unreal's Sky Light, whose intensity multiplies the sky's radiance.
///
/// Below the horizon CS2 puts a virtual ground of albedo `skyambientbounce`; Unreal's Sky Light takes a radiance
/// there instead, approximated as albedo × the light reaching flat ground (sun at its elevation + the sky's upper
/// hemisphere), in the same units as the sky texture.
fn ambient_manifest(sun: Option<&Entity>, sky: Option<&SkyExport>) -> Json {
    let key = |k: &str| sun.and_then(|e| e.number(k));
    let from_texture = sun
        .and_then(|e| e.string("skytexture"))
        .is_some_and(|s| !s.is_empty());
    let scale = if from_texture {
        key("skytexturescale").unwrap_or(1.0)
    } else {
        key("skyintensity").unwrap_or(1.0)
    };
    let linear = |k: &str| {
        sun.and_then(|e| e.vector3(k))
            .map(|c| c.map(|v| srgb_to_linear(v / 255.0)))
    };
    let albedo = linear("skyambientbounce").unwrap_or([0.0; 3]);
    let sky_light = if from_texture {
        sky.map(|s| s.upper_mean.map(|c| c * scale))
            .unwrap_or([0.0; 3])
    } else {
        linear("skycolor").unwrap_or([1.0; 3]).map(|c| c * scale)
    };
    let sun_on_ground = sun.map_or([0.0; 3], |e| {
        let strength = e.number("brightness").unwrap_or(1.0)
            * e.number("brightnessscale").unwrap_or(1.0)
            * e.angles()[0].to_radians().sin().max(0.0);
        light_color(e).map(|c| c * strength)
    });
    let ground: [f32; 3] = std::array::from_fn(|i| albedo[i] * (sun_on_ground[i] + sky_light[i]));
    json!({
        "source": if from_texture { "sky_texture" } else { "sky_color" },
        "sky_light_intensity": scale * LUX_PER_UNIT / std::f32::consts::PI,
        // How much the sky light's bounce was scaled in CS2's bake ≈ the Sky Light's Indirect Lighting Intensity.
        "indirect_intensity": key("skybouncescale").unwrap_or(1.0),
        "sky_color": linear("skycolor").map(|c| c.to_vec()),
        "ground_albedo": albedo.to_vec(),
        "lower_hemisphere_color": ground.to_vec(),
    })
}

/// Exposure, tone mapping and bloom from the master `post_processing_volume` and its `.vpost`.
///
/// CS2's Hable tone curve, with the parameters every stock map uses (shoulder 0, linear 0.001, toe 1/1/1), is a
/// straight line, and measured on screenshots CS2 shows `exposure × scene` (sky pixels of known radiance in a sunlit
/// de_mirage view: display / radiance = 0.80 = `minexposure`; `docs/formats/lighting.md`). Unreal, with its tone
/// curve off, shows `luminance × 2^−EV100`, and a CS2 scene value of 1 is a luminance of [`LUX_PER_UNIT`] / π, so the
/// matching EV100 is `log2(LUX_PER_UNIT / (π × exposure × 2^bias))`.
fn post_manifest(content: &Content, map: &Map, warnings: &mut Vec<String>) -> Json {
    let volumes: Vec<&Entity> = map.entities_of("post_processing_volume").collect();
    let Some(volume) = volumes
        .iter()
        .find(|e| e.flag("master") == Some(true))
        .or(volumes.first())
    else {
        return Json::Null;
    };
    let auto_exposure = volume.flag("enableexposure").unwrap_or(false);
    let (min, max) = if auto_exposure {
        (
            volume.number("minexposure").unwrap_or(1.0),
            volume.number("maxexposure").unwrap_or(1.0),
        )
    } else {
        (1.0, 1.0)
    };

    let mut tonemap = Json::Null;
    let mut bias = 0.0;
    let mut bloom = Json::Null;
    if let Some(path) = volume.string("postprocessing").filter(|p| !p.is_empty()) {
        match read_vpost(content, path) {
            Ok(root) => {
                let float = |o: Option<&crate::kv3::Value>, k: &str| {
                    o.and_then(|o| o.get(k))
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32)
                };
                let params = root.get("m_toneMapParams");
                if root.get("m_bHasTonemapParams").is_some_and(kv3_true) {
                    bias = float(params, "m_flExposureBias").unwrap_or(0.0);
                    tonemap = json!({
                        // A linear curve's white point does not scale the picture (see above); a Hable curve's
                        // normalises it (Unreal: import_map.py's Hable tone mapper).
                        "white_point": float(params, "m_flWhitePoint"),
                        "exposure_bias": bias,
                        "shoulder_strength": float(params, "m_flShoulderStrength"),
                        "linear_strength": float(params, "m_flLinearStrength"),
                        "linear_angle": float(params, "m_flLinearAngle"),
                        "toe_strength": float(params, "m_flToeStrength"),
                        "toe_numerator": float(params, "m_flToeNum"),
                        "toe_denominator": float(params, "m_flToeDenom"),
                    });
                }
                let params = root.get("m_bloomParams");
                if root.get("m_bHasBloomParams").is_some_and(kv3_true) {
                    let weights: Vec<f32> = params
                        .and_then(|p| p.get("m_flBlurWeight"))
                        .and_then(|w| w.as_array())
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|v| v.as_f64().map(|v| v as f32))
                        .collect();
                    bloom = json!({
                        "blend_mode": params.and_then(|p| p.get("m_blendMode")).and_then(|v| v.as_str()),
                        "screen_strength": float(params, "m_flScreenBloomStrength"),
                        "bloom_strength": float(params, "m_flBloomStrength"),
                        "threshold": float(params, "m_flBloomThreshold"),
                        "threshold_width": float(params, "m_flBloomThresholdWidth"),
                        "blur_weights": weights,
                    });
                }
            }
            Err(e) => warnings.push(format!("post processing {path}: {e}")),
        }
    }
    let ev100 =
        |exposure: f32| (LUX_PER_UNIT / (std::f32::consts::PI * exposure * 2f32.powf(bias))).log2();
    json!({
        "postprocessing": volume.string("postprocessing"),
        "auto_exposure": auto_exposure,
        "min_exposure": min,
        "max_exposure": max,
        // Brightest picture (max exposure, dark places) → lowest EV100; sunlit views sit at min exposure.
        "ev100_min": ev100(max),
        "ev100_max": ev100(min),
        "tonemap": tonemap,
        // For reference: turning CS2's bloom off (r_csgo_render_post_bloom 0) changes nothing measurable on
        // de_mirage, so the Unreal side does not reproduce it.
        "bloom": bloom,
    })
}

fn read_vpost(content: &Content, path: &str) -> Result<crate::kv3::Value, ExportError> {
    let data = content.read_resource(path)?;
    Ok(Resource::parse(&data)?.kv3(BlockKind::DATA)?.root)
}

fn kv3_true(value: &crate::kv3::Value) -> bool {
    match value.unflagged() {
        crate::kv3::Value::Bool(b) => *b,
        other => other.as_f64().is_some_and(|v| v != 0.0),
    }
}

/// `env_cubemap_fog` (distance and height fog coloured by the sky cube map) for the manifest, in Unreal
/// centimetres. CS2's fog opacity is `max_opacity × ((distance − start) / (end − start))^falloff_exponent`, faded
/// with height between `height_start` and `height_end`.
fn fog_manifest(map: &Map) -> Json {
    let Some(fog) = map.entities_of("env_cubemap_fog").next() else {
        return Json::Null;
    };
    let cm = |k: &str, default: f32| fog.number(k).unwrap_or(default) * coords::INCH_TO_CM;
    json!({
        "start_distance": cm("cubemapfogstartdistance", 0.0),
        "end_distance": cm("cubemapfogenddistance", 8000.0),
        "falloff_exponent": fog.number("cubemapfogfalloffexponent").unwrap_or(1.0),
        "max_opacity": fog.number("cubemapfogmaxopacity").unwrap_or(1.0),
        "height_fog": fog.flag("cubemapheightfog").unwrap_or(false),
        "height_start": cm("cubemapfogheightstart", 0.0),
        "height_end": cm("cubemapfogheightend", 0.0),
        "height_exponent": fog.number("cubemapfogheightexponent").unwrap_or(1.0),
    })
}

/// The OFAST manifest: everything Unreal sets up directly rather than through glTF.
fn manifest(
    content: &Content,
    map: &Map,
    sky: Option<(String, SkyExport)>,
    ambient: Json,
    warnings: &mut Vec<String>,
) -> Json {
    let sun = map.entities_of("light_environment").next();
    let sky = sky.map(|(file, sky)| {
        json!({
            "hdr": file,
            "material": sky.material,
            "layout": "equirect: u = 0.5 + atan2(x, -y) / 2pi, v = acos(z) / pi (Unreal axes)",
            // Multiplier for the visible sky: the render-only bias, and CS2 radiance → Unreal luminance.
            "sky_intensity": 2f32.powf(sky.render_only_bias) * LUX_PER_UNIT / std::f32::consts::PI,
            "baked_scale": sky.baked_scale.to_vec(),
            "render_only_bias": sky.render_only_bias,
            "mean": sky.mean.to_vec(),
        })
    });
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
        "lux_per_unit": LUX_PER_UNIT,
        "sun": sun.map(sun_manifest),
        "ambient": ambient,
        "sky": sky,
        "post": post_manifest(content, map, warnings),
        "fog": fog_manifest(map),
        "spawns": { "ct": spawns("info_player_counterterrorist"), "t": spawns("info_player_terrorist") },
    })
}
