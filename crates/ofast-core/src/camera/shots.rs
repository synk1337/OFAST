//! Shot cameras (M7): cameras from outside the players, made from the round itself, for the last seconds before a
//! kill.
//!
//! | Kind | The camera |
//! |---|---|
//! | [`ShotKind::Pov`] | the killer's eyes (no camera here: the player's own first-person camera) |
//! | [`ShotKind::Victim`] | the victim's eyes, up to the kill (no camera here either) |
//! | [`ShotKind::Behind`] | over the killer's right shoulder, looking where he looks |
//! | [`ShotKind::Orbit`] | circling the killer slowly at chest height, looking at him |
//! | [`ShotKind::Tripod`] | a fixed point that sees both players at the kill, turning to follow them |
//! | [`ShotKind::Above`] | high above the killer, looking down at him |
//!
//! Every camera keeps out of walls with the map's collision (`physics::ragdoll::WorldCollision`, the shapes that stop
//! a body: glass and grates too): a camera on the far side of a wall is pulled in front of it at once, and eases back
//! out slowly, so a wall passing behind the killer does not make the camera jump back and forth. Without the
//! collision the cameras are placed as if the map were open. Moves are smoothed ([`super::smooth`]): the cameras
//! follow a player's movement and view, not their mouse's jitter.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::smooth::{lambda, smooth_view, whittaker};
use super::{CameraKey, Vec3, add, forward, length, look_at, scale, sub, unit, wrap_degrees};
use crate::demo::players::RoundTrack;
use crate::physics::ragdoll::WorldCollision;

/// Which camera a kill is shown through (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShotKind {
    /// The killer's eyes.
    #[default]
    Pov,
    /// The victim's eyes.
    Victim,
    /// Over the killer's shoulder.
    Behind,
    /// Around the killer.
    Orbit,
    /// A fixed point that sees both players.
    Tripod,
    /// Above the killer.
    Above,
}

impl ShotKind {
    /// Every kind, in the order the Studio offers them.
    pub const ALL: [ShotKind; 6] = [
        ShotKind::Pov,
        ShotKind::Victim,
        ShotKind::Behind,
        ShotKind::Orbit,
        ShotKind::Tripod,
        ShotKind::Above,
    ];

    /// Its name in files and on the command line (`pov`, `victim`, `behind`, `orbit`, `tripod`, `above`).
    pub fn name(self) -> &'static str {
        match self {
            ShotKind::Pov => "pov",
            ShotKind::Victim => "victim",
            ShotKind::Behind => "behind",
            ShotKind::Orbit => "orbit",
            ShotKind::Tripod => "tripod",
            ShotKind::Above => "above",
        }
    }

    /// The kind [`name`](Self::name) names.
    pub fn parse(name: &str) -> Option<ShotKind> {
        ShotKind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Whether it is a camera of its own ([`build`]), not a player's eyes.
    pub fn is_camera(self) -> bool {
        !matches!(self, ShotKind::Pov | ShotKind::Victim)
    }
}

/// How long before its kill a shot starts (seconds); the rest of the kill's cut stays on the killer's eyes.
pub const LEAD_S: f64 = 4.0;

/// How far the cameras keep from walls (units).
pub const WALL_MARGIN: f64 = 6.0;

/// How fast a camera pulled in by a wall eases back out (units per second).
const EASE_OUT: f64 = 60.0;

/// The nearest a camera comes to the point it is held from (units).
const MIN_REACH: f64 = 12.0;

/// Chest height above the feet (units).
const CHEST: f64 = 40.0;

/// Behind: how far back, right and up from the killer's eyes (units), and the field of view.
const BEHIND: [f64; 3] = [64.0, 16.0, 6.0];
const BEHIND_FOV: f64 = 80.0;

/// Orbit: radius from the chest, height above it (units), turn (degrees a second), field of view.
const ORBIT_RADIUS: f64 = 100.0;
const ORBIT_HEIGHT: f64 = 22.0;
const ORBIT_SPEED: f64 = 12.0;
const ORBIT_FOV: f64 = 70.0;

/// Above: height over the feet (units), how far behind (a share of the height), field of view.
const ABOVE_HEIGHT: f64 = 300.0;
const ABOVE_BACK: f64 = 0.3;
const ABOVE_FOV: f64 = 75.0;

/// One player on one tick, as the shots read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// The demo tick.
    pub tick: u32,
    /// Feet (the pawn's origin).
    pub feet: Vec3,
    /// Eyes (feet + view offset).
    pub eyes: Vec3,
    /// `(pitch, yaw)` of the eyes, degrees.
    pub view: [f64; 2],
    /// Alive.
    pub alive: bool,
}

impl Pose {
    fn chest(&self) -> Vec3 {
        add(self.feet, [0.0, 0.0, CHEST])
    }
}

/// The round's players tick by tick ([`Pose`]), by controller.
#[derive(Debug, Clone, Default)]
pub struct Poses {
    ticks: Vec<u32>,
    players: HashMap<u32, Vec<Option<Pose>>>,
    names: HashMap<u32, String>,
}

impl Poses {
    /// Every player's poses on the track's sample ticks.
    pub fn new(track: &RoundTrack) -> Poses {
        let ticks: Vec<u32> = track.samples.iter().map(|s| s.tick).collect();
        let mut players: HashMap<u32, Vec<Option<Pose>>> = HashMap::new();
        let mut names = HashMap::new();
        for (i, sample) in track.samples.iter().enumerate() {
            for p in &sample.players {
                names.insert(p.controller, p.name.clone());
                let poses = players
                    .entry(p.controller)
                    .or_insert_with(|| vec![None; ticks.len()]);
                poses[i] = Some(Pose {
                    tick: sample.tick,
                    feet: p.position.map(f64::from),
                    eyes: p.eye_position().map(f64::from),
                    view: [f64::from(p.pitch), f64::from(p.yaw)],
                    alive: p.alive,
                });
            }
        }
        Poses {
            ticks,
            players,
            names,
        }
    }

    /// The sample ticks.
    pub fn ticks(&self) -> &[u32] {
        &self.ticks
    }

    /// A player's poses on every sample tick (`None` without a pawn), or `None` for no such player.
    pub fn of(&self, controller: u32) -> Option<&[Option<Pose>]> {
        self.players.get(&controller).map(Vec::as_slice)
    }

    /// A player's last known name.
    pub fn name(&self, controller: u32) -> Option<&str> {
        self.names.get(&controller).map(String::as_str)
    }

    /// The controllers with a pawn at some tick.
    pub fn controllers(&self) -> impl Iterator<Item = u32> + '_ {
        self.players.keys().copied()
    }

    /// A player's poses from `start` to `end` (ticks, inclusive), each missing one filled with the nearest known one
    /// (a gap between samples, or a pawn gone); `None` when the player has none there.
    fn span(&self, controller: u32, start: u32, end: u32) -> Option<Vec<Pose>> {
        let poses = self.of(controller)?;
        let first = self.ticks.partition_point(|&t| t < start);
        let last = self.ticks.partition_point(|&t| t <= end);
        let out: Vec<Option<Pose>> = poses[first..last].to_vec();
        // Each gap filled from the pose before it; before the first known pose, from that pose.
        let known = out.iter().position(Option::is_some)?;
        let mut last_known = out[known].expect("known");
        Some(
            out.iter()
                .enumerate()
                .map(|(i, pose)| {
                    let mut p = pose.unwrap_or(last_known);
                    p.tick = self.ticks[first + i];
                    last_known = p;
                    p
                })
                .collect(),
        )
    }
}

/// What a shot frames ([`build`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotSpec {
    /// The camera.
    pub kind: ShotKind,
    /// Who it is about (the killer).
    pub subject: u32,
    /// The other player (the victim), if any.
    pub other: Option<u32>,
    /// First tick.
    pub start: u32,
    /// The moment it builds to (the kill's tick).
    pub focus: u32,
    /// Last tick.
    pub end: u32,
}

/// The camera of a shot ([`ShotKind::is_camera`] kinds) on every sample tick from its start to its end, with the kind
/// it is: a tripod no place of which sees both players (a kill through a wall) is a [`ShotKind::Behind`] camera
/// instead. `None` when the subject has no pose in that time (or the kind is a player's eyes). `tick_interval` is the
/// demo's.
pub fn build(
    spec: &ShotSpec,
    poses: &Poses,
    collision: Option<&WorldCollision>,
    tick_interval: f64,
) -> Option<(ShotKind, Vec<CameraKey>)> {
    let subject = poses.span(spec.subject, spec.start, spec.end)?;
    let other = spec
        .other
        .and_then(|o| poses.span(o, spec.start, spec.end))
        .filter(|o| o.len() == subject.len());
    let focus = subject
        .iter()
        .position(|p| p.tick >= spec.focus)
        .unwrap_or(subject.len() - 1);
    let world = World {
        collision,
        tick_interval,
    };
    let keys = match spec.kind {
        ShotKind::Pov | ShotKind::Victim => return None,
        ShotKind::Behind => behind(&subject, &world),
        ShotKind::Orbit => orbit(&subject, other.as_deref(), focus, &world),
        ShotKind::Tripod => match tripod(&subject, other.as_deref(), focus, &world) {
            Some(keys) => keys,
            None => return Some((ShotKind::Behind, behind(&subject, &world))),
        },
        ShotKind::Above => above(&subject, other.as_deref(), focus, &world),
    };
    Some((spec.kind, keys))
}

/// The map as the cameras see it.
struct World<'a> {
    collision: Option<&'a WorldCollision>,
    tick_interval: f64,
}

impl World<'_> {
    /// How far from `from` towards `to` is free of the world (at most their distance).
    fn reach(&self, from: Vec3, to: Vec3) -> f64 {
        let d = sub(to, from);
        let distance = length(d);
        let Some(collision) = self.collision else {
            return distance;
        };
        if distance <= 0.0 {
            return 0.0;
        }
        let direction = unit(d).map(|v| v as f32);
        collision
            .ray_hit(from.map(|v| v as f32), direction, distance as f32)
            .map_or(distance, |hit| f64::from(hit.distance))
    }

    /// `to` can be seen from `from`.
    fn sees(&self, from: Vec3, to: Vec3) -> bool {
        self.reach(from, to) >= length(sub(to, from)) - 1.0
    }

    /// A point inside the world's solids, or touching them within the wall margin.
    fn blocked(&self, point: Vec3) -> bool {
        self.collision.is_some_and(|c| {
            c.sphere_contact(point.map(|v| v as f32), WALL_MARGIN as f32)
                .is_some()
        })
    }

    /// Cameras held at `wanted` from `anchors` (same length), pulled in front of walls at once and eased back out
    /// ([`EASE_OUT`]).
    fn hold_out(&self, anchors: &[Vec3], wanted: &[Vec3]) -> Vec<Vec3> {
        let step = EASE_OUT * self.tick_interval;
        let mut reach_before = f64::INFINITY;
        anchors
            .iter()
            .zip(wanted)
            .map(|(&anchor, &want)| {
                let offset = sub(want, anchor);
                let distance = length(offset);
                // Free when nothing is within the margin past the wanted point; else the margin before the wall.
                let past = add(want, scale(unit(offset), WALL_MARGIN));
                let reach = self.reach(anchor, past);
                let free = if reach >= distance + WALL_MARGIN - 1e-6 {
                    distance
                } else {
                    (reach - WALL_MARGIN).max(MIN_REACH.min(distance))
                };
                let reach = free.min(reach_before + step).min(distance);
                reach_before = reach;
                add(anchor, scale(unit(offset), reach))
            })
            .collect()
    }
}

/// Each axis of `points` smoothed with `lambda`.
fn smooth_points(points: &[Vec3], lambda: f64) -> Vec<Vec3> {
    let ones = vec![1.0; points.len()];
    let axes: [Vec<f64>; 3] = std::array::from_fn(|k| {
        whittaker(
            &points.iter().map(|p| p[k]).collect::<Vec<_>>(),
            &ones,
            lambda,
        )
    });
    (0..points.len())
        .map(|i| [axes[0][i], axes[1][i], axes[2][i]])
        .collect()
}

/// Cameras at `positions` looking at `targets`, their look smoothed with `smoothing` (a [`lambda`] strength).
fn looking(
    poses: &[Pose],
    positions: &[Vec3],
    targets: &[Vec3],
    smoothing: f64,
    fov: f64,
) -> Vec<CameraKey> {
    let view: Vec<[f64; 2]> = positions
        .iter()
        .zip(targets)
        .map(|(&p, &t)| look_at(p, t))
        .collect();
    let view = smooth_view(&view, &vec![false; view.len()], lambda(smoothing));
    poses
        .iter()
        .zip(positions)
        .zip(view)
        .map(|((pose, &position), [pitch, yaw])| CameraKey {
            tick: f64::from(pose.tick),
            position,
            angles: [pitch, yaw, 0.0],
            fov,
        })
        .collect()
}

/// The right of a view yawing `yaw` (degrees), level.
fn right_of(yaw: f64) -> Vec3 {
    let (s, c) = yaw.to_radians().sin_cos();
    [s, -c, 0.0]
}

/// Yaw (degrees) from the subject to the other player at the focus, or the subject's own view there.
fn focus_yaw(subject: &[Pose], other: Option<&[Pose]>, focus: usize) -> f64 {
    other.map_or(subject[focus].view[1], |o| {
        look_at(subject[focus].eyes, o[focus].chest())[1]
    })
}

fn behind(subject: &[Pose], world: &World) -> Vec<CameraKey> {
    let view = smooth_view(
        &subject.iter().map(|p| p.view).collect::<Vec<_>>(),
        &vec![false; subject.len()],
        lambda(0.85),
    );
    let eyes = smooth_points(
        &subject.iter().map(|p| p.eyes).collect::<Vec<_>>(),
        lambda(0.4),
    );
    let [back, right, up] = BEHIND;
    let wanted: Vec<Vec3> = eyes
        .iter()
        .zip(&view)
        .map(|(&e, v)| {
            let b = scale(forward(0.0, v[1]), -back);
            add(add(add(e, b), scale(right_of(v[1]), right)), [0.0, 0.0, up])
        })
        .collect();
    let positions = world.hold_out(&eyes, &wanted);
    // Looking at where the killer looks, 400 units out.
    let targets: Vec<Vec3> = eyes
        .iter()
        .zip(&view)
        .map(|(&e, v)| add(e, scale(forward(v[0] * 0.7, v[1]), 400.0)))
        .collect();
    looking(subject, &positions, &targets, 0.3, BEHIND_FOV)
}

fn orbit(subject: &[Pose], other: Option<&[Pose]>, focus: usize, world: &World) -> Vec<CameraKey> {
    let centres = smooth_points(
        &subject.iter().map(Pose::chest).collect::<Vec<_>>(),
        lambda(0.9),
    );
    let towards = focus_yaw(subject, other, focus);
    let ticks: Vec<f64> = subject.iter().map(|p| f64::from(p.tick)).collect();
    let focus_tick = ticks[focus];
    // Start behind the killer (seeing what he sees at the kill), then the sides, then in front: the clearest wins.
    let mut best: Option<(f64, Vec<Vec3>)> = None;
    for offset in [180.0, 140.0, -140.0, 100.0, -100.0, 60.0, -60.0, 0.0] {
        for direction in [1.0, -1.0] {
            let wanted: Vec<Vec3> = centres
                .iter()
                .zip(&ticks)
                .map(|(&c, &t)| {
                    let angle = towards
                        + offset
                        + direction * ORBIT_SPEED * (t - focus_tick) * world.tick_interval;
                    add(
                        c,
                        add(
                            scale(forward(0.0, angle), ORBIT_RADIUS),
                            [0.0, 0.0, ORBIT_HEIGHT],
                        ),
                    )
                })
                .collect();
            let positions = world.hold_out(&centres, &wanted);
            let clear = positions
                .iter()
                .zip(&centres)
                .map(|(&p, &c)| length(sub(p, c)) / ORBIT_RADIUS)
                .sum::<f64>()
                / positions.len() as f64;
            if best.as_ref().is_none_or(|(b, _)| clear > b + 0.02) {
                best = Some((clear, positions));
            }
        }
    }
    let positions = best.expect("candidates").1;
    let targets: Vec<Vec3> = centres.iter().map(|&c| add(c, [0.0, 0.0, 8.0])).collect();
    looking(subject, &positions, &targets, 0.2, ORBIT_FOV)
}

/// A fixed camera that sees both players at the focus; `None` when no place around them does.
fn tripod(
    subject: &[Pose],
    other: Option<&[Pose]>,
    focus: usize,
    world: &World,
) -> Option<Vec<CameraKey>> {
    let killer = subject[focus].eyes;
    let target = other.map_or_else(
        || add(killer, scale(forward(0.0, subject[focus].view[1]), 300.0)),
        |o| o[focus].chest(),
    );
    // What must be seen of the victim: his head (a headshot over cover shows no more).
    let head = other.map_or(target, |o| o[focus].eyes);
    let between = sub(target, killer);
    let span = length([between[0], between[1], 0.0]).max(64.0);
    let along = unit([between[0], between[1], 0.0]);
    let across = [-along[1], along[0], 0.0];
    let middle = scale(add(killer, target), 0.5);
    let top = killer[2].max(target[2]);
    // The ticks scored: every 8th from two seconds before the focus to the end.
    let first = focus.saturating_sub((2.0 / world.tick_interval) as usize);
    let scored: Vec<usize> = (first..subject.len()).step_by(8).chain([focus]).collect();
    // Candidates: across the line between the two (a side view of both), then around the killer (behind him looking
    // past him at the victim, beside him) for places a narrow corridor leaves.
    let mut candidates: Vec<(Vec3, f64)> = Vec::new();
    for side in [1.0, -1.0] {
        for shift in [-0.3, 0.0, 0.3] {
            for distance in [0.7, 1.0, 1.4] {
                for height in [24.0, 72.0] {
                    let away = (distance * span).clamp(150.0, 900.0);
                    let mut camera = add(
                        middle,
                        add(scale(along, shift * span), scale(across, side * away)),
                    );
                    camera[2] = top + height;
                    candidates.push((camera, away));
                }
            }
        }
    }
    // Around each of them, turned from the line to the other: behind the killer looking past him at the victim, and
    // behind the victim looking back at the killer (the reverse shot).
    for (from, to, lift) in [
        (killer, target, [12.0, 48.0]),
        (target, killer, [24.0, 60.0]),
    ] {
        let towards = look_at(from, to)[1];
        for turn in [180.0, 150.0, -150.0, 120.0, -120.0, 90.0, -90.0] {
            for distance in [80.0, 140.0, 220.0] {
                for height in lift {
                    let camera = add(
                        add(from, scale(forward(0.0, towards + turn), distance)),
                        [0.0, 0.0, height],
                    );
                    candidates.push((camera, distance));
                }
            }
        }
    }
    let mut best: Option<(f64, Vec3)> = None;
    for (camera, away) in candidates {
        if world.blocked(camera) || !world.sees(camera, killer) || !world.sees(camera, head) {
            continue;
        }
        let spread = angle_between(sub(killer, camera), sub(target, camera));
        if spread > 75.0 {
            continue;
        }
        let mut score = 0.0;
        for &i in &scored {
            score += f64::from(u8::from(world.sees(camera, subject[i].chest())));
            if let Some(o) = other.filter(|o| o[i].alive) {
                score += 0.5 * f64::from(u8::from(world.sees(camera, o[i].eyes)));
            }
        }
        // Nearer the players reads better: a little for every 100 units less.
        score -= away / 400.0;
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, camera));
        }
    }
    let camera = best?.1;
    let spread = angle_between(sub(killer, camera), sub(target, camera));
    let fov = (spread + 30.0).clamp(45.0, 85.0);
    let positions = vec![camera; subject.len()];
    let targets: Vec<Vec3> = subject
        .iter()
        .enumerate()
        .map(|(i, p)| match other.filter(|o| o[i].alive) {
            Some(o) => add(scale(p.chest(), 0.65), scale(o[i].chest(), 0.35)),
            None => p.chest(),
        })
        .collect();
    Some(looking(subject, &positions, &targets, 0.85, fov))
}

fn above(subject: &[Pose], other: Option<&[Pose]>, focus: usize, world: &World) -> Vec<CameraKey> {
    let feet = smooth_points(
        &subject.iter().map(|p| p.feet).collect::<Vec<_>>(),
        lambda(0.9),
    );
    let yaw = focus_yaw(subject, other, focus);
    let back = scale(forward(0.0, yaw), -ABOVE_BACK * ABOVE_HEIGHT);
    let anchors: Vec<Vec3> = feet.iter().map(|&f| add(f, [0.0, 0.0, CHEST])).collect();
    let wanted: Vec<Vec3> = feet
        .iter()
        .map(|&f| add(add(f, back), [0.0, 0.0, ABOVE_HEIGHT]))
        .collect();
    let positions = world.hold_out(&anchors, &wanted);
    let targets: Vec<Vec3> = feet
        .iter()
        .map(|&f| add(add(f, scale(forward(0.0, yaw), 40.0)), [0.0, 0.0, 30.0]))
        .collect();
    looking(subject, &positions, &targets, 0.5, ABOVE_FOV)
}

/// The angle between two directions, degrees.
fn angle_between(a: Vec3, b: Vec3) -> f64 {
    let c = super::dot(unit(a), unit(b)).clamp(-1.0, 1.0);
    c.acos().to_degrees()
}

/// The largest turn between neighbouring keys (degrees of yaw), for tests and checks.
pub fn largest_turn(keys: &[CameraKey]) -> f64 {
    keys.windows(2)
        .map(|w| wrap_degrees(w[1].angles[1] - w[0].angles[1]).abs())
        .fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 64.0;

    /// A killer walking along +x at eye height 64, looking at a victim standing 500 units ahead.
    fn poses(ticks: u32) -> (Vec<Pose>, Vec<Pose>) {
        let killer = (0..ticks)
            .map(|t| {
                let x = f64::from(t) * 2.0;
                let feet = [x, 0.0, 0.0];
                let eyes = [x, 0.0, 64.0];
                let victim = [500.0, 100.0, 40.0];
                Pose {
                    tick: 1000 + t,
                    feet,
                    eyes,
                    view: look_at(eyes, victim),
                    alive: true,
                }
            })
            .collect();
        let victim = (0..ticks)
            .map(|t| Pose {
                tick: 1000 + t,
                feet: [500.0, 100.0, 0.0],
                eyes: [500.0, 100.0, 64.0],
                view: [0.0, 180.0],
                alive: t < ticks - 20,
            })
            .collect();
        (killer, victim)
    }

    fn open() -> World<'static> {
        World {
            collision: None,
            tick_interval: DT,
        }
    }

    #[test]
    fn kinds_have_names() {
        for kind in ShotKind::ALL {
            assert_eq!(ShotKind::parse(kind.name()), Some(kind));
        }
        assert!(!ShotKind::Pov.is_camera() && ShotKind::Orbit.is_camera());
    }

    #[test]
    fn behind_sits_behind_and_right_looking_ahead() {
        let (killer, _) = poses(128);
        let keys = behind(&killer, &open());
        let k = &keys[64];
        let eyes = killer[64].eyes;
        // Behind (smaller x), to the right (the view yaws a little left of +x: right is -y), a little above.
        assert!(k.position[0] < eyes[0] - 50.0, "{k:?}");
        assert!(k.position[2] > eyes[2]);
        let f = forward(k.angles[0], k.angles[1]);
        assert!(f[0] > 0.9, "looking ahead: {f:?}");
        assert!(largest_turn(&keys) < 1.0);
    }

    #[test]
    fn walls_pull_cameras_in() {
        // A wall 30 units behind the killer: a box from x = −40 to −30 across everything.
        let mut collision = WorldCollision::default();
        collision.add_box([-1000.0, -1000.0, -1000.0], [-30.0, 1000.0, 1000.0]);
        let world = World {
            collision: Some(&collision),
            tick_interval: DT,
        };
        let anchors = vec![[0.0, 0.0, 64.0]; 3];
        let wanted = vec![[-64.0, 0.0, 64.0]; 3];
        let held = world.hold_out(&anchors, &wanted);
        for p in held {
            assert!(p[0] > -30.0 + WALL_MARGIN - 1e-3, "{p:?}");
        }
        // Easing back out when the wall is gone: at most EASE_OUT a second.
        let open = open();
        let anchors = vec![[0.0, 0.0, 64.0]; 2];
        let wanted = vec![[-20.0, 0.0, 64.0], [-64.0, 0.0, 64.0]];
        let held = open.hold_out(&anchors, &wanted);
        assert!((held[1][0] - (-20.0 - EASE_OUT * DT)).abs() < 1e-9);
    }

    #[test]
    fn orbit_circles_at_its_radius_looking_at_the_killer() {
        let (killer, victim) = poses(256);
        let keys = orbit(&killer, Some(&victim), 200, &open());
        for (k, p) in keys.iter().zip(&killer).step_by(16) {
            let d = length(sub(
                [k.position[0], k.position[1], 0.0],
                [p.feet[0], p.feet[1], 0.0],
            ));
            assert!((d - ORBIT_RADIUS).abs() < 6.0, "{d}");
            let f = forward(k.angles[0], k.angles[1]);
            let to = unit(sub(p.chest(), k.position));
            assert!(super::super::dot(f, to) > 0.97);
        }
        // In the open the first candidate wins: behind the killer at the kill.
        let k = &keys[200];
        assert!(k.position[0] < killer[200].feet[0]);
    }

    #[test]
    fn tripod_stays_put_and_sees_both() {
        let (killer, victim) = poses(256);
        let keys = tripod(&killer, Some(&victim), 200, &open()).unwrap();
        assert!(keys.iter().all(|k| k.position == keys[0].position));
        let spread = angle_between(
            sub(killer[200].eyes, keys[200].position),
            sub(victim[200].chest(), keys[200].position),
        );
        assert!(spread <= 75.0 && keys[200].fov >= spread);
    }

    #[test]
    fn above_is_above() {
        let (killer, victim) = poses(64);
        let keys = above(&killer, Some(&victim), 50, &open());
        for (k, p) in keys.iter().zip(&killer) {
            assert!(k.position[2] > p.feet[2] + 250.0);
            assert!(k.angles[0] > 45.0, "looking down: {k:?}");
        }
    }
}
