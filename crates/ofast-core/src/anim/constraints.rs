//! A model's procedural bones: the twist, point and aim constraints of its `BoneConstraintList`.
//!
//! An agent has more bones than its clips drive: twist bones along the arms and thighs (`arm_lower_R_TWIST`,
//! `leg_upper_L_TWIST1`…, skeleton flag `0x400000`), a `head_0_TWIST` between neck and head, the shoulder blades
//! (`scapula_L` / `_R`) and their aim helpers. CS2 places them after the animation, from constraints the model
//! carries in its keyvalues: `DATA` → `m_modelInfo.m_keyValueText`, a KV3 text document
//! (`format:vmdlkeys4`) whose `BoneConstraintList` array holds one object per constraint, applied in that order in
//! model space. Without them a forearm turned far (the flashed pose) leaves its sleeve untwisted at the wrist, and a
//! raised arm leaves the shoulder blade where it was.
//!
//! What CS2's agent and glove models use (91 of the 92 carry constraints): 1183 `CTiltTwistConstraint`, 182
//! `CAimConstraint`, 91 `CPointConstraint` and 160 `CBoneConstraintDotToMorph` (the eyes' look morphs, which drive
//! no bone: not applied). An agent like `ctm_sas` has 13 tilt-twists (head, upper and lower arms, thighs), one point
//! (`head_0_TWIST` halfway between `neck_0` and `head_0`) and two aims (each scapula towards `scap_L_AIMAT` /
//! `scap_R_AIMAT` on the spine, its up towards `scap_AIMUP`).
//!
//! Each constraint lists **slaves** (`m_slaves`: the bones it moves, each with a weight) and **targets**
//! (`m_targets`: the bones it reads, each with a weight and an offset `m_qOffset` / `m_vOffset` in the target's
//! frame). Bones are named by hash: MurmurHash2 of the lower-cased bone name with seed `0x31415926` ([`bone_hash`];
//! all 26 hashes of `ctm_sas` resolve), or by `m_sName` when that is set.
//!
//! | Class | What it does to its slave |
//! |---|---|
//! | `CTiltTwistConstraint` | takes the target's rotation relative to its parent, removes the offset, measures its twist about axis `m_nTargetAxis` (0 = x) ignoring the tilt of that axis (`twist_angle`), and sets the slave's local rotation to that angle × the slave's weight about axis `m_nSlaveAxis` |
//! | `CPointConstraint` | moves the slave to the weighted mean of the targets' positions |
//! | `CAimConstraint` | turns the slave so its +Y points at the weighted mean of the targets (all but the last when there are several) and its +Z towards an up direction, then by `m_qAimOffset`; `m_nUpType` 0 = `m_vUpVector` in the last target's frame, 1 = `m_vUpVector` in model space, 2 = towards the last target, 3 = `m_vUpVector` in the slave's parent frame |
//!
//! A slave's children move with it (none of the 15 slaves of any CS2 agent has children). Each frame stands alone:
//! a twist bone with weight 0.5 covers ±90°, so a roll crossing ±180° flips it by 180°. Source 2 Viewer unwraps the
//! weighted angle from frame to frame, which changes no rotation (θ and θ + 2π are the same turn); it is left out.
//! A tilt-twist whose target has no parent in this skeleton (a glove model's arm-only skeleton) is left out: it was
//! authored against that parent.
//!
//! Knowledge from ValveResourceFormat (MIT; `Renderer/Animation/BoneConstraintSolver.cs`,
//! `ModelAnimation/BoneConstraints.cs`), which mirrors Source 2's solver; checked here by the bind pose coming out
//! unchanged on every agent (`tests/cs2_anim.rs`). Independent implementation. Details: `docs/formats/animation.md`.

use super::layers::{conjugate, slerp};
use super::matrix_to_quat;
use crate::kv3::{self, Value};
use crate::model::{
    Affine, Model, Skeleton, affine, compose, invert_rigid, normalize_quat, quat_mul,
};

/// Seed of the hash that names bones in constraints ([`bone_hash`]).
pub const BONE_HASH_SEED: u32 = 0x3141_5926;

/// The 32-bit hash a constraint names a bone by: MurmurHash2 of the lower-cased name, seed [`BONE_HASH_SEED`]
/// (Source 2's string token).
pub fn bone_hash(name: &str) -> u32 {
    const M: u32 = 0x5bd1_e995;
    let data = name.to_ascii_lowercase().into_bytes();
    let mut h = BONE_HASH_SEED ^ data.len() as u32;
    let (words, tail) = data.as_chunks::<4>();
    for word in words {
        let mut k = u32::from_le_bytes(*word).wrapping_mul(M);
        k ^= k >> 24;
        h = h.wrapping_mul(M) ^ k.wrapping_mul(M);
    }
    if !tail.is_empty() {
        for (i, &byte) in tail.iter().enumerate() {
            h ^= u32::from(byte) << (8 * i);
        }
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

/// A bone a constraint moves.
#[derive(Debug, Clone)]
struct Slave {
    bone: usize,
    parent: Option<usize>,
    weight: f32,
    /// Every bone below it, carried along when it moves.
    descendants: Vec<usize>,
}

/// A bone a constraint reads.
#[derive(Debug, Clone, Copy)]
struct Target {
    bone: usize,
    /// `m_qOffset` / `m_vOffset`, in the target's frame.
    offset: Affine,
    weight: f32,
}

#[derive(Debug, Clone)]
enum Constraint {
    TiltTwist {
        slave: Slave,
        target: usize,
        target_parent: usize,
        /// The inverse of the target's `m_qOffset`.
        inverse_offset: [f32; 4],
        target_axis: usize,
        slave_axis: usize,
    },
    Point {
        slaves: Vec<Slave>,
        targets: Vec<Target>,
    },
    Aim {
        slaves: Vec<Slave>,
        targets: Vec<Target>,
        up_vector: [f32; 3],
        aim_offset: [f32; 4],
        up_type: i64,
    },
}

/// The constraints of one model, resolved against its skeleton, in the order CS2 applies them.
#[derive(Debug, Clone, Default)]
pub struct BoneConstraints {
    constraints: Vec<Constraint>,
    /// Constraints left out, and why: classes that move no bone (`CBoneConstraintDotToMorph`), unknown classes,
    /// bones or attachments the skeleton lacks.
    pub skipped: Vec<String>,
}

impl BoneConstraints {
    /// The constraints in `model`'s keyvalues (none when it has no `BoneConstraintList`).
    pub fn of(model: &Model) -> Self {
        if model.key_values_text.is_empty() {
            return Self::default();
        }
        Self::from_text(&model.key_values_text, &model.skeleton)
    }

    /// The constraints in a keyvalues text (`m_keyValueText`), resolved against `skeleton`.
    pub fn from_text(text: &str, skeleton: &Skeleton) -> Self {
        match kv3::text::parse(text) {
            Ok(file) => Self::from_value(&file.root, skeleton),
            Err(e) => Self {
                constraints: Vec::new(),
                skipped: vec![format!("keyvalues unreadable: {e}")],
            },
        }
    }

    /// The constraints of a parsed keyvalues root (its `BoneConstraintList`).
    pub fn from_value(root: &Value, skeleton: &Skeleton) -> Self {
        let mut out = Self::default();
        let Some(list) = root.get("BoneConstraintList").and_then(Value::as_array) else {
            return out;
        };
        let resolver = Resolver::new(skeleton);
        for (n, definition) in list.iter().enumerate() {
            let class = definition
                .get("_class")
                .and_then(Value::as_str)
                .unwrap_or("");
            match resolver.constraint(class, definition) {
                Ok(constraint) => out.constraints.push(constraint),
                Err(why) => out.skipped.push(format!("#{n} {class}: {why}")),
            }
        }
        out
    }

    /// Number of constraints applied.
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// `true` when nothing is applied.
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }

    /// Applies every constraint, in order, to the model-space bone transforms `world` (the skeleton's bones, as
    /// [`super::Retargeter::world`] gives them).
    pub fn apply(&self, world: &mut [Affine]) {
        for constraint in &self.constraints {
            match constraint {
                Constraint::TiltTwist {
                    slave,
                    target,
                    target_parent,
                    inverse_offset,
                    target_axis,
                    slave_axis,
                } => {
                    let rotation = quat_mul(
                        conjugate(rotation_of(&world[*target_parent])),
                        rotation_of(&world[*target]),
                    );
                    let angle = twist_angle(quat_mul(*inverse_offset, rotation), *target_axis)
                        * slave.weight;
                    set_local_rotation(world, slave, axis_angle(*slave_axis, angle));
                }
                Constraint::Point { slaves, targets } => {
                    let Some((position, total)) = blend_positions(targets, world) else {
                        continue;
                    };
                    for slave in slaves {
                        let weight = total * slave.weight;
                        if weight.abs() <= 1e-4 {
                            continue;
                        }
                        let mut moved = world[slave.bone];
                        for (k, row) in moved.iter_mut().enumerate() {
                            row[3] = if (weight - 1.0).abs() <= 1e-4 {
                                position[k]
                            } else {
                                row[3] + (position[k] - row[3]) * weight
                            };
                        }
                        write_back(world, slave, moved);
                    }
                }
                Constraint::Aim {
                    slaves,
                    targets,
                    up_vector,
                    aim_offset,
                    up_type,
                } => aim(world, slaves, targets, *up_vector, *aim_offset, *up_type),
            }
        }
    }
}

/// Bone lookups for reading constraints.
struct Resolver<'a> {
    skeleton: &'a Skeleton,
    hashes: Vec<u32>,
}

impl<'a> Resolver<'a> {
    fn new(skeleton: &'a Skeleton) -> Self {
        Self {
            skeleton,
            hashes: skeleton.bones.iter().map(|b| bone_hash(&b.name)).collect(),
        }
    }

    /// The bone an `m_slaves` / `m_targets` entry names: by `m_sName` when set, else by `m_nBoneHash`.
    fn bone(&self, entry: &Value) -> Result<usize, String> {
        let hash = match entry.get("m_sName").and_then(Value::as_str) {
            Some(name) if !name.is_empty() => bone_hash(name),
            _ => entry
                .get("m_nBoneHash")
                .and_then(Value::as_i64)
                .and_then(|h| u32::try_from(h).ok())
                .ok_or("an entry without a bone")?,
        };
        self.hashes
            .iter()
            .position(|&h| h == hash)
            .ok_or_else(|| format!("no bone with hash {hash}"))
    }

    /// One `m_slaves` entry. A missing weight is 0, as Source 2 Viewer reads it.
    fn slave(&self, entry: &Value) -> Result<Slave, String> {
        let bone = self.bone(entry)?;
        Ok(Slave {
            bone,
            parent: self.skeleton.bones[bone].parent,
            weight: number(entry.get("m_flWeight")).unwrap_or(0.0),
            descendants: self.descendants(bone),
        })
    }

    fn slaves(&self, definition: &Value) -> Result<Vec<Slave>, String> {
        let entries = definition
            .get("m_slaves")
            .and_then(Value::as_array)
            .unwrap_or_default();
        entries.iter().map(|entry| self.slave(entry)).collect()
    }

    fn targets(&self, definition: &Value) -> Result<Vec<Target>, String> {
        let entries = definition
            .get("m_targets")
            .and_then(Value::as_array)
            .unwrap_or_default();
        entries
            .iter()
            .map(|entry| {
                if entry.get("m_bIsAttachment").and_then(Value::as_bool) == Some(true) {
                    // No CS2 agent aims at an attachment; left out rather than guessed.
                    return Err("a target on an attachment".to_owned());
                }
                Ok(Target {
                    bone: self.bone(entry)?,
                    offset: affine(
                        quaternion(entry.get("m_qOffset")),
                        vector(entry.get("m_vOffset")),
                    ),
                    weight: number(entry.get("m_flWeight")).unwrap_or(0.0),
                })
            })
            .collect()
    }

    /// Every bone below `bone` (parents come before children, so one pass finds them).
    fn descendants(&self, bone: usize) -> Vec<usize> {
        let mut below = vec![false; self.skeleton.bones.len()];
        let mut out = Vec::new();
        for (i, b) in self.skeleton.bones.iter().enumerate() {
            if b.parent.is_some_and(|p| p == bone || below[p]) {
                below[i] = true;
                out.push(i);
            }
        }
        out
    }

    fn constraint(&self, class: &str, definition: &Value) -> Result<Constraint, String> {
        let axis = |key: &str| {
            let a = definition.get(key).and_then(Value::as_i64).unwrap_or(0);
            a.clamp(0, 2) as usize
        };
        match class {
            "CTiltTwistConstraint" => {
                // Only the first slave and the first target count.
                let first_of = |key: &str| {
                    definition
                        .get(key)
                        .and_then(Value::as_array)
                        .and_then(|entries| entries.first())
                };
                let slave = self.slave(first_of("m_slaves").ok_or("no slave")?)?;
                let first = first_of("m_targets").ok_or("no target")?;
                let target = self.bone(first)?;
                // The twist is measured against the target's parent: without it (a glove model's skeleton starts
                // at the upper arm) the angle would be read in model space.
                let target_parent = self.skeleton.bones[target]
                    .parent
                    .ok_or("the target has no parent in this skeleton")?;
                Ok(Constraint::TiltTwist {
                    slave,
                    target,
                    target_parent,
                    inverse_offset: conjugate(normalize_quat(quaternion(first.get("m_qOffset")))),
                    target_axis: axis("m_nTargetAxis"),
                    slave_axis: axis("m_nSlaveAxis"),
                })
            }
            "CPointConstraint" => Ok(Constraint::Point {
                slaves: self.slaves(definition)?,
                targets: self.targets(definition)?,
            }),
            "CAimConstraint" => Ok(Constraint::Aim {
                slaves: self.slaves(definition)?,
                targets: self.targets(definition)?,
                up_vector: vector(definition.get("m_vUpVector")),
                aim_offset: normalize_quat(quaternion(definition.get("m_qAimOffset"))),
                up_type: definition
                    .get("m_nUpType")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            }),
            "CBoneConstraintDotToMorph" => Err("drives a morph, no bone".into()),
            _ => Err("class not supported".into()),
        }
    }
}

fn number(value: Option<&Value>) -> Option<f32> {
    value.and_then(Value::as_f64).map(|v| v as f32)
}

fn vector(value: Option<&Value>) -> [f32; 3] {
    let items = value.and_then(Value::as_array).unwrap_or_default();
    std::array::from_fn(|k| number(items.get(k)).unwrap_or(0.0))
}

/// A quaternion `[x, y, z, w]`; identity when missing.
fn quaternion(value: Option<&Value>) -> [f32; 4] {
    let items = value.and_then(Value::as_array).unwrap_or_default();
    if items.len() < 4 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    std::array::from_fn(|k| number(items.get(k)).unwrap_or(0.0))
}

fn rotation_of(m: &Affine) -> [f32; 4] {
    matrix_to_quat(m)
}

fn position_of(m: &Affine) -> [f32; 3] {
    [m[0][3], m[1][3], m[2][3]]
}

/// The rotation by `angle` radians about the unit axis `axis` (0 = x, 1 = y, 2 = z).
fn axis_angle(axis: usize, angle: f32) -> [f32; 4] {
    let mut q = [0.0, 0.0, 0.0, (angle / 2.0).cos()];
    q[axis] = (angle / 2.0).sin();
    q
}

/// The twist of `rotation` about the unit axis `axis`, ignoring the tilt of that axis — Source 2's tilt-twist
/// decomposition (as Source 2 Viewer reads it): with no tilt, the turn of the next axis within the plane of the
/// other two; else the turn of the other two axes about the tilted axis, measured against the plane the tilt
/// happened in. Radians, −π … π.
fn twist_angle(rotation: [f32; 4], axis: usize) -> f32 {
    let r = crate::model::quat_matrix(normalize_quat(rotation));
    // Component `j` of where unit axis `i` turns to: column `i` of the matrix.
    let basis = |i: usize, j: usize| r[j][i];
    let (axis1, axis2) = ((axis + 1) % 3, (axis + 2) % 3);
    const EPSILON: f32 = 1e-4;
    // Rounded to 7 decimals first, as Source 2 does.
    let diagonal = ((basis(axis, axis) * 1e7 + 0.5).floor() / 1e7).clamp(-1.0, 1.0);
    let tilt = if diagonal < 0.0 {
        (diagonal + 1.0).acos()
    } else {
        (1.0 - diagonal).acos()
    };
    if (tilt - std::f32::consts::FRAC_PI_2).abs() > EPSILON {
        // Tilted: the direction the axis leaned to, then the turn of the other two axes about it.
        let (mut a, mut b) = (basis(axis, axis1), basis(axis, axis2));
        let inverse_length = 1.0 / (a * a + b * b).sqrt().max(f32::MIN_POSITIVE);
        a *= inverse_length;
        b *= inverse_length;
        let (m1, m2) = (basis(axis1, axis), basis(axis2, axis));
        return (m2 * a - m1 * b).atan2(-m1 * a - m2 * b);
    }
    let diagonal1 = basis(axis1, axis1);
    if (diagonal1 - 1.0).abs() <= EPSILON {
        return 0.0;
    }
    basis(axis1, axis2).atan2(diagonal1)
}

/// The slave's rotation relative to its parent replaced by `local` (its offset from the parent kept).
fn set_local_rotation(world: &mut [Affine], slave: &Slave, local: [f32; 4]) {
    let current = world[slave.bone];
    let moved = match slave.parent {
        Some(p) => {
            let parent = world[p];
            let offset = position_of(&compose(&invert_rigid(&parent), &current));
            compose(&parent, &affine(local, offset))
        }
        None => affine(local, position_of(&current)),
    };
    write_back(world, slave, moved);
}

/// Puts the slave at `moved` and carries its descendants along.
fn write_back(world: &mut [Affine], slave: &Slave, moved: Affine) {
    let delta = compose(&moved, &invert_rigid(&world[slave.bone]));
    world[slave.bone] = moved;
    for &d in &slave.descendants {
        world[d] = compose(&delta, &world[d]);
    }
}

/// A target's transform with its offset.
fn target_transform(target: &Target, world: &[Affine]) -> Affine {
    compose(&world[target.bone], &target.offset)
}

/// The weighted mean of the targets' positions and their total weight; `None` without targets.
fn blend_positions(targets: &[Target], world: &[Affine]) -> Option<([f32; 3], f32)> {
    if targets.is_empty() {
        return None;
    }
    let mut sum = [0.0f32; 3];
    let mut total = 0.0;
    for target in targets {
        let p = position_of(&target_transform(target, world));
        for k in 0..3 {
            sum[k] += p[k] * target.weight;
        }
        total += target.weight;
    }
    if total != 0.0 {
        sum = sum.map(|c| c / total);
    }
    Some((sum, total))
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let m = crate::model::quat_matrix(q);
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * v[k]).sum())
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    (length > 1e-6).then(|| v.map(|c| c / length))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| a[k] * b[k]).sum()
}

/// The rotation turning +Y along `forward` and +Z towards `up`; `None` when they are parallel or zero.
fn look_rotation(forward: [f32; 3], up: [f32; 3]) -> Option<[f32; 4]> {
    let forward = normalized(forward)?;
    let along = dot(up, forward);
    let up = normalized(std::array::from_fn(|k| up[k] - forward[k] * along))?;
    let side = normalized(cross(forward, up))?;
    // Columns: where x, y and z go.
    let m: Affine = std::array::from_fn(|i| [side[i], forward[i], up[i], 0.0]);
    Some(matrix_to_quat(&m))
}

/// `q` with the sign that makes it nearest `reference` (same rotation).
fn align(reference: [f32; 4], q: [f32; 4]) -> [f32; 4] {
    if (0..4).map(|k| reference[k] * q[k]).sum::<f32>() < 0.0 {
        q.map(|c| -c)
    } else {
        q
    }
}

/// `CAimConstraint` (see the module docs).
fn aim(
    world: &mut [Affine],
    slaves: &[Slave],
    targets: &[Target],
    up_vector: [f32; 3],
    aim_offset: [f32; 4],
    up_type: i64,
) {
    // Up types 0 and 2 read the last target, which then takes no part in the aim.
    let up_target = match up_type {
        0 | 2 => match targets.last() {
            Some(last) if targets.len() > 1 => Some(target_transform(last, world)),
            _ => return,
        },
        _ => None,
    };
    let aimed = if targets.len() > 1 {
        &targets[..targets.len() - 1]
    } else {
        targets
    };
    let Some((goal, total)) = blend_positions(aimed, world) else {
        return;
    };
    for slave in slaves {
        let weight = total * slave.weight;
        if weight.abs() <= 1e-4 {
            continue;
        }
        let current = world[slave.bone];
        let parent = slave
            .parent
            .map_or(affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]), |p| world[p]);
        let parent_rotation = rotation_of(&parent);
        let up = match (up_type, &up_target) {
            (1, _) => Some(up_vector),
            (3, _) => Some(rotate(parent_rotation, up_vector)),
            (2, Some(t)) => normalized(std::array::from_fn(|k| {
                position_of(t)[k] - position_of(&current)[k]
            })),
            (_, Some(t)) => Some(rotate(rotation_of(t), up_vector)),
            // Other up types: `m_vUpVector` as it is, as Source 2 Viewer falls back.
            _ => Some(up_vector),
        };
        let inverse_parent = invert_rigid(&parent);
        let local_goal = position_of(&compose(
            &inverse_parent,
            &affine([0.0, 0.0, 0.0, 1.0], goal),
        ));
        let local_position = position_of(&compose(&inverse_parent, &current));
        let direction = std::array::from_fn(|k| local_goal[k] - local_position[k]);
        // A slave on its goal, or an up along the aim, has no direction: left as it is.
        let Some(look) =
            up.and_then(|up| look_rotation(direction, rotate(conjugate(parent_rotation), up)))
        else {
            continue;
        };
        let local = quat_mul(look, align(look, aim_offset));
        let rotation = quat_mul(parent_rotation, align(parent_rotation, local));
        let rotation = if (weight - 1.0).abs() <= 1e-4 {
            rotation
        } else {
            slerp(rotation_of(&current), rotation, weight)
        };
        write_back(world, slave, affine(rotation, position_of(&current)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Bone;
    use std::f32::consts::FRAC_PI_2;

    fn bone(name: &str, parent: Option<usize>, position: [f32; 3], rotation: [f32; 4]) -> Bone {
        Bone {
            name: name.into(),
            parent,
            position,
            rotation,
            flags: 0,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn same(a: &Affine, b: &Affine) -> bool {
        (0..3).all(|i| (0..4).all(|j| (a[i][j] - b[i][j]).abs() < 1e-3))
    }

    #[test]
    fn bone_hashes_match_the_models() {
        // From `ctm_sas`'s constraints.
        assert_eq!(bone_hash("head_0_TWIST"), 2957772116);
        assert_eq!(bone_hash("head_0"), 1462898209);
        assert_eq!(bone_hash("neck_0"), 2008498285);
        assert_eq!(bone_hash("arm_lower_R_TWIST1"), 400205573);
    }

    #[test]
    fn twist_ignores_tilt() {
        for axis in 0..3 {
            for angle in [0.3, -1.2, 2.5] {
                // A pure twist reads back.
                assert!(
                    close(twist_angle(axis_angle(axis, angle), axis), angle),
                    "{axis} {angle}"
                );
                // A tilt alone (about another axis) is no twist.
                assert!(close(
                    twist_angle(axis_angle((axis + 1) % 3, angle), axis),
                    0.0
                ));
                // A twist, then a tilt, keeps the twist.
                let tilted = quat_mul(axis_angle((axis + 2) % 3, 0.4), axis_angle(axis, angle));
                assert!(close(twist_angle(tilted, axis), angle), "{axis} {angle}");
            }
        }
    }

    /// An arm: `root`, `forearm` (bind turned 90° about z), its `twist` bone and the `hand`.
    fn arm() -> Skeleton {
        let turned = axis_angle(2, FRAC_PI_2);
        Skeleton {
            bones: vec![
                bone("root", None, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                bone("forearm", Some(0), [0.0, 0.0, 50.0], turned),
                bone(
                    "forearm_TWIST",
                    Some(1),
                    [5.0, 0.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ),
                bone("hand", Some(1), [10.0, 0.0, 0.0], turned),
                bone("hand_tip", Some(3), [2.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
            ],
        }
    }

    /// One constraint in keyvalues text.
    fn keyvalues(constraint: &str) -> String {
        format!("{{ BoneConstraintList = [ {constraint} ] }}")
    }

    fn entry(name: &str, weight: f32, offset: [f32; 4]) -> String {
        format!(
            "{{ m_nBoneHash = {} m_flWeight = {weight} m_qOffset = [ {}, {}, {}, {} ] m_vOffset = [ 0.0, 0.0, 0.0 ] }}",
            bone_hash(name),
            offset[0],
            offset[1],
            offset[2],
            offset[3]
        )
    }

    #[test]
    fn tilt_twist_shares_the_hands_roll() {
        let skeleton = arm();
        let turned = axis_angle(2, FRAC_PI_2);
        let text = keyvalues(&format!(
            "{{ _class = \"CTiltTwistConstraint\" m_slaves = [ {} ] m_targets = [ {} ] m_nTargetAxis = 0 m_nSlaveAxis = 0 }}",
            entry("forearm_TWIST", 0.5, [0.0, 0.0, 0.0, 1.0]),
            entry("hand", 1.0, turned)
        ));
        let constraints = BoneConstraints::from_text(&text, &skeleton);
        assert_eq!((constraints.len(), constraints.skipped.len()), (1, 0));
        let bind = skeleton.world_transforms();

        // At bind pose the offset cancels the hand's own turn: nothing moves.
        let mut world = bind.clone();
        constraints.apply(&mut world);
        assert!(world.iter().zip(&bind).all(|(a, b)| same(a, b)));

        // The hand rolled 60° about its x: the twist bone takes half, about its own x.
        let rolled = |degrees: f32| {
            let mut world = bind.clone();
            let roll = quat_mul(turned, axis_angle(0, degrees.to_radians()));
            world[3] = compose(&world[1], &affine(roll, [10.0, 0.0, 0.0]));
            world[4] = compose(&world[3], &affine([0.0, 0.0, 0.0, 1.0], [2.0, 0.0, 0.0]));
            constraints.apply(&mut world);
            compose(&invert_rigid(&world[1]), &world[2])
        };
        let half =
            |degrees: f32| affine(axis_angle(0, (degrees / 2.0).to_radians()), [5.0, 0.0, 0.0]);
        assert!(same(&rolled(60.0), &half(60.0)));
        assert!(same(&rolled(-60.0), &half(-60.0)));
        // Past ±180° the measure wraps: a roll of 200° reads as −160°, and the half-twist bone flips to −80°.
        assert!(same(&rolled(200.0), &half(-160.0)));
    }

    #[test]
    fn twists_measured_against_a_missing_parent_are_left_out() {
        // A glove-like skeleton: the twist's target is a root.
        let skeleton = Skeleton {
            bones: vec![
                bone("hand", None, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                bone(
                    "forearm_TWIST",
                    Some(0),
                    [5.0, 0.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ),
            ],
        };
        let text = keyvalues(&format!(
            "{{ _class = \"CTiltTwistConstraint\" m_slaves = [ {} ] m_targets = [ {} ] }}",
            entry("forearm_TWIST", 0.5, [0.0, 0.0, 0.0, 1.0]),
            entry("hand", 1.0, [0.0, 0.0, 0.0, 1.0])
        ));
        let constraints = BoneConstraints::from_text(&text, &skeleton);
        assert!(constraints.is_empty());
        assert!(
            constraints.skipped[0].contains("no parent"),
            "{:?}",
            constraints.skipped
        );
    }

    #[test]
    fn point_goes_between_its_targets_and_carries_children() {
        let skeleton = arm();
        let text = keyvalues(&format!(
            "{{ _class = \"CPointConstraint\" m_slaves = [ {} ] m_targets = [ {}, {} ] }}",
            entry("hand", 1.0, [0.0, 0.0, 0.0, 1.0]),
            entry("root", 0.5, [0.0, 0.0, 0.0, 1.0]),
            entry("forearm", 0.5, [0.0, 0.0, 0.0, 1.0])
        ));
        let constraints = BoneConstraints::from_text(&text, &skeleton);
        let mut world = skeleton.world_transforms();
        let tip_from_hand = compose(&invert_rigid(&world[3]), &world[4]);
        constraints.apply(&mut world);
        assert_eq!(position_of(&world[3]), [0.0, 0.0, 25.0]);
        // The child moved with it.
        assert!(same(
            &compose(&invert_rigid(&world[3]), &world[4]),
            &tip_from_hand
        ));
    }

    #[test]
    fn aim_points_y_at_the_target_and_z_up() {
        let skeleton = Skeleton {
            bones: vec![
                bone("spine", None, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                bone("scapula", Some(0), [0.0, 0.0, 10.0], [0.0, 0.0, 0.0, 1.0]),
                bone("aim_at", Some(0), [10.0, 0.0, 10.0], [0.0, 0.0, 0.0, 1.0]),
                bone("aim_up", Some(0), [0.0, 0.0, 30.0], [0.0, 0.0, 0.0, 1.0]),
            ],
        };
        let text = keyvalues(&format!(
            "{{ _class = \"CAimConstraint\" m_slaves = [ {} ] m_targets = [ {}, {} ] m_vUpVector = [ 0.0, 0.0, 1.0 ] \
             m_qAimOffset = [ 0.0, 0.0, 0.0, 1.0 ] m_nUpType = 2 }}",
            entry("scapula", 1.0, [0.0, 0.0, 0.0, 1.0]),
            entry("aim_at", 1.0, [0.0, 0.0, 0.0, 1.0]),
            entry("aim_up", 0.0, [0.0, 0.0, 0.0, 1.0])
        ));
        let constraints = BoneConstraints::from_text(&text, &skeleton);
        let mut world = skeleton.world_transforms();
        constraints.apply(&mut world);
        let m = world[1];
        // +Y (column 1) along +x towards `aim_at`, +Z (column 2) up towards `aim_up`.
        assert!(close(m[0][1], 1.0) && close(m[2][2], 1.0), "{m:?}");
        assert_eq!(position_of(&m), [0.0, 0.0, 10.0]);
    }

    #[test]
    fn bones_that_drive_no_bone_are_skipped() {
        let text = keyvalues(
            "{ _class = \"CBoneConstraintDotToMorph\" m_sBoneName = \"eyeball_r\" }, \
             { _class = \"CPointConstraint\" m_slaves = [ { m_nBoneHash = 12345 } ] m_targets = [ ] }",
        );
        let constraints = BoneConstraints::from_text(&text, &arm());
        assert!(constraints.is_empty());
        assert_eq!(constraints.skipped.len(), 2, "{:?}", constraints.skipped);
        assert!(BoneConstraints::from_text("{ a = ", &arm()).skipped[0].contains("unreadable"));
    }
}
