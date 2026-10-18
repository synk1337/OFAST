//! A model's skeleton (`DATA` → `m_modelSkeleton`) and the bind-pose math OFAST needs to skin meshes.
//!
//! # Layout
//!
//! `m_modelSkeleton` stores the bones as parallel arrays, one entry per bone:
//!
//! | Key | Value |
//! |---|---|
//! | `m_boneName` | bone name (`root_motion`, `pelvis`, `hand_R`, …) |
//! | `m_nParent` | index of the parent bone, `-1` for a root |
//! | `m_bonePosParent` | translation from the parent, in inches, in the parent's frame |
//! | `m_boneRotParent` | rotation from the parent, a quaternion `(x, y, z, w)` |
//! | `m_boneScaleParent` | scale, ~1 on every CS2 agent (0.999999 – 1.000017): ignored |
//! | `m_nFlag` | `0x10` physics, `0x20` attachment, `0x40` animation, `0x80` mesh, `0x100` hitbox, `0x400 …` LODs, `0x400000` procedural (twist bones) |
//!
//! A bone's bind pose in model space is `W = W_parent · T(position) · R(rotation)` (column vectors). Parents always
//! come before their children (checked on all 93 agent and glove models), which [`Skeleton::world_transforms`]
//! relies on. CS2 agents: 86 – 94 bones from `root_motion`, whose rotation (0.5, 0.5, 0.5, 0.5) turns the A-pose to
//! face +X. Each mesh also carries its own `m_skeleton` with inverse bind poses; they equal the inverse of these world
//! transforms (to 3e-3), so OFAST computes them from the chain instead.
//!
//! Vertices name bones through a per-mesh remapping table ([`super::Model::bone_remap`]).
//!
//! Knowledge from ValveResourceFormat (MIT; `ModelAnimation/Skeleton.cs`, `Bone.cs`), checked on local CS2 files;
//! independent implementation. Details: `docs/formats/model.md`.

use crate::kv3::Value;

/// One bone of a skeleton, in its parent's frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    /// Bone name as stored (case matters for display; attachments and hitboxes spell some names in lower case).
    pub name: String,
    /// Parent bone index, `None` for a root.
    pub parent: Option<usize>,
    /// Translation from the parent, inches, parent frame.
    pub position: [f32; 3],
    /// Rotation from the parent, unit quaternion `(x, y, z, w)`.
    pub rotation: [f32; 4],
    /// `m_nFlag` bits (see the module docs).
    pub flags: u32,
}

/// A model's bones, parents before children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    /// Bones in model order: vertex bone indices (after remapping) and animation tracks refer to this order.
    pub bones: Vec<Bone>,
}

/// A rigid transform as a 3 × 4 matrix: rotation in columns 0 – 2, translation in column 3 (rows are x, y, z).
pub type Affine = [[f32; 4]; 3];

impl Skeleton {
    /// Reads `m_modelSkeleton` from a model's `DATA` block (an empty skeleton when it has none).
    pub fn parse(data: &Value) -> Self {
        let Some(skeleton) = data.get("m_modelSkeleton") else {
            return Self::default();
        };
        let names: Vec<&str> = skeleton
            .get("m_boneName")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|v| v.as_str().unwrap_or_default())
            .collect();
        let parents = list(skeleton.get("m_nParent"));
        let positions = list(skeleton.get("m_bonePosParent"));
        let rotations = list(skeleton.get("m_boneRotParent"));
        let flags = list(skeleton.get("m_nFlag"));
        let bones = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let number = |v: Option<&Value>, k: usize| -> f32 {
                    let mut values = Vec::new();
                    if let Some(v) = v {
                        crate::map::numbers(v, &mut values);
                    }
                    values.get(k).copied().unwrap_or(0.0)
                };
                let position = std::array::from_fn(|k| number(positions.get(i).copied(), k));
                let mut rotation = std::array::from_fn(|k| number(rotations.get(i).copied(), k));
                if rotations.get(i).is_none() {
                    rotation = [0.0, 0.0, 0.0, 1.0];
                }
                let parent = parents.get(i).and_then(|v| v.as_i64()).unwrap_or(-1);
                Bone {
                    name: (*name).to_owned(),
                    // A parent at or after the bone itself would break the parents-first order: treated as a root.
                    parent: (parent >= 0 && (parent as usize) < i).then_some(parent as usize),
                    position,
                    rotation: normalize_quat(rotation),
                    flags: flags.get(i).and_then(|v| v.as_i64()).unwrap_or(0) as u32,
                }
            })
            .collect();
        Self { bones }
    }

    /// Whether the skeleton can skin anything: CS2's placeholder models (`characters/models/*` stubs, props) have
    /// none or a single `dummy` bone.
    pub fn is_real(&self) -> bool {
        self.bones.len() > 1
    }

    /// Index of the bone named `name` (exact first, then ignoring case: attachments spell `hand_R` as `hand_r`).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name).or_else(|| {
            self.bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(name))
        })
    }

    /// Each bone's bind pose in model space (Source axes, inches).
    pub fn world_transforms(&self) -> Vec<Affine> {
        let mut world: Vec<Affine> = Vec::with_capacity(self.bones.len());
        for bone in &self.bones {
            let local = affine(bone.rotation, bone.position);
            let w = match bone.parent {
                Some(p) => compose(&world[p], &local),
                None => local,
            };
            world.push(w);
        }
        world
    }
}

/// The values of a KV3 array (empty when absent).
fn list(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .collect()
}

/// A quaternion scaled to unit length (identity when it is zero).
pub fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len < 1e-12 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        q.map(|c| c / len)
    }
}

/// Hamilton product `a ⊗ b` of quaternions `(x, y, z, w)`: rotate by `b`, then by `a`.
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The rotation matrix of a unit quaternion `(x, y, z, w)`, rows first.
pub fn quat_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// The transform "rotate by `rotation`, then move by `translation`".
pub fn affine(rotation: [f32; 4], translation: [f32; 3]) -> Affine {
    let r = quat_matrix(rotation);
    std::array::from_fn(|i| [r[i][0], r[i][1], r[i][2], translation[i]])
}

/// `a · b`: apply `b`, then `a`.
pub fn compose(a: &Affine, b: &Affine) -> Affine {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let mut v: f32 = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            if j == 3 {
                v += a[i][3];
            }
            v
        })
    })
}

/// The inverse of a rigid transform (rotation transposed, translation turned back).
pub fn invert_rigid(m: &Affine) -> Affine {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            if j < 3 {
                m[j][i]
            } else {
                -(0..3).map(|k| m[k][i] * m[k][3]).sum::<f32>()
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv3::Value;

    fn floats(values: &[f32]) -> Value {
        Value::Array(values.iter().map(|&v| Value::Float(f64::from(v))).collect())
    }

    /// A three-bone chain like an agent's start: a rotated root, then two children.
    fn sample() -> Value {
        let h = 0.5f32;
        let skeleton = Value::Object(
            [
                (
                    "m_boneName",
                    Value::Array(
                        ["root_motion", "pelvis", "spine_0"]
                            .iter()
                            .map(|s| Value::String((*s).into()))
                            .collect(),
                    ),
                ),
                (
                    "m_nParent",
                    Value::Array(vec![Value::Int(-1), Value::Int(0), Value::Int(1)]),
                ),
                (
                    "m_bonePosParent",
                    Value::Array(vec![
                        floats(&[0.0, 0.0, 0.0]),
                        floats(&[0.0, 42.8248, -2.8535]),
                        floats(&[0.0, 0.0, 5.0]),
                    ]),
                ),
                (
                    "m_boneRotParent",
                    Value::Array(vec![
                        floats(&[h, h, h, h]),
                        floats(&[0.0, 0.0, 0.0, 1.0]),
                        floats(&[0.0, 0.0, 0.0, 1.0]),
                    ]),
                ),
                (
                    "m_nFlag",
                    Value::Array(vec![Value::Int(0x40), Value::Int(0xC0), Value::Int(0xC0)]),
                ),
            ]
            .into_iter()
            .collect(),
        );
        Value::Object([("m_modelSkeleton", skeleton)].into_iter().collect())
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn parses_the_parallel_arrays() {
        let s = Skeleton::parse(&sample());
        assert_eq!(s.bones.len(), 3);
        assert_eq!(s.bones[1].name, "pelvis");
        assert_eq!(s.bones[0].parent, None);
        assert_eq!(s.bones[2].parent, Some(1));
        assert_eq!(s.bones[1].flags, 0xC0);
        assert!(s.is_real());
        assert_eq!(s.find("PELVIS"), Some(1));
    }

    #[test]
    fn the_root_rotation_stands_the_pelvis_up() {
        // As on ctm_sas: the pelvis's local offset (0, 42.8, -2.85) under root_motion's (0.5, 0.5, 0.5, 0.5) lands at
        // (-2.85, 0, 42.8) in model space, 42.8 inches up.
        let world = Skeleton::parse(&sample()).world_transforms();
        let pelvis = [world[1][0][3], world[1][1][3], world[1][2][3]];
        assert!(close(pelvis, [-2.8535, 0.0, 42.8248]), "{pelvis:?}");
    }

    #[test]
    fn inverse_undoes_the_world_transform() {
        for w in Skeleton::parse(&sample()).world_transforms() {
            let id = compose(&invert_rigid(&w), &w);
            for (i, row) in id.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    let expected = if i == j { 1.0 } else { 0.0 };
                    assert!((v - expected).abs() < 1e-5, "{id:?}");
                }
            }
        }
    }

    #[test]
    fn quaternion_product_matches_matrix_product() {
        let a = normalize_quat([0.1, 0.7, -0.2, 0.6]);
        let b = normalize_quat([-0.4, 0.2, 0.5, 0.3]);
        let via_quat = quat_matrix(quat_mul(a, b));
        let (ma, mb) = (quat_matrix(a), quat_matrix(b));
        for i in 0..3 {
            for j in 0..3 {
                let v: f32 = (0..3).map(|k| ma[i][k] * mb[k][j]).sum();
                assert!((v - via_quat[i][j]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_model_without_skeleton_has_no_bones() {
        assert!(!Skeleton::parse(&Value::Null).is_real());
    }
}
