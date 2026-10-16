//! One round of a demo as a replay file for Unreal: `<demo>_r<round>.ofast-replay.json`.
//!
//! The file holds everything the Unreal side needs to rebuild the round as a Level Sequence, in CS2's own units
//! and axes (Source units = inches, x/y/z as in the game, angles in degrees) — `tools/unreal/import_demo.py`
//! converts with the same rules as the map import (`docs/formats/replay.md`):
//!
//! - the round's tick range and the tick its freeze time ended;
//! - the players (controller entity index, name, Steam id as text, team);
//! - per player and per sampled tick: feet position, eye height above the feet, eye pitch and yaw, alive or not —
//!   one array entry per tick in `samples.ticks`, `null` where the player had no pawn;
//! - kills (`player_death`): tick, victim, attacker, assister (controller indices; `null` = nobody, e.g. fall
//!   damage), weapon, headshot and the other flags CS2's kill feed shows;
//! - round milestones (`round_freeze_end`, bomb events, `round_end`), with their keys as sent.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use super::ExportError;
use crate::demo::events::{EventValue, GameEvent};
use crate::demo::players::{RoundTrack, track_round};

/// Format version written into the file; bump on incompatible changes.
pub const REPLAY_VERSION: u32 = 1;

/// Game events kept as round milestones (kills have their own list).
const MILESTONES: &[&str] = &[
    "round_freeze_end",
    "bomb_beginplant",
    "bomb_planted",
    "bomb_begindefuse",
    "bomb_defused",
    "bomb_exploded",
    "round_end",
];

/// Kill flags copied from `player_death` (what the kill feed shows).
const KILL_FLAGS: &[&str] = &[
    "headshot",
    "penetrated",
    "noscope",
    "thrusmoke",
    "attackerblind",
    "assistedflash",
    "attackerinair",
];

/// What [`export_round`] wrote.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    /// The replay file.
    pub path: PathBuf,
    /// Map name from the demo header.
    pub map: String,
    /// Players in the round.
    pub players: usize,
    /// Sampled ticks.
    pub ticks: usize,
    /// Kills in the round.
    pub kills: usize,
    /// Round length in seconds.
    pub seconds: f32,
}

/// Reads round `round` of a demo and writes its replay file into `out_dir`. `demo_name` names the file.
pub fn export_round(
    data: &[u8],
    demo_name: &str,
    round: u32,
    out_dir: &Path,
) -> Result<ReplayReport, ExportError> {
    let track = track_round(data, round)?;
    let replay = replay_json(&track, demo_name);
    fs::create_dir_all(out_dir)?;
    let path = out_dir.join(format!("{demo_name}_r{round}.ofast-replay.json"));
    fs::write(
        &path,
        serde_json::to_string(&replay).expect("JSON serializes"),
    )?;
    Ok(ReplayReport {
        path,
        map: track.map_name.clone(),
        players: replay["players"].as_array().map_or(0, Vec::len),
        ticks: track.samples.len(),
        kills: replay["kills"].as_array().map_or(0, Vec::len),
        seconds: (track.end_tick - track.start_tick) as f32 * track.tick_interval,
    })
}

/// Builds the replay document of a tracked round.
pub fn replay_json(track: &RoundTrack, demo_name: &str) -> Json {
    // Players: everyone who had a pawn at some tick, with their last known name, Steam id and team.
    let mut players: BTreeMap<u32, Json> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            players.insert(
                p.controller,
                json!({
                    "controller": p.controller,
                    "name": p.name,
                    // As text: 64-bit ids do not survive JavaScript's or Python's float JSON readers everywhere.
                    "steam_id": p.steam_id.to_string(),
                    "team": p.team,
                }),
            );
        }
    }

    // Per-player arrays aligned with `ticks`.
    let ticks: Vec<u32> = track.samples.iter().map(|s| s.tick).collect();
    let mut tracks: BTreeMap<u32, [Vec<Json>; 4]> = players
        .keys()
        .map(|&c| (c, std::array::from_fn(|_| Vec::with_capacity(ticks.len()))))
        .collect();
    for sample in &track.samples {
        for (controller, [position, eye, angles, alive]) in &mut tracks {
            match sample.players.iter().find(|p| p.controller == *controller) {
                Some(p) => {
                    position.push(json!(p.position.map(|v| round(v, 100.0))));
                    eye.push(json!(round(p.view_offset[2], 10_000.0)));
                    angles.push(json!([round(p.pitch, 100.0), round(p.yaw, 100.0)]));
                    alive.push(json!(p.alive));
                }
                None => {
                    for array in [position, eye, angles, alive] {
                        array.push(Json::Null);
                    }
                }
            }
        }
    }
    let samples: serde_json::Map<String, Json> = tracks
        .into_iter()
        .map(|(controller, [position, eye_height, angles, alive])| {
            (
                controller.to_string(),
                json!({ "position": position, "eye_height": eye_height, "angles": angles, "alive": alive }),
            )
        })
        .collect();

    let kills: Vec<Json> = track
        .events
        .iter()
        .filter(|e| e.name == "player_death")
        .map(|e| {
            let mut kill = json!({
                "tick": e.tick,
                "victim": e.controller("userid"),
                "attacker": e.controller("attacker"),
                "assister": e.controller("assister"),
                "weapon": e.string("weapon").unwrap_or_default(),
            });
            for flag in KILL_FLAGS {
                if let Some(value) = e.bool(flag) {
                    kill[*flag] = json!(value);
                }
            }
            kill
        })
        .collect();
    let milestones: Vec<Json> = track
        .events
        .iter()
        .filter(|e| MILESTONES.contains(&e.name.as_str()))
        .map(event_json)
        .collect();
    let freeze_end = track
        .events
        .iter()
        .find(|e| e.name == "round_freeze_end")
        .map(|e| e.tick);

    json!({
        "format": "ofast-replay",
        "version": REPLAY_VERSION,
        "demo": demo_name,
        "map": track.map_name,
        "round": track.round,
        "tick_interval": track.tick_interval,
        "units": "Source units (inches) and axes; angles in degrees (pitch down-positive, yaw counter-clockwise from +x)",
        "start_tick": track.start_tick,
        "freeze_end_tick": freeze_end,
        "end_tick": track.end_tick,
        "players": players.into_values().collect::<Vec<_>>(),
        "samples": { "ticks": ticks, "players": samples },
        "kills": kills,
        "events": milestones,
    })
}

/// A game event as JSON: tick, name and its keys.
fn event_json(event: &GameEvent) -> Json {
    let mut out = json!({ "tick": event.tick, "name": event.name });
    for (key, value) in &event.keys {
        out[key.as_str()] = match value {
            EventValue::String(s) => json!(s),
            EventValue::Float(f) => json!(f),
            EventValue::Int(i) => json!(i),
            EventValue::Bool(b) => json!(b),
            EventValue::U64(u) => json!(u.to_string()),
        };
    }
    out
}

/// Rounds to 1/`scale` (keeps the file small; 0.01 unit is 0.25 mm).
fn round(value: f32, scale: f32) -> f64 {
    (f64::from(value) * f64::from(scale)).round() / f64::from(scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::players::{PlayerSample, TickSample};

    fn player(controller: u32, x: f32, alive: bool) -> PlayerSample {
        PlayerSample {
            controller,
            name: format!("p{controller}"),
            steam_id: 76561198000000000 + u64::from(controller),
            team: 2 + controller % 2,
            position: [x, 2.0, 3.0],
            view_offset: [0.0, 0.0, 63.96875],
            pitch: 1.5,
            yaw: -90.0,
            alive,
        }
    }

    #[test]
    fn tracks_align_with_ticks_and_kills_name_controllers() {
        let track = RoundTrack {
            round: 2,
            start_tick: 100,
            end_tick: 102,
            tick_interval: 1.0 / 64.0,
            map_name: "de_test".into(),
            samples: vec![
                TickSample {
                    tick: 100,
                    players: vec![player(1, 0.0, true), player(2, 5.0, true)],
                },
                TickSample {
                    tick: 101,
                    players: vec![player(1, 1.0, true)], // player 2 has no pawn this tick
                },
                TickSample {
                    tick: 102,
                    players: vec![player(1, 2.0, true), player(2, 5.0, false)],
                },
            ],
            events: vec![GameEvent {
                tick: 102,
                name: "player_death".into(),
                keys: vec![
                    ("userid".into(), EventValue::Int(1)),
                    ("attacker".into(), EventValue::Int(0)),
                    ("assister".into(), EventValue::Int(65535)),
                    ("weapon".into(), EventValue::String("ak47".into())),
                    ("headshot".into(), EventValue::Bool(true)),
                ],
            }],
        };
        let replay = replay_json(&track, "demo");
        assert_eq!(replay["samples"]["ticks"], json!([100, 101, 102]));
        let p2 = &replay["samples"]["players"]["2"];
        assert_eq!(p2["position"][1], Json::Null);
        assert_eq!(p2["alive"], json!([true, null, false]));
        assert_eq!(
            replay["samples"]["players"]["1"]["eye_height"][0],
            json!(63.9688)
        );
        let kill = &replay["kills"][0];
        assert_eq!(kill["victim"], json!(2));
        assert_eq!(kill["attacker"], json!(1));
        assert_eq!(kill["assister"], Json::Null);
        assert_eq!(kill["headshot"], json!(true));
        assert_eq!(replay["players"][1]["steam_id"], json!("76561198000000002"));
    }
}
