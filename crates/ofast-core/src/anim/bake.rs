//! Each player's body animation over a round, computed from the demo's rig inputs (decision 013, "design A").
//!
//! For every tick a player is alive, the weapon in hand picks one of CS2's three locomotion sets (rifle, pistol,
//! knife — [`WeaponSet::for_item`]), the velocity turned into the body's frame (forward, left) and the crouch amount
//! pick the clips and weights ([`Locomotion::blend`]), and the shared phase advances by the tick's length over the
//! blend's cycle length. Poses are then sampled at a fixed frame rate from the round's first tick, ready to key on a
//! model's skeleton (after [`super::Retargeter`]).
//!
//! What is baked so far is the ground locomotion only (M4.4): no weapon actions (shoot, reload, deploy), no aim on
//! the spine, no jumps or ladders, no ragdolls.

use super::locomotion::LocomotionBlend;
use super::{AnimError, Locomotion, Transform};
use crate::demo::players::RoundTrack;

/// Which of CS2's three locomotion sets a weapon uses (`worldmodel_locomotion.vnmgraph+<set>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponSet {
    /// Rifles, SMGs, shotguns, machine guns, snipers.
    Rifle,
    /// Pistols and the Zeus.
    Pistol,
    /// Knives, and (an assumption, not yet checked in CS2) grenades, the C4 and empty hands.
    Knife,
}

impl WeaponSet {
    /// All three, in a fixed order.
    pub const ALL: [WeaponSet; 3] = [WeaponSet::Rifle, WeaponSet::Pistol, WeaponSet::Knife];

    /// The set for a weapon's item definition index (`None` = nothing in hand).
    pub fn for_item(item: Option<u32>) -> Self {
        match item {
            // Desert Eagle, Dual Berettas, Five-SeveN, Glock-18, Tec-9, Zeus x27, P2000, P250, USP-S, CZ75-Auto,
            // R8 Revolver.
            Some(1 | 2 | 3 | 4 | 30 | 31 | 32 | 36 | 61 | 63 | 64) => Self::Pistol,
            // Knives (default CT 42, default T 59, golden 41, skins 500+), grenades (43 – 48), C4 (49),
            // Healthshot (57), nothing.
            Some(41 | 42 | 59 | 43..=49 | 57 | 80..) | None => Self::Knife,
            Some(_) => Self::Rifle,
        }
    }

    /// The set's name in CS2's file names (`rifle`, `pistol`, `knife`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Rifle => "rifle",
            Self::Pistol => "pistol",
            Self::Knife => "knife",
        }
    }
}

/// The three locomotion sets, read from CS2's graphs.
#[derive(Debug, Clone)]
pub struct LocomotionSets {
    /// `worldmodel_locomotion.vnmgraph+rifle`.
    pub rifle: Locomotion,
    /// `worldmodel_locomotion.vnmgraph+pistol`.
    pub pistol: Locomotion,
    /// `worldmodel_locomotion.vnmgraph+knife`.
    pub knife: Locomotion,
}

impl LocomotionSets {
    /// The locomotion of one set.
    pub fn get(&self, set: WeaponSet) -> &Locomotion {
        match set {
            WeaponSet::Rifle => &self.rifle,
            WeaponSet::Pistol => &self.pistol,
            WeaponSet::Knife => &self.knife,
        }
    }
}

/// A world velocity (Source axes, units/s) in the body's frame: `(forward, left)` for a body facing `yaw` degrees.
pub fn body_relative(velocity: [f32; 3], yaw: f32) -> [f32; 2] {
    let (sin, cos) = yaw.to_radians().sin_cos();
    [
        velocity[0] * cos + velocity[1] * sin,
        -velocity[0] * sin + velocity[1] * cos,
    ]
}

/// The locomotion state of one living tick.
struct Step {
    tick: u32,
    set: WeaponSet,
    blend: LocomotionBlend,
    /// Phase reached at this tick (0 – 1).
    phase: f32,
}

/// One player's poses (clip-skeleton bones) at `fps` frames per second from the round's first tick to its last:
/// `None` while the player has no living pawn.
pub fn bake_player(
    track: &RoundTrack,
    controller: u32,
    sets: &LocomotionSets,
    fps: f32,
) -> Result<Vec<Option<Vec<Transform>>>, AnimError> {
    // The phase runs on through deaths and absences; it only has to be continuous while the player moves.
    let mut phase = 0.0f32;
    let mut previous: Option<u32> = None;
    let steps: Vec<Option<Step>> = track
        .samples
        .iter()
        .map(|sample| {
            let player = sample
                .players
                .iter()
                .find(|p| p.controller == controller && p.alive)?;
            let rig = &player.rig;
            let set = WeaponSet::for_item(rig.weapon.as_ref().map(|w| w.item));
            let blend = sets
                .get(set)
                .blend(body_relative(rig.velocity, rig.body_yaw), rig.duck_amount);
            let dt = previous.map_or(0.0, |t| {
                sample.tick.saturating_sub(t) as f32 * track.tick_interval
            });
            phase = (phase + dt / blend.duration).rem_euclid(1.0);
            previous = Some(sample.tick);
            Some(Step {
                tick: sample.tick,
                set,
                blend,
                phase,
            })
        })
        .collect();

    let seconds = track.end_tick.saturating_sub(track.start_tick) as f32 * track.tick_interval;
    let frames = (seconds * fps).floor() as usize + 1;
    let mut out = Vec::with_capacity(frames);
    let mut index = 0usize;
    for k in 0..frames {
        // The demo tick this frame shows (fractional), and the last sample at or before it.
        let tick = track.start_tick as f32 + k as f32 / fps / track.tick_interval;
        while index + 1 < track.samples.len() && track.samples[index + 1].tick as f32 <= tick {
            index += 1;
        }
        let pose = match steps.get(index).and_then(Option::as_ref) {
            Some(step) => {
                let extra = (tick - step.tick as f32).max(0.0) * track.tick_interval;
                let phase = step.phase + extra / step.blend.duration;
                Some(sets.get(step.set).sample(&step.blend, phase)?)
            }
            None => None,
        };
        out.push(pose);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_turns_into_the_body_frame() {
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
        // Facing +y (yaw 90°) and moving +y: straight ahead.
        assert!(close(body_relative([0.0, 250.0, 0.0], 90.0), [250.0, 0.0]));
        // Facing +x and moving −y: strafing right (negative left).
        assert!(close(body_relative([0.0, -200.0, 0.0], 0.0), [0.0, -200.0]));
        // Facing −x (yaw 180°) and moving +x: backwards.
        assert!(close(
            body_relative([100.0, 0.0, 5.0], 180.0),
            [-100.0, 0.0]
        ));
    }

    #[test]
    fn weapons_pick_their_locomotion_set() {
        assert_eq!(WeaponSet::for_item(Some(7)), WeaponSet::Rifle); // AK-47
        assert_eq!(WeaponSet::for_item(Some(16)), WeaponSet::Rifle); // M4A4
        assert_eq!(WeaponSet::for_item(Some(4)), WeaponSet::Pistol); // Glock-18
        assert_eq!(WeaponSet::for_item(Some(61)), WeaponSet::Pistol); // USP-S
        assert_eq!(WeaponSet::for_item(Some(42)), WeaponSet::Knife); // CT knife
        assert_eq!(WeaponSet::for_item(Some(509)), WeaponSet::Knife); // a knife skin
        assert_eq!(WeaponSet::for_item(Some(44)), WeaponSet::Knife); // HE grenade
        assert_eq!(WeaponSet::for_item(None), WeaponSet::Knife);
    }
}
