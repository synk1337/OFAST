//! CS2's ground locomotion, rebuilt from its own graph (`animation/graphs/worldmodel/worldmodel_locomotion.vnmgraph`
//! and its `+rifle`, `+pistol`, `+knife` variations).
//!
//! # What the graph does while a player moves on the ground
//!
//! A 1D blend on the crouch amount (control parameter `move_crouch_amount`, 0 – 1) between two 2D blends on the
//! body-relative velocity (`move_speed_x` forward, `move_speed_y` to the left, units/s):
//!
//! | Blend | Points |
//! |---|---|
//! | standing | idle at (0, 0); `run_<dir>` at 225 units/s (diagonals at ±159, ±159); `walk_<dir>` at 136 (±96) |
//! | crouched | `idle_crouch` at (0, 0); `crouch_<dir>` at 96 units/s (±67) |
//!
//! Each 2D blend carries its triangulation (`m_indices`) and outline (`m_hullIndices`). A velocity inside a triangle
//! blends that triangle's three clips with barycentric weights; one outside the outline (a knife runs at 250) takes
//! the nearest point of the outline. The clips play in step: all at the same normalised phase, which advances by
//! `dt / D`, `D` being the weighted average of the sources' durations (a clip's length divided by its speed
//! multiplier — the idles play at 0.167, so mixing in an idle slows the stride at low speeds). That is how
//! synchronised blending works in AnimGraph 2 (Esoterica's design), simplified to one phase for all clips: the
//! clips' foot sync events (`m_syncTrack`) are not read yet.
//!
//! The graph's other states — starting and stopping, planted direction changes, turning on the spot, jumps, air,
//! ladders — are not rebuilt yet, and the three variations move the legs identically: only the upper body differs.

use super::{AnimError, Clip, Transform};
use crate::kv3::Value;
use crate::model::normalize_quat;

/// Control parameter the crouch blend follows.
const CROUCH_PARAMETER: &str = "move_crouch_amount";

/// One source of a 2D blend: a clip and how fast it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendSource {
    /// Index into [`Locomotion::clips`].
    pub clip: usize,
    /// Playback speed: the clip node's `m_flSpeedMultiplier` (0.167 for the idles, 1 otherwise).
    pub speed: f32,
}

/// A 2D blend space: clips placed at velocities, with the graph's own triangulation.
#[derive(Debug, Clone, PartialEq)]
pub struct Blend2D {
    /// Where each source sits, `(forward, left)` units/s.
    pub points: Vec<[f32; 2]>,
    /// The clip at each point.
    pub sources: Vec<BlendSource>,
    /// Triangles over the points (indices into `points`).
    pub triangles: Vec<[usize; 3]>,
    /// The outline, as a closed list of point indices (first = last).
    pub hull: Vec<usize>,
}

impl Blend2D {
    /// The sources' weights at velocity `p`: `(source index, weight)`, weights positive and summing to 1.
    pub fn weights(&self, p: [f32; 2]) -> Vec<(usize, f32)> {
        for t in &self.triangles {
            let [a, b, c] = t.map(|i| self.points[i]);
            if let Some(w) = barycentric(p, a, b, c) {
                return normalised(t.iter().copied().zip(w).collect());
            }
        }
        // Outside every triangle: the nearest point of the outline, between two neighbouring points.
        let mut best: Option<(f32, usize, usize, f32)> = None;
        for edge in self.hull.windows(2) {
            let (i, j) = (edge[0], edge[1]);
            let (a, b) = (self.points[i], self.points[j]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let length2 = d[0] * d[0] + d[1] * d[1];
            let t = if length2 > 0.0 {
                (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + d[0] * t, a[1] + d[1] * t];
            let distance2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
            if best.is_none_or(|b| distance2 < b.0) {
                best = Some((distance2, i, j, t));
            }
        }
        match best {
            Some((_, i, j, t)) => normalised(vec![(i, 1.0 - t), (j, t)]),
            // No outline: the first source (an empty blend never comes from a real graph).
            None => vec![(0, 1.0)],
        }
    }
}

/// Barycentric weights of `p` in triangle `abc`, if it lies inside (with a small tolerance on the edges).
fn barycentric(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> Option<[f32; 3]> {
    let (v0, v1, v2) = (
        [b[0] - a[0], b[1] - a[1]],
        [c[0] - a[0], c[1] - a[1]],
        [p[0] - a[0], p[1] - a[1]],
    );
    let den = v0[0] * v1[1] - v1[0] * v0[1];
    if den.abs() < 1e-9 {
        return None;
    }
    let v = (v2[0] * v1[1] - v1[0] * v2[1]) / den;
    let w = (v0[0] * v2[1] - v2[0] * v0[1]) / den;
    let u = 1.0 - v - w;
    const EPSILON: f32 = -1e-4;
    (u >= EPSILON && v >= EPSILON && w >= EPSILON).then(|| [u.max(0.0), v.max(0.0), w.max(0.0)])
}

/// Drops near-zero weights and rescales the rest to sum to 1.
fn normalised(weights: Vec<(usize, f32)>) -> Vec<(usize, f32)> {
    let kept: Vec<(usize, f32)> = weights.into_iter().filter(|&(_, w)| w > 1e-5).collect();
    let total: f32 = kept.iter().map(|&(_, w)| w).sum();
    kept.into_iter().map(|(i, w)| (i, w / total)).collect()
}

/// The clips playing at one moment and the length of their shared cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct LocomotionBlend {
    /// `(index into Locomotion::clips, weight)`, weights summing to 1.
    pub clips: Vec<(usize, f32)>,
    /// Seconds per cycle: the weighted average of the clips' durations at their playback speeds.
    pub duration: f32,
}

/// One locomotion set (rifle, pistol or knife): the standing and crouched blends and their clips.
#[derive(Debug, Clone, PartialEq)]
pub struct Locomotion {
    /// The graph variation (`Rifle`, `Pistol`, `Knife`).
    pub variation: String,
    /// Clip paths (as in the graph, without `_c`) and the decoded clips the blends use.
    pub clips: Vec<(String, Clip)>,
    /// Standing: idle, walk and run.
    pub standing: Blend2D,
    /// Crouched: crouch idle and crouch walk.
    pub crouched: Blend2D,
}

impl Locomotion {
    /// Reads the moving-on-the-ground blends from a locomotion graph's `DATA` block; `load_clip` decodes a clip by
    /// path (each distinct clip is loaded once).
    pub fn from_graph<E: From<AnimError>>(
        graph: &Value,
        mut load_clip: impl FnMut(&str) -> Result<Clip, E>,
    ) -> Result<Self, E> {
        let nodes = graph
            .get("m_nodes")
            .and_then(Value::as_array)
            .ok_or(AnimError::MissingKey("m_nodes"))?;
        let resources: Vec<&str> = graph
            .get("m_resources")
            .and_then(Value::as_array)
            .ok_or(AnimError::MissingKey("m_resources"))?
            .iter()
            .map(|v| v.as_str().unwrap_or_default())
            .collect();
        let parameters: Vec<&str> = graph
            .get("m_controlParameterIDs")
            .and_then(Value::as_array)
            .ok_or(AnimError::MissingKey("m_controlParameterIDs"))?
            .iter()
            .map(|v| v.as_str().unwrap_or_default())
            .collect();
        let class = |i: usize| -> &str {
            nodes
                .get(i)
                .and_then(|n| n.get("_class"))
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        // The crouch blend between two 2D blends: unique in the locomotion graph (the jump and air blends follow
        // the eased crouch amount instead).
        let crouch_node = nodes
            .iter()
            .find(|n| {
                n.get("_class").and_then(Value::as_str) == Some("CNmBlend1DNode::CDefinition")
                    && n.get("m_nInputParameterValueNodeIdx")
                        .and_then(Value::as_i64)
                        .and_then(|i| parameters.get(usize::try_from(i).ok()?))
                        == Some(&CROUCH_PARAMETER)
                    && source_indices(n).len() == 2
                    && source_indices(n)
                        .iter()
                        .all(|&s| class(s) == "CNmBlend2DNode::CDefinition")
            })
            .ok_or(AnimError::Graph(
                "no crouch blend between two 2D blends".into(),
            ))?;
        let [standing, crouched] = [0, 1].map(|k| source_indices(crouch_node)[k]);

        let mut clips: Vec<(String, Clip)> = Vec::new();
        let mut blend = |index: usize| -> Result<Blend2D, E> {
            let node = &nodes[index];
            let mut values = Vec::new();
            if let Some(v) = node.get("m_values") {
                crate::map::numbers(v, &mut values);
            }
            let points: Vec<[f32; 2]> = values.as_chunks::<2>().0.to_vec();
            let integers = |key: &'static str| -> Vec<usize> {
                node.get(key)
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|v| usize::try_from(v.as_i64()?).ok())
                    .collect()
            };
            let triangles: Vec<[usize; 3]> = integers("m_indices").as_chunks::<3>().0.to_vec();
            let hull = integers("m_hullIndices");
            let mut sources = Vec::new();
            for source in source_indices(node) {
                let (path, speed) = clip_of(nodes, &resources, source)?;
                let clip = match clips.iter().position(|(p, _)| *p == path) {
                    Some(i) => i,
                    None => {
                        clips.push((path.clone(), load_clip(&path)?));
                        clips.len() - 1
                    }
                };
                sources.push(BlendSource { clip, speed });
            }
            let valid = points.len() == sources.len()
                && !sources.is_empty()
                && triangles
                    .iter()
                    .flatten()
                    .chain(&hull)
                    .all(|&i| i < points.len());
            if !valid {
                return Err(
                    AnimError::Graph(format!("2D blend node {index} is inconsistent")).into(),
                );
            }
            Ok(Blend2D {
                points,
                sources,
                triangles,
                hull,
            })
        };
        let standing = blend(standing)?;
        let crouched = blend(crouched)?;
        Ok(Self {
            variation: graph
                .get("m_variationID")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            clips,
            standing,
            crouched,
        })
    }

    /// The clips and cycle length for a body-relative velocity `(forward, left)` in units/s and a crouch amount.
    pub fn blend(&self, velocity: [f32; 2], crouch: f32) -> LocomotionBlend {
        let crouch = crouch.clamp(0.0, 1.0);
        let mut clips: Vec<(usize, f32)> = Vec::new();
        let mut duration = 0.0;
        for (space, share) in [(&self.standing, 1.0 - crouch), (&self.crouched, crouch)] {
            if share <= 0.0 {
                continue;
            }
            for (source, weight) in space.weights(velocity) {
                let source = &space.sources[source];
                let weight = weight * share;
                let clip = &self.clips[source.clip].1;
                duration += weight * clip.duration / source.speed.max(1e-3);
                match clips.iter_mut().find(|(c, _)| *c == source.clip) {
                    Some((_, w)) => *w += weight,
                    None => clips.push((source.clip, weight)),
                }
            }
        }
        LocomotionBlend {
            clips,
            duration: duration.max(1e-3),
        }
    }

    /// The blended pose at normalised `phase` (0 – 1 through the cycle), one transform per clip-skeleton bone.
    pub fn sample(&self, blend: &LocomotionBlend, phase: f32) -> Result<Vec<Transform>, AnimError> {
        let phase = phase.rem_euclid(1.0);
        let mut poses = Vec::with_capacity(blend.clips.len());
        for &(clip, weight) in &blend.clips {
            let clip = &self.clips[clip].1;
            poses.push((clip.sample(phase * clip.duration, true)?, weight));
        }
        Ok(blend_poses(&poses))
    }
}

/// The weighted average of several poses: translations and scales linearly, rotations as a normalised weighted sum
/// of quaternions turned to the first pose's hemisphere.
pub fn blend_poses(poses: &[(Vec<Transform>, f32)]) -> Vec<Transform> {
    let Some((first, _)) = poses.first() else {
        return Vec::new();
    };
    (0..first.len())
        .map(|bone| {
            let reference = first[bone].rotation;
            let mut translation = [0.0f32; 3];
            let mut rotation = [0.0f32; 4];
            let mut scale = 0.0;
            for (pose, weight) in poses {
                let t = pose.get(bone).unwrap_or(&first[bone]);
                let dot: f32 = (0..4).map(|k| t.rotation[k] * reference[k]).sum();
                let sign = if dot < 0.0 { -1.0 } else { 1.0 };
                for (sum, value) in translation.iter_mut().zip(t.translation) {
                    *sum += weight * value;
                }
                for (sum, value) in rotation.iter_mut().zip(t.rotation) {
                    *sum += weight * sign * value;
                }
                scale += weight * t.scale;
            }
            Transform {
                translation,
                rotation: normalize_quat(rotation),
                scale,
            }
        })
        .collect()
}

/// A node's `m_sourceNodeIndices`.
fn source_indices(node: &Value) -> Vec<usize> {
    node.get("m_sourceNodeIndices")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| usize::try_from(v.as_i64()?).ok())
        .collect()
}

/// The clip under a blend source — directly, or under duration-scale nodes — and its playback speed.
fn clip_of(
    nodes: &[Value],
    resources: &[&str],
    mut index: usize,
) -> Result<(String, f32), AnimError> {
    let mut speed = 1.0f32;
    for _ in 0..8 {
        let node = nodes
            .get(index)
            .ok_or_else(|| AnimError::Graph(format!("node {index} missing")))?;
        let number = |key: &str| node.get(key).and_then(Value::as_f64);
        match node.get("_class").and_then(Value::as_str) {
            Some("CNmClipNode::CDefinition") => {
                let slot = number("m_nDataSlotIdx").unwrap_or(-1.0);
                let path = usize::try_from(slot as i64)
                    .ok()
                    .and_then(|s| resources.get(s))
                    .ok_or_else(|| AnimError::Graph(format!("clip node {index} has no clip")))?;
                speed *= number("m_flSpeedMultiplier").unwrap_or(1.0) as f32;
                return Ok(((*path).to_owned(), speed));
            }
            // A duration scale lengthens (> 1) or shortens the child's cycle; 1 in the locomotion blends.
            Some("CNmDurationScaleNode::CDefinition") => {
                let scale = number("m_flDefaultInputValue").unwrap_or(1.0) as f32;
                speed /= scale.max(1e-3);
                index = number("m_nChildNodeIdx")
                    .and_then(|i| usize::try_from(i as i64).ok())
                    .ok_or_else(|| AnimError::Graph(format!("node {index} has no child")))?;
            }
            other => {
                return Err(AnimError::Graph(format!(
                    "blend source {index} is a {}",
                    other.unwrap_or("node without class")
                )));
            }
        }
    }
    Err(AnimError::Graph(format!("blend source {index}: too deep")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standing blend's layout from `worldmodel_locomotion.vnmgraph+rifle` (points, triangles, outline).
    fn standing() -> Blend2D {
        let points = vec![
            [0.0, 0.0],
            [225.0, 0.0],
            [159.0, -159.0],
            [0.0, -225.0],
            [-159.0, -159.0],
            [-225.0, 0.0],
            [-159.0, 159.0],
            [0.0, 225.0],
            [159.0, 159.0],
            [136.0, 0.0],
            [96.0, -96.0],
            [0.0, -136.0],
            [-96.0, -96.0],
            [-136.0, 0.0],
            [-96.0, 96.0],
            [0.0, 136.0],
            [96.0, 96.0],
        ];
        let indices = [
            4, 5, 13, 13, 5, 6, 11, 4, 12, 4, 11, 3, 3, 11, 2, 2, 11, 10, 11, 12, 0, 2, 10, 9, 11,
            0, 10, 10, 0, 9, 0, 16, 9, 2, 9, 1, 0, 12, 13, 13, 6, 14, 12, 4, 13, 14, 6, 15, 0, 14,
            15, 0, 13, 14, 15, 7, 8, 16, 0, 15, 15, 6, 7, 1, 9, 8, 9, 16, 8, 16, 15, 8,
        ];
        Blend2D {
            sources: (0..points.len())
                .map(|clip| BlendSource { clip, speed: 1.0 })
                .collect(),
            points,
            triangles: indices.as_chunks::<3>().0.to_vec(),
            hull: vec![5, 6, 7, 8, 1, 2, 3, 4, 5],
        }
    }

    fn weight_of(weights: &[(usize, f32)], source: usize) -> f32 {
        weights
            .iter()
            .find(|(s, _)| *s == source)
            .map_or(0.0, |(_, w)| *w)
    }

    #[test]
    fn standing_still_is_idle_and_running_forward_is_run_n() {
        let blend = standing();
        assert_eq!(blend.weights([0.0, 0.0]), vec![(0, 1.0)]);
        let run = blend.weights([225.0, 0.0]);
        assert!((weight_of(&run, 1) - 1.0).abs() < 1e-5, "{run:?}");
        // Halfway between walk_n (136) and run_n (225).
        let between = blend.weights([180.5, 0.0]);
        assert!((weight_of(&between, 1) - 0.5).abs() < 1e-3, "{between:?}");
        assert!((weight_of(&between, 9) - 0.5).abs() < 1e-3, "{between:?}");
    }

    #[test]
    fn weights_sum_to_one_everywhere_and_outside_the_outline_take_its_edge() {
        let blend = standing();
        for x in (-300..=300).step_by(25) {
            for y in (-300..=300).step_by(25) {
                let w = blend.weights([x as f32, y as f32]);
                let total: f32 = w.iter().map(|(_, w)| w).sum();
                assert!((total - 1.0).abs() < 1e-4, "({x}, {y}): {w:?}");
                assert!(w.iter().all(|&(_, w)| w > 0.0));
            }
        }
        // A knife runs at 250 straight ahead: beyond run_n, which takes it all.
        let knife = blend.weights([250.0, 0.0]);
        assert!((weight_of(&knife, 1) - 1.0).abs() < 1e-5, "{knife:?}");
        // Strafing right faster than the run: between the outline's run_e and its neighbours.
        let strafe = blend.weights([0.0, -260.0]);
        assert!((weight_of(&strafe, 3) - 1.0).abs() < 1e-5, "{strafe:?}");
    }

    fn still(rotation_z: f32, x: f32) -> Transform {
        let half = rotation_z / 2.0;
        Transform {
            translation: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, half.sin(), half.cos()],
            scale: 1.0,
        }
    }

    #[test]
    fn blending_poses_averages_translations_and_rotations() {
        let a = vec![still(0.0, 0.0)];
        let b = vec![still(std::f32::consts::FRAC_PI_2, 10.0)];
        let mixed = blend_poses(&[(a, 0.5), (b, 0.5)]);
        assert!((mixed[0].translation[0] - 5.0).abs() < 1e-5);
        // Halfway between 0 and 90° about z: 45°.
        let angle = 2.0 * mixed[0].rotation[2].atan2(mixed[0].rotation[3]);
        assert!(
            (angle - std::f32::consts::FRAC_PI_4).abs() < 1e-4,
            "{angle}"
        );
    }
}
