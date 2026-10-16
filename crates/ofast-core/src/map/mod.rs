//! Compiled maps: the world (`world.vwrld_c`), its world nodes (`*.vwnod_c`) and entity lumps (`*.vents_c`).
//!
//! # How a CS2 map is stored
//!
//! ```text
//! game/csgo/maps/de_mirage.vpk            (single-file VPK, mounted before pak01)
//!  └─ maps/de_mirage/
//!      ├─ world.vwrld_c                   world: list of world nodes, entity lumps, lighting info
//!      ├─ worldnodes/n0.vwnod_c           what to draw: scene objects + aggregates (+ clutter)
//!      ├─ worldnodes/*.vmdl_c             the baked geometry the scene objects point at
//!      ├─ entities/default_ents.vents_c   entities: lights, dynamic props, spawns, sky, triggers…
//!      └─ lightmaps/*.vtex_c              baked lighting (not used by OFAST yet: Unreal relights)
//! ```
//!
//! - **Scene objects** draw a whole model with a transform (identity for baked world geometry, whose vertices are
//!   already in world space).
//! - **Aggregate scene objects** draw *individual draw calls* ("fragments") of one model; most fragments are
//!   already in world space, some carry their own transform (instancing of static props).
//! - **Static props are not entities** in CS2: they are baked into scene objects and aggregates. Dynamic props,
//!   brush entities, lights, spawns and the sky are entities.
//!
//! Details: `docs/formats/map.md`. Knowledge from ValveResourceFormat (MIT) and the SteamDatabase schema dumps;
//! independent implementation.

mod entities;

pub use entities::{Entity, EntityValue};

use crate::content::Content;
use crate::kv3::Value;
use crate::resource::{BlockKind, Resource, ResourceError};
use crate::vpk::VpkError;

/// Everything that can go wrong while reading a map.
#[derive(Debug, thiserror::Error)]
pub enum MapError {
    /// A file is missing from the archives.
    #[error(transparent)]
    Vpk(#[from] VpkError),
    /// A resource could not be parsed.
    #[error(transparent)]
    Resource(#[from] ResourceError),
}

/// A 3×4 affine transform in Source space: rows `(R_i0, R_i1, R_i2, T_i)`, i.e. column vectors, translation in the
/// last column.
pub type Transform = [[f32; 4]; 3];

/// The identity transform.
pub const IDENTITY: Transform = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// `m_nObjectTypeFlags` bits that matter to OFAST.
pub mod object_flags {
    /// Shadow-only geometry ("block light" tool meshes): not visible, skip when exporting.
    pub const BLOCK_LIGHT: u32 = 16;
    /// A decal/overlay drawn on top of other geometry.
    pub const OVERLAY: u32 = 8192;
}

/// A whole model placed in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneObject {
    /// Model path (as stored: no `_c`).
    pub model: String,
    /// Placement in Source space.
    pub transform: Transform,
    /// Tint colour (linear RGBA, white when unset).
    pub tint: [f32; 4],
    /// `OBJECT_TYPE_*` flags (see [`object_flags`]).
    pub flags: u32,
    /// Draw order among overlays.
    pub overlay_order: i64,
}

/// One fragment of an aggregate: a single draw call of the aggregate's model.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    /// Index into the model's draw calls (all scene objects' draw calls, in order).
    pub draw_call: usize,
    /// Own placement, or `None` when the vertices are already in world space.
    pub transform: Option<Transform>,
    /// Tint colour (RGB 0–1, alpha 1).
    pub tint: [f32; 4],
}

/// A model whose draw calls are placed individually.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    /// Model path (as stored: no `_c`).
    pub model: String,
    /// Fragments to draw.
    pub fragments: Vec<Fragment>,
}

/// The drawable content of one world node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldNode {
    /// Whole-model placements.
    pub scene_objects: Vec<SceneObject>,
    /// Per-draw-call placements.
    pub aggregates: Vec<Aggregate>,
}

/// A parsed map.
#[derive(Debug, Clone, Default)]
pub struct Map {
    /// Map name, e.g. `de_mirage`.
    pub name: String,
    /// World nodes (geometry).
    pub nodes: Vec<WorldNode>,
    /// All entities from all entity lumps (child lumps included).
    pub entities: Vec<Entity>,
}

impl Map {
    /// Loads `maps/<name>/world.vwrld_c`, its world nodes and its entity lumps from `content`
    /// (use [`Content::for_map`] so the map's own archive is searched first).
    pub fn load(content: &Content, name: &str) -> Result<Self, MapError> {
        let world_data = content.read(&format!("maps/{name}/world.vwrld_c"))?;
        let world = Resource::parse(&world_data)?.kv3(BlockKind::DATA)?.root;

        let mut map = Map {
            name: name.to_owned(),
            ..Map::default()
        };
        for node in array(world.get("m_worldNodes")) {
            if let Some(prefix) = node.get("m_worldNodePrefix").and_then(Value::as_str) {
                let data = content.read_resource(&format!("{prefix}.vwnod"))?;
                let root = Resource::parse(&data)?.kv3(BlockKind::DATA)?.root;
                map.nodes.push(parse_world_node(&root));
            }
        }

        // Entity lumps can reference child lumps; load them all (breadth first, each once).
        let mut pending: Vec<String> = array(world.get("m_entityLumps"))
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(lump) = pending.pop() {
            if !seen.insert(lump.clone()) {
                continue;
            }
            let data = content.read_resource(&lump)?;
            let root = Resource::parse(&data)?.kv3(BlockKind::DATA)?.root;
            for child in array(root.get("m_childLumps")) {
                if let Some(child) = child.as_str() {
                    pending.push(child.to_owned());
                }
            }
            for entity in array(root.get("m_entityKeyValues")) {
                if let Some(entity) = Entity::from_lump_entry(entity) {
                    map.entities.push(entity);
                }
            }
        }
        Ok(map)
    }

    /// Entities of a class (`"light_environment"`, `"info_player_counterterrorist"`, …).
    pub fn entities_of<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a Entity> + 'a {
        self.entities
            .iter()
            .filter(move |e| e.class_name() == Some(class))
    }

    /// The map's 3D skybox, from its `skybox_reference` entity.
    pub fn skybox_reference(&self) -> Option<SkyboxReference> {
        let entity = self.entities_of("skybox_reference").next()?;
        let target = entity.string("targetmapname")?.replace('\\', "/");
        // "maps/prefabs/de_mirage/3dskybox_mirage_legacy.vmap" → world "prefabs/de_mirage/3dskybox_mirage_legacy"
        let without_ext = target.strip_suffix(".vmap").unwrap_or(&target);
        let world = without_ext
            .strip_prefix("maps/")
            .unwrap_or(without_ext)
            .to_owned();
        Some(SkyboxReference {
            archive: format!("{without_ext}.vpk"),
            world,
            origin: entity.origin(),
            angles: entity.angles(),
        })
    }

    /// The `sky_camera` of a skybox world: its origin and scale (how many main-map units one skybox unit stands
    /// for; 16 when the key is missing, the editor's default).
    pub fn sky_camera(&self) -> Option<([f32; 3], f32)> {
        let camera = self.entities_of("sky_camera").next()?;
        let scale = camera.number("scale").filter(|s| *s > 0.0).unwrap_or(16.0);
        Some((camera.origin(), scale))
    }
}

/// Where a map's 3D skybox lives and where it is anchored in the main map.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyboxReference {
    /// The skybox's own archive, relative to `game/csgo` (e.g. `maps/prefabs/de_mirage/3dskybox_mirage_legacy.vpk`).
    pub archive: String,
    /// The skybox world name for [`Map::load`] (e.g. `prefabs/de_mirage/3dskybox_mirage_legacy`).
    pub world: String,
    /// The `skybox_reference` origin in the main map.
    pub origin: [f32; 3],
    /// The `skybox_reference` angles (pitch, yaw, roll) — the whole skybox turns with them.
    pub angles: [f32; 3],
}

/// Transform that places skybox geometry in the main world: scaled up around the sky camera, then turned and moved
/// like the reference entity — `world = reference.origin + R(reference.angles) · scale · (p − camera)`.
///
/// The engine keeps the skybox small and draws it from a second camera at `(eye − reference) / scale + camera`;
/// baking the scale into the geometry gives the same picture. Same composition as Source 2 Viewer's `SkyTransform`.
pub fn sky_to_world(camera: [f32; 3], scale: f32, reference: &SkyboxReference) -> Transform {
    let placed = entity_transform(reference.origin, reference.angles, [scale; 3]);
    let mut to_camera = IDENTITY;
    for (row, c) in to_camera.iter_mut().zip(camera) {
        row[3] = -c;
    }
    compose(&placed, &to_camera)
}

fn array(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).unwrap_or_default()
}

/// All numbers inside a value, flattened (handles nested arrays and typed arrays alike).
pub(crate) fn numbers(value: &Value, out: &mut Vec<f32>) {
    match value.unflagged() {
        Value::Array(items) => items.iter().for_each(|v| numbers(v, out)),
        other => {
            if let Some(f) = other.as_f64() {
                out.push(f as f32);
            }
        }
    }
}

/// Reads a 3×4 transform stored as three 4-vectors or twelve floats (row-major).
fn transform(value: Option<&Value>) -> Option<Transform> {
    let mut flat = Vec::with_capacity(12);
    numbers(value?, &mut flat);
    (flat.len() == 12).then(|| {
        [
            [flat[0], flat[1], flat[2], flat[3]],
            [flat[4], flat[5], flat[6], flat[7]],
            [flat[8], flat[9], flat[10], flat[11]],
        ]
    })
}

/// `m_nObjectTypeFlags` is stored as an int for combinations and as an enum name for single flags.
fn object_type_flags(value: Option<&Value>) -> u32 {
    match value.map(Value::unflagged) {
        Some(Value::String(name)) => name
            .split('|')
            .map(|part| match part.trim() {
                "OBJECT_TYPE_MODEL" => 8,
                "OBJECT_TYPE_BLOCK_LIGHT" => object_flags::BLOCK_LIGHT,
                "OBJECT_TYPE_NO_SHADOWS" => 32,
                "OBJECT_TYPE_RENDER_TO_CUBEMAPS" => 1024,
                "OBJECT_TYPE_OVERLAY" => object_flags::OVERLAY,
                "OBJECT_TYPE_PRECOMPUTED_VISMEMBERS" => 16384,
                "OBJECT_TYPE_DISABLE_VIS_CULLING" => 65536,
                "OBJECT_TYPE_BAKED_GEOMETRY" => 131_072,
                _ => 0,
            })
            .fold(0, |acc, bit| acc | bit),
        Some(other) => other.as_i64().unwrap_or(0) as u32,
        None => 0,
    }
}

fn parse_world_node(root: &Value) -> WorldNode {
    let mut node = WorldNode::default();
    for object in array(root.get("m_sceneObjects")) {
        let model = object
            .get("m_renderableModel")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if model.is_empty() {
            continue; // `m_renderable` (mesh-only) objects are not used by CS2 maps so far
        }
        let mut tint = [1.0; 4];
        let mut values = Vec::new();
        if let Some(v) = object.get("m_vTintColor") {
            numbers(v, &mut values);
        }
        if values.len() == 4 {
            tint = [values[0], values[1], values[2], values[3]];
            if tint[3] == 0.0 {
                tint[3] = 1.0;
            }
        }
        node.scene_objects.push(SceneObject {
            model: model.to_owned(),
            transform: transform(object.get("m_vTransform")).unwrap_or(IDENTITY),
            tint,
            flags: object_type_flags(object.get("m_nObjectTypeFlags")),
            overlay_order: object
                .get("m_nOverlayRenderOrder")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        });
    }

    for aggregate in array(root.get("m_aggregateSceneObjects")) {
        let Some(model) = aggregate.get("m_renderableModel").and_then(Value::as_str) else {
            continue;
        };
        let transforms = array(aggregate.get("m_fragmentTransforms"));
        let mut next_transform = 0;
        let mut fragments = Vec::new();
        for mesh in array(aggregate.get("m_aggregateMeshes")) {
            let has_transform = matches!(
                mesh.get("m_bHasTransform").map(Value::unflagged),
                Some(Value::Bool(true))
            );
            let fragment_transform = if has_transform {
                // Transforms are consumed in order by the fragments that have one.
                let t = transforms
                    .get(next_transform)
                    .and_then(|v| transform(Some(v)));
                next_transform += 1;
                t
            } else {
                None
            };
            let mut rgb = Vec::new();
            if let Some(v) = mesh.get("m_vTintColor") {
                numbers(v, &mut rgb);
            }
            let tint = if rgb.len() >= 3 {
                [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0, 1.0]
            } else {
                [1.0; 4]
            };
            fragments.push(Fragment {
                draw_call: mesh
                    .get("m_nDrawCallIndex")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .max(0) as usize,
                transform: fragment_transform,
                tint,
            });
        }
        node.aggregates.push(Aggregate {
            model: model.to_owned(),
            fragments,
        });
    }
    node
}

/// Multiplies two 3×4 transforms (`a` applied after `b`).
pub fn compose(a: &Transform, b: &Transform) -> Transform {
    let mut out = [[0.0; 4]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell =
                (0..3).map(|k| a[r][k] * b[k][c]).sum::<f32>() + if c == 3 { a[r][3] } else { 0.0 };
        }
    }
    out
}

/// Builds a transform from an entity's `origin`, `angles` (pitch, yaw, roll in degrees) and `scales`.
///
/// Rotation = `Rz(yaw) · Ry(pitch) · Rx(roll)` (roll applied first); positive pitch tilts +X downward.
pub fn entity_transform(origin: [f32; 3], angles: [f32; 3], scales: [f32; 3]) -> Transform {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();
    // Columns of the rotation: forward (X), left (Y), up (Z).
    let rot = [
        [cp * cy, sr * sp * cy - cr * sy, cr * sp * cy + sr * sy],
        [cp * sy, sr * sp * sy + cr * cy, cr * sp * sy - sr * cy],
        [-sp, sr * cp, cr * cp],
    ];
    let mut t = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..3 {
            t[i][j] = rot[i][j] * scales[j];
        }
        t[i][3] = origin[i];
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Transform, b: &Transform) -> bool {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn yaw_90_turns_forward_into_left() {
        let t = entity_transform([10.0, 20.0, 30.0], [0.0, 90.0, 0.0], [1.0; 3]);
        // Forward (+X) column becomes +Y.
        assert!((t[0][0]).abs() < 1e-6 && (t[1][0] - 1.0).abs() < 1e-6);
        assert_eq!([t[0][3], t[1][3], t[2][3]], [10.0, 20.0, 30.0]);
    }

    #[test]
    fn positive_pitch_looks_down() {
        let t = entity_transform([0.0; 3], [30.0, 0.0, 0.0], [1.0; 3]);
        assert!(t[2][0] < 0.0, "forward has a negative Z component");
    }

    #[test]
    fn compose_with_identity_is_a_no_op() {
        let t = entity_transform([1.0, 2.0, 3.0], [10.0, 20.0, 30.0], [2.0; 3]);
        assert!(close(&compose(&IDENTITY, &t), &t));
        assert!(close(&compose(&t, &IDENTITY), &t));
    }

    fn apply(t: &Transform, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| t[i][0] * p[0] + t[i][1] * p[1] + t[i][2] * p[2] + t[i][3])
    }

    #[test]
    fn sky_geometry_scales_around_the_sky_camera() {
        // de_mirage: sky camera at (5208, -968, 44.5), scale 16, reference at the origin.
        let reference = SkyboxReference {
            archive: String::new(),
            world: String::new(),
            origin: [0.0; 3],
            angles: [0.0; 3],
        };
        let t = sky_to_world([5208.0, -968.0, 44.5], 16.0, &reference);
        assert_eq!(apply(&t, [5208.0, -968.0, 44.5]), [0.0; 3]);
        assert_eq!(apply(&t, [5209.0, -968.0, 45.5]), [16.0, 0.0, 16.0]);

        // A turned, moved reference turns the scaled offset and adds its origin.
        let turned = SkyboxReference {
            origin: [100.0, 0.0, 0.0],
            angles: [0.0, 90.0, 0.0],
            ..reference
        };
        let p = apply(&sky_to_world([0.0; 3], 16.0, &turned), [1.0, 0.0, 0.0]);
        assert!(
            (p[0] - 100.0).abs() < 1e-3 && (p[1] - 16.0).abs() < 1e-3,
            "{p:?}"
        );
    }

    #[test]
    fn transforms_parse_from_flat_or_nested_floats() {
        let flat = Value::Array((0..12).map(|i| Value::Float(f64::from(i))).collect());
        let nested = Value::Array(
            (0..3)
                .map(|r| Value::Array((0..4).map(|c| Value::Float(f64::from(r * 4 + c))).collect()))
                .collect(),
        );
        assert_eq!(transform(Some(&flat)), transform(Some(&nested)));
        assert_eq!(transform(Some(&flat)).unwrap()[1][3], 7.0);
    }

    #[test]
    fn object_flags_accept_names_and_numbers() {
        assert_eq!(
            object_type_flags(Some(&Value::String("OBJECT_TYPE_OVERLAY".into()))),
            object_flags::OVERLAY
        );
        assert_eq!(object_type_flags(Some(&Value::Int(1040))), 1040);
    }
}
