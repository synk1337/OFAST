//! Putting a clip pose (on its `.vnmskel` bones) onto a model's skeleton, by bone name and model-space pose.

use super::{NmSkeleton, Transform};
use crate::model::{Affine, Skeleton, affine, compose, invert_rigid};

/// Maps a model's bones onto a clip skeleton's, once per skeleton pair.
///
/// A model bone named like a clip bone (exact, then ignoring case) takes that bone's model-space pose; any other
/// bone (twist, jiggle, aim helpers the clip skeleton lacks) keeps its bind-pose offset from its parent. This is
/// Source 2 Viewer's rule (`SkeletonRetargeter.cs`): matching world poses rather than local ones makes the result
/// independent of how each skeleton splits a transform between a bone and its parent (an agent puts the A-pose's
/// turn on `root_motion`, the clip skeleton on `pelvis`).
#[derive(Debug, Clone)]
pub struct Retargeter {
    /// For each model bone, the clip bone driving it.
    model_to_clip: Vec<Option<usize>>,
    /// Each model bone's bind pose relative to its parent.
    bind_local: Vec<Affine>,
    /// Model bone parents.
    parents: Vec<Option<usize>>,
    /// Clip bone parents.
    clip_parents: Vec<Option<usize>>,
}

impl Retargeter {
    /// Builds the bone map between `model` and the clip skeleton `clip`.
    pub fn new(model: &Skeleton, clip: &NmSkeleton) -> Self {
        Self {
            model_to_clip: model.bones.iter().map(|b| clip.find(&b.name)).collect(),
            bind_local: model
                .bones
                .iter()
                .map(|b| affine(b.rotation, b.position))
                .collect(),
            parents: model.bones.iter().map(|b| b.parent).collect(),
            clip_parents: clip.parents.clone(),
        }
    }

    /// How many model bones a clip drives (the others follow at bind pose).
    pub fn mapped_bones(&self) -> usize {
        self.model_to_clip.iter().flatten().count()
    }

    /// Model-space poses of the model's bones for a clip pose (one transform per clip track).
    pub fn world(&self, pose: &[Transform]) -> Vec<Affine> {
        let mut clip_world: Vec<Affine> = Vec::with_capacity(pose.len());
        for (i, bone) in pose.iter().enumerate() {
            let local = bone.to_affine();
            clip_world.push(match self.clip_parents.get(i).copied().flatten() {
                Some(p) => compose(&clip_world[p], &local),
                None => local,
            });
        }
        let mut world: Vec<Affine> = Vec::with_capacity(self.parents.len());
        for m in 0..self.parents.len() {
            let w = match self.model_to_clip[m].and_then(|c| clip_world.get(c)) {
                Some(w) => *w,
                None => match self.parents[m] {
                    Some(p) => compose(&world[p], &self.bind_local[m]),
                    None => self.bind_local[m],
                },
            };
            world.push(w);
        }
        world
    }

    /// The model's bones relative to their parents for a clip pose: translation (inches) and rotation, ready to
    /// key on the model's own bones.
    pub fn locals(&self, pose: &[Transform]) -> Vec<([f32; 3], [f32; 4])> {
        self.locals_of(&self.world(pose))
    }

    /// [`Self::locals`] from model-space poses already computed with [`Self::world`].
    pub fn locals_of(&self, world: &[Affine]) -> Vec<([f32; 3], [f32; 4])> {
        (0..self.parents.len())
            .map(|m| {
                let local = match self.parents[m] {
                    Some(p) => compose(&invert_rigid(&world[p]), &world[m]),
                    None => world[m],
                };
                (
                    [local[0][3], local[1][3], local[2][3]],
                    matrix_to_quat(&local),
                )
            })
            .collect()
    }
}

/// The unit quaternion `(x, y, z, w)` of a 3 × 4 matrix's rotation part (Shepperd's method).
pub(crate) fn matrix_to_quat(m: &Affine) -> [f32; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
            0.25 * s,
        ]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [
            0.25 * s,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[2][1] - m[1][2]) / s,
        ]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [
            (m[0][1] + m[1][0]) / s,
            0.25 * s,
            (m[1][2] + m[2][1]) / s,
            (m[0][2] - m[2][0]) / s,
        ]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [
            (m[0][2] + m[2][0]) / s,
            (m[1][2] + m[2][1]) / s,
            0.25 * s,
            (m[1][0] - m[0][1]) / s,
        ]
    };
    crate::model::normalize_quat(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bone, normalize_quat, quat_matrix};

    fn bone(name: &str, parent: Option<usize>, position: [f32; 3], rotation: [f32; 4]) -> Bone {
        Bone {
            name: name.into(),
            parent,
            position,
            rotation,
            flags: 0,
        }
    }

    /// An agent-like model (the A-pose turn on root_motion, an extra twist bone) and a clip skeleton that puts the
    /// turn on pelvis instead.
    fn skeletons() -> (Skeleton, NmSkeleton) {
        let h = 0.5;
        let model = Skeleton {
            bones: vec![
                bone("root_motion", None, [0.0; 3], [h, h, h, h]),
                bone("pelvis", Some(0), [0.0, 42.8, -2.85], [0.0, 0.0, 0.0, 1.0]),
                bone(
                    "thigh_twist_L",
                    Some(1),
                    [1.0, 2.0, 3.0],
                    [0.0, 0.0, 0.0, 1.0],
                ),
                bone("spine_0", Some(1), [5.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
            ],
        };
        let clip = NmSkeleton {
            bones: vec!["root_motion".into(), "pelvis".into(), "spine_0".into()],
            parents: vec![None, Some(0), Some(1)],
            reference: vec![
                Transform::IDENTITY,
                Transform {
                    translation: [-2.85, 0.0, 42.8],
                    rotation: [h, h, h, h],
                    scale: 1.0,
                },
                Transform {
                    translation: [5.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: 1.0,
                },
            ],
        };
        (model, clip)
    }

    #[test]
    fn the_reference_pose_lands_on_the_bind_pose() {
        let (model, clip) = skeletons();
        let r = Retargeter::new(&model, &clip);
        assert_eq!(r.mapped_bones(), 3);
        let world = r.world(&clip.reference);
        let bind = model.world_transforms();
        // Every bone below the root lands on its bind pose, the unmapped twist bone included. The root itself takes
        // the clip's root (the identity; the agent's root_motion carries the A-pose turn instead), as in Source 2
        // Viewer: no skinned vertex follows root_motion.
        for (m, (w, b)) in world.iter().zip(&bind).enumerate().skip(1) {
            for i in 0..3 {
                for j in 0..4 {
                    assert!((w[i][j] - b[i][j]).abs() < 1e-4, "bone {m}: {w:?} vs {b:?}");
                }
            }
        }
        assert_eq!(world[0], Transform::IDENTITY.to_affine());
    }

    #[test]
    fn unmapped_bones_follow_their_parent_and_locals_rebuild_the_pose() {
        let (model, clip) = skeletons();
        let r = Retargeter::new(&model, &clip);
        // Lift and turn the pelvis: the twist bone (not in the clip) must move with it.
        let mut pose = clip.reference.clone();
        pose[1].translation[2] += 10.0;
        pose[1].rotation = normalize_quat([0.6, 0.4, 0.5, 0.5]);
        let world = r.world(&pose);
        let pelvis = world[1];
        let twist = world[2];
        let expected = compose(&pelvis, &affine([0.0, 0.0, 0.0, 1.0], [1.0, 2.0, 3.0]));
        for i in 0..3 {
            for j in 0..4 {
                assert!((twist[i][j] - expected[i][j]).abs() < 1e-4);
            }
        }
        // Chaining the locals gives the world poses back.
        let locals = r.locals(&pose);
        let mut rebuilt: Vec<Affine> = Vec::new();
        for (m, (t, q)) in locals.iter().enumerate() {
            let local = affine(*q, *t);
            rebuilt.push(match model.bones[m].parent {
                Some(p) => compose(&rebuilt[p], &local),
                None => local,
            });
        }
        for (a, b) in rebuilt.iter().zip(&world) {
            for i in 0..3 {
                for j in 0..4 {
                    assert!((a[i][j] - b[i][j]).abs() < 1e-3);
                }
            }
        }
    }

    #[test]
    fn matrix_to_quat_inverts_quat_matrix() {
        for q in [
            normalize_quat([0.1, 0.2, 0.3, 0.9]),
            normalize_quat([0.9, 0.1, -0.2, 0.05]),
            normalize_quat([-0.1, 0.95, 0.2, 0.1]),
            normalize_quat([0.2, -0.1, 0.97, 0.05]),
        ] {
            let r = quat_matrix(q);
            let m: Affine = std::array::from_fn(|i| [r[i][0], r[i][1], r[i][2], 0.0]);
            let back = matrix_to_quat(&m);
            let dot: f32 = (0..4).map(|k| q[k] * back[k]).sum();
            assert!(dot.abs() > 0.9999, "{q:?} {back:?}");
        }
    }
}
