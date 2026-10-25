//! Rubikon physics data, the `PHYS` block of a compiled model: an agent's ragdoll and a map's world collision.
//!
//! The same structure serves both. A model's `PHYS` block (KV3) holds:
//!
//! | Key | What |
//! |---|---|
//! | `m_boneNames`, `m_bonesHash` | for each **part** (rigid body), the bone it follows (an agent's ragdoll: 15, from `pelvis` to `ankle_r`) |
//! | `m_bindPose` | each part's transform at bind pose, model space: a row-major 3 × 4 matrix (12 floats) |
//! | `m_parts` | the bodies: `m_flMass`, `m_flInertiaScale`, `m_flLinearDamping`, `m_flAngularDamping`, and `m_rnShape` with `m_spheres`, `m_capsules`, `m_hulls`, `m_meshes`, each in the part's frame and tagged with a collision attribute |
//! | `m_joints` | constraints between two parts: `m_nType` (4 = conical, a ragdoll joint), `m_nBody1` / `m_nBody2`, `m_Frame1` / `m_Frame2` (the joint's frame in each body: position, scale, quaternion, 8 floats), `m_SwingLimit` / `m_TwistLimit` (radians, each with its `m_bEnable…` flag), `m_flFriction` |
//! | `m_collisionAttributes` | collision groups and tags: `m_CollisionGroupString` ("Default", "ConditionallySolid"…), `m_InteractAsStrings` (`playerclip`, `npcclip`, `passbullets`…) |
//!
//! Shapes: a sphere is `m_Sphere` (`m_vCenter`, `m_flRadius`); a capsule `m_Capsule` (`m_vCenter`: its two end
//! points, `m_flRadius`); a convex hull `m_Hull` with `m_VertexPositions` (or `m_Vertices` in files compiled before
//! late 2023) as `f32` triples and `m_Planes` as 16-byte planes (normal, offset: a point `p` is inside when
//! `normal · p − offset < 0` for every plane), both binary blobs, and `m_Bounds`; a triangle mesh `m_Mesh` with
//! `m_Vertices` (`f32` triples), `m_Triangles` (three `i32` per triangle; or objects with `m_nIndex`), `m_Materials`
//! (each triangle's surface property, when it has its own) and `m_vMin` / `m_vMax`. A shape's surface property is
//! `m_nSurfacePropertyIndex`, an index into the block's `m_surfacePropertyHashes` (the names' hashes:
//! [`surface_hash`]); a map's world meshes give each triangle its own instead.
//!
//! An agent's ragdoll (`ctm_sas`): 15 capsules, masses 35 (pelvis) down to the hands, 14 conical joints with swing
//! cones of 15° – 69° and twist ranges such as −73° … 36°. A map's `maps/<map>/world_physics.vmdl_c` (de_nuke: 4.7
//! MB) holds the world's collision as thousands of hulls and meshes in world space, most of them "Default", the
//! rest clips.
//!
//! Knowledge from ValveResourceFormat (MIT; `RubikonPhysics/Part.cs`, `Joint.cs`, `Shapes/Hull.cs`, `Shapes/Mesh.cs`),
//! checked on local CS2 files; independent implementation. Details: `docs/formats/model.md`.
//!
//! [`ragdoll`] simulates an agent's ragdoll against a map's world collision.

pub mod ragdoll;

use crate::kv3::Value;
use crate::model::Affine;
use crate::resource::{BlockKind, Resource, ResourceError};

/// Errors reading a `PHYS` block.
#[derive(Debug, thiserror::Error)]
pub enum PhysicsError {
    /// The container or the KV3 block could not be read.
    #[error(transparent)]
    Resource(#[from] ResourceError),
    /// A field is missing or does not have the expected shape.
    #[error("PHYS: {0}")]
    Malformed(String),
}

/// One rigid body.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// `m_flMass` (CS2's agents: 35 for the pelvis, down to 1 – 2 for the hands; 0 for the static world).
    pub mass: f32,
    /// `m_flInertiaScale`: multiplies the inertia the shapes give (7 on the agents' parts).
    pub inertia_scale: f32,
    /// `m_flLinearDamping`.
    pub linear_damping: f32,
    /// `m_flAngularDamping`.
    pub angular_damping: f32,
    /// `m_flLinearDrag` (1 on the agents' parts).
    pub linear_drag: f32,
    /// `m_flAngularDrag` (1 on the agents' parts).
    pub angular_drag: f32,
    /// The part's shapes, in its frame.
    pub shapes: Vec<Shape>,
}

/// A collision shape and the collision attribute it is tagged with.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Index into [`PhysicsData::attributes`].
    pub attribute: usize,
    /// `m_nSurfacePropertyIndex`: index into [`PhysicsData::surfaces`].
    pub surface: usize,
    /// The geometry.
    pub kind: ShapeKind,
}

/// The geometry of a [`Shape`], in its part's frame (inches).
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    /// A sphere.
    Sphere {
        /// Its centre.
        center: [f32; 3],
        /// Its radius.
        radius: f32,
    },
    /// A capsule: the points within `radius` of the segment `a`–`b`.
    Capsule {
        /// One end of the segment.
        a: [f32; 3],
        /// The other end.
        b: [f32; 3],
        /// The radius.
        radius: f32,
    },
    /// A convex hull.
    Hull {
        /// Its corners.
        vertices: Vec<[f32; 3]>,
        /// Its face planes: outward normal and offset (inside when `normal · p − offset < 0` for all).
        planes: Vec<([f32; 3], f32)>,
        /// Bounding box minimum.
        min: [f32; 3],
        /// Bounding box maximum.
        max: [f32; 3],
    },
    /// A triangle mesh.
    Mesh {
        /// Its vertices.
        vertices: Vec<[f32; 3]>,
        /// Vertex indices, three per triangle.
        triangles: Vec<[u32; 3]>,
        /// `m_Materials`: each triangle's surface, an index into [`PhysicsData::surfaces`] like
        /// [`Shape::surface`] (de_nuke's first world mesh, named `" [gravel]"`, has the shape's surface 1, `metal`, and
        /// its triangles' 7); empty when the mesh has none (then [`Shape::surface`] is every triangle's).
        materials: Vec<u32>,
        /// Bounding box minimum.
        min: [f32; 3],
        /// Bounding box maximum.
        max: [f32; 3],
    },
}

/// A joint between two parts.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    /// `m_nType`: 1 spherical, 2 prismatic, 3 revolute, 4 conical (CS2's ragdoll joints), 6 weld, 16 wheel.
    pub kind: i64,
    /// The first part (the parent on a ragdoll).
    pub body1: usize,
    /// The second part.
    pub body2: usize,
    /// The joint's frame in the first part's frame.
    pub frame1: Affine,
    /// The joint's frame in the second part's frame.
    pub frame2: Affine,
    /// `m_SwingLimit` (radians), when enabled.
    pub swing: Option<(f32, f32)>,
    /// `m_TwistLimit` (radians), when enabled.
    pub twist: Option<(f32, f32)>,
    /// `m_flFriction`.
    pub friction: f32,
    /// `m_bEnableCollision`: the two parts collide with each other.
    pub collide: bool,
}

/// A collision group and its interaction tags.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CollisionAttribute {
    /// `m_CollisionGroupString` ("Default", "ConditionallySolid"…; the case varies).
    pub group: String,
    /// `m_InteractAsStrings` (`playerclip`, `npcclip`, `passbullets`…).
    pub interact_as: Vec<String>,
}

impl CollisionAttribute {
    /// The "Default" group: solid to everything.
    pub fn is_default(&self) -> bool {
        self.group.eq_ignore_ascii_case("default")
    }

    /// Solid to a falling body (a ragdoll): the "Default" group, and the "ConditionallySolid" shapes that let only
    /// bullets through (`passbullets`: grates and floor panels, the floor of de_nuke's B site among them), glass
    /// (`window`) and blockers tagged `solid`. The others are clips that stop only some objects: players and NPCs
    /// (`playerclip`, `npcclip`), grenades (`csgo_grenadeclip`), navigation (`navclip`), sound (`blocksound`) and the
    /// sky (`sky`). The tags seen on de_nuke, de_mirage, de_inferno, de_dust2 and de_anubis.
    pub fn stops_bodies(&self) -> bool {
        self.is_default()
            || self
                .interact_as
                .iter()
                .any(|tag| matches!(tag.as_str(), "passbullets" | "window" | "solid"))
    }
}

impl CollisionAttribute {
    /// Stops a bullet: what stops a falling body ([`CollisionAttribute::stops_bodies`]) except the shapes tagged
    /// `passbullets` (grates and floor panels bullets go through) and glass (`window`: a bullet breaks it and goes
    /// on, the demo then sends the breaking glass on its own).
    pub fn stops_bullets(&self) -> bool {
        self.stops_bodies()
            && !self
                .interact_as
                .iter()
                .any(|tag| matches!(tag.as_str(), "passbullets" | "window"))
    }
}

/// A model's physics: its parts, joints and collision attributes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PhysicsData {
    /// For each part, the bone it follows (empty for the world).
    pub bones: Vec<String>,
    /// For each part with a bone, its transform at bind pose (model space).
    pub bind_pose: Vec<Affine>,
    /// The rigid bodies.
    pub parts: Vec<Part>,
    /// The joints between parts.
    pub joints: Vec<Joint>,
    /// Collision attributes the shapes refer to.
    pub attributes: Vec<CollisionAttribute>,
    /// `m_surfacePropertyHashes`: the surface properties the shapes refer to, each the hash of its name
    /// ([`surface_hash`]).
    pub surfaces: Vec<u32>,
}

/// The hash a surface property is named by in physics data (`m_surfacePropertyHashes`) and in
/// `surfaceproperties.vsurf` (`m_nameHash`): MurmurHash2 of the lower-cased name, seed `0x31415926`, as bones'
/// ([`crate::anim::constraints::bone_hash`]). `default` hashes to 1977497166.
pub fn surface_hash(name: &str) -> u32 {
    crate::anim::constraints::bone_hash(name)
}

impl PhysicsData {
    /// Reads the `PHYS` block of a compiled model; `None` when it has none.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Option<Self>, PhysicsError> {
        let kind = BlockKind(*b"PHYS");
        if resource.block(kind).is_none() {
            return Ok(None);
        }
        Ok(Some(Self::parse(&resource.kv3(kind)?.root)?))
    }

    /// Reads a decoded `PHYS` block.
    pub fn parse(phys: &Value) -> Result<Self, PhysicsError> {
        let bones: Vec<String> = phys
            .get("m_boneNames")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let bind_pose = phys
            .get("m_bindPose")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|m| {
                let f = floats(Some(m));
                if f.len() < 12 {
                    return Err(malformed("a bind pose matrix of fewer than 12 floats"));
                }
                Ok(std::array::from_fn(|i| {
                    std::array::from_fn(|j| f[i * 4 + j])
                }))
            })
            .collect::<Result<Vec<Affine>, _>>()?;
        let parts = phys
            .get("m_parts")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(part)
            .collect::<Result<Vec<_>, _>>()?;
        let joints = phys
            .get("m_joints")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|j| joint(j, parts.len()))
            .collect::<Result<Vec<_>, _>>()?;
        // A model's parts each follow a bone; the world's have none.
        if !bones.is_empty() && bones.len() != parts.len() {
            return Err(malformed("bone names and parts differ in number"));
        }
        let attributes = phys
            .get("m_collisionAttributes")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|a| CollisionAttribute {
                group: a
                    .get("m_CollisionGroupString")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                interact_as: a
                    .get("m_InteractAsStrings")
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
            })
            .collect();
        let surfaces = phys
            .get("m_surfacePropertyHashes")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_i64().and_then(|h| u32::try_from(h).ok()))
            .collect();
        Ok(Self {
            bones,
            bind_pose,
            parts,
            joints,
            attributes,
            surfaces,
        })
    }

    /// The surface property hash of `shape` ([`surface_hash`]), if its index is in range.
    pub fn surface(&self, shape: &Shape) -> Option<u32> {
        self.surfaces.get(shape.surface).copied()
    }

    /// The attribute of `shape`, if it exists.
    pub fn attribute(&self, shape: &Shape) -> Option<&CollisionAttribute> {
        self.attributes.get(shape.attribute)
    }
}

fn malformed(what: &str) -> PhysicsError {
    PhysicsError::Malformed(what.to_owned())
}

fn number(value: Option<&Value>) -> Option<f32> {
    value.and_then(Value::as_f64).map(|v| v as f32)
}

/// The numbers of an array value (empty when it is not one).
fn floats(value: Option<&Value>) -> Vec<f32> {
    value
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect()
}

fn vector(value: Option<&Value>) -> [f32; 3] {
    let f = floats(value);
    std::array::from_fn(|k| f.get(k).copied().unwrap_or(0.0))
}

/// A binary blob's bytes.
fn blob(value: Option<&Value>) -> Option<&[u8]> {
    match value?.unflagged() {
        Value::Blob(bytes) => Some(bytes),
        _ => None,
    }
}

/// Little-endian `f32`s of a blob, or the numbers of an array.
fn f32s(value: Option<&Value>) -> Vec<f32> {
    match blob(value) {
        Some(bytes) => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        None => floats(value),
    }
}

/// `[x, y, z]` points: a blob of `f32` triples, or an array of arrays.
fn points(value: Option<&Value>) -> Vec<[f32; 3]> {
    match (blob(value), value.and_then(Value::as_array)) {
        (Some(_), _) => f32s(value).as_chunks::<3>().0.to_vec(),
        (None, Some(items)) if items.first().is_some_and(|v| v.as_array().is_some()) => {
            items.iter().map(|v| vector(Some(v))).collect()
        }
        _ => Vec::new(),
    }
}

/// A transform stored as 8 floats: position, scale (ignored), quaternion `x, y, z, w`.
fn frame(value: Option<&Value>) -> Result<Affine, PhysicsError> {
    let f = floats(value);
    if f.len() < 8 {
        return Err(malformed("a joint frame of fewer than 8 floats"));
    }
    let rotation = crate::model::normalize_quat([f[4], f[5], f[6], f[7]]);
    Ok(crate::model::affine(rotation, [f[0], f[1], f[2]]))
}

fn limit(definition: &Value, enabled: &str, range: &str) -> Option<(f32, f32)> {
    let range = definition.get(range)?;
    (definition.get(enabled).and_then(Value::as_bool) == Some(true)).then(|| {
        (
            number(range.get("m_flMin")).unwrap_or(0.0),
            number(range.get("m_flMax")).unwrap_or(0.0),
        )
    })
}

fn joint(definition: &Value, parts: usize) -> Result<Joint, PhysicsError> {
    let body = |key: &str| {
        definition
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|b| usize::try_from(b).ok())
            .filter(|&b| b < parts)
            .ok_or_else(|| malformed(&format!("joint `{key}` is not a part")))
    };
    Ok(Joint {
        kind: definition
            .get("m_nType")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        body1: body("m_nBody1")?,
        body2: body("m_nBody2")?,
        frame1: frame(definition.get("m_Frame1"))?,
        frame2: frame(definition.get("m_Frame2"))?,
        swing: limit(definition, "m_bEnableSwingLimit", "m_SwingLimit"),
        twist: limit(definition, "m_bEnableTwistLimit", "m_TwistLimit"),
        friction: number(definition.get("m_flFriction")).unwrap_or(0.0),
        collide: definition
            .get("m_bEnableCollision")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn part(definition: &Value) -> Result<Part, PhysicsError> {
    let shape = definition.get("m_rnShape");
    let list = |key: &str| {
        shape
            .and_then(|s| s.get(key))
            .and_then(Value::as_array)
            .unwrap_or_default()
    };
    let index = |s: &Value, key: &str| {
        s.get(key)
            .and_then(Value::as_i64)
            .and_then(|a| usize::try_from(a).ok())
            .unwrap_or(0)
    };
    let attribute = |s: &Value| index(s, "m_nCollisionAttributeIndex");
    let surface = |s: &Value| index(s, "m_nSurfacePropertyIndex");
    let mut shapes = Vec::new();
    for s in list("m_spheres") {
        let sphere = s.get("m_Sphere");
        shapes.push(Shape {
            attribute: attribute(s),
            surface: surface(s),
            kind: ShapeKind::Sphere {
                center: vector(sphere.and_then(|c| c.get("m_vCenter"))),
                radius: number(sphere.and_then(|c| c.get("m_flRadius"))).unwrap_or(0.0),
            },
        });
    }
    for s in list("m_capsules") {
        let capsule = s.get("m_Capsule");
        let ends = points(capsule.and_then(|c| c.get("m_vCenter")));
        let [a, b] = ends[..] else {
            return Err(malformed("a capsule without two end points"));
        };
        shapes.push(Shape {
            attribute: attribute(s),
            surface: surface(s),
            kind: ShapeKind::Capsule {
                a,
                b,
                radius: number(capsule.and_then(|c| c.get("m_flRadius"))).unwrap_or(0.0),
            },
        });
    }
    for s in list("m_hulls") {
        let hull = s.get("m_Hull");
        let field = |key: &str| hull.and_then(|h| h.get(key));
        let vertices = match field("m_VertexPositions") {
            Some(v) => points(Some(v)),
            None => points(field("m_Vertices")),
        };
        let planes = match field("m_Planes").and_then(Value::as_array) {
            // Older files: objects with `m_vNormal` and `m_flOffset`.
            Some(items) if blob(field("m_Planes")).is_none() => items
                .iter()
                .map(|p| {
                    (
                        vector(p.get("m_vNormal")),
                        number(p.get("m_flOffset")).unwrap_or(0.0),
                    )
                })
                .collect(),
            _ => f32s(field("m_Planes"))
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| ([c[0], c[1], c[2]], c[3]))
                .collect(),
        };
        let bounds = field("m_Bounds");
        shapes.push(Shape {
            attribute: attribute(s),
            surface: surface(s),
            kind: ShapeKind::Hull {
                vertices,
                planes,
                min: vector(bounds.and_then(|b| b.get("m_vMinBounds"))),
                max: vector(bounds.and_then(|b| b.get("m_vMaxBounds"))),
            },
        });
    }
    for s in list("m_meshes") {
        let mesh = s.get("m_Mesh");
        let field = |key: &str| mesh.and_then(|m| m.get(key));
        let vertices = points(field("m_Vertices"));
        let triangles: Vec<[u32; 3]> = match blob(field("m_Triangles")) {
            Some(bytes) => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect::<Vec<u32>>()
                .as_chunks::<3>()
                .0
                .to_vec(),
            None => field("m_Triangles")
                .and_then(Value::as_array)
                .unwrap_or_default()
                .iter()
                .map(|t| {
                    let f = floats(t.get("m_nIndex"));
                    std::array::from_fn(|k| f.get(k).copied().unwrap_or(0.0) as u32)
                })
                .collect(),
        };
        if triangles
            .iter()
            .flatten()
            .any(|&i| i as usize >= vertices.len())
        {
            return Err(malformed("a mesh triangle names a vertex it does not have"));
        }
        // Each triangle's own surface, when the mesh has them (a map's world meshes do).
        let materials: Vec<u32> = field("m_Materials")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|m| m.as_i64().and_then(|i| u32::try_from(i).ok()).unwrap_or(0))
            .collect();
        shapes.push(Shape {
            attribute: attribute(s),
            surface: surface(s),
            kind: ShapeKind::Mesh {
                vertices,
                materials: if materials.len() == triangles.len() {
                    materials
                } else {
                    Vec::new()
                },
                triangles,
                min: vector(field("m_vMin")),
                max: vector(field("m_vMax")),
            },
        });
    }
    Ok(Part {
        mass: number(definition.get("m_flMass")).unwrap_or(0.0),
        inertia_scale: number(definition.get("m_flInertiaScale")).unwrap_or(1.0),
        linear_damping: number(definition.get("m_flLinearDamping")).unwrap_or(0.0),
        angular_damping: number(definition.get("m_flAngularDamping")).unwrap_or(0.0),
        linear_drag: number(definition.get("m_flLinearDrag")).unwrap_or(0.0),
        angular_drag: number(definition.get("m_flAngularDrag")).unwrap_or(0.0),
        shapes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv3::text;

    /// A two-part ragdoll and a world hull and mesh, in the KV3 layout CS2 uses.
    fn sample() -> Value {
        let plane = |n: [f32; 3], d: f32| {
            n.iter()
                .chain(std::iter::once(&d))
                .flat_map(|f| f.to_le_bytes())
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let floats_blob = |values: &[f32]| {
            values
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let ints_blob = |values: &[u32]| {
            values
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let text = format!(
            "{{
                m_boneNames = [ \"pelvis\", \"leg_upper_l\" ]
                m_bindPose = [ [ 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 40.0 ],
                               [ 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 4.0, 0.0, 0.0, 1.0, 38.0 ] ]
                m_parts = [
                    {{ m_flMass = 35.0 m_flInertiaScale = 7.0 m_flLinearDamping = 0.0 m_flAngularDamping = 2.0
                       m_rnShape = {{ m_spheres = [ ] m_capsules = [ {{ m_nCollisionAttributeIndex = 0
                           m_Capsule = {{ m_vCenter = [ [ 3.9, 0.0, 0.27 ], [ 16.2, 0.0, 0.27 ] ] m_flRadius = 4.0 }} }} ]
                           m_hulls = [ ] m_meshes = [ ] }} }},
                    {{ m_flMass = 0.0 m_rnShape = {{ m_spheres = [ {{ m_nCollisionAttributeIndex = 1
                           m_Sphere = {{ m_vCenter = [ 0.0, 0.0, 1.0 ] m_flRadius = 2.0 }} }} ]
                       m_capsules = [ ]
                       m_hulls = [ {{ m_nCollisionAttributeIndex = 1 m_Hull = {{
                           m_Bounds = {{ m_vMinBounds = [ 0.0, 0.0, 0.0 ] m_vMaxBounds = [ 1.0, 1.0, 1.0 ] }}
                           m_VertexPositions = #[ {} ]
                           m_Planes = #[ {} ] }} }} ]
                       m_meshes = [ {{ m_nCollisionAttributeIndex = 0 m_nSurfacePropertyIndex = 1 m_Mesh = {{
                           m_vMin = [ 0.0, 0.0, 0.0 ] m_vMax = [ 1.0, 1.0, 0.0 ]
                           m_Materials = [ 2 ]
                           m_Vertices = #[ {} ]
                           m_Triangles = #[ {} ] }} }} ] }} }}
                ]
                m_surfacePropertyHashes = [ 1977497166, 417707814, 2858402244 ]
                m_joints = [ {{ m_nType = 4 m_nBody1 = 0 m_nBody2 = 1
                    m_Frame1 = [ -3.7, 0.87, 3.7, 1.0, 0.0, 0.0, 0.0, 1.0 ]
                    m_Frame2 = [ 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.70710677, 0.70710677 ]
                    m_bEnableCollision = false
                    m_bEnableSwingLimit = true m_SwingLimit = {{ m_flMin = 0.0 m_flMax = 0.26 }}
                    m_bEnableTwistLimit = false m_TwistLimit = {{ m_flMin = -1.28 m_flMax = 0.62 }}
                    m_flFriction = 0.38 }} ]
                m_collisionAttributes = [
                    {{ m_CollisionGroupString = \"Default\" m_InteractAsStrings = [ ] }},
                    {{ m_CollisionGroupString = \"ConditionallySolid\" m_InteractAsStrings = [ \"playerclip\" ] }} ]
            }}",
            floats_blob(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
            plane([1.0, 0.0, 0.0], 1.0),
            floats_blob(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            ints_blob(&[0, 1, 2]),
        );
        text::parse(&text).expect("sample parses").root
    }

    #[test]
    fn reads_parts_joints_and_shapes() {
        let physics = PhysicsData::parse(&sample()).expect("reads");
        assert_eq!(physics.bones, ["pelvis", "leg_upper_l"]);
        assert_eq!(physics.bind_pose[1][1][3], 4.0);
        assert_eq!(physics.bind_pose[1][2][3], 38.0);
        let pelvis = &physics.parts[0];
        assert_eq!(
            (pelvis.mass, pelvis.inertia_scale, pelvis.angular_damping),
            (35.0, 7.0, 2.0)
        );
        assert_eq!(
            pelvis.shapes[0].kind,
            ShapeKind::Capsule {
                a: [3.9, 0.0, 0.27],
                b: [16.2, 0.0, 0.27],
                radius: 4.0
            }
        );
        let world = &physics.parts[1];
        assert!(matches!(
            world.shapes[0].kind,
            ShapeKind::Sphere { radius: 2.0, .. }
        ));
        let ShapeKind::Hull {
            vertices,
            planes,
            max,
            ..
        } = &world.shapes[1].kind
        else {
            panic!("hull");
        };
        assert_eq!(
            (vertices.len(), planes[0], *max),
            (2, ([1.0, 0.0, 0.0], 1.0), [1.0; 3])
        );
        let ShapeKind::Mesh {
            vertices,
            triangles,
            materials,
            ..
        } = &world.shapes[2].kind
        else {
            panic!("mesh");
        };
        assert_eq!((vertices.len(), triangles[0]), (3, [0, 1, 2]));
        // The mesh's own surface and its triangle's: indices into the hashes.
        assert_eq!(
            (world.shapes[2].surface, materials.as_slice()),
            (1, &[2][..])
        );
        assert_eq!(physics.surface(&world.shapes[2]), Some(417_707_814));
        assert_eq!(physics.surface(&world.shapes[0]), Some(1_977_497_166));
        assert_eq!(surface_hash("default"), 1_977_497_166);
        let joint = &physics.joints[0];
        assert_eq!((joint.kind, joint.body1, joint.body2), (4, 0, 1));
        assert_eq!(joint.swing, Some((0.0, 0.26)));
        assert_eq!(joint.twist, None, "a disabled limit is no limit");
        assert_eq!(joint.frame1[0][3], -3.7);
        // A quarter turn about z in the second frame: x goes to y.
        assert!((joint.frame2[1][0] - 1.0).abs() < 1e-6);
        assert!(
            physics
                .attribute(&world.shapes[2])
                .is_some_and(CollisionAttribute::is_default)
        );
        assert!(!physics.attributes[1].is_default());
        assert_eq!(physics.attributes[1].interact_as, ["playerclip"]);
    }

    #[test]
    fn rejects_broken_references() {
        let mut broken = text::to_string(&crate::kv3::Kv3File {
            format: crate::kv3::Guid::FORMAT_GENERIC,
            root: sample(),
        });
        let text = broken.clone();
        broken = broken.replace("m_nBody2 = 1", "m_nBody2 = 7");
        let root = text::parse(&broken).expect("parses").root;
        assert!(matches!(
            PhysicsData::parse(&root),
            Err(PhysicsError::Malformed(_))
        ));
        // Three bone names for two parts.
        let extra = text.replace("\"leg_upper_l\"", "\"leg_upper_l\", \"leg_lower_l\"");
        let root = text::parse(&extra).expect("parses").root;
        assert!(matches!(
            PhysicsData::parse(&root),
            Err(PhysicsError::Malformed(_))
        ));
    }
}
