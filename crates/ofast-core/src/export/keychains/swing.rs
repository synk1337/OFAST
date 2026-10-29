//! The charm's swing (M10): CS2 hangs a keychain's charm as cloth; OFAST swings it as a chain of rods through its
//! joints, moved by where the weapon takes its ring in the world (`docs/research/cs2-skins.md`, "Keychains").
//!
//! # CS2's cloth
//!
//! Each charm model carries a cloth model (its physics block's `m_pFeModel`, Source 2's softbody: on the blank
//! `kc_missinglink_default`, 20 nodes, the 4 of the ring held by the animation, quads and rods between the others,
//! gravity 360 inches/s² on every free node, no point damping, hinge limits of ±45° and ±67.5° between the joints).
//! The charm's mesh is skinned to three bones, `joint1` (the ring, at the model's origin), `joint2` and `joint3`
//! (the charm hangs from it), one bone per vertex, which the cloth drives.
//!
//! # OFAST's chain
//!
//! OFAST does not run that solver. It moves the joints as a chain of rods:
//!
//! - the ring (`joint1`, held by the cloth's static nodes) follows the weapon, and with it the second joint, where
//!   the swing hangs from (the link between them lies flat at rest); each joint after it, and a weight point (the
//!   centre of the last joint's vertices), is a point moved by gravity (the cloth's 360 inches/s²) and its own speed
//!   (Verlet integration, [`SUBSTEPS`] steps a frame), slowed by [`DAMPING`] (CS2's cloth has no point damping, but
//!   its charms come to rest: the value is a choice, not measured);
//! - each rod keeps its rest length and turns at most [`LIMIT`] ([`LAST_LIMIT`] the charm's) from where its joint's
//!   rest pose points it against the rod before (the cloth's hinge limits), the chain followed from the ring;
//! - a joint's rotation is the shortest turn from its rod's rest direction to where the rod points.
//!
//! The swing starts at rest when the charm shows (the weapon drawn) and when the ring jumps farther than [`RESET`]
//! in a frame (a respawn).

use crate::model::{Affine, Model};

/// Gravity on the charm's free nodes, inches per second² (the cloth's `flGravity`).
const GRAVITY: f32 = 360.0;
/// How fast a swing dies down: its speed falls by e^(−DAMPING · t), t in seconds (a swing keeps 5 % of its speed
/// after a second). The charm's pendulum swings at about 2.7 times a second (√(g / l), l ≈ 1.2 inches), the pace of
/// a player's A–D strafes: holding an angle, the charm swings to its hinges, as a real one would.
pub const DAMPING: f32 = 3.0;
/// Integration steps per frame.
pub const SUBSTEPS: usize = 4;
/// The most a rod turns from its rest direction against the rod before (the cloth's ±45° hinges).
pub const LIMIT: f32 = std::f32::consts::FRAC_PI_4;
/// The charm's own rod's limit (the cloth's ±67.5° hinge at the charm).
pub const LAST_LIMIT: f32 = 3.0 * std::f32::consts::FRAC_PI_8;
/// A ring moving farther than this in one frame, inches, starts the swing again at rest.
pub const RESET: f32 = 64.0;

/// A rotation matrix, rows first.
type Mat3 = [[f32; 3]; 3];

/// Points in the world, inches.
type Points = Vec<[f32; 3]>;

/// A charm's joints as a chain, read from its model ([`Chain::of`]).
#[derive(Debug, Clone)]
pub struct Chain {
    /// Each joint's rest frame in the charm's space: the model's bones, each the child of the one before.
    rest: Vec<Affine>,
    /// The weight point the last joint swings, in the charm's space.
    weight: [f32; 3],
}

impl Chain {
    /// `charm`'s bones as a chain, with the centre of its last bone's vertices as the weight; `None` when the bones
    /// are not one chain or no vertex hangs on the last.
    pub fn of(charm: &Model) -> Option<Self> {
        let bones = &charm.skeleton.bones;
        let chained = !bones.is_empty()
            && bones
                .iter()
                .enumerate()
                .all(|(i, b)| b.parent == i.checked_sub(1));
        if !chained {
            return None;
        }
        let last = bones.len() - 1;
        let (mut sum, mut count) = ([0.0f32; 3], 0usize);
        for mesh in charm.default_meshes() {
            let remap = charm.bone_remap(mesh.mesh_index);
            for draw in &mesh.draw_calls {
                let Ok(g) = mesh.geometry(draw) else { continue };
                for (p, joints) in g.positions.iter().zip(&g.joints) {
                    if remap.get(joints[0] as usize).copied() == Some(last as u32) {
                        (0..3).for_each(|k| sum[k] += p[k]);
                        count += 1;
                    }
                }
            }
        }
        (count > 0).then(|| Self {
            rest: charm.skeleton.world_transforms(),
            weight: sum.map(|c| c / count as f32),
        })
    }

    /// Each joint's rotation in the charm's space, frame by frame, as the charm swings with its space placed at
    /// `frames` in the world (Source axes, inches; `None` while it does not show: the rest pose) at `fps` frames per
    /// second (module docs).
    pub fn swing(&self, frames: &[Option<Affine>], fps: f32) -> Vec<Vec<Mat3>> {
        let rest_rotations: Vec<Mat3> = self.rest.iter().map(rotation).collect();
        // The ring (the first joint, the cloth's static nodes) stays with the weapon; the swing hangs from the
        // second joint (its link to the ring lies flat at rest: under gravity it would turn the ring).
        let fixed = usize::from(self.rest.len() > 1);
        let swinging = &self.rest[fixed..];
        let n = swinging.len();
        // The rest points (the swinging joints, then the weight) and each rod's rest direction and length.
        let points: Vec<[f32; 3]> = swinging.iter().map(origin).chain([self.weight]).collect();
        let rods: Vec<([f32; 3], f32)> = (0..n)
            .map(|i| {
                let d = sub(points[i + 1], points[i]);
                (normalize(d), length(d))
            })
            .collect();
        let limits: Vec<f32> = (0..n)
            .map(|i| if i + 1 == n { LAST_LIMIT } else { LIMIT })
            .collect();
        let dt = 1.0 / (fps.max(1.0) * SUBSTEPS as f32);
        let keep = (-DAMPING * dt).exp();
        // The points in the world now and a step before, and the frame before's placement.
        let mut state: Option<(Points, Points)> = None;
        let mut before: Option<Affine> = None;
        let mut out = Vec::with_capacity(frames.len());
        for frame in frames {
            let Some(h) = frame else {
                state = None;
                before = None;
                out.push(rest_rotations.clone());
                continue;
            };
            let at_rest: Vec<[f32; 3]> = points.iter().map(|&p| apply(h, p)).collect();
            let from = before.filter(|b| length(sub(origin(b), origin(h))) <= RESET);
            let (now, last) = match (&mut state, from) {
                (Some(s), Some(_)) => (&mut s.0, &mut s.1),
                _ => {
                    let s = state.insert((at_rest.clone(), at_rest.clone()));
                    (&mut s.0, &mut s.1)
                }
            };
            let ring_from = from.map_or(at_rest[0], |b| apply(&b, points[0]));
            let mut turns = Vec::new();
            for step in 1..=SUBSTEPS {
                let t = step as f32 / SUBSTEPS as f32;
                now[0] = std::array::from_fn(|k| ring_from[k] + (at_rest[0][k] - ring_from[k]) * t);
                last[0] = now[0];
                for i in 1..=n {
                    let speed = sub(now[i], last[i]);
                    last[i] = now[i];
                    now[i] = std::array::from_fn(|k| now[i][k] + speed[k] * keep);
                    now[i][2] -= GRAVITY * dt * dt;
                }
                turns = follow(now, &rods, &limits, &rotation(h));
            }
            // Each joint: its rest rotation turned with its rod, back in the charm's space.
            let to_charm = transpose(&rotation(h));
            out.push(
                rest_rotations[..fixed]
                    .iter()
                    .copied()
                    .chain(
                        (0..n).map(|i| mul(&to_charm, &mul(&turns[i], &rest_rotations[fixed + i]))),
                    )
                    .collect(),
            );
            before = Some(*h);
        }
        out
    }

    /// The joints' local transforms (translation, quaternion) for one frame's `rotations` ([`Chain::swing`]): the
    /// first under `offset` (the charm's space → its parent bone's frame), each other under the joint before.
    pub fn locals(&self, offset: &Affine, rotations: &[Mat3]) -> Vec<([f32; 3], [f32; 4])> {
        (0..self.rest.len())
            .map(|i| {
                let (translation, turn) = match i.checked_sub(1) {
                    None => (apply(offset, origin(&self.rest[0])), rotation(offset)),
                    Some(p) => {
                        let local = crate::model::compose(
                            &crate::model::invert_rigid(&self.rest[p]),
                            &self.rest[i],
                        );
                        (origin(&local), transpose(&rotations[p]))
                    }
                };
                let r = mul(&turn, &rotations[i]);
                let m: Affine = std::array::from_fn(|row| [r[row][0], r[row][1], r[row][2], 0.0]);
                (translation, crate::anim::matrix_to_quat(&m))
            })
            .collect()
    }
}

/// Follows the chain from the ring (`points[0]`): each rod put back to its length, turned at most its limit from
/// its rest direction under the turn of the rod before (the first under the charm's placement `placed`); the points
/// moved to match. Returns each joint's world rotation without its rest rotation (the turn of its rod, with the
/// placement's).
fn follow(
    points: &mut [[f32; 3]],
    rods: &[([f32; 3], f32)],
    limits: &[f32],
    placed: &Mat3,
) -> Vec<Mat3> {
    let mut turn = *placed;
    let mut turns = Vec::with_capacity(rods.len());
    for (i, (&(rest, len), &limit)) in rods.iter().zip(limits).enumerate() {
        let expected = mul_vec(&turn, rest);
        let mut actual = normalize(sub(points[i + 1], points[i]));
        if length(actual) < 0.5 {
            actual = expected;
        }
        let actual = clamp_angle(expected, actual, limit);
        points[i + 1] = std::array::from_fn(|k| points[i][k] + actual[k] * len);
        turn = mul(&arc(expected, actual), &turn);
        turns.push(turn);
    }
    turns
}

/// `to` turned back towards `from` (unit vectors) to at most `limit` radians from it.
fn clamp_angle(from: [f32; 3], to: [f32; 3], limit: f32) -> [f32; 3] {
    let cos = dot(from, to).clamp(-1.0, 1.0);
    if cos.acos() <= limit {
        return to;
    }
    let side = normalize(sub(to, from.map(|c| c * cos)));
    if length(side) < 0.5 {
        return from;
    }
    let (s, c) = limit.sin_cos();
    std::array::from_fn(|k| from[k] * c + side[k] * s)
}

/// The shortest rotation taking unit vector `a` to unit vector `b`.
fn arc(a: [f32; 3], b: [f32; 3]) -> Mat3 {
    let v = cross(a, b);
    let c = dot(a, b);
    if c < -0.9999 {
        // Half a turn about any axis across `a`.
        let axis = normalize(if a[0].abs() < 0.9 {
            cross(a, [1.0, 0.0, 0.0])
        } else {
            cross(a, [0.0, 1.0, 0.0])
        });
        return std::array::from_fn(|i| {
            std::array::from_fn(|j| 2.0 * axis[i] * axis[j] - if i == j { 1.0 } else { 0.0 })
        });
    }
    let k = 1.0 / (1.0 + c);
    let skew = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let sq: f32 = (0..3).map(|m| skew[i][m] * skew[m][j]).sum();
            (if i == j { 1.0 } else { 0.0 }) + skew[i][j] + sq * k
        })
    })
}

fn rotation(m: &Affine) -> Mat3 {
    std::array::from_fn(|i| [m[i][0], m[i][1], m[i][2]])
}

fn origin(m: &Affine) -> [f32; 3] {
    [m[0][3], m[1][3], m[2][3]]
}

fn apply(m: &Affine, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * p[k]).sum::<f32>() + m[i][3])
}

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn mul_vec(a: &Mat3, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| (0..3).map(|k| a[i][k] * v[k]).sum())
}

fn transpose(a: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| a[j][i]))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| a[k] - b[k])
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| a[k] * b[k]).sum()
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l < 1e-9 { [0.0; 3] } else { a.map(|c| c / l) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::affine;

    /// A charm of three joints hanging down from the ring: 0.1, 0.4, then a weight 1 inch below the last.
    fn chain() -> Chain {
        Chain {
            rest: vec![
                affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0]),
                affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -0.1]),
                affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -0.5]),
            ],
            weight: [0.0, 0.0, -1.5],
        }
    }

    /// The direction the charm's rod (last joint → weight) points in the charm's space on a frame of rotations.
    fn charm_down(rotations: &[Mat3]) -> [f32; 3] {
        mul_vec(&rotations[2], [0.0, 0.0, -1.0])
    }

    #[test]
    fn a_still_charm_hangs_at_rest() {
        let still = affine([0.0, 0.0, 0.0, 1.0], [10.0, 20.0, 30.0]);
        let frames = chain().swing(&vec![Some(still); 64], 64.0);
        let down = charm_down(&frames[63]);
        assert!(dot(down, [0.0, 0.0, -1.0]) > 0.9999, "{down:?}");
    }

    #[test]
    fn a_charm_lags_behind_a_running_weapon_and_settles() {
        // The weapon runs along +x at 250 inches/s for half a second, then stops for three.
        let mut frames = Vec::new();
        let mut x = 0.0;
        for k in 0..224 {
            if k < 32 {
                x += 250.0 / 64.0;
            }
            frames.push(Some(affine([0.0, 0.0, 0.0, 1.0], [x, 0.0, 0.0])));
        }
        let swung = chain().swing(&frames, 64.0);
        // Starting off, the charm trails behind (−x); then it swings, and three seconds later it hangs again.
        assert!(
            charm_down(&swung[4])[0] < -0.05,
            "{:?}",
            charm_down(&swung[4])
        );
        assert!(dot(charm_down(&swung[223]), [0.0, 0.0, -1.0]) > 0.995);
    }

    #[test]
    fn the_ring_stays_with_the_weapon_when_its_link_lies_flat() {
        // As on the blank charm: the second joint 0.13 inch beside the ring, the rest below it.
        let flat = Chain {
            rest: vec![
                affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0]),
                affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.13, 0.0]),
                affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.13, -0.4]),
            ],
            weight: [0.0, 0.13, -1.2],
        };
        let still = affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]);
        let swung = flat.swing(&vec![Some(still); 128], 64.0);
        for rotations in [&swung[1], &swung[127]] {
            for r in rotations.iter() {
                assert!(
                    (r[0][0] - 1.0).abs() < 1e-5 && (r[1][1] - 1.0).abs() < 1e-5,
                    "{r:?}"
                );
            }
        }
    }

    #[test]
    fn a_weapon_turned_on_its_side_lets_the_charm_fall_within_its_hinges() {
        // The weapon rolled 90° about x: the rest pose points the charm along the world's +y; gravity pulls it to
        // −z, the hinges stop it at 112.5° in all (the two swinging joints), more than the 90° it needs.
        let half = std::f32::consts::FRAC_PI_4;
        let rolled = affine([half.sin(), 0.0, 0.0, half.cos()], [0.0; 3]);
        let swung = chain().swing(&vec![Some(rolled); 640], 64.0);
        let world_down = mul_vec(&rotation(&rolled), charm_down(&swung[639]));
        assert!(world_down[2] < -0.95, "{world_down:?}");
    }

    #[test]
    fn a_tilted_still_charm_settles() {
        // Tilted 15° about x and held still, as yuurih's M4A4 on a quiet stretch: the charm comes to hang down
        // and stays.
        let half = 15f32.to_radians() / 2.0;
        let tilted = affine([half.sin(), 0.0, 0.0, half.cos()], [0.0; 3]);
        let swung = chain().swing(&vec![Some(tilted); 640], 64.0);
        let down = |k: usize| mul_vec(&rotation(&tilted), charm_down(&swung[k]));
        for k in [320, 480, 639] {
            assert!(down(k)[2] < -0.999, "frame {k}: {:?}", down(k));
        }
    }

    #[test]
    fn locals_at_rest_are_the_bind_pose() {
        let chain = chain();
        let rest: Vec<Mat3> = chain.rest.iter().map(rotation).collect();
        let offset = affine([0.0, 0.0, 0.0, 1.0], [1.0, 2.0, 3.0]);
        let locals = chain.locals(&offset, &rest);
        assert_eq!(locals[0].0, [1.0, 2.0, 3.0]);
        assert!((locals[1].0[2] + 0.1).abs() < 1e-6 && (locals[2].0[2] + 0.4).abs() < 1e-6);
        for (_, q) in locals {
            assert!((q[3].abs() - 1.0).abs() < 1e-5, "{q:?}");
        }
    }
}
