//! Recoil on the first-person view (M5): how far each player's camera and viewmodel turn off their eye angles.
//!
//! CS2 draws a player's own view along their eye angles plus 0.45 of the aim punch's offset to the bullets
//! ([`crate::demo::recoil::VIEW_SCALE`]) plus the view punch, and their viewmodel another 0.325 of it past that
//! ([`VIEWMODEL_EXTRA`]). The view punch is in the demo (each shot sets it, and it decays: [`view_punch_at`]); the aim
//! punch is not, and is rebuilt from the player's shots ([`AimPunch`]): each shot's kick from the view punch's jump
//! at that shot (it adds [`VIEW_PUNCH_SCALE`] of the kick; [`kicks`]), or, without one, from the gun's recoil table
//! at the shot's recoil index ([`RecoilPattern`]). The jump is the one to trust: CS2's semi-automatic pistols (the
//! Glock, the Five-SeveN) take each kick from an entry of their table that is not their recoil index's. Checked
//! (`examples/aim_punch.rs`): at every shot the aim (the fire-bullets message's) minus the eye angles there is twice
//! the punch; the rebuilt one is within a median 0.07 – 0.09° of it on 4 rounds of 3 demos (the eye angles come
//! only once a tick), and a full-auto spray's first view punch jump is its table's first kick to 0.0001°.
//!
//! Written to the agents manifest as `punch`: per player (controller, as text), `[tick, camera pitch, camera yaw,
//! viewmodel pitch, viewmodel yaw]` (degrees, Source's: negative pitch up, positive yaw left) on each tick either is
//! not nothing, and one of nothing after each stretch of them: the camera's offset from the eye angles, and the
//! viewmodel's from the camera.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as Json, json};

use super::shots::ShotData;
use crate::demo::players::RoundTrack;
use crate::demo::recoil::{
    AimPunch, RecoilPattern, VIEW_PUNCH_SCALE, VIEW_SCALE, VIEWMODEL_SCALE, ViewPunch,
    view_punch_at,
};
use crate::demo::shots::{Shot, shot_times};

/// How far the viewmodel turns past the camera, in punches.
pub const VIEWMODEL_EXTRA: f32 = VIEWMODEL_SCALE - VIEW_SCALE;

/// Below this (degrees) an offset is nothing.
const NOTHING: f32 = 0.001;

/// A shot's view punch is set this many ticks before it at the earliest and [`JUMP_LATEST`] after it at the latest
/// (`examples/aim_punch.rs`: on the local match demo at the shot's moment exactly; on tournament demos, whose shots
/// carry no sub-tick moment and are placed at their tick interval's start, 0.6 – 1.9 ticks later).
pub const JUMP_EARLIEST: f64 = 0.5;
/// See [`JUMP_EARLIEST`].
pub const JUMP_LATEST: f64 = 2.5;

/// Each shot's kick to the aim punch: when (demo ticks) and the velocity it adds (degrees per second, pitch and
/// yaw). From the first view punch jump after the shot's moment (within [`JUMP_EARLIEST`] – [`JUMP_LATEST`], each
/// jump one shot's) that kicks up and as hard as the gun's table kick within a factor of 2 (anything else that sets
/// the view punch, a landing's push down, is not the shot's), at the jump's moment; else from the gun's table at the
/// shot's recoil index, at the shot's. `shots`: the player's, in order, with their moments; `jumps`: the view
/// punch's, in order.
pub fn kicks(
    shots: &[(f64, &Shot)],
    jumps: &[(f64, [f32; 2])],
    data: &ShotData,
) -> Vec<(f64, [f32; 2])> {
    let mut used = vec![false; jumps.len()];
    let mut out = Vec::with_capacity(shots.len());
    for &(time, shot) in shots {
        let table = data.weapon(shot.item).map(|w| {
            let (angle, magnitude) = RecoilPattern::new(&w.recoil)
                .kick(shot.mode as usize, shot.recoil_index.max(0.0) as usize);
            let (sin, cos) = angle.to_radians().sin_cos();
            [-cos * magnitude, -sin * magnitude]
        });
        let strength = |v: [f32; 2]| v[0].hypot(v[1]);
        let plausible = |jump: [f32; 2]| {
            let kick = jump.map(|j| j / VIEW_PUNCH_SCALE);
            kick[0] < 0.0
                && table.is_none_or(|t| {
                    let ratio = strength(kick) / strength(t).max(f32::MIN_POSITIVE);
                    (0.5..=2.0).contains(&ratio)
                })
        };
        let seen = jumps.iter().enumerate().find(|&(i, &(when, jump))| {
            !used[i]
                && when - time >= -JUMP_EARLIEST
                && when - time < JUMP_LATEST
                && plausible(jump)
        });
        match (seen, table) {
            (Some((i, &(when, jump))), _) => {
                used[i] = true;
                out.push((when, jump.map(|j| j / VIEW_PUNCH_SCALE)));
            }
            (None, Some(kick)) => out.push((time, kick)),
            (None, None) => {}
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// The round's `punch` (module docs).
pub fn punch_json(track: &RoundTrack, data: &ShotData) -> Json {
    let dt = track.tick_interval;
    let times = shot_times(
        track
            .shots
            .iter()
            .map(|(_, s)| s)
            .chain(&track.earlier_shots),
    );
    let mut by_player: BTreeMap<u32, Vec<(f64, &Shot)>> = BTreeMap::new();
    for ((controller, shot), time) in track.shots.iter().zip(times) {
        by_player.entry(*controller).or_default().push((time, shot));
    }
    // Every player the round shows: the view punch reaches those who did not shoot too.
    let players: BTreeSet<u32> = track
        .samples
        .iter()
        .flat_map(|s| s.players.iter().map(|p| p.controller))
        .collect();
    let mut out = serde_json::Map::new();
    for controller in players {
        // The view punch as last set, sample by sample, and its jumps (each over what was left of the last).
        let mut sets: Vec<(u32, Option<ViewPunch>)> = Vec::new();
        let mut jumps: Vec<(f64, [f32; 2])> = Vec::new();
        let mut last: Option<ViewPunch> = None;
        for sample in &track.samples {
            let set = sample
                .players
                .iter()
                .find(|p| p.controller == controller)
                .and_then(|p| p.rig.view_punch);
            if let Some(set) = set
                && last.is_none_or(|l| l.1 != set.1)
            {
                let left = view_punch_at(last, set.1, dt);
                jumps.push((set.1, [set.0[0] - left[0], set.0[1] - left[1]]));
                last = Some(set);
            }
            sets.push((sample.tick, set));
        }
        let shots = by_player.get(&controller).map_or(&[][..], Vec::as_slice);
        let kicks = kicks(shots, &jumps, data);
        // The aim punch, tick by tick: each kick lands in its own tick interval, the punch moving on a tick at a
        // time between them (across a gap between samples too).
        let mut punch = AimPunch::default();
        let mut at = sets.first().map_or(0, |s| s.0.saturating_sub(1));
        let mut next = 0;
        let mut keys = Vec::new();
        let mut active = false;
        for (tick, set) in sets {
            while let Some(&(time, velocity)) = kicks.get(next) {
                if time >= f64::from(tick) {
                    break;
                }
                next += 1;
                let interval = (time.floor().max(0.0) as u32).max(at);
                while at < interval {
                    punch.advance(dt);
                    at += 1;
                }
                punch.velocity[0] += velocity[0];
                punch.velocity[1] += velocity[1];
            }
            while at < tick {
                punch.advance(dt);
                at += 1;
            }
            let view = view_punch_at(set, f64::from(tick), dt);
            let camera: [f32; 2] = std::array::from_fn(|k| punch.angle[k] * VIEW_SCALE + view[k]);
            let viewmodel = punch.angle.map(|a| a * VIEWMODEL_EXTRA);
            let something = camera.iter().chain(&viewmodel).any(|v| v.abs() >= NOTHING);
            if something || active {
                let r = |v: f32| (f64::from(v) * 1000.0).round() / 1000.0;
                let (camera, viewmodel) = if something {
                    (camera, viewmodel)
                } else {
                    ([0.0; 2], [0.0; 2])
                };
                keys.push(json!([
                    tick,
                    r(camera[0]),
                    r(camera[1]),
                    r(viewmodel[0]),
                    r(viewmodel[1])
                ]));
            }
            active = something;
        }
        if !keys.is_empty() {
            out.insert(controller.to_string(), Json::Array(keys));
        }
    }
    Json::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::shots::Shot;

    fn shot(tick: u32, recoil_index: f32) -> Shot {
        Shot {
            tick,
            pawn: 0,
            item: 7,
            mode: 0,
            eye: [0.0; 3],
            angles: [0.0; 3],
            seed: 0,
            inaccuracy: 0.0,
            spread: 0.0,
            recoil_index,
            bullets_left: 0,
            server_tick: None,
            attack: None,
        }
    }

    #[test]
    fn kicks_come_from_the_shots_own_view_punch_jumps() {
        let data = ShotData::default();
        let (a, b) = (shot(100, 0.0), shot(107, 1.0));
        let shots = [(99.0, &a), (106.0, &b)];
        // The first shot's jump 1.2 ticks after it; a landing's push down near the second; the second's own jump.
        let jumps = [
            (100.2, [-1.1, 0.2]),
            (106.3, [0.75, 0.0]),
            (107.4, [-1.3, -0.1]),
        ];
        let got = kicks(&shots, &jumps, &data);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, 100.2);
        assert!((got[0].1[0] - (-1.1 / VIEW_PUNCH_SCALE)).abs() < 1e-3);
        // The landing is not taken for the second shot's kick.
        assert_eq!(got[1].0, 107.4);
        // Without a jump (and no weapon known), no kick.
        assert!(kicks(&shots, &[], &data).is_empty());
    }
}
