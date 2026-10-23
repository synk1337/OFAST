//! One round of a demo as a replay file for Unreal: `<demo>_r<round>.ofast-replay.json`.
//!
//! The file holds everything the Unreal side needs to rebuild the round as a Level Sequence, in CS2's own units
//! and axes (Source units = inches, x/y/z as in the game, angles in degrees) — `tools/unreal/import_demo.py`
//! converts with the same rules as the map import (`docs/formats/replay.md`):
//!
//! - the round's tick range and the tick its freeze time ended;
//! - the players (controller entity index, name, Steam id as text, team);
//! - per player and per sampled tick: feet position, eye height above the feet, eye pitch and yaw, alive or not, and
//!   what drives the player's animation (body yaw, velocity, on the ground, crouch amount, walking, scoped,
//!   defusing, on a ladder, the weapon in hand, its animation state, reloading) — one array entry per tick in `samples.ticks`,
//!   `null` where the player had no pawn;
//! - the round's jumps and landings (tick, player), and the weapons seen (item definition index → entity class);
//! - kills (`player_death`): tick, victim, attacker, assister (controller indices; `null` = nobody, e.g. fall
//!   damage), weapon, headshot and the other flags CS2's kill feed shows;
//! - round milestones (`round_freeze_end`, bomb events, `round_end`), with their keys as sent;
//! - the weapons lying in the world (a dead player's gun, a dropped one): one entry per stretch an entity lay
//!   unowned, with its look and its origin and rotation keyed on the ticks it moved;
//! - the map's rotating doors, with their closed position (which names them in the map) and their rotation keyed on
//!   the ticks they swung.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use super::ExportError;
use crate::demo::events::{EventValue, GameEvent};
use crate::demo::players::{
    DoorSample, DroppedWeapon, PlayerSample, RoundTrack, TickSample, track_round,
};

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
    write_replay(&track, demo_name, out_dir)
}

/// Writes the replay file of an already tracked round into `out_dir` (`<demo_name>_r<round>.ofast-replay.json`).
pub fn write_replay(
    track: &RoundTrack,
    demo_name: &str,
    out_dir: &Path,
) -> Result<ReplayReport, ExportError> {
    let round = track.round;
    let replay = replay_json(track, demo_name);
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
                    // The player's own first-person view: FOV (horizontal at 4:3) and offsets (inches: right,
                    // forward, up), as last known; the hand changes during rounds (`samples`' `left_handed`).
                    "viewmodel": {
                        "fov": p.viewmodel.fov,
                        "offset": p.viewmodel.offset,
                    },
                }),
            );
        }
    }

    // Per-player arrays aligned with `ticks`, one per key of `TRACK_KEYS`.
    let ticks: Vec<u32> = track.samples.iter().map(|s| s.tick).collect();
    let mut tracks: BTreeMap<u32, Vec<Vec<Json>>> = players
        .keys()
        .map(|&c| (c, vec![Vec::with_capacity(ticks.len()); TRACK_KEYS.len()]))
        .collect();
    for sample in &track.samples {
        for (controller, arrays) in &mut tracks {
            let values = sample
                .players
                .iter()
                .find(|p| p.controller == *controller)
                .map(track_values);
            for (k, array) in arrays.iter_mut().enumerate() {
                array.push(values.as_ref().map_or(Json::Null, |v| v[k].clone()));
            }
        }
    }
    let samples: serde_json::Map<String, Json> = tracks
        .into_iter()
        .map(|(controller, arrays)| {
            let fields: serde_json::Map<String, Json> = TRACK_KEYS
                .iter()
                .zip(arrays)
                .map(|(key, array)| ((*key).to_owned(), Json::Array(array)))
                .collect();
            (controller.to_string(), Json::Object(fields))
        })
        .collect();

    // Jumps and landings: each distinct "last jump / landing tick" a player showed during the round.
    let mut jumps = std::collections::BTreeSet::new();
    let mut landings = std::collections::BTreeSet::new();
    let mut weapons: BTreeMap<u32, String> = BTreeMap::new();
    for p in track.samples.iter().flat_map(|s| &s.players) {
        let in_round = |tick: &u32| (track.start_tick..=track.end_tick).contains(tick);
        if let Some(tick) = p.rig.last_jump_tick.filter(in_round) {
            jumps.insert((tick, p.controller));
        }
        if let Some(tick) = p.rig.last_landing_tick.filter(in_round) {
            landings.insert((tick, p.controller));
        }
        if let Some(w) = &p.rig.weapon {
            weapons.entry(w.item).or_insert_with(|| w.class.clone());
        }
    }
    let tick_list = |set: std::collections::BTreeSet<(u32, u32)>| -> Vec<Json> {
        set.into_iter()
            .map(|(tick, player)| json!({ "tick": tick, "player": player }))
            .collect()
    };

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
        "weapons": weapons.into_iter().map(|(item, class)| (item.to_string(), json!(class))).collect::<serde_json::Map<_, _>>(),
        "jumps": tick_list(jumps),
        "landings": tick_list(landings),
        "kills": kills,
        "events": milestones,
        "dropped_weapons": dropped_json(track),
        "doors": doors_json(track),
    })
}

/// The weapons lying in the world: one entry per stretch an entity lay unowned (a weapon picked up and dropped again
/// has two), with `first_tick` and `last_tick` (the last tick it lay there), its look, and its keys
/// ([`keyed_stretches`]).
fn dropped_json(track: &RoundTrack) -> Vec<Json> {
    keyed_stretches(
        track,
        |sample| {
            sample
                .dropped
                .iter()
                .map(|w| (w.entity, w, w.position, w.angles))
                .collect()
        },
        // An index reused by another weapon starts a new stretch.
        |a: &DroppedWeapon, b: &DroppedWeapon| a.item == b.item && a.class == b.class,
    )
    .into_iter()
    .map(|s| {
        json!({
            "entity": s.item.entity,
            "item": s.item.item,
            "class": s.item.class,
            // As text, like the Steam ids: a 64-bit resource id.
            "model": format!("{:016x}", s.item.model),
            "mesh_groups": s.item.mesh_groups,
            "first_tick": s.first,
            "last_tick": s.last,
            "keys": s.keys,
        })
    })
    .collect()
}

/// The map's rotating doors: one entry per door entity of the round, with `closed_position` (the map entity's
/// origin, which names the door in the map export) and `closed_angles`, its model, and its keys
/// ([`keyed_stretches`]: the door's origin and rotation on the ticks it swung).
fn doors_json(track: &RoundTrack) -> Vec<Json> {
    keyed_stretches(
        track,
        |sample| {
            sample
                .doors
                .iter()
                .map(|d| (d.entity, d, d.position, d.angles))
                .collect()
        },
        |a: &DoorSample, b: &DoorSample| a.closed_position == b.closed_position,
    )
    .into_iter()
    .map(|s| {
        json!({
            "entity": s.item.entity,
            "model": format!("{:016x}", s.item.model),
            "closed_position": s.item.closed_position.map(|v| round(v, 100.0)),
            "closed_angles": s.item.closed_angles.map(|v| round(v, 100.0)),
            "first_tick": s.first,
            "last_tick": s.last,
            "keys": s.keys,
        })
    })
    .collect()
}

/// One stretch an entity was there, keyed for [`keyed_stretches`].
struct Stretch<'t, T> {
    entity: u32,
    /// The entity as first seen in the stretch.
    item: &'t T,
    first: u32,
    last: u32,
    /// `[tick, x, y, z, pitch, yaw, roll]` at the first tick and every tick the transform changed.
    keys: Vec<Json>,
    /// Transform at the latest key, and its tick.
    value: [f64; 6],
    keyed: u32,
}

/// The stretches `items` (entity index, item, position, angles per sample) were there, in order of first tick and
/// entity: a stretch ends when the entity is gone from a sample or `same` says another one took its index. Its keys
/// hold the transform at its first tick, every tick it changed (positions and angles rounded to 0.01) and its last
/// tick. Before a change that follows a rest, the resting transform is keyed again on the previous sampled tick, so a
/// straight line between keys never drifts through the rest.
fn keyed_stretches<'t, T>(
    track: &'t RoundTrack,
    items: impl Fn(&'t TickSample) -> Vec<(u32, &'t T, [f32; 3], [f32; 3])>,
    same: impl Fn(&T, &T) -> bool,
) -> Vec<Stretch<'t, T>> {
    let key = |tick: u32, v: &[f64; 6]| json!([tick, v[0], v[1], v[2], v[3], v[4], v[5]]);
    let mut open: BTreeMap<u32, Stretch<'t, T>> = BTreeMap::new();
    let mut done = Vec::new();
    for sample in &track.samples {
        let tick = sample.tick;
        let present = items(sample);
        for &(entity, item, position, angles) in &present {
            let [x, y, z] = position.map(|v| round(v, 100.0));
            let [p, yaw, r] = angles.map(|v| round(v, 100.0));
            let value = [x, y, z, p, yaw, r];
            match open.get_mut(&entity) {
                Some(s) if same(s.item, item) => {
                    if value != s.value {
                        if s.keyed < s.last {
                            let rest = s.value;
                            s.keys.push(key(s.last, &rest));
                        }
                        s.keys.push(key(tick, &value));
                        s.value = value;
                        s.keyed = tick;
                    }
                    s.last = tick;
                }
                _ => {
                    if let Some(old) = open.remove(&entity) {
                        done.push(old);
                    }
                    open.insert(
                        entity,
                        Stretch {
                            entity,
                            item,
                            first: tick,
                            last: tick,
                            keys: vec![key(tick, &value)],
                            value,
                            keyed: tick,
                        },
                    );
                }
            }
        }
        // Gone this tick (a weapon picked up, an entity removed).
        let gone: Vec<u32> = open
            .keys()
            .copied()
            .filter(|e| !present.iter().any(|(entity, ..)| entity == e))
            .collect();
        for entity in gone {
            if let Some(s) = open.remove(&entity) {
                done.push(s);
            }
        }
    }
    done.extend(open.into_values());
    // Each stretch ends on a key: a reader joining stretches (a door's entity made again) never interpolates
    // across the gap between them.
    for s in &mut done {
        if s.keyed < s.last {
            let rest = s.value;
            s.keys.push(key(s.last, &rest));
        }
    }
    done.sort_by_key(|s| (s.first, s.entity));
    done
}

/// Names of the per-player arrays in `samples.players.<controller>`, in the order [`track_values`] fills them.
const TRACK_KEYS: [&str; 16] = [
    "position",
    "eye_height",
    "angles",
    "alive",
    "body_yaw",
    "velocity",
    "on_ground",
    "duck",
    "walking",
    "scoped",
    "defusing",
    "ladder",
    "weapon",
    "weapon_state",
    "reloading",
    "left_handed",
];

/// One player's values at one tick, in [`TRACK_KEYS`] order. Without a weapon in hand the three weapon values are
/// `null`. `left_handed` is the viewmodel's hand, which players switch with a key in the middle of a round (one pro
/// of the checked match did so 31 times in a round).
fn track_values(p: &PlayerSample) -> [Json; TRACK_KEYS.len()] {
    let rig = &p.rig;
    let weapon = rig.weapon.as_ref();
    [
        json!(p.position.map(|v| round(v, 100.0))),
        json!(round(p.view_offset[2], 10_000.0)),
        json!([round(p.pitch, 100.0), round(p.yaw, 100.0)]),
        json!(p.alive),
        json!(round(rig.body_yaw, 100.0)),
        json!(rig.velocity.map(|v| round(v, 10.0))),
        json!(rig.on_ground),
        json!(round(rig.duck_amount, 1000.0)),
        json!(rig.walking),
        json!(rig.scoped),
        json!(rig.defusing),
        json!(rig.on_ladder),
        weapon.map_or(Json::Null, |w| json!(w.item)),
        weapon.map_or(Json::Null, |w| json!(w.anim_state)),
        weapon.map_or(Json::Null, |w| json!(w.reloading)),
        json!(p.viewmodel.left_handed),
    ]
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
    use crate::demo::players::{ActiveWeapon, RigInputs, TickSample};

    /// Player 1 runs with an AK-47 and jumped at tick 101 (and once before the round, at tick 50); player 2 holds
    /// nothing.
    fn player(controller: u32, x: f32, alive: bool) -> PlayerSample {
        let rig = if controller == 1 {
            RigInputs {
                velocity: [215.04, -1.0, 0.0],
                on_ground: x < 1.5,
                duck_amount: 0.25,
                last_jump_tick: Some(if x < 1.0 { 50 } else { 101 }),
                weapon: Some(ActiveWeapon {
                    entity: 300,
                    item: 7,
                    class: "CAK47".into(),
                    anim_state: 50,
                    reloading: false,
                    ..ActiveWeapon::default()
                }),
                ..RigInputs::default()
            }
        } else {
            RigInputs::default()
        };
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
            rig,
            appearance: Default::default(),
            weapons: Vec::new(),
            death: None,
            viewmodel: Default::default(),
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
                    dropped: vec![dropped(300, 7, 10.0), dropped(301, 44, 0.0)],
                    doors: vec![door(0.0)],
                },
                TickSample {
                    tick: 101,
                    players: vec![player(1, 1.0, true)], // player 2 has no pawn this tick
                    // The HE grenade was picked up; the AK-47 lies still.
                    dropped: vec![dropped(300, 7, 10.0)],
                    doors: vec![door(0.0)],
                },
                TickSample {
                    tick: 102,
                    players: vec![player(1, 2.0, true), player(2, 5.0, false)],
                    // The AK-47 is pushed; the door starts to open.
                    dropped: vec![dropped(300, 7, 12.0)],
                    doors: vec![door(6.5)],
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
        assert_eq!(
            replay["players"][1]["viewmodel"],
            json!({ "fov": 60.0, "offset": [1.0, 1.0, -1.0] })
        );

        // Rig inputs: arrays aligned with the ticks like the positions.
        let p1 = &replay["samples"]["players"]["1"];
        assert_eq!(p1["velocity"][0], json!([215.0, -1.0, 0.0]));
        assert_eq!(p1["on_ground"], json!([true, true, false]));
        assert_eq!(p1["duck"][0], json!(0.25));
        assert_eq!(p1["weapon"], json!([7, 7, 7]));
        assert_eq!(p1["weapon_state"][2], json!(50));
        assert_eq!(p2["weapon"], json!([null, null, null]));
        assert_eq!(p2["reloading"][1], Json::Null);
        assert_eq!(p1["left_handed"], json!([false, false, false]));
        assert_eq!(replay["weapons"], json!({ "7": "CAK47" }));
        // The jump before the round (tick 50) is not one of its jumps.
        assert_eq!(replay["jumps"], json!([{ "tick": 101, "player": 1 }]));
        assert_eq!(replay["landings"], json!([]));

        // Dropped weapons: the AK-47 lies from 100 to 102, keyed at 100, again at 101 (its rest, before the push)
        // and at 102; the HE grenade lay one tick.
        let dropped = replay["dropped_weapons"].as_array().unwrap();
        assert_eq!(dropped.len(), 2);
        let ak = dropped.iter().find(|d| d["item"] == json!(7)).unwrap();
        assert_eq!(
            (ak["first_tick"].clone(), ak["last_tick"].clone()),
            (json!(100), json!(102))
        );
        assert_eq!(
            ak["keys"],
            json!([
                [100, 10.0, 2.0, 3.0, 0.0, 90.0, -90.0],
                [101, 10.0, 2.0, 3.0, 0.0, 90.0, -90.0],
                [102, 12.0, 2.0, 3.0, 0.0, 90.0, -90.0]
            ])
        );
        assert_eq!(ak["model"], json!("00000000000000aa"));
        let he = dropped.iter().find(|d| d["item"] == json!(44)).unwrap();
        assert_eq!(
            (he["first_tick"].clone(), he["last_tick"].clone()),
            (json!(100), json!(100))
        );

        // The door: closed at 100 and 101, swung 6.5° at 102.
        let door = &replay["doors"][0];
        assert_eq!(door["closed_position"], json!([257.0, -1310.0, -416.0]));
        assert_eq!(
            door["keys"],
            json!([
                [100, 257.0, -1310.0, -416.0, 0.0, 0.0, 0.0],
                [101, 257.0, -1310.0, -416.0, 0.0, 0.0, 0.0],
                [102, 257.0, -1310.0, -416.0, 0.0, 6.5, 0.0]
            ])
        );
    }

    /// de_nuke's door by A site, swung `yaw` degrees from closed.
    fn door(yaw: f32) -> DoorSample {
        DoorSample {
            entity: 237,
            model: 0xbb,
            closed_position: [257.0, -1310.0, -416.0],
            closed_angles: [0.0; 3],
            position: [257.0, -1310.0, -416.0],
            angles: [0.0, yaw, 0.0],
            state: if yaw == 0.0 { 0 } else { 1 },
        }
    }

    /// A weapon lying at (x, 2, 3) on its side.
    fn dropped(entity: u32, item: u32, x: f32) -> DroppedWeapon {
        DroppedWeapon {
            entity,
            item,
            class: if item == 7 { "CAK47" } else { "CHEGrenade" }.into(),
            model: 0xaa,
            mesh_groups: 1,
            position: [x, 2.0, 3.0],
            angles: [0.0, 90.0, -90.0],
        }
    }
}
