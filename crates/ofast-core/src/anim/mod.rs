//! CS2 animation (AnimGraph 2): clip skeletons (`.vnmskel_c`), compressed clips (`.vnmclip_c`) and retargeting a
//! clip's poses onto a model's own skeleton.
//!
//! # How CS2 animates a player
//!
//! ```text
//! animation/skeletons/characters/worldmodel.vnmskel   75 bones, the skeleton every third-person clip is made for
//! animation/anims/world/<class>/<set>/*.vnmclip       clips: per bone, a quantised pose per frame (30 fps)
//! animation/graphs/worldmodel/worldmodel.vnmgraph     the graph that blends clips from gameplay parameters
//! agents/models/<agent>/<agent>.vmdl                  the agent: its own skeleton (86 – 94 bones), skinned meshes
//! ```
//!
//! A clip's tracks follow its `.vnmskel` bone order, not the model's: the agent skeleton adds twist, jiggle and aim
//! bones. [`Retargeter`] puts a clip pose on the model the way Source 2 Viewer does (`SkeletonRetargeter.cs`): model
//! bones named like a clip bone take that bone's *model-space* pose, the others follow their parent at bind pose,
//! then each bone's local transform is taken back from those world poses.
//!
//! Additive clips (`m_bIsAdditive`: per-weapon idles, shoot, flinch, breathing) store deltas over another pose;
//! [`Clip::is_additive`] tells them apart and [`layers::add_layer`] applies them.
//!
//! # Modules
//!
//! | Module | What |
//! |---|---|
//! | [`graph`] | reading a compiled graph and evaluating its state entry conditions |
//! | [`locomotion`] | the ground locomotion blends of `worldmodel_locomotion.vnmgraph` |
//! | [`weapon`] | a weapon's idle, shoot, draw and reload clips, found through the main and weapon graphs |
//! | [`layers`] | bone masks, additive and model-space layers |
//! | [`aim`] | CS2's aim node, rebuilt: spine, weapon, hand IK |
//! | [`bake`] | each player's pose over a round from the demo's rig inputs |
//! | [`viewmodel`] | each player's first-person arms and weapon over a round, from the demo's weapon states |
//! | [`constraints`] | the model's procedural bones (twist bones, shoulder blades) from its `BoneConstraintList` |
//!
//! Knowledge from ValveResourceFormat (MIT; `ModelAnimation2/AnimationClip.cs`, `SkeletonRetargeter.cs`, the
//! AnimGraph runtime and `Gameplay/AimCS.cs`), checked on local CS2 files; independent implementation. Details:
//! `docs/formats/animation.md`.

pub mod aim;
pub mod bake;
mod clip;
pub mod constraints;
pub mod eval;
pub mod graph;
pub mod layers;
pub mod locomotion;
mod retarget;
mod skeleton;
pub mod states;
pub mod viewmodel;
pub mod weapon;

pub use clip::{Clip, ClipEvent, TrackSettings};
pub use constraints::BoneConstraints;
pub use locomotion::Locomotion;
pub use retarget::Retargeter;
pub(crate) use retarget::matrix_to_quat;
pub use skeleton::NmSkeleton;

use crate::model::{Affine, normalize_quat, quat_matrix};

/// A bone pose relative to its parent: translation (inches), rotation (unit quaternion `x, y, z, w`), uniform
/// scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Translation from the parent, inches, parent frame.
    pub translation: [f32; 3],
    /// Rotation from the parent, `(x, y, z, w)`.
    pub rotation: [f32; 4],
    /// Uniform scale (1 on CS2's player clips; a delta from 0 in additive clips).
    pub scale: f32,
}

impl Transform {
    /// No translation, no rotation, scale 1.
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: 1.0,
    };

    /// Reads CS2's 8-float transform `[tx, ty, tz, scale, qx, qy, qz, qw]` (`.vnmskel` reference poses).
    pub fn from_floats(v: &[f32]) -> Self {
        let at = |k: usize| v.get(k).copied().unwrap_or(0.0);
        Self {
            translation: [at(0), at(1), at(2)],
            scale: if v.len() > 3 { at(3) } else { 1.0 },
            rotation: normalize_quat([at(4), at(5), at(6), if v.len() > 7 { at(7) } else { 1.0 }]),
        }
    }

    /// As a rigid 3 × 4 matrix (scale left out: it is 1 on every non-additive player pose checked).
    pub fn to_affine(&self) -> Affine {
        let r = quat_matrix(self.rotation);
        std::array::from_fn(|i| [r[i][0], r[i][1], r[i][2], self.translation[i]])
    }

    /// Linear interpolation of translation and scale, shortest-path normalised interpolation of the rotation.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        // Take the shorter way round: q and −q are the same rotation.
        let dot: f32 = (0..4).map(|k| self.rotation[k] * other.rotation[k]).sum();
        let sign = if dot < 0.0 { -1.0 } else { 1.0 };
        Self {
            translation: std::array::from_fn(|k| mix(self.translation[k], other.translation[k])),
            rotation: normalize_quat(std::array::from_fn(|k| {
                mix(self.rotation[k], sign * other.rotation[k])
            })),
            scale: mix(self.scale, other.scale),
        }
    }
}

/// Everything that can go wrong while reading an animation resource.
#[derive(Debug, thiserror::Error)]
pub enum AnimError {
    /// A required key is missing.
    #[error("missing key {0}")]
    MissingKey(&'static str),
    /// The compressed pose data ends before a frame's tracks are read.
    #[error("frame {frame}: pose data ends at u16 {at} of {len}")]
    Truncated {
        /// Frame being decoded.
        frame: usize,
        /// Position reached (u16 units).
        at: usize,
        /// Length of the pose data (u16 units).
        len: usize,
    },
    /// An animation graph does not have the structure OFAST reads (a CS2 update may have changed it).
    #[error("animation graph: {0}")]
    Graph(String),
}
