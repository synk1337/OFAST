//! Brass (M6): the cases a gun throws out as it fires, flown as CS2's clients fly them.
//!
//! A gun's model plays its `shoot` sequence with each shot, and that sequence's events (the model's `ANIM` block:
//! `m_animArray[].m_eventArray[]`) start a particle system on the `shell_eject` attachment
//! (`AE_CL_CREATE_PARTICLE_EFFECT_CFG`, configuration `shell_eject`; [`casing_system`]): one of
//! `particles/weapons/cs_weapon_fx/weapon_shell_casing_*` (`_rifle` on 11 guns, `_9mm` on 13, `_awp`, `_57`,
//! `_shotgun`, `_deagle`, `_45acp`, `_shotgun_mag7`; read 2026-10-25). Such a system ([`Casing::read`]) is one
//! particle drawn as the case's model (`C_OP_RenderModels`), emitted a moment after the event, thrown out in the
//! attachment's frame (`C_INIT_CreateWithinSphereTransform`'s local speed), falling and slowed
//! (`C_OP_BasicMovement`), tumbling about a random axis (`C_OP_RotateVector`), bouncing off the world
//! (`C_OP_WorldTraceConstraint`: it sticks once slower than its stop speed), faded out at the end of its life.
//!
//! OFAST flies each case ([`fly`]) the same way, leaving the port the system's delay after the shot was fired (the
//! shot's `time`, between ticks: [`crate::demo::shots::shot_times`]), from the gun's `shell_eject` as the exported
//! agent holds it then ([`Muzzle::eject`]), against the map's collision as falling bodies see it; and, for the
//! shooter's own view, from their viewmodel's, in the viewmodel rig's space with the world's down as the shot's
//! camera had it and no collision (it leaves the screen in a few frames: [`VIEW_LIFE`]). In Unreal that case rides on
//! the viewmodel, mirrored with it for a left-handed player as CS2 mirrors the viewmodel and its port. The random
//! draws are seeded by the shot, so every export flies a case alike.
//!
//! Not as CS2 does: the trace constraint's bounce and slide read particle attributes whose values in these systems
//! are not clear (the roll and the lifetime): their written amounts are used (0.5 and 0.5); how the case model's
//! axes sit on the particle's (`C_INIT_NormalAlignToCP` on the attachment's −y) is not modelled: it starts in the
//! attachment's frame.
//!
//! Seen in CS2 (the de_nuke pro demo, round 5, YEKINDAR's AK): from the chase camera a case flies off to his right,
//! as OFAST's does (the AK's `shell_eject` turns x to the gun's right, and the system throws along x); through his
//! own eyes a case crosses the lower left of the view two ticks after each shot: he plays left-handed
//! (`m_bLeftHanded`), and OFAST's first-person case, on his mirrored viewmodel, goes left too (10 inches in its first
//! 4 ticks, in Unreal).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as Json, json};

use super::players::{EJECT_TICKS, Muzzle};
use super::shots::{ShotData, data_root};
use crate::content::Content;
use crate::demo::players::RoundTrack;
use crate::demo::shots::{UniformRandom, angle_vectors, shot_times};
use crate::kv3::Value;
use crate::model::Affine;
use crate::physics::ragdoll::WorldCollision;
use crate::resource::{BlockKind, Resource};

/// A casing particle system's flight (module docs), as its file sets it.
#[derive(Debug, Clone, PartialEq)]
pub struct Casing {
    /// The case's model (`C_OP_RenderModels`' first: `weapons/models/shared/shells/ak47/ak47_casing.vmdl` for the
    /// rifles).
    pub model: String,
    /// Seconds from the shot to the case leaving the port (`C_OP_InstantaneousEmitter`'s start time: 0.04).
    pub delay: f32,
    /// Seconds it lives (`C_INIT_InitFloat` of the lifetime: 3); faded out at the end.
    pub life: f32,
    /// Its speed out of the port, per axis of the `shell_eject` frame, between the first and the second (inches per
    /// second; the rifles' (200, 15, 50) – (250, −5, 75)).
    pub speed: [[f32; 3]; 2],
    /// Gravity (`C_OP_BasicMovement`'s, inches per second²: (0, 0, −600)).
    pub gravity: [f32; 3],
    /// Drag: the share of its speed lost per thirtieth of a second (0.05; Source's particles count their drag per
    /// thirtieth, an assumption for CS2's).
    pub drag: f32,
    /// How fast it tumbles, degrees per second, between the two (`C_OP_RotateVector`: 300 – 1500).
    pub spin: [f32; 2],
    /// What it keeps of its speed into a surface it bounces off (`C_OP_WorldTraceConstraint`'s bounce amount).
    pub bounce: f32,
    /// What it keeps of its speed along the surface (the slide amount).
    pub slide: f32,
    /// The speed below which it stops where it touched (inches per second: 24).
    pub stop: f32,
}

/// The `_class` entries of a particle system's list `key` (`m_Operators`…) named `class`.
fn operators<'a>(root: &'a Value, key: &str, class: &'a str) -> impl Iterator<Item = &'a Value> {
    root.get(key)
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(move |o| o.get("_class").and_then(Value::as_str) == Some(class))
}

/// A float input's literal value (`m_flLiteralValue`).
fn literal(input: Option<&Value>) -> Option<f32> {
    let v = input?;
    v.get("m_flLiteralValue")
        .unwrap_or(v)
        .as_f64()
        .map(|x| x as f32)
}

/// A vector: a vector input's literal (`m_vLiteralValue`), or a plain array.
fn vector(input: Option<&Value>) -> Option<[f32; 3]> {
    let v = input?;
    let array = v.get("m_vLiteralValue").unwrap_or(v).as_array()?;
    let c = |k: usize| array.get(k).and_then(Value::as_f64).map(|x| x as f32);
    Some([c(0)?, c(1)?, c(2)?])
}

impl Casing {
    /// Reads a casing system (`particles/weapons/cs_weapon_fx/weapon_shell_casing_rifle.vpcf`). A value the file
    /// leaves out takes Source 2's default (no delay, a life of 1 s, no gravity, drag or spin, no bounce).
    pub fn read(content: &Content, path: &str) -> Result<Casing, String> {
        let root = data_root(content, path)?;
        let first = |key: &str, class: &'static str| operators(&root, key, class).next();
        let model = first("m_Renderers", "C_OP_RenderModels")
            .and_then(|r| r.get("m_ModelList"))
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|m| m.get("m_model"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path}: no model to draw"))?
            .to_owned();
        let delay = literal(
            first("m_Emitters", "C_OP_InstantaneousEmitter").and_then(|e| e.get("m_flStartTime")),
        )
        .unwrap_or(0.0);
        // The lifetime is particle field 1.
        let life = operators(&root, "m_Initializers", "C_INIT_InitFloat")
            .find(|i| i.get("m_nOutputField").and_then(Value::as_i64) == Some(1))
            .and_then(|i| literal(i.get("m_InputValue")))
            .unwrap_or(1.0);
        let sphere = first("m_Initializers", "C_INIT_CreateWithinSphereTransform");
        let speed = |key: &str| vector(sphere.and_then(|s| s.get(key))).unwrap_or([0.0; 3]);
        let movement = first("m_Operators", "C_OP_BasicMovement");
        let rotate = first("m_Operators", "C_OP_RotateVector");
        let trace = first("m_Constraints", "C_OP_WorldTraceConstraint");
        let amount = |key: &str| literal(trace.and_then(|t| t.get(key))).unwrap_or(0.0);
        let rate = |key: &str| literal(rotate.and_then(|r| r.get(key))).unwrap_or(0.0);
        Ok(Casing {
            model,
            delay,
            life,
            speed: [
                speed("m_LocalCoordinateSystemSpeedMin"),
                speed("m_LocalCoordinateSystemSpeedMax"),
            ],
            gravity: vector(movement.and_then(|m| m.get("m_Gravity"))).unwrap_or([0.0; 3]),
            drag: literal(movement.and_then(|m| m.get("m_fDrag"))).unwrap_or(0.0),
            spin: [rate("m_flRotRateMin"), rate("m_flRotRateMax")],
            bounce: amount("m_flBounceAmount"),
            slide: amount("m_flSlideAmount"),
            stop: amount("m_flStopSpeed"),
        })
    }
}

/// The casing system a gun's model throws its cases with (module docs): the system of the first
/// `AE_CL_CREATE_PARTICLE_EFFECT_CFG` event on configuration `shell_eject` in its sequences, the `shoot` ones first.
/// `None`: the model throws none (knives, grenades, the revolver) or cannot be read.
pub fn casing_system(content: &Content, model: &str) -> Option<String> {
    let bytes = content.read_resource(model).ok()?;
    let resource = Resource::parse(&bytes).ok()?;
    let anims = resource.kv3(BlockKind(*b"ANIM")).ok()?.root;
    let mut sequences: Vec<&Value> = anims
        .get("m_animArray")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .collect();
    sequences.sort_by_key(|s| {
        !s.get("m_name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.starts_with("shoot"))
    });
    sequences.iter().find_map(|s| {
        s.get("m_eventArray")
            .and_then(Value::as_array)?
            .iter()
            .filter(|e| {
                e.get("m_sEventName").and_then(Value::as_str)
                    == Some("AE_CL_CREATE_PARTICLE_EFFECT_CFG")
            })
            .filter_map(|e| e.get("m_EventData"))
            .find(|d| d.get("config").and_then(Value::as_str) == Some("shell_eject"))
            .and_then(|d| d.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

/// The casings of the guns among `models` (weapon model paths), by gun model; a gun whose system cannot be read is
/// left out with a warning, one that throws none silently.
pub fn load_casings<'a>(
    content: &Content,
    models: impl IntoIterator<Item = &'a str>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Casing> {
    let mut systems: BTreeMap<String, Option<Casing>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for model in models {
        let Some(system) = casing_system(content, model) else {
            continue;
        };
        let casing =
            systems
                .entry(system.clone())
                .or_insert_with(|| match Casing::read(content, &system) {
                    Ok(casing) => Some(casing),
                    Err(e) => {
                        warnings.push(format!("{e}: no brass"));
                        None
                    }
                });
        if let Some(casing) = casing {
            out.insert(model.to_owned(), casing.clone());
        }
    }
    out
}

/// How long a case is flown in the viewmodel's space (seconds): by then it has fallen over 100 inches below the port,
/// out of any view, and the case CS2 leaves on the floor is the world's.
pub const VIEW_LIFE: f32 = 1.0;
/// Substeps per frame of a case's flight.
const SUBSTEPS: usize = 4;
/// How far above a surface a case's origin rests (inches: about a rifle case's half thickness).
const REST_HEIGHT: f32 = 0.25;

/// A case's flight ([`fly`]): its pose (Source axes, inches; the rotation's columns are the case's x, y, z) every
/// frame from when it leaves the port, until it lies still or its time is up.
#[derive(Debug, Clone, PartialEq)]
pub struct Flight {
    /// One pose per frame.
    pub poses: Vec<([f32; 3], [[f32; 3]; 3])>,
    /// Whether it came to rest (the last pose is where it lies).
    pub still: bool,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `m` turned about the unit `axis` by `angle` radians (Rodrigues).
fn turned(m: [[f32; 3]; 3], axis: [f32; 3], angle: f32) -> [[f32; 3]; 3] {
    let (s, c) = angle.sin_cos();
    let [x, y, z] = axis;
    let r = [
        [
            c + x * x * (1.0 - c),
            x * y * (1.0 - c) - z * s,
            x * z * (1.0 - c) + y * s,
        ],
        [
            y * x * (1.0 - c) + z * s,
            c + y * y * (1.0 - c),
            y * z * (1.0 - c) - x * s,
        ],
        [
            z * x * (1.0 - c) - y * s,
            z * y * (1.0 - c) + x * s,
            c + z * z * (1.0 - c),
        ],
    ];
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| r[i][k] * m[k][j]).sum()))
}

/// A case in flight ([`fly`]): where it is, how fast it goes, how it is turned.
struct InFlight<'a> {
    casing: &'a Casing,
    gravity: [f32; 3],
    collision: Option<&'a WorldCollision>,
    axis: [f32; 3],
    rate: f32,
    at: [f32; 3],
    velocity: [f32; 3],
    turn: [[f32; 3]; 3],
}

impl InFlight<'_> {
    /// Moves it on by `dt` seconds (one substep); `true` when it came to rest.
    fn step(&mut self, dt: f32) -> bool {
        let keep = (1.0 - self.casing.drag).max(0.0).powf(dt * 30.0);
        self.velocity = std::array::from_fn(|k| (self.velocity[k] + self.gravity[k] * dt) * keep);
        let speed = dot(self.velocity, self.velocity).sqrt();
        if speed < 1e-6 {
            return false;
        }
        let way = self.velocity.map(|c| c / speed);
        let step = speed * dt;
        match self
            .collision
            .and_then(|c| c.ray_hit(self.at, way, step + REST_HEIGHT))
        {
            Some(hit) => {
                self.at = std::array::from_fn(|k| {
                    self.at[k] + way[k] * (hit.distance - REST_HEIGHT).max(0.0)
                });
                let into = dot(self.velocity, hit.normal);
                self.velocity = std::array::from_fn(|k| {
                    let normal = hit.normal[k] * into;
                    (self.velocity[k] - normal) * self.casing.slide - normal * self.casing.bounce
                });
                if dot(self.velocity, self.velocity).sqrt() < self.casing.stop {
                    return true;
                }
            }
            None => self.at = std::array::from_fn(|k| self.at[k] + self.velocity[k] * dt),
        }
        self.turn = turned(self.turn, self.axis, self.rate * dt);
        false
    }
}

/// Flies one case of `casing` out of `port` (the `shell_eject` frame, in the space it flies in) for `life` seconds
/// at `fps` frames per second, under `gravity` (that space's), bouncing off `collision` (`None`: nothing to meet);
/// `seed` picks its speed within the system's range and its tumble. `head_start`: how long (seconds, under a frame)
/// it has already flown at its first frame (it leaves the port between two frames).
#[allow(clippy::too_many_arguments)]
pub fn fly(
    casing: &Casing,
    port: &Affine,
    gravity: [f32; 3],
    collision: Option<&WorldCollision>,
    seed: i32,
    fps: f32,
    life: f32,
    head_start: f32,
) -> Flight {
    let mut random = UniformRandom::new(seed);
    let local: [f32; 3] =
        std::array::from_fn(|k| random.float(casing.speed[0][k], casing.speed[1][k]));
    let axis = {
        let a: [f32; 3] = std::array::from_fn(|_| random.float(-1.0, 1.0));
        let length = dot(a, a).sqrt();
        if length > 1e-3 {
            a.map(|c| c / length)
        } else {
            [0.0, 0.0, 1.0]
        }
    };
    let rate = random.float(casing.spin[0], casing.spin[1]).to_radians();
    let mut case = InFlight {
        casing,
        gravity,
        collision,
        axis,
        rate,
        at: [port[0][3], port[1][3], port[2][3]],
        velocity: std::array::from_fn(|i| (0..3).map(|k| port[i][k] * local[k]).sum()),
        turn: std::array::from_fn(|i| std::array::from_fn(|j| port[i][j])),
    };
    let dt = 1.0 / (fps * SUBSTEPS as f32);
    let frames = (life * fps).ceil().max(1.0) as usize;
    let mut poses = Vec::with_capacity(frames);
    // Where it is at the first frame, having left the port `head_start` before it (a step of no time could still
    // meet a surface within the rest height, and bounce off it where it has not moved).
    let mut still =
        head_start > 0.0 && (0..SUBSTEPS).any(|_| case.step(head_start / SUBSTEPS as f32));
    for _ in 0..frames {
        poses.push((case.at, case.turn));
        if still {
            return Flight { poses, still };
        }
        still = (0..SUBSTEPS).any(|_| case.step(dt));
        if still {
            // Where it lies, as the next frame's pose.
            poses.push((case.at, case.turn));
            return Flight { poses, still };
        }
    }
    Flight {
        poses,
        still: false,
    }
}

/// The port at `ticks` after the shot's tick, from its samples on that tick and the [`EJECT_TICKS`] − 1 after it
/// ([`Muzzle::eject`]): where it is, between the two samples about it (the first before the shot, the last after);
/// how it is turned, the nearer one's. With one of the two missing (the gun not shown on that tick), the other;
/// `None` with both.
fn port_at(eject: &[Option<Affine>; EJECT_TICKS], ticks: f64) -> Option<Affine> {
    let u = ticks.clamp(0.0, (EJECT_TICKS - 1) as f64);
    let i = (u.floor() as usize).min(EJECT_TICKS - 2);
    let t = (u - i as f64) as f32;
    match (eject[i], eject[i + 1]) {
        (Some(a), Some(b)) => {
            let mut port = if t < 0.5 { a } else { b };
            for ((row, a), b) in port.iter_mut().zip(a).zip(b) {
                row[3] = a[3] + (b[3] - a[3]) * t;
            }
            Some(port)
        }
        (one, other) => one.or(other),
    }
}

/// Source's angles (pitch, yaw, roll, degrees) of a rotation whose columns are x (forward), y (left), z (up): as
/// Source's `MatrixAngles`.
fn angles_of(m: [[f32; 3]; 3]) -> [f32; 3] {
    let forward = [m[0][0], m[1][0], m[2][0]];
    let (left, up) = ([m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]);
    let across = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt();
    let pitch = (-forward[2]).atan2(across);
    if across > 1e-3 {
        [
            pitch.to_degrees(),
            forward[1].atan2(forward[0]).to_degrees(),
            left[2].atan2(up[2]).to_degrees(),
        ]
    } else {
        [
            pitch.to_degrees(),
            (-left[0]).atan2(left[1]).to_degrees(),
            0.0,
        ]
    }
}

/// The round's cases as the agents manifest's `brass` (module docs), and the case models they use. Per shot of a
/// gun that throws them ([`ShotData::weapon`]'s model in `casings`), a case from where `muzzles` puts its port
/// (`view` false: in the world, bouncing off `collision`) and one from where `view_muzzles` does (`view` true: in
/// the viewmodel rig's space, for the shooter's own view): `controller`, `tick` (the shot's), `model` (the case's),
/// `view`, `first_tick` and `last_tick` (from leaving the port to the end of its life), `still` (whether it came to
/// rest: it stays at its last key) and `keys` `[tick, x, y, z, pitch, yaw, roll]` (Source axes and angles, like
/// the dropped weapons' keys), one per tick while it moves.
pub fn brass_json(
    track: &RoundTrack,
    muzzles: &[Muzzle],
    view_muzzles: &[Muzzle],
    data: &ShotData,
    casings: &BTreeMap<String, Casing>,
    collision: Option<&WorldCollision>,
) -> (Vec<Json>, BTreeSet<String>) {
    let fps = 1.0 / track.tick_interval;
    let mut out = Vec::new();
    let mut models = BTreeSet::new();
    // When each shot was fired (as `shots`' `time`: a tick before its message on a tournament demo).
    let times = shot_times(
        track
            .shots
            .iter()
            .map(|(_, s)| s)
            .chain(&track.earlier_shots),
    );
    for ((controller, shot), time) in track.shots.iter().zip(times) {
        let Some(casing) = data
            .weapon(shot.item)
            .and_then(|w| w.model.as_deref())
            .and_then(|m| casings.get(m))
        else {
            continue;
        };
        let seed = (shot.tick as i32)
            .wrapping_mul(7919)
            .wrapping_add(*controller as i32);
        let of_shot = |m: &&Muzzle| m.controller == *controller && m.tick == shot.tick;
        // The world's down in the shot's camera (x forward, y left, z up), for a case in the viewmodel's space.
        let [forward, right, up] = angle_vectors(shot.angles);
        let left = right.map(|c| -c);
        let view_gravity = [
            dot(casing.gravity, forward),
            dot(casing.gravity, left),
            dot(casing.gravity, up),
        ];
        let flights = [
            (
                false,
                muzzles.iter().find(of_shot),
                casing.gravity,
                collision,
                casing.life,
            ),
            (
                true,
                view_muzzles.iter().find(of_shot),
                view_gravity,
                None,
                VIEW_LIFE.min(casing.life),
            ),
        ];
        // It leaves the port the system's delay after the shot was fired (CS2's shows two ticks after a tournament
        // demo's shot message: YEKINDAR's, tick 37685), from where the port is then, keyed from the next tick on.
        let leaves = time + f64::from(casing.delay) * f64::from(fps);
        let first = leaves.ceil().max(0.0) as u32;
        let head_start = ((f64::from(first) - leaves).max(0.0) / f64::from(fps)) as f32;
        for (view, muzzle, gravity, collision, life) in flights {
            let Some(port) = muzzle.and_then(|m| port_at(&m.eject, leaves - f64::from(shot.tick)))
            else {
                continue;
            };
            let flight = fly(
                casing, &port, gravity, collision, seed, fps, life, head_start,
            );
            let keys: Vec<Json> = flight
                .poses
                .iter()
                .enumerate()
                .map(|(k, (at, turn))| {
                    let [pitch, yaw, roll] = angles_of(*turn);
                    let r = |v: f32| (f64::from(v) * 100.0).round() / 100.0;
                    json!([
                        first + k as u32,
                        r(at[0]),
                        r(at[1]),
                        r(at[2]),
                        r(pitch),
                        r(yaw),
                        r(roll)
                    ])
                })
                .collect();
            models.insert(casing.model.clone());
            out.push(json!({
                "controller": controller,
                "tick": shot.tick,
                "model": casing.model,
                "view": view,
                "first_tick": first,
                "last_tick": first + (life * fps).round() as u32,
                "still": flight.still,
                "keys": keys,
            }));
        }
    }
    (out, models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::affine;

    fn rifle() -> Casing {
        Casing {
            model: "weapons/models/shared/shells/ak47/ak47_casing.vmdl".into(),
            delay: 0.04,
            life: 3.0,
            speed: [[200.0, 15.0, 50.0], [250.0, -5.0, 75.0]],
            gravity: [0.0, 0.0, -600.0],
            drag: 0.05,
            spin: [300.0, 1500.0],
            bounce: 0.5,
            slide: 0.5,
            stop: 24.0,
        }
    }

    #[test]
    fn cases_fly_out_fall_and_come_to_rest() {
        let casing = rifle();
        let mut floor = WorldCollision::default();
        floor.add_box([-1000.0, -1000.0, -100.0], [1000.0, 1000.0, 0.0]);
        // A port 50 inches up, its x along the world's y.
        let port = affine([0.0, 0.0, 0.5f32.sqrt(), 0.5f32.sqrt()], [0.0, 0.0, 50.0]);
        let flight = fly(
            &casing,
            &port,
            casing.gravity,
            Some(&floor),
            1,
            64.0,
            casing.life,
            0.0,
        );
        assert!(flight.still, "a case on a floor comes to rest");
        let (first, last) = (flight.poses[0].0, flight.poses.last().expect("poses").0);
        assert_eq!(first, [0.0, 0.0, 50.0]);
        // Thrown out along the port's x (the world's y), and lying on the floor.
        assert!(last[1] > 50.0, "{last:?}");
        assert!(last[2] >= 0.0 && last[2] < 1.0, "{last:?}");
        // Seeded: the same flight again; another seed, another.
        let again = fly(
            &casing,
            &port,
            casing.gravity,
            Some(&floor),
            1,
            64.0,
            casing.life,
            0.0,
        );
        assert_eq!(again, flight);
        let other = fly(
            &casing,
            &port,
            casing.gravity,
            Some(&floor),
            2,
            64.0,
            casing.life,
            0.0,
        );
        assert_ne!(other.poses.last(), flight.poses.last());
        // It tumbles at most at the system's top rate: 1500 degrees per second, 23.4 per frame.
        let turn = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| {
            let trace: f32 = (0..3)
                .map(|i| (0..3).map(|k| a[k][i] * b[k][i]).sum::<f32>())
                .sum();
            ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
        };
        let step = turn(flight.poses[0].1, flight.poses[1].1);
        assert!(
            step > 300.0 / 64.0 - 0.1 && step < 1500.0 / 64.0 + 0.1,
            "{step}"
        );
        // Without anything to meet it falls on for its whole life.
        let open = fly(&casing, &port, casing.gravity, None, 1, 64.0, 1.0, 0.0);
        assert!(!open.still);
        assert_eq!(open.poses.len(), 64);
        // Slowed by its drag (at most about 390 inches per second), it has fallen over 100 inches from 50 up.
        assert!(open.poses[63].0[2] < -50.0, "{:?}", open.poses[63].0);
        // Leaving the port half a frame before its first frame, it is half a frame's way out there.
        let late = fly(
            &casing,
            &port,
            casing.gravity,
            None,
            1,
            64.0,
            1.0,
            0.5 / 64.0,
        );
        let (out, next) = (late.poses[0].0[1], open.poses[1].0[1]);
        assert!(out > 0.3 * next && out < 0.7 * next, "{out} of {next}");
        // A port within the rest height of a surface, the case leaving it on the frame: its first pose is the
        // port's, not a bounce.
        let low = affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.1]);
        let on_floor = fly(
            &casing,
            &low,
            casing.gravity,
            Some(&floor),
            1,
            64.0,
            1.0,
            0.0,
        );
        assert_eq!(on_floor.poses[0].0, [0.0, 0.0, 0.1]);
    }

    #[test]
    fn the_port_is_where_the_gun_holds_it_when_the_case_leaves() {
        let at = |x: f32, turn: [f32; 4]| Some(affine(turn, [x, 0.0, 0.0]));
        let (straight, half) = (
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.5f32.sqrt(), 0.5f32.sqrt()],
        );
        let eject = [
            at(0.0, straight),
            at(4.0, straight),
            at(8.0, half),
            at(12.0, half),
        ];
        // Between two ticks: where it is between them, turned as the nearer.
        let p = port_at(&eject, 1.25).expect("port");
        assert!((p[0][3] - 5.0).abs() < 1e-5, "{p:?}");
        assert_eq!(p[0][0], 1.0);
        let p = port_at(&eject, 1.75).expect("port");
        assert!((p[0][3] - 7.0).abs() < 1e-5, "{p:?}");
        assert!(p[0][0].abs() < 1e-6, "{p:?}");
        // Before the shot's tick, its first; after the last, its last.
        assert_eq!(port_at(&eject, -0.5), eject[0]);
        assert_eq!(port_at(&eject, 5.0), eject[3]);
        // A missing sample: the other one; both missing: none.
        let mut gaps = eject;
        gaps[2] = None;
        assert_eq!(port_at(&gaps, 1.5), eject[1]);
        gaps[1] = None;
        assert_eq!(port_at(&gaps, 1.5), None);
    }

    #[test]
    fn angles_round_trip_through_source_axes() {
        let m = angle_vectors([30.0, 120.0, 10.0]);
        // angle_vectors gives forward, right, up as rows; as columns x, y (left), z.
        let columns: [[f32; 3]; 3] = std::array::from_fn(|i| [m[0][i], -m[1][i], m[2][i]]);
        let a = angles_of(columns);
        for (got, want) in a.iter().zip([30.0, 120.0, 10.0]) {
            assert!((got - want).abs() < 1e-3, "{a:?}");
        }
    }
}
