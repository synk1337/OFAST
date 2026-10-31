//! A round's cameras (M7, decision 021): every player's eyes, the shot cameras chosen for its kills, HLAE cameras,
//! and the camera cuts, written as `<demo>_r<round>.cameras.json` (`docs/formats/cameras.md`) for
//! `tools/unreal/import_demo.py` (a round's first import) and `tools/unreal/import_cameras.py` (its cameras changed
//! in a round already built).
//!
//! The cuts follow the kills: each kill's cut runs from the one before to half a second after it (the last to the
//! round's end), through the killer's eyes unless the [`CameraPlan`] picks another shot for that kill; a shot of its
//! own covers the last [`LEAD_S`] seconds before the kill, the victim's eyes up to the kill's tick, and the killer's
//! eyes show the rest of the cut. A round about one player ([`CameraPlan::player`]) keeps to their kills: the cuts
//! stay on their eyes (or their kills' shots) from the round's start to [`DEATH_HOLD_S`] after their death, where it
//! ends ([`RoundCameras::last_tick`]); switching between players each kill made a round hard to follow. An HLAE
//! camera takes over the cuts for its whole time in the round.
//!
//! Each player's eyes are the first-person camera: the eyes' position, and the eye angles turned by recoil
//! ([`super::punch`]: 0.9 × the aim punch + the view punch), smoothed when the plan asks
//! ([`crate::camera::smooth`], pinned to the original angles on the ticks around each of the player's shots and
//! kills).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use super::ExportError;
use super::players::{ITEMS_GAME, load_world_physics};
use super::punch::{Recoil, recoil_by_tick};
use super::shots::ShotData;
use crate::camera::hlae::{Clock, read_cam, read_campath};
use crate::camera::shots::{LEAD_S, Poses, ShotKind, ShotSpec, build};
use crate::camera::smooth::{PIN_AROUND_SHOT, lambda, smooth_view};
use crate::camera::{CameraKey, DEFAULT_FOV};
use crate::content::Content;
use crate::demo::players::RoundTrack;
use crate::demo::recoil::VIEW_SCALE;
use crate::physics::ragdoll::WorldCollision;

/// The cameras file's `format` and `version`.
pub const CAMERAS_FORMAT: &str = "ofast-cameras";
/// See [`CAMERAS_FORMAT`].
pub const CAMERAS_VERSION: u32 = 1;

/// How long after its kill a kill's cut stays on (seconds).
pub const CUT_AFTER_KILL_S: f64 = 0.5;

/// The picture an HLAE `.cam` file is taken to be recorded at (width / height): it stores the field of view of its
/// picture without saying which.
pub const HLAE_ASPECT: f64 = 16.0 / 9.0;

/// What the user chose for a round's cameras (the Studio's, or `ofast export-demo --shots --smooth --hlae`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraPlan {
    /// The shot of each kill not seen through the killer's eyes, by the kill's tick.
    pub shots: BTreeMap<u32, ShotKind>,
    /// First-person smoothing, 0 (none) to 1 (strong; [`lambda`]).
    pub smoothing: f64,
    /// HLAE camera files to add (`mirv_campath` `.xml`, `mirv_camio` `.cam`).
    pub hlae: Vec<PathBuf>,
    /// The player the round is about, their Steam id (a bot: their name): the round is seen through their eyes from
    /// its start to [`DEATH_HOLD_S`] after their death (its end if they live), their kills through the shots picked
    /// for them. `None`: each kill through its killer's eyes in turn.
    pub player: Option<String>,
}

/// How long a round about one player ([`CameraPlan::player`]) goes on after their death (seconds): their last view,
/// then the end.
pub const DEATH_HOLD_S: f64 = 2.0;

/// What a cut looks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// A player's eyes (controller).
    Eyes(u32),
    /// One of [`RoundCameras::cameras`].
    Camera(usize),
}

/// A camera cut: `start` to `end` (ticks; `end` not included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// First tick.
    pub start: u32,
    /// The tick after the last.
    pub end: u32,
    /// Through what.
    pub view: View,
}

/// A camera of its own (a shot's, or an HLAE file's).
#[derive(Debug, Clone, PartialEq)]
pub struct NamedCamera {
    /// Shown in Unreal (`Behind XANTARES, kill at 152112`).
    pub name: String,
    /// `behind`, `orbit`, `tripod`, `above` ([`ShotKind::name`]) or `hlae`.
    pub kind: String,
    /// The player it follows, if any.
    pub subject: Option<u32>,
    /// Its keys, by tick.
    pub keys: Vec<CameraKey>,
}

/// Everything in a round's cameras file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoundCameras {
    /// Each player's last known name, by controller (Unreal finds their cameras by it: `POV <name>`).
    pub players: BTreeMap<u32, String>,
    /// Each player's eyes, by controller.
    pub eyes: BTreeMap<u32, Vec<CameraKey>>,
    /// The cameras of their own.
    pub cameras: Vec<NamedCamera>,
    /// The cuts, in order.
    pub cuts: Vec<Cut>,
    /// The player the round is about ([`CameraPlan::player`], their controller), if any.
    pub subject: Option<u32>,
    /// The last tick shown: the round's last, or [`DEATH_HOLD_S`] after the death of the player it is about.
    pub last_tick: u32,
    /// Where each player's eyes focus, by controller: `(tick, distance in units)` every [`FOCUS_STEP`] ticks
    /// (the distance along the view to the first wall or player in it, smoothed), for the depth of field of a round
    /// built in the realistic look.
    pub focus: BTreeMap<u32, Vec<(u32, f64)>>,
    /// What could not be done as asked.
    pub warnings: Vec<String>,
}

/// How often a player's focus is measured (ticks): a focus puller follows the aim, not each twitch of it.
pub const FOCUS_STEP: u32 = 4;
/// The farthest a focus goes (units): past it, the sky or the far end of a long street, all sharp enough.
pub const FOCUS_REACH: f64 = 4000.0;
/// The nearest (units): a wall pressed against is not focused closer.
pub const FOCUS_NEAR: f64 = 40.0;
/// A player's body as the focus sees it: a standing cylinder this wide (radius, units) from the feet to 8 units above
/// the eyes.
const FOCUS_BODY_RADIUS: f64 = 16.0;
/// How quickly the focus follows what is in the middle of the view (seconds, its smoothing's time constant, in the
/// distance's logarithm: a pull from a near wall to a far player as fast as the other way).
pub const FOCUS_LAG_S: f64 = 0.15;

/// Where each player's eyes focus ([`RoundCameras::focus`]): every [`FOCUS_STEP`] ticks of their `eyes`, the
/// distance along the view to the first thing in it, the world (`collision`; without it, only players) or another
/// living player's body, between [`FOCUS_NEAR`] and [`FOCUS_REACH`], smoothed over [`FOCUS_LAG_S`] seconds.
fn focus(
    poses: &Poses,
    eyes: &BTreeMap<u32, Vec<CameraKey>>,
    collision: Option<&WorldCollision>,
    interval: f64,
) -> BTreeMap<u32, Vec<(u32, f64)>> {
    let ticks = poses.ticks();
    let keep = 1.0 - (-f64::from(FOCUS_STEP) * interval / FOCUS_LAG_S).exp();
    let mut out = BTreeMap::new();
    for (&controller, keys) in eyes {
        let mut track: Vec<(u32, f64)> = Vec::new();
        let mut smoothed: Option<f64> = None;
        for key in keys
            .iter()
            .filter(|k| (k.tick as u32).is_multiple_of(FOCUS_STEP))
        {
            let tick = key.tick as u32;
            let from = key.position;
            let along = crate::camera::forward(key.angles[0], key.angles[1]);
            let mut nearest = collision
                .and_then(|w| {
                    w.ray_hit(
                        from.map(|v| v as f32),
                        along.map(|v| v as f32),
                        FOCUS_REACH as f32,
                    )
                })
                .map_or(FOCUS_REACH, |hit| f64::from(hit.distance));
            if let Ok(at) = ticks.binary_search(&tick) {
                for other in poses.controllers().filter(|&c| c != controller) {
                    let Some(Some(pose)) = poses.of(other).map(|p| p[at]) else {
                        continue;
                    };
                    if pose.alive
                        && let Some(d) = body_along(from, along, pose.feet, pose.eyes[2] + 8.0)
                    {
                        nearest = nearest.min(d);
                    }
                }
            }
            let distance = nearest.clamp(FOCUS_NEAR, FOCUS_REACH);
            let value = smoothed.map_or(distance.ln(), |s| s + keep * (distance.ln() - s));
            smoothed = Some(value);
            track.push((tick, value.exp()));
        }
        out.insert(controller, track);
    }
    out
}

/// Where a ray from `from` along the unit `along` meets a player's body standing at `feet` up to `top` (a standing
/// cylinder of [`FOCUS_BODY_RADIUS`]): the distance along the ray where it passes nearest the body's axis, if it
/// passes within the radius there, in front, between the feet and the top.
fn body_along(
    from: crate::camera::Vec3,
    along: crate::camera::Vec3,
    feet: crate::camera::Vec3,
    top: f64,
) -> Option<f64> {
    let flat = along[0] * along[0] + along[1] * along[1];
    if flat < 1e-6 {
        return None;
    }
    let s = ((feet[0] - from[0]) * along[0] + (feet[1] - from[1]) * along[1]) / flat;
    let (x, y, z) = (
        from[0] + s * along[0],
        from[1] + s * along[1],
        from[2] + s * along[2],
    );
    let off = ((x - feet[0]).powi(2) + (y - feet[1]).powi(2)).sqrt();
    (s > 0.0 && off <= FOCUS_BODY_RADIUS && z >= feet[2] && z <= top).then_some(s)
}

/// An HLAE camera file as camera keys on the demo's ticks (a campath sampled on every tick); `name` from the file.
pub fn load_hlae(path: &Path, clock: Clock) -> Result<NamedCamera, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let keys = if text.trim_start().starts_with("advancedfx Cam") {
        read_cam(&text)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .keys(clock, HLAE_ASPECT)
    } else {
        read_campath(&text)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .keys(clock)
    };
    if keys.is_empty() {
        return Err(format!("{}: no camera in it", path.display()));
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("hlae");
    Ok(NamedCamera {
        name: format!("HLAE {stem}"),
        kind: "hlae".into(),
        subject: None,
        keys,
    })
}

/// The controller of the player `wanted` names ([`CameraPlan::player`]): a Steam id, or a name for a bot (Steam id
/// 0); `None` when no player of the round has it.
fn subject(track: &RoundTrack, poses: &Poses, wanted: &str) -> Option<u32> {
    let steam_id = wanted.parse::<u64>().ok().filter(|&id| id != 0);
    track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .find(|p| match steam_id {
            Some(id) => p.steam_id == id,
            None => p.steam_id == 0 && p.name == wanted,
        })
        .map(|p| p.controller)
        .filter(|&c| poses.of(c).is_some())
}

/// The tick `controller` died in the round (their `player_death`), if they did.
fn death_of(track: &RoundTrack, controller: u32) -> Option<u32> {
    track
        .events
        .iter()
        .filter(|e| e.name == "player_death" && e.controller("userid") == Some(controller))
        .map(|e| e.tick)
        .min()
}

/// The round's kills a camera can follow: `(tick, killer, victim)` of each `player_death` whose killer is a player of
/// the round, in order.
fn kills(track: &RoundTrack, poses: &Poses) -> Vec<(u32, u32, Option<u32>)> {
    let mut kills: Vec<(u32, u32, Option<u32>)> = track
        .events
        .iter()
        .filter(|e| e.name == "player_death")
        .filter_map(|e| {
            let killer = e.controller("attacker")?;
            poses
                .of(killer)
                .map(|_| (e.tick, killer, e.controller("userid")))
        })
        .collect();
    kills.sort_by_key(|k| k.0);
    kills
}

/// Every camera of a round (module docs), with the map's `collision` for the shot cameras and every player's `recoil`
/// for their eyes (either can be missing: shots then ignore walls, eyes follow the eye angles alone). HLAE cameras
/// come already read ([`load_hlae`]).
pub fn round_cameras(
    track: &RoundTrack,
    plan: &CameraPlan,
    hlae: Vec<NamedCamera>,
    collision: Option<&WorldCollision>,
    recoil: Option<&BTreeMap<u32, Vec<Recoil>>>,
) -> RoundCameras {
    let poses = Poses::new(track);
    let interval = f64::from(track.tick_interval);
    let all_kills = kills(track, &poses);
    let mut out = RoundCameras {
        players: poses
            .controllers()
            .map(|c| (c, poses.name(c).unwrap_or_default().to_owned()))
            .collect(),
        eyes: eyes(track, &poses, &all_kills, plan.smoothing, recoil),
        last_tick: track.end_tick,
        ..RoundCameras::default()
    };
    out.focus = focus(&poses, &out.eyes, collision, interval);
    let mut end_tick = track.end_tick + 1;
    // A round about one player: their kills only, and it ends a little after their death.
    let mut kills = all_kills;
    if let Some(wanted) = &plan.player {
        match subject(track, &poses, wanted) {
            Some(subject) => {
                out.subject = Some(subject);
                if let Some(death) = death_of(track, subject) {
                    let hold = (DEATH_HOLD_S / interval).round() as u32;
                    end_tick = end_tick.min(death + hold + 1);
                }
                out.last_tick = end_tick - 1;
                kills.retain(|k| k.1 == subject && k.0 < end_tick);
                if kills.is_empty() {
                    out.cuts.push(Cut {
                        start: track.start_tick,
                        end: end_tick,
                        view: View::Eyes(subject),
                    });
                }
            }
            None => out.warnings.push(format!(
                "no player {wanted} in round {}: each kill through its killer's eyes",
                track.round
            )),
        }
    }
    let after = (CUT_AFTER_KILL_S / interval).round() as u32;
    let lead = (LEAD_S / interval).round() as u32;
    let mut start = track.start_tick;
    for (i, &(tick, killer, victim)) in kills.iter().enumerate() {
        let end = if i + 1 == kills.len() {
            end_tick
        } else {
            end_tick.min(tick + after)
        };
        if end <= start {
            continue;
        }
        let kind = plan.shots.get(&tick).copied().unwrap_or_default();
        let shot_start = start.max(tick.saturating_sub(lead));
        let mut cut = |from: u32, to: u32, view: View| {
            if to > from {
                out.cuts.push(Cut {
                    start: from,
                    end: to,
                    view,
                });
            }
        };
        match kind {
            ShotKind::Pov => cut(start, end, View::Eyes(killer)),
            ShotKind::Victim => match victim.filter(|v| poses.of(*v).is_some()) {
                Some(v) => {
                    // A kill right after the last one's cut began: its own cut may start past it.
                    let death = (tick + 1).clamp(start, end);
                    cut(start, shot_start, View::Eyes(killer));
                    cut(shot_start, death, View::Eyes(v));
                    cut(death, end, View::Eyes(killer));
                }
                None => cut(start, end, View::Eyes(killer)),
            },
            _ => {
                let spec = ShotSpec {
                    kind,
                    subject: killer,
                    other: victim.filter(|&v| v != killer),
                    start: shot_start,
                    focus: tick,
                    end: end - 1,
                };
                match build(&spec, &poses, collision, interval) {
                    Some((made, keys)) => {
                        if made != kind {
                            out.warnings.push(format!(
                                "kill at {tick}: no place sees both players for a {} shot: {} instead",
                                kind.name(),
                                made.name()
                            ));
                        }
                        let name = poses.name(killer).unwrap_or("player");
                        out.cameras.push(NamedCamera {
                            name: format!("{} {name}, kill at {tick}", title(made)),
                            kind: made.name().into(),
                            subject: Some(killer),
                            keys,
                        });
                        cut(start, shot_start, View::Eyes(killer));
                        cut(shot_start, end, View::Camera(out.cameras.len() - 1));
                    }
                    None => {
                        out.warnings.push(format!(
                            "kill at {tick}: no {} shot (the killer has no pose then): through his eyes",
                            kind.name()
                        ));
                        cut(start, end, View::Eyes(killer));
                    }
                }
            }
        }
        start = end;
    }
    for camera in hlae {
        let first = camera.keys.first().map_or(0.0, |k| k.tick);
        let last = camera.keys.last().map_or(0.0, |k| k.tick);
        // Whole ticks within the camera's time (a key a hair off a whole tick counts as on it).
        let from = ((first - 1e-6).ceil().max(0.0) as u32).max(track.start_tick);
        let to = (((last + 1e-6).floor().max(0.0) as u32) + 1).min(end_tick);
        if to <= from {
            out.warnings.push(format!(
                "{}: ticks {first:.0} – {last:.0} are not in the round ({} – {})",
                camera.name, track.start_tick, track.end_tick
            ));
            continue;
        }
        out.cameras.push(camera);
        let view = View::Camera(out.cameras.len() - 1);
        out.cuts = overlay(
            &out.cuts,
            Cut {
                start: from,
                end: to,
                view,
            },
        );
    }
    out
}

/// `cuts` with `over` laid on top: the cuts it covers shortened or split around it.
fn overlay(cuts: &[Cut], over: Cut) -> Vec<Cut> {
    let mut out = Vec::with_capacity(cuts.len() + 2);
    for &c in cuts {
        if c.end <= over.start || c.start >= over.end {
            out.push(c);
            continue;
        }
        if c.start < over.start {
            out.push(Cut {
                end: over.start,
                ..c
            });
        }
        if c.end > over.end {
            out.push(Cut {
                start: over.end,
                ..c
            });
        }
    }
    out.push(over);
    out.sort_by_key(|c| c.start);
    out
}

fn title(kind: ShotKind) -> &'static str {
    match kind {
        ShotKind::Pov => "Eyes of",
        ShotKind::Victim => "Victim of",
        ShotKind::Behind => "Behind",
        ShotKind::Orbit => "Orbit around",
        ShotKind::Tripod => "Tripod on",
        ShotKind::Above => "Above",
    }
}

/// Each player's eyes (module docs).
fn eyes(
    track: &RoundTrack,
    poses: &Poses,
    kills: &[(u32, u32, Option<u32>)],
    smoothing: f64,
    recoil: Option<&BTreeMap<u32, Vec<Recoil>>>,
) -> BTreeMap<u32, Vec<CameraKey>> {
    let mut out = BTreeMap::new();
    for controller in poses.controllers() {
        let Some(player) = poses.of(controller) else {
            continue;
        };
        let known: Vec<_> = player.iter().flatten().collect();
        let mut view: Vec<[f64; 2]> = known.iter().map(|p| p.view).collect();
        if smoothing > 0.0 {
            // Pinned around the player's shots and kills: the aim the bullets were built from.
            let mut moments: Vec<u32> = track
                .shots
                .iter()
                .filter(|(c, _)| *c == controller)
                .map(|(_, s)| s.tick)
                .collect();
            moments.extend(kills.iter().filter(|k| k.1 == controller).map(|k| k.0));
            let pinned: Vec<bool> = known
                .iter()
                .map(|p| {
                    moments
                        .iter()
                        .any(|&m| p.tick + PIN_AROUND_SHOT >= m && p.tick <= m + PIN_AROUND_SHOT)
                })
                .collect();
            view = smooth_view(&view, &pinned, lambda(smoothing));
        }
        let punch: BTreeMap<u32, [f64; 2]> = recoil
            .and_then(|r| r.get(&controller))
            .map(|ticks| {
                ticks
                    .iter()
                    .map(|r| {
                        let camera: [f64; 2] =
                            std::array::from_fn(|k| f64::from(r.aim[k] * VIEW_SCALE + r.view[k]));
                        (r.tick, camera)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let keys = known
            .iter()
            .zip(view)
            .map(|(p, [pitch, yaw])| {
                let [dp, dy] = punch.get(&p.tick).copied().unwrap_or([0.0; 2]);
                CameraKey {
                    tick: f64::from(p.tick),
                    position: p.eyes,
                    angles: [pitch + dp, crate::camera::wrap_degrees(yaw + dy), 0.0],
                    fov: DEFAULT_FOV,
                }
            })
            .collect();
        out.insert(controller, keys);
    }
    out
}

/// Rounds to `places` decimals for the file.
fn rounded(value: f64, places: i32) -> f64 {
    let s = 10f64.powi(places);
    (value * s).round() / s
}

/// The cameras file (`docs/formats/cameras.md`).
pub fn cameras_json(
    cameras: &RoundCameras,
    track: &RoundTrack,
    demo_name: &str,
    plan: &CameraPlan,
) -> Json {
    let eyes: serde_json::Map<String, Json> = cameras
        .eyes
        .iter()
        .map(|(controller, keys)| {
            let rows: Vec<Json> = keys
                .iter()
                .map(|k| {
                    json!([
                        k.tick as u32,
                        rounded(k.position[0], 3),
                        rounded(k.position[1], 3),
                        rounded(k.position[2], 3),
                        rounded(k.angles[0], 4),
                        rounded(k.angles[1], 4)
                    ])
                })
                .collect();
            (controller.to_string(), Json::Array(rows))
        })
        .collect();
    let named: Vec<Json> = cameras
        .cameras
        .iter()
        .map(|c| {
            let keys: Vec<Json> = c
                .keys
                .iter()
                .map(|k| {
                    json!([
                        rounded(k.tick, 4),
                        rounded(k.position[0], 3),
                        rounded(k.position[1], 3),
                        rounded(k.position[2], 3),
                        rounded(k.angles[0], 4),
                        rounded(k.angles[1], 4),
                        rounded(k.angles[2], 4),
                        rounded(k.fov, 3)
                    ])
                })
                .collect();
            json!({ "name": c.name, "kind": c.kind, "subject": c.subject, "keys": keys })
        })
        .collect();
    let cuts: Vec<Json> = cameras
        .cuts
        .iter()
        .map(|c| match c.view {
            View::Eyes(controller) => json!({ "start": c.start, "end": c.end, "eyes": controller }),
            View::Camera(index) => json!({ "start": c.start, "end": c.end, "camera": index }),
        })
        .collect();
    json!({
        "format": CAMERAS_FORMAT,
        "version": CAMERAS_VERSION,
        "demo": demo_name,
        "map": track.map_name,
        "round": track.round,
        "start_tick": track.start_tick,
        "end_tick": track.end_tick,
        "tick_interval": track.tick_interval,
        "server_tick_offset": track.server_tick_offset,
        "plan": plan,
        "players": cameras.players.iter().map(|(c, n)| (c.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
        "eyes": eyes,
        "cameras": named,
        "cuts": cuts,
        "subject": cameras.subject,
        "last_tick": cameras.last_tick,
        "focus": cameras
            .focus
            .iter()
            .map(|(c, keys)| {
                let rows: Vec<Json> = keys.iter().map(|&(t, d)| json!([t, rounded(d, 0)])).collect();
                (c.to_string(), Json::Array(rows))
            })
            .collect::<serde_json::Map<_, _>>(),
    })
}

/// The cameras of their own in a cameras file's text (its `cameras`; the eyes and cuts are left out), and its clock
/// (a file without `server_tick_offset`, as one written from Unreal may be, counts HLAE times from the demo's tick
/// 0).
pub fn read_cameras(text: &str) -> Result<(Clock, Vec<NamedCamera>), String> {
    let json: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if json["format"] != CAMERAS_FORMAT {
        return Err(format!("not an OFAST cameras file ({CAMERAS_FORMAT})"));
    }
    let clock = Clock {
        tick_interval: json["tick_interval"].as_f64().ok_or("no tick_interval")?,
        server_tick_offset: json["server_tick_offset"].as_i64().unwrap_or(0),
    };
    let mut cameras = Vec::new();
    for (i, camera) in json["cameras"].as_array().into_iter().flatten().enumerate() {
        let keys = camera["keys"]
            .as_array()
            .ok_or_else(|| format!("camera {i}: no keys"))?
            .iter()
            .map(|row| {
                let v: Vec<f64> = row
                    .as_array()
                    .map(|r| r.iter().filter_map(Json::as_f64).collect())
                    .unwrap_or_default();
                match v[..] {
                    [tick, x, y, z, pitch, yaw, roll, fov] => Ok(CameraKey {
                        tick,
                        position: [x, y, z],
                        angles: [pitch, yaw, roll],
                        fov,
                    }),
                    _ => Err(format!(
                        "camera {i}: a key of {} numbers, 8 expected",
                        v.len()
                    )),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        cameras.push(NamedCamera {
            name: camera["name"].as_str().unwrap_or("camera").to_owned(),
            kind: camera["kind"].as_str().unwrap_or("").to_owned(),
            subject: camera["subject"].as_u64().map(|s| s as u32),
            keys,
        });
    }
    Ok((clock, cameras))
}

/// What [`export_cameras`] wrote.
#[derive(Debug, Clone)]
pub struct CamerasReport {
    /// The cameras file.
    pub path: PathBuf,
    /// Cameras of their own.
    pub cameras: usize,
    /// Cuts.
    pub cuts: usize,
    /// What could not be done as asked.
    pub warnings: Vec<String>,
}

/// The cameras file's name for a round.
pub fn cameras_file_name(demo_name: &str, round: u32) -> String {
    format!("{demo_name}_r{round}.cameras.json")
}

/// Writes a round's cameras file into `out_dir` (module docs). With `content` (the game's archives and the map's),
/// the shot cameras keep out of walls and the eyes take the recoil; without it, neither (a warning says so).
pub fn export_cameras(
    content: Option<&Content>,
    track: &RoundTrack,
    plan: &CameraPlan,
    demo_name: &str,
    out_dir: &Path,
) -> Result<CamerasReport, ExportError> {
    let mut warnings = Vec::new();
    let clock = Clock {
        tick_interval: f64::from(track.tick_interval),
        server_tick_offset: track.server_tick_offset.unwrap_or(0),
    };
    if !plan.hlae.is_empty() && track.server_tick_offset.is_none() {
        warnings.push("the demo has no server tick: HLAE times are taken as demo ticks".into());
    }
    let hlae = plan
        .hlae
        .iter()
        .map(|path| load_hlae(path, clock))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ExportError::Camera)?;
    let (collision, recoil) = match content {
        Some(content) => {
            let collision = load_world_physics(content, &track.map_name)
                .inspect_err(|e| warnings.push(format!("no collision: {e}: cameras ignore walls")))
                .ok()
                .map(|w| WorldCollision::from_physics(&w));
            let items = content
                .read(ITEMS_GAME)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned());
            let data = ShotData::load(content, items.as_deref(), &mut warnings);
            (collision, Some(recoil_by_tick(track, &data)))
        }
        None => {
            warnings.push("no CS2 content: shot cameras ignore walls, eyes have no recoil".into());
            (None, None)
        }
    };
    let mut cameras = round_cameras(track, plan, hlae, collision.as_ref(), recoil.as_ref());
    warnings.append(&mut cameras.warnings);
    let json = cameras_json(&cameras, track, demo_name, plan);
    fs::create_dir_all(out_dir)?;
    let path = out_dir.join(cameras_file_name(demo_name, track.round));
    fs::write(
        &path,
        serde_json::to_string(&json).expect("JSON serializes"),
    )?;
    Ok(CamerasReport {
        path,
        cameras: cameras.cameras.len(),
        cuts: cameras.cuts.len(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::hlae::Campath;
    use crate::demo::events::{EventValue, GameEvent};
    use crate::demo::players::{PlayerSample, TickSample};

    fn player(controller: u32, x: f32, yaw: f32) -> PlayerSample {
        PlayerSample {
            controller,
            name: format!("p{controller}"),
            steam_id: 0,
            team: 2 + controller % 2,
            position: [x, 0.0, 0.0],
            view_offset: [0.0, 0.0, 64.0],
            pitch: 0.0,
            yaw,
            alive: true,
            rig: Default::default(),
            appearance: Default::default(),
            weapons: Vec::new(),
            death: None,
            viewmodel: Default::default(),
        }
    }

    fn death(tick: u32, attacker: u32, victim: u32) -> GameEvent {
        GameEvent {
            tick,
            name: "player_death".into(),
            keys: vec![
                ("attacker".into(), EventValue::Int(i64::from(attacker) - 1)),
                ("userid".into(), EventValue::Int(i64::from(victim) - 1)),
            ],
        }
    }

    /// A 10-second round from tick 1000: player 1 kills player 2 at 1300 and player 3 at 1600.
    fn track() -> RoundTrack {
        RoundTrack {
            round: 3,
            start_tick: 1000,
            end_tick: 1639,
            tick_interval: 1.0 / 64.0,
            map_name: "de_test".into(),
            samples: (1000..1640)
                .map(|tick| TickSample {
                    tick,
                    players: vec![
                        player(1, (tick - 1000) as f32, 0.0),
                        player(2, 500.0, 180.0),
                        player(3, 900.0, 180.0),
                    ],
                    dropped: Vec::new(),
                    doors: Vec::new(),
                    props: Vec::new(),
                    grenades: Vec::new(),
                    fires: Vec::new(),
                })
                .collect(),
            events: vec![death(1300, 1, 2), death(1600, 1, 3)],
            smoke_logs: Vec::new(),
            shots: Vec::new(),
            player_hits: Vec::new(),
            earlier_shots: Vec::new(),
            earlier_hits: Vec::new(),
            server_tick_offset: Some(2403),
            skins: Default::default(),
            stattrak: Default::default(),
        }
    }

    #[test]
    fn eyes_focus_on_the_player_in_front() {
        let track = track();
        let cameras = round_cameras(&track, &CameraPlan::default(), Vec::new(), None, None);
        let at = |c: u32, tick: u32| {
            cameras.focus[&c]
                .iter()
                .find(|&&(t, _)| t == tick)
                .map(|&(_, d)| d)
                .unwrap()
        };
        // Player 3 at x = 900 looks back at player 2, 400 units away, a steady focus while player 1 walks up to
        // player 2; once player 1 is past (x 636 at the end), the focus pulls in to them.
        assert!((at(3, 1000) - 400.0).abs() < 1e-6);
        assert!((at(3, 1400) - 400.0).abs() < 1e-6);
        assert!(at(3, 1636) < 300.0);
        // Player 1 starts 500 units from player 2, and focuses there at once.
        assert!((at(1, 1000) - 500.0).abs() < 1e-6);
        // Player 2 looks back along -x: at player 1 coming, then, once player 1 is past (tick 1500 on), at nothing:
        // the focus pulls out towards FOCUS_REACH, smoothly.
        assert!((at(2, 1100) - 400.0).abs() < 20.0);
        let late = at(2, 1636);
        assert!(late > 2000.0 && late <= FOCUS_REACH, "{late}");
        assert_eq!(cameras.focus[&1][1].0 - cameras.focus[&1][0].0, FOCUS_STEP);
        // A ray over a player's head misses them.
        let up = crate::camera::forward(-30.0, 0.0);
        assert_eq!(
            body_along([0.0, 0.0, 64.0], up, [300.0, 0.0, 0.0], 72.0),
            None
        );
        let level = crate::camera::forward(0.0, 0.0);
        assert_eq!(
            body_along([0.0, 0.0, 64.0], level, [300.0, 0.0, 0.0], 72.0),
            Some(300.0)
        );
        // Written to the file, a row per measured tick.
        let file = cameras_json(&cameras, &track, "demo", &CameraPlan::default());
        assert_eq!(file["focus"]["3"][0], json!([1000, 400.0]));
    }

    #[test]
    fn kills_cut_to_their_killers_by_default() {
        let t = track();
        let cameras = round_cameras(&t, &CameraPlan::default(), Vec::new(), None, None);
        assert_eq!(
            cameras.cuts,
            vec![
                Cut {
                    start: 1000,
                    end: 1332,
                    view: View::Eyes(1)
                },
                Cut {
                    start: 1332,
                    end: 1640,
                    view: View::Eyes(1)
                },
            ]
        );
        assert!(cameras.cameras.is_empty());
        assert_eq!(cameras.eyes.len(), 3);
        let eyes = &cameras.eyes[&1][100];
        assert_eq!(eyes.position, [100.0, 0.0, 64.0]);
        assert_eq!(eyes.fov, DEFAULT_FOV);
    }

    #[test]
    fn a_round_about_one_player_stays_on_them() {
        let mut t = track();
        // Player 3 kills player 1 at 1400 (player 1's kill of player 3 at 1600 is gone).
        t.events = vec![death(1300, 1, 2), death(1400, 3, 1)];
        let about = |t: &RoundTrack, player: &str| {
            let plan = CameraPlan {
                player: Some(player.into()),
                ..CameraPlan::default()
            };
            round_cameras(t, &plan, Vec::new(), None, None)
        };
        // Player 1 kills at 1300 and dies at 1400: their eyes to 2 s after, where the round ends.
        let one = about(&t, "p1");
        let hold = (DEATH_HOLD_S * 64.0).round() as u32;
        assert_eq!(one.subject, Some(1));
        assert_eq!(one.last_tick, 1400 + hold);
        assert_eq!(
            one.cuts,
            vec![Cut {
                start: 1000,
                end: 1400 + hold + 1,
                view: View::Eyes(1)
            }]
        );
        // Player 3 lives: their eyes to the round's end, never player 1's (who killed first).
        let three = about(&t, "p3");
        assert_eq!(three.last_tick, 1639);
        assert!(three.cuts.iter().all(|c| c.view == View::Eyes(3)));
        assert_eq!(three.cuts.first().map(|c| c.start), Some(1000));
        assert_eq!(three.cuts.last().map(|c| c.end), Some(1640));
        // A player without a kill, named by Steam id.
        for sample in &mut t.samples {
            sample.players[1].steam_id = 76_561_198_000_000_002;
        }
        let two = about(&t, "76561198000000002");
        assert_eq!(two.subject, Some(2));
        assert_eq!(
            two.cuts,
            vec![Cut {
                start: 1000,
                end: 1300 + hold + 1,
                view: View::Eyes(2)
            }]
        );
        // Nobody of that name: each killer, with a warning.
        let nobody = about(&t, "nobody");
        assert_eq!(nobody.subject, None);
        assert_eq!(nobody.last_tick, 1639);
        assert_eq!(nobody.warnings.len(), 1);
        assert_eq!(nobody.cuts.first().map(|c| c.view), Some(View::Eyes(1)));
    }

    #[test]
    fn a_shot_takes_the_seconds_before_its_kill() {
        let t = track();
        let plan = CameraPlan {
            shots: BTreeMap::from([(1300, ShotKind::Orbit), (1600, ShotKind::Victim)]),
            ..CameraPlan::default()
        };
        let cameras = round_cameras(&t, &plan, Vec::new(), None, None);
        // Kill 1 at 1300: eyes until 4 s before (1044), then the orbit to 1332. Kill 2 at 1600: its cut starts at
        // 1332, the victim's eyes from 1344 to the kill, then the killer's.
        assert_eq!(
            cameras.cuts,
            vec![
                Cut {
                    start: 1000,
                    end: 1044,
                    view: View::Eyes(1)
                },
                Cut {
                    start: 1044,
                    end: 1332,
                    view: View::Camera(0)
                },
                Cut {
                    start: 1332,
                    end: 1344,
                    view: View::Eyes(1)
                },
                Cut {
                    start: 1344,
                    end: 1601,
                    view: View::Eyes(3)
                },
                Cut {
                    start: 1601,
                    end: 1640,
                    view: View::Eyes(1)
                },
            ]
        );
        let orbit = &cameras.cameras[0];
        assert_eq!(orbit.kind, "orbit");
        assert_eq!(orbit.keys.first().unwrap().tick, 1044.0);
        assert_eq!(orbit.keys.last().unwrap().tick, 1331.0);
        assert_eq!(orbit.name, "Orbit around p1, kill at 1300");
    }

    #[test]
    fn hlae_cameras_take_over_their_time() {
        let cuts = vec![
            Cut {
                start: 0,
                end: 10,
                view: View::Eyes(1),
            },
            Cut {
                start: 10,
                end: 20,
                view: View::Eyes(2),
            },
        ];
        let over = Cut {
            start: 5,
            end: 12,
            view: View::Camera(0),
        };
        assert_eq!(
            overlay(&cuts, over),
            vec![
                Cut {
                    start: 0,
                    end: 5,
                    view: View::Eyes(1)
                },
                over,
                Cut {
                    start: 12,
                    end: 20,
                    view: View::Eyes(2)
                },
            ]
        );
        // A campath inside the round.
        let t = track();
        let clock = Clock {
            tick_interval: 1.0 / 64.0,
            server_tick_offset: 2403,
        };
        let keys = (0..4)
            .map(|i| CameraKey {
                tick: 1100.0 + f64::from(i) * 50.0,
                position: [0.0, f64::from(i) * 10.0, 100.0],
                angles: [0.0, 0.0, 0.0],
                fov: 90.0,
            })
            .collect::<Vec<_>>();
        let sampled = Campath::from_keys(&keys, clock).keys(clock);
        let camera = NamedCamera {
            name: "HLAE test".into(),
            kind: "hlae".into(),
            subject: None,
            keys: sampled,
        };
        let cameras = round_cameras(&t, &CameraPlan::default(), vec![camera], None, None);
        assert!(cameras.cuts.contains(&Cut {
            start: 1100,
            end: 1251,
            view: View::Camera(0)
        }));
        assert_eq!(cameras.cuts.first().unwrap().start, 1000);
        assert_eq!(cameras.cuts.last().unwrap().end, 1640);
    }

    #[test]
    fn trade_kills_never_overlap_their_cuts() {
        // Player 3 kills player 1 five ticks after his kill: inside the first kill's half second.
        let mut t = track();
        t.events.push(death(1305, 3, 1));
        t.events.sort_by_key(|e| e.tick);
        for kind in ShotKind::ALL {
            let plan = CameraPlan {
                shots: [1300, 1305, 1600]
                    .into_iter()
                    .map(|tick| (tick, kind))
                    .collect(),
                ..CameraPlan::default()
            };
            let cameras = round_cameras(&t, &plan, Vec::new(), None, None);
            for pair in cameras.cuts.windows(2) {
                assert!(
                    pair[0].end <= pair[1].start && pair[0].start < pair[0].end,
                    "{kind:?}: {pair:?}"
                );
            }
            assert_eq!(cameras.cuts.last().unwrap().end, 1640);
        }
    }

    #[test]
    fn cameras_files_read_back() {
        let t = track();
        let plan = CameraPlan {
            shots: BTreeMap::from([(1300, ShotKind::Above)]),
            ..CameraPlan::default()
        };
        let cameras = round_cameras(&t, &plan, Vec::new(), None, None);
        let text = cameras_json(&cameras, &t, "demo", &plan).to_string();
        let (clock, list) = read_cameras(&text).unwrap();
        assert_eq!(clock.server_tick_offset, 2403);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, "above");
        assert_eq!(list[0].keys.len(), cameras.cameras[0].keys.len());
        assert!(read_cameras("{}").is_err());
    }

    #[test]
    fn smoothing_keeps_the_aim_at_shots() {
        let mut t = track();
        // Player 1's yaw jitters; he shoots at 1200.
        for (i, s) in t.samples.iter_mut().enumerate() {
            s.players[0].yaw = if i % 2 == 0 { 2.0 } else { -2.0 };
        }
        t.shots.push((
            1,
            crate::demo::shots::Shot {
                tick: 1200,
                pawn: 1,
                item: 7,
                mode: 0,
                eye: [200.0, 0.0, 64.0],
                angles: [0.0, 2.0, 0.0],
                seed: 1,
                inaccuracy: 0.0,
                spread: 0.0,
                recoil_index: 0.0,
                bullets_left: 30,
                server_tick: None,
                attack: None,
            },
        ));
        let plan = CameraPlan {
            smoothing: 0.6,
            ..CameraPlan::default()
        };
        let cameras = round_cameras(&t, &plan, Vec::new(), None, None);
        let eyes = &cameras.eyes[&1];
        let at = |tick: u32| eyes.iter().find(|k| k.tick == f64::from(tick)).unwrap();
        // Pinned at the shot (its tick is even from 1000: yaw 2).
        assert!((at(1200).angles[1] - 2.0).abs() < 1e-4);
        // Smoothed away from it.
        assert!(at(1100).angles[1].abs() < 0.5, "{:?}", at(1100));
    }
}
