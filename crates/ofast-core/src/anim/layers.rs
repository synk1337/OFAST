//! Layering poses the way CS2's graphs do: bone masks, additive layers and model-space layers.
//!
//! Poses here are one [`Transform`] per bone of a clip skeleton, relative to the bone's parent (parents come before
//! children). Quaternions are `(x, y, z, w)`; `a × b` applies `b` first, so a bone's model-space rotation is
//! `parent × local`.
//!
//! - **Bone mask** (`.vnmskel` `m_maskDefinitions`): listed bones get their weight; every other bone takes the weight
//!   of its nearest listed ancestor, or 0 without one. CS2's `UpperBody` lists `spine_0` and the weapon bones, so
//!   the spine, arms, head and weapon get 1 and the pelvis and legs 0.
//! - **Additive layer**: a pose of deltas, applied with weight `w` (times the bone's mask weight): rotation
//!   `base × slerp(identity, delta, w)` (the delta turns the bone in its own frame), translation and scale
//!   `base + w × delta` (an additive pose's scales are deltas from 0). Checked on CS2's data: the additive
//!   `idle_ak` over the rifle locomotion idle gives its `+non_additive` version within 0.04°.
//! - **Model-space layer**: for each masked bone the model-space rotation moves towards the layer's, and the local
//!   rotation is recomputed under the (already blended) parent; translations blend locally. Unmasked bones keep
//!   their local transform and so follow their parent. The result then blends with the base by the layer weight.
//!
//! The rules follow AnimGraph 2 (Esoterica's design) as Source 2 Viewer reimplements it (MIT: `Runtime_BlendNodes`,
//! `Runtime_Skeleton`, `FrameBone`); OFAST's code is its own.

use super::{NmSkeleton, Transform};
use crate::kv3::Value;
use crate::model::{normalize_quat, quat_mul};

/// Per-bone weights over a clip skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneMask(pub Vec<f32>);

impl BoneMask {
    /// The mask named `id` in a `.vnmskel`'s `DATA` (`m_maskDefinitions[].m_ID`), resolved over its bones.
    pub fn from_skeleton(data: &Value, skeleton: &NmSkeleton, id: &str) -> Option<Self> {
        let definition = data
            .get("m_maskDefinitions")
            .and_then(Value::as_array)?
            .iter()
            .find(|d| d.get("m_ID").and_then(Value::as_str) == Some(id))?;
        let list = definition.get("m_primaryWeightList")?;
        let names = list.get("m_boneIDs").and_then(Value::as_array)?;
        let mut weights = Vec::new();
        if let Some(w) = list.get("m_weights") {
            crate::map::numbers(w, &mut weights);
        }
        let listed: Vec<(usize, f32)> = names
            .iter()
            .zip(weights)
            .filter_map(|(name, w)| Some((skeleton.find(name.as_str()?)?, w)))
            .collect();
        Some(Self::resolve(&skeleton.parents, &listed))
    }

    /// Weights for `listed` bones, the others taking their nearest listed ancestor's weight (0 without one).
    pub fn resolve(parents: &[Option<usize>], listed: &[(usize, f32)]) -> Self {
        let mut set: Vec<Option<f32>> = vec![None; parents.len()];
        for &(bone, weight) in listed {
            if let Some(slot) = set.get_mut(bone) {
                *slot = Some(weight.clamp(0.0, 1.0));
            }
        }
        // Parents come first, so one pass down the list fills children from resolved parents.
        let mut weights = vec![0.0f32; parents.len()];
        for bone in 0..parents.len() {
            weights[bone] = match set[bone] {
                Some(w) => w,
                None => parents[bone].map_or(0.0, |p| {
                    if set[p].is_some() || weights[p] > 0.0 {
                        weights[p]
                    } else {
                        0.0
                    }
                }),
            };
        }
        Self(weights)
    }
}

/// Spherical interpolation from `a` to `b` (the shorter way), `t` in 0 – 1.
pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot: f32 = (0..4).map(|k| a[k] * b[k]).sum();
    let b = if dot < 0.0 {
        dot = -dot;
        b.map(|c| -c)
    } else {
        b
    };
    if dot > 0.9995 {
        return normalize_quat(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t));
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let (sa, sb) = (((1.0 - t) * theta).sin(), (t * theta).sin());
    let s = theta.sin();
    normalize_quat(std::array::from_fn(|k| (a[k] * sa + b[k] * sb) / s))
}

/// The inverse of a unit quaternion.
pub fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Applies an additive pose `delta` over `base` with weight `weight` (times the mask's per-bone weight).
pub fn add_layer(
    base: &mut [Transform],
    delta: &[Transform],
    weight: f32,
    mask: Option<&BoneMask>,
) {
    for (bone, (b, d)) in base.iter_mut().zip(delta).enumerate() {
        let w = weight * mask.map_or(1.0, |m| m.0.get(bone).copied().unwrap_or(0.0));
        if w <= 0.0 {
            continue;
        }
        b.rotation = normalize_quat(quat_mul(
            b.rotation,
            slerp([0.0, 0.0, 0.0, 1.0], d.rotation, w),
        ));
        for k in 0..3 {
            b.translation[k] += w * d.translation[k];
        }
        b.scale += w * d.scale;
    }
}

/// Blends `layer` over `base` in model space with weight `weight`, masked by `mask` (see the module docs).
pub fn model_space_layer(
    base: &mut [Transform],
    layer: &[Transform],
    weight: f32,
    mask: &BoneMask,
    parents: &[Option<usize>],
) {
    if weight <= 0.0 {
        return;
    }
    let count = base.len().min(layer.len()).min(parents.len());
    let global = |pose: &[Transform]| -> Vec<[f32; 4]> {
        let mut g: Vec<[f32; 4]> = Vec::with_capacity(count);
        for bone in 0..count {
            let local = pose[bone].rotation;
            g.push(match parents[bone] {
                Some(p) if p < bone => normalize_quat(quat_mul(g[p], local)),
                _ => local,
            });
        }
        g
    };
    let (base_global, layer_global) = (global(base), global(layer));
    let mut result: Vec<Transform> = base[..count].to_vec();
    let mut result_global: Vec<[f32; 4]> = Vec::with_capacity(count);
    for bone in 0..count {
        let m = mask.0.get(bone).copied().unwrap_or(0.0);
        let parent = parents[bone].filter(|&p| p < bone);
        if m > 0.0 {
            result[bone].translation = std::array::from_fn(|k| {
                base[bone].translation[k]
                    + (layer[bone].translation[k] - base[bone].translation[k]) * m
            });
            result[bone].scale = base[bone].scale + (layer[bone].scale - base[bone].scale) * m;
            let target = slerp(base_global[bone], layer_global[bone], m);
            result[bone].rotation = match parent {
                Some(p) => normalize_quat(quat_mul(conjugate(result_global[p]), target)),
                None => target,
            };
            result_global.push(target);
        } else {
            result_global.push(match parent {
                Some(p) => normalize_quat(quat_mul(result_global[p], result[bone].rotation)),
                None => result[bone].rotation,
            });
        }
    }
    for (b, r) in base.iter_mut().zip(&result) {
        *b = b.lerp(r, weight);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_z(degrees: f32) -> [f32; 4] {
        let half = degrees.to_radians() / 2.0;
        [0.0, 0.0, half.sin(), half.cos()]
    }

    fn bone(rotation: [f32; 4]) -> Transform {
        Transform {
            rotation,
            ..Transform::IDENTITY
        }
    }

    #[test]
    fn masks_fill_children_from_listed_ancestors() {
        // 0 root, 1 pelvis, 2 spine (listed), 3 arm (child of spine), 4 leg (child of pelvis).
        let parents = [None, Some(0), Some(1), Some(2), Some(1)];
        let mask = BoneMask::resolve(&parents, &[(2, 1.0)]);
        assert_eq!(mask.0, [0.0, 0.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn additive_layers_turn_bones_in_their_own_frame() {
        let mut base = vec![bone(turn_z(30.0))];
        let delta = vec![Transform {
            translation: [1.0, 0.0, 0.0],
            rotation: turn_z(20.0),
            scale: 0.0,
        }];
        add_layer(&mut base, &delta, 0.5, None);
        let angle = 2.0 * base[0].rotation[2].atan2(base[0].rotation[3]).to_degrees();
        assert!((angle - 40.0).abs() < 1e-3, "{angle}");
        assert_eq!(base[0].translation, [0.5, 0.0, 0.0]);
        assert_eq!(base[0].scale, 1.0);
    }

    #[test]
    fn model_space_layers_follow_the_mask() {
        // Two bones in a chain; only the child is masked. The layer turns the parent by 90°, which must not reach
        // the result; the child's model-space rotation becomes the layer's (90° + 10°).
        let parents = [None, Some(0)];
        let mut base = vec![bone(turn_z(0.0)), bone(turn_z(0.0))];
        let layer = vec![bone(turn_z(90.0)), bone(turn_z(10.0))];
        let mask = BoneMask(vec![0.0, 1.0]);
        model_space_layer(&mut base, &layer, 1.0, &mask, &parents);
        let angle = |q: [f32; 4]| 2.0 * q[2].atan2(q[3]).to_degrees();
        assert!(angle(base[0].rotation).abs() < 1e-3);
        assert!(
            (angle(base[1].rotation) - 100.0).abs() < 1e-2,
            "{}",
            angle(base[1].rotation)
        );
    }
}
