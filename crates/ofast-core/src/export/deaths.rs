//! Players' deaths: for each death in a round, CS2's ragdoll of the player's agent simulated from the death on
//! ([`crate::physics::ragdoll`]) and written onto the agent's bones, so the body falls and stays where it lies.
//!
//! A death is the first frame where the player's pawn is there but no longer alive. From it until the player lives
//! again (or the round ends):
//!
//! - the ragdoll's parts start on the last living pose, moving as that pose moved over its last frame (the run, the
//!   animation), plus the killing hit: the pawn's `m_vRagdollDamageForce` × [`DEATH_FORCE_SCALE`] as an impulse at
//!   `m_vRagdollDamagePosition` on the part of the hit bone (`m_nRagdollDamageBone`, or the nearest of its parents
//!   that has a part);
//! - each frame steps the simulation against the map's world collision (until the body rests: it then stays);
//! - the parts' bones take the parts' frames; every other bone keeps its offset from its parent as it was in the
//!   last living pose (hands, fingers, twist bones, which the model's constraints then place again), the root keeps
//!   its own.
//!
//! Poses are in the agent's model space relative to the actor Unreal places from the replay's samples (the pawn's
//! position and body yaw at that tick, which stay where the pawn died), so the glTF animation holds the fall.
//!
//! The force scale is OFAST's choice, to be checked against CS2 (whose solver and force handling are not public).

use crate::demo::players::{PlayerSample, RoundTrack};
use crate::model::{Affine, Model, affine, compose, invert_rigid};
use crate::physics::PhysicsData;
use crate::physics::ragdoll::{Ragdoll, WorldCollision};

/// Multiplies the killing hit's force (`m_vRagdollDamageForce`: 20000 – 30000 for rifle bullets) into the impulse
/// given to the hit part (mass × inches per second).
pub const DEATH_FORCE_SCALE: f32 = 0.1;

/// The deaths baked for one player.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BakedDeaths {
    /// Frames where a death begins.
    pub first_frames: Vec<usize>,
    /// Frames simulated (the rest held still).
    pub simulated: usize,
    /// One per death: where the body went.
    pub summaries: Vec<DeathSummary>,
}

/// Where one body went.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DeathSummary {
    /// The frame the death begins.
    pub first_frame: usize,
    /// The frame after its last (the player lives again, or the round's frame count).
    pub end_frame: usize,
    /// Its demo tick.
    pub tick: f32,
    /// The mean of the ragdoll parts' origins when the player died, world (inches).
    pub start: [f32; 3],
    /// The same at the end (at rest, or when the round ended).
    pub rest: [f32; 3],
    /// Frames until the body lay still (`None`: still moving when the round ended).
    pub frames_to_rest: Option<usize>,
}

impl DeathSummary {
    /// How far the body's centre went, inches.
    pub fn distance(&self) -> f32 {
        (0..3)
            .map(|k| (self.rest[k] - self.start[k]).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}

/// The transform Unreal gives the player's actor at a sample: the feet's position, turned by the body yaw about z
/// (Source axes, inches).
pub(crate) fn actor(p: &PlayerSample) -> Affine {
    let half = p.rig.body_yaw.to_radians() / 2.0;
    affine([0.0, 0.0, half.sin(), half.cos()], p.position)
}

/// The linear (inches per second) and angular (radians per second) velocity that moves `from` to `to` in `dt`.
fn velocity(from: &Affine, to: &Affine, dt: f32) -> ([f32; 3], [f32; 3]) {
    let linear = std::array::from_fn(|k| (to[k][3] - from[k][3]) / dt);
    // The turn from one rotation to the other: R = to · fromᵀ, as an axis-angle vector.
    let r: [[f32; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| (0..3).map(|k| to[i][k] * from[j][k]).sum())
    });
    let angle = ((r[0][0] + r[1][1] + r[2][2] - 1.0) / 2.0)
        .clamp(-1.0, 1.0)
        .acos();
    let axis = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    let s = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let angular = if s < 1e-6 {
        [0.0; 3]
    } else {
        axis.map(|c| c / s * angle / dt)
    };
    (linear, angular)
}

/// Overwrites the frames of `controller`'s deaths in `worlds` (the agent's model-space bone transforms, one list per
/// frame; the living frames as baked) with the ragdoll's pose. `frames` are the exported frames' (tick, sample
/// index), `fps` their rate. `collision` is the map's world collision, or `None` to lay a floor under each death.
#[allow(clippy::too_many_arguments)]
pub fn bake_deaths(
    track: &RoundTrack,
    controller: u32,
    frames: &[(f32, usize)],
    fps: f32,
    agent: &Model,
    ragdoll: &PhysicsData,
    collision: Option<&WorldCollision>,
    worlds: &mut [Vec<Affine>],
) -> BakedDeaths {
    let mut out = BakedDeaths::default();
    // One bone per part (the parts follow them): otherwise not a ragdoll this can place.
    if ragdoll.parts.is_empty()
        || ragdoll.bones.len() != ragdoll.parts.len()
        || frames.len() != worlds.len()
    {
        return out;
    }
    let bones = &agent.skeleton.bones;
    let find = |name: &str| bones.iter().position(|b| b.name.eq_ignore_ascii_case(name));
    let Some(part_bones) = ragdoll
        .bones
        .iter()
        .map(|name| find(name))
        .collect::<Option<Vec<usize>>>()
    else {
        return out;
    };
    // The part of each bone: its own, or its nearest parent's.
    let part_of = |mut bone: usize| loop {
        if let Some(part) = part_bones.iter().position(|&b| b == bone) {
            return Some(part);
        }
        bone = bones.get(bone)?.parent?;
    };
    let player = |k: usize| {
        track.samples[frames[k].1]
            .players
            .iter()
            .find(|p| p.controller == controller)
    };
    let dt = 1.0 / fps;
    let mut k = 1;
    while k < frames.len() {
        let (Some(before), Some(now)) = (player(k - 1), player(k)) else {
            k += 1;
            continue;
        };
        if !(before.alive && !now.alive) {
            k += 1;
            continue;
        }
        // A death at frame `k`: until the player lives again, or the round ends.
        let first = k;
        let end = (k..frames.len())
            .find(|&j| player(j).is_some_and(|p| p.alive))
            .unwrap_or(frames.len());
        let living = &worlds[first - 1];
        let actor_then = actor(before);
        let start: Vec<Affine> = part_bones
            .iter()
            .map(|&b| compose(&actor_then, &living[b]))
            .collect();
        // The parts' motion over the last living frame.
        let velocities: Vec<_> = match (first >= 2).then(|| player(first - 2)).flatten() {
            Some(earlier) => {
                let actor_earlier = actor(earlier);
                part_bones
                    .iter()
                    .zip(&start)
                    .map(|(&b, now)| {
                        velocity(&compose(&actor_earlier, &worlds[first - 2][b]), now, dt)
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let mut body = Ragdoll::new(ragdoll, &start, &velocities);
        if let Some(death) = &now.death
            && let Some(part) = death.bone.and_then(|b| part_of(b as usize))
        {
            body.apply_impulse(
                part,
                death.position,
                death.force.map(|f| f * DEATH_FORCE_SCALE),
            );
        }
        // Without the map's collision: a floor where the player last stood (not under feet in the air: a player
        // killed mid-jump falls to the ground he left).
        let floor;
        let world = match collision {
            Some(world) => world,
            None => {
                let mut feet = now.position;
                if let Some(ground) = (0..first)
                    .rev()
                    .filter_map(player)
                    .find(|p| p.alive && p.rig.on_ground)
                {
                    feet[2] = ground.position[2];
                }
                let mut w = WorldCollision::default();
                w.add_box(
                    [feet[0] - 2000.0, feet[1] - 2000.0, feet[2] - 100.0],
                    [feet[0] + 2000.0, feet[1] + 2000.0, feet[2]],
                );
                floor = w;
                &floor
            }
        };
        // The last living pose's offsets of the bones without a part, kept through the fall.
        let offsets: Vec<Option<Affine>> = bones
            .iter()
            .enumerate()
            .map(|(b, bone)| {
                bone.parent
                    .map(|p| compose(&invert_rigid(&living[p]), &living[b]))
            })
            .collect();
        let living = living.clone();
        let mut actor_now = actor_then;
        let centre = |parts: &[Affine]| {
            let n = parts.len().max(1) as f32;
            std::array::from_fn(|k| parts.iter().map(|m| m[k][3]).sum::<f32>() / n)
        };
        let mut summary = DeathSummary {
            first_frame: first,
            end_frame: end,
            tick: frames[first].0,
            start: centre(&start),
            rest: centre(&start),
            frames_to_rest: None,
        };
        for (j, slot) in worlds.iter_mut().enumerate().take(end).skip(first) {
            if !body.at_rest() {
                body.step(dt, world);
                out.simulated += 1;
                if body.at_rest() {
                    summary.frames_to_rest = Some(j + 1 - first);
                }
            }
            if let Some(p) = player(j) {
                actor_now = actor(p);
            }
            let to_model = invert_rigid(&actor_now);
            let parts = body.frames();
            let mut pose = living.clone();
            for b in 0..bones.len() {
                if let Some(part) = part_bones.iter().position(|&pb| pb == b) {
                    pose[b] = compose(&to_model, &parts[part]);
                } else if let (Some(parent), Some(offset)) = (bones[b].parent, offsets[b]) {
                    pose[b] = compose(&pose[parent], &offset);
                }
            }
            *slot = pose;
        }
        summary.rest = centre(&body.frames());
        out.first_frames.push(first);
        out.summaries.push(summary);
        k = end.max(first + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_reads_a_move_and_a_turn() {
        let from = affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]);
        let half = 0.05f32;
        let to = affine([0.0, 0.0, half.sin(), half.cos()], [2.0, 0.0, 0.0]);
        let (linear, angular) = velocity(&from, &to, 0.5);
        assert!((linear[0] - 4.0).abs() < 1e-5);
        // 0.1 rad about z in 0.5 s.
        assert!((angular[2] - 0.2).abs() < 1e-4, "{angular:?}");
    }
}
