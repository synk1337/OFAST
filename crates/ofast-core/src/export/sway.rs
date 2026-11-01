//! The first-person gun's motion with the player (M5): the bob while running, the gun lowered while running and
//! while falling, and its lag behind fast turns. CS2's client adds these to the viewmodel from the player's movement,
//! in code (no networked state and no cvar since 2024: decision 016), so they are modelled here from the demo's
//! velocity, ground flag and eye angles, as small turns of the viewmodel off the camera, written with the recoil's
//! ([`super::punch::punch_json`]'s viewmodel pitch and yaw).
//!
//! Measured on CS2's own first-person frames (2026-11-01, Aurora vs FURIA on Nuke, round 17, played at 0.25 speed
//! and the gun followed on 2560 × 1440 frames). CS2 draws a spectated player's gun at the viewer's own viewmodel
//! settings, it seems (XANTARES, FOV 60, showed a gun smaller than OFAST draws at 60; yuurih, 68, the same size), so
//! every frame here was at 68: about 25 pixels a degree.
//!
//! - **Bob**, only when running: XANTARES with a Galil at 215 units/s swings ±12.5 px (0.5°) side to side every 45
//!   ticks and 15 px (0.6°) up and down twice as often, lowest about 7 ticks after each side; yuurih with a knife at
//!   250 units/s ±16 px (0.64°) every 42 ticks, 16 px (0.64°) up and down. Walking (112 units/s) shows none.
//! - **Lowered while running**: when XANTARES slows from 207 to 112 units/s the gun rises 46 px (1.85°) within 9
//!   ticks.
//! - **Lowered while falling**: Wicadia's gun sinks about 50 px (2°) as he falls to 538 units/s and is back up within
//!   4 ticks of landing; no dip after it.
//! - **Lag behind turns**: small, within what these frames can tell: KSCERATO's gun moved 37 px over a 911°/s turn
//!   while he sped up (the bob's share unknown), FalleN's a few px at 668°/s. Both play left-handed, and their guns
//!   moved with the turn: CS2 mirrors the whole viewmodel for the left hand, lag included
//!   (`tools/unreal/import_demo.py` mirrors the yaw likewise).

use std::collections::BTreeMap;
use std::f32::consts::TAU;

use crate::demo::players::RoundTrack;

/// Running speed (units/s) at which the bob and the lowering start, and at which they are full (smoothstep between):
/// none at a walk (112), most of it at 171, all of it from 200.
pub const RUN_FROM: f32 = 120.0;
/// See [`RUN_FROM`].
pub const RUN_TO: f32 = 200.0;
/// The bob's size scales with speed over this (units/s): a knife's run.
pub const BOB_SPEED: f32 = 250.0;
/// One side-to-side bob at [`BOB_SPEED`], seconds (42 ticks), and how it lengthens at lower speeds:
/// `BOB_PERIOD × (BOB_SPEED / speed) ^ BOB_PERIOD_EXPONENT` (45 ticks at 215).
pub const BOB_PERIOD: f32 = 0.656;
/// See [`BOB_PERIOD`].
pub const BOB_PERIOD_EXPONENT: f32 = 0.45;
/// Side-to-side bob at [`BOB_SPEED`], degrees either way, scaled by (speed over it) ^ 1.5 (0.51° at 215).
pub const BOB_SIDE: f32 = 0.64;
/// Up-and-down bob at [`BOB_SPEED`], degrees from top to bottom, twice per side-to-side bob, scaled by speed over it
/// (0.55° at 215).
pub const BOB_DOWN: f32 = 0.64;
/// How far the up-and-down bob's lowest point comes after each side's (radians of the side-to-side bob: 7 ticks of
/// 45).
pub const BOB_DOWN_LAG: f32 = 0.98;
/// The gun lowered while running, degrees, and how fast it follows (seconds).
pub const RUN_LOWER: f32 = 1.85;
/// See [`RUN_LOWER`].
pub const RUN_LOWER_TIME: f32 = 0.06;
/// The gun lowered while falling: degrees at [`FALL_FULL`] units/s down (50 px), and how fast it follows (seconds).
pub const FALL_LOWER: f32 = 2.0;
/// See [`FALL_LOWER`].
pub const FALL_FULL: f32 = 540.0;
/// See [`FALL_LOWER`].
pub const FALL_TIME: f32 = 0.05;
/// Lag behind turns: degrees per degree/second of turning (0.72° at 900°/s), at most [`LAG_MAX`], following in
/// [`LAG_TIME`] seconds.
pub const LAG: f32 = 0.0008;
/// See [`LAG`].
pub const LAG_MAX: f32 = 1.5;
/// See [`LAG`].
pub const LAG_TIME: f32 = 0.1;

/// One player's gun motion on one tick: (pitch, yaw) degrees off the camera, Source's (positive pitch down, positive
/// yaw left), as the recoil's.
pub type Motion = (u32, [f32; 2]);

/// How much of a run `speed` is (0 – 1): smoothstep from [`RUN_FROM`] to [`RUN_TO`].
pub fn run_share(speed: f32) -> f32 {
    let t = ((speed - RUN_FROM) / (RUN_TO - RUN_FROM)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Every player's gun motion on every sample tick of the round (module docs).
pub fn motion_by_tick(track: &RoundTrack) -> BTreeMap<u32, Vec<Motion>> {
    let mut states: BTreeMap<u32, (State, Vec<Motion>)> = BTreeMap::new();
    for sample in &track.samples {
        for player in &sample.players {
            let (state, out) = states.entry(player.controller).or_default();
            let dt = state.tick.map_or(0.0, |t| {
                sample.tick.saturating_sub(t) as f32 * track.tick_interval
            });
            let rig = &player.rig;
            let turn = state.angles.map_or([0.0; 2], |[pitch, yaw]| {
                if dt > 0.0 {
                    [(player.pitch - pitch) / dt, wrap(player.yaw - yaw) / dt]
                } else {
                    [0.0; 2]
                }
            });
            let grounded = rig.on_ground && !rig.on_ladder;
            let motion = state.step(
                dt,
                rig.velocity[0].hypot(rig.velocity[1]),
                rig.velocity[2],
                grounded,
                turn,
            );
            state.tick = Some(sample.tick);
            state.angles = Some([player.pitch, player.yaw]);
            out.push((sample.tick, motion));
        }
    }
    states.into_iter().map(|(c, (_, out))| (c, out)).collect()
}

/// Degrees into -180 – 180.
fn wrap(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// `value` moved toward `target` over `dt` seconds with time constant `time`.
fn follow(value: f32, target: f32, dt: f32, time: f32) -> f32 {
    value + (target - value) * (1.0 - (-dt / time).exp())
}

/// One player's motion so far.
#[derive(Debug, Default, Clone)]
struct State {
    tick: Option<u32>,
    angles: Option<[f32; 2]>,
    /// The side-to-side bob's phase, radians.
    phase: f32,
    lower: f32,
    fall: f32,
    lag: [f32; 2],
}

impl State {
    /// The next tick's motion, `dt` seconds on: horizontal `speed` and vertical `rise` (units/s), on the ground or
    /// not, turning at `turn` (pitch, yaw, degrees/s).
    fn step(&mut self, dt: f32, speed: f32, rise: f32, grounded: bool, turn: [f32; 2]) -> [f32; 2] {
        let run = if grounded { run_share(speed) } else { 0.0 };
        if grounded && speed > 1.0 {
            let period = BOB_PERIOD * (BOB_SPEED / speed).powf(BOB_PERIOD_EXPONENT);
            self.phase = (self.phase + TAU * dt / period).rem_euclid(TAU);
        }
        let pace = (speed / BOB_SPEED).min(1.1);
        let size = run * pace;
        let side = size * pace.sqrt() * BOB_SIDE * self.phase.sin();
        // Lowest a quarter turn after BOB_DOWN_LAG, and again half a turn later.
        let down = size * BOB_DOWN * 0.5 * (1.0 - (2.0 * (self.phase - BOB_DOWN_LAG)).cos());
        self.lower = follow(self.lower, run * RUN_LOWER, dt, RUN_LOWER_TIME);
        let falling = if grounded {
            0.0
        } else {
            (-rise / FALL_FULL).clamp(0.0, 1.0) * FALL_LOWER
        };
        self.fall = follow(self.fall, falling, dt, FALL_TIME);
        for (lag, rate) in self.lag.iter_mut().zip(turn) {
            *lag = follow(*lag, (-rate * LAG).clamp(-LAG_MAX, LAG_MAX), dt, LAG_TIME);
        }
        [
            down + self.lower + self.fall + self.lag[0],
            side + self.lag[1],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 64.0;

    /// `ticks` ticks of steady movement from a fresh state; the motion on each.
    fn run(ticks: usize, speed: f32, rise: f32, grounded: bool, turn: [f32; 2]) -> Vec<[f32; 2]> {
        let mut state = State::default();
        (0..ticks)
            .map(|_| state.step(DT, speed, rise, grounded, turn))
            .collect()
    }

    #[test]
    fn a_run_bobs_and_lowers_the_gun_and_a_walk_does_not() {
        let knife = run(640, 250.0, 0.0, true, [0.0; 2]);
        let late = &knife[320..];
        let side = late.iter().map(|m| m[1]).fold(f32::MIN, f32::max);
        assert!((side - BOB_SIDE).abs() < 0.01, "side {side}");
        let pitches: Vec<f32> = late.iter().map(|m| m[0]).collect();
        let low = pitches.iter().copied().fold(f32::MIN, f32::max);
        let high = pitches.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            (high - RUN_LOWER).abs() < 0.01 && (low - high - BOB_DOWN).abs() < 0.01,
            "{high} {low}"
        );
        // One side-to-side bob in 42 ticks at 250 units/s, 45 at 215.
        for (speed, ticks) in [(250.0, 42.0), (215.0, 45.0)] {
            let period = BOB_PERIOD * (BOB_SPEED / speed).powf(BOB_PERIOD_EXPONENT) * 64.0;
            assert!((period - ticks).abs() < 0.6, "{speed}: {period}");
        }
        let walk = run(640, 112.0, 0.0, true, [0.0; 2]);
        assert!(walk.iter().all(|m| m[0].abs() < 1e-6 && m[1].abs() < 1e-6));
    }

    #[test]
    fn a_fall_lowers_the_gun_and_a_turn_drags_it_behind() {
        let fall = run(64, 0.0, -540.0, false, [0.0; 2]);
        assert!((fall[63][0] - FALL_LOWER).abs() < 0.01);
        // Turning left (yaw up) leaves the gun turned right (yaw down), 0.72° at 900°/s.
        let turn = run(64, 0.0, 0.0, true, [0.0, 900.0]);
        assert!((turn[63][1] + 0.72).abs() < 0.01, "{}", turn[63][1]);
        let flick = run(64, 0.0, 0.0, true, [0.0, 5000.0]);
        assert!((flick[63][1] + LAG_MAX).abs() < 0.01);
    }

    #[test]
    fn angles_wrap() {
        assert_eq!(wrap(350.0), -10.0);
        assert_eq!(wrap(-190.0), 170.0);
    }
}
