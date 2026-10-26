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
//!   the ticks they swung;
//! - the map's physics props (hard hats, mugs), their origin and rotation keyed on the ticks they moved, the first
//!   key where the map entity puts them;
//! - the thrown grenades (flights keyed like the dropped weapons, detonations) and the fires they lit; each smoke's
//!   cloud is simulated ([`crate::demo::smoke`]) and written next to the file as an atlas texture
//!   ([`super::smoke`]), which its `volume` names.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use super::ExportError;
use super::smoke::{Blast, atlas_layout, smoke_atlas, write_smoke_atlas};
use crate::demo::events::{EventValue, GameEvent};
use crate::demo::grenades::{FireSample, GrenadeKind, GrenadeSample, SmokeLog};
use crate::demo::players::{
    DoorSample, DroppedWeapon, PlayerSample, PropSample, RoundTrack, TickSample, flash_starts,
    track_round,
};
use crate::demo::smoke::{self, SmokeSettings};

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
    /// What could not be written as it should, e.g. a smoke whose voxel log stops reading partway (its cloud then
    /// stays as it was at the last frame read).
    pub warnings: Vec<String>,
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
    // Each smoke's cloud, simulated, as the texture its `volume` names.
    let prefix = format!("{demo_name}_r{round}");
    let mut warnings = Vec::new();
    for log in &track.smoke_logs {
        let frames = smoke::frames(log);
        if let Some(error) = smoke::log_error(log) {
            warnings.push(format!(
                "smoke #{}: {error}; its cloud is simulated from the {} frames before",
                log.entity,
                frames.len()
            ));
        }
        if !frames.is_empty() {
            let settings = SmokeSettings::default();
            let blasts = smoke_blasts(track, log, &frames, settings.hole_radius);
            let atlas = smoke_atlas(&frames, settings, &blasts, track.tick_interval);
            write_smoke_atlas(&out_dir.join(smoke_texture_name(log, &prefix)), &atlas)?;
        }
    }
    Ok(ReplayReport {
        path,
        map: track.map_name.clone(),
        players: replay["players"].as_array().map_or(0, Vec::len),
        ticks: track.samples.len(),
        kills: replay["kills"].as_array().map_or(0, Vec::len),
        seconds: (track.end_tick - track.start_tick) as f32 * track.tick_interval,
        warnings,
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

    // Blindings: the ticks a living pawn's `m_flFlashDuration` changes to a new value above 0 (`flash_starts`, the
    // animation bake's rule too), with `m_flFlashMaxAlpha`. The previous sample's values of the pawns alive in it: a
    // value seen first, or after a gap (dead, absent), starts nothing.
    let mut blindings: BTreeMap<u32, Vec<Json>> = BTreeMap::new();
    let mut seen: BTreeMap<u32, f32> = BTreeMap::new();
    for sample in &track.samples {
        let mut now: BTreeMap<u32, f32> = BTreeMap::new();
        for p in sample.players.iter().filter(|p| p.alive) {
            let duration = p.rig.flash_duration;
            if flash_starts(seen.get(&p.controller).copied(), duration) {
                blindings.entry(p.controller).or_default().push(json!({
                    "tick": sample.tick,
                    "duration": round(duration, 1000.0),
                    "alpha": p.rig.flash_max_alpha,
                }));
            }
            now.insert(p.controller, duration);
        }
        seen = now;
    }
    for (controller, list) in blindings {
        if let Some(player) = players.get_mut(&controller) {
            player["blindings"] = Json::Array(list);
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
        "props": props_json(track),
        "grenades": grenades_json(track, &format!("{demo_name}_r{}", track.round)),
        "fires": fires_json(track),
    })
}

/// The tick and position of the round's game event `name` (`hegrenade_detonate`…) whose `entityid` is `entity`,
/// between `first` and `last` (a slot reused later has events of its own).
fn entity_event(
    track: &RoundTrack,
    name: &str,
    entity: u32,
    first: u32,
    last: u32,
) -> Option<(u32, [f64; 3])> {
    let event = track.events.iter().find(|e| {
        e.name == name
            && e.int("entityid") == Some(i64::from(entity))
            && (first..=last + 1).contains(&e.tick)
    })?;
    let float = |key: &str| match event.get(key) {
        Some(EventValue::Float(v)) => round(*v, 100.0),
        _ => 0.0,
    };
    Some((event.tick, [float("x"), float("y"), float("z")]))
}

/// The thrown grenades: one entry per projectile entity (an index reused by a later grenade starts another), with
/// its kind, model, thrower (controller), `first_tick`, and its flight keyed like the dropped weapons
/// ([`keyed_stretches`]) up to `last_tick`, the last tick its model shows — an HE grenade's and a flashbang's
/// detonation (`hegrenade_detonate`, `flashbang_detonate`: the entity stays in the demo for seconds after, unseen),
/// a smoke's expiry (`smokegrenade_expired`), else the entity's last tick (a molotov's landing, where its fire
/// starts). `detonation` is the detonation event's tick and position; `smoke` a smoke's cloud: `start_tick`,
/// `end_tick`, `position`, `color` (0 – 255) and its `volume` ([`smoke_volume_json`]). `file_prefix` starts the
/// smoke textures' names.
fn grenades_json(track: &RoundTrack, file_prefix: &str) -> Vec<Json> {
    keyed_stretches(
        track,
        |sample| {
            sample
                .grenades
                .iter()
                .map(|g| (g.entity, g, g.position, g.angles))
                .collect()
        },
        |a: &GrenadeSample, b: &GrenadeSample| a.serial == b.serial && a.kind == b.kind,
    )
    .into_iter()
    .map(|s| {
        let g = s.item;
        let detonate = match g.kind {
            GrenadeKind::HighExplosive => Some("hegrenade_detonate"),
            GrenadeKind::Flashbang => Some("flashbang_detonate"),
            GrenadeKind::Smoke => Some("smokegrenade_detonate"),
            GrenadeKind::Decoy => Some("decoy_detonate"),
            GrenadeKind::Molotov => None,
        };
        let detonation =
            detonate.and_then(|name| entity_event(track, name, s.entity, s.first, s.last));
        let expired = (g.kind == GrenadeKind::Smoke)
            .then(|| entity_event(track, "smokegrenade_expired", s.entity, s.first, s.last))
            .flatten();
        // The last tick the model shows.
        let last = match (g.kind, detonation, expired) {
            (GrenadeKind::HighExplosive | GrenadeKind::Flashbang, Some((tick, _)), _) => {
                tick.min(s.last)
            }
            (GrenadeKind::Smoke, _, Some((tick, _))) => tick.min(s.last),
            _ => s.last,
        };
        let mut keys: Vec<Json> = s
            .keys
            .iter()
            .filter(|k| k[0].as_u64().is_some_and(|t| t <= u64::from(last)))
            .cloned()
            .collect();
        // End on a key at the last tick (the transform of the latest key before it).
        if let Some(latest) = keys.last().cloned()
            && latest[0].as_u64() != Some(u64::from(last))
        {
            let mut end = latest;
            end[0] = json!(last);
            keys.push(end);
        }
        // The cloud as sent on the stretch's last sample that had one.
        let smoke = track
            .samples
            .iter()
            .filter(|t| (s.first..=s.last).contains(&t.tick))
            .filter_map(|t| {
                t.grenades
                    .iter()
                    .find(|o| o.entity == s.entity && o.serial == g.serial)
            })
            .filter_map(|o| o.smoke)
            .next_back();
        json!({
            "entity": s.entity,
            "kind": g.kind.name(),
            "model": format!("{:016x}", g.model),
            "thrower": g.thrower,
            "first_tick": s.first,
            "last_tick": last,
            "keys": keys,
            "detonation": detonation.map(|(tick, position)| json!({ "tick": tick, "position": position })),
            "smoke": smoke.map(|c| json!({
                "start_tick": c.start_tick,
                "end_tick": expired.map_or(s.last, |(tick, _)| tick),
                "position": c.position.map(|v| round(v, 100.0)),
                "color": c.color,
                "volume": track
                    .smoke_logs
                    .iter()
                    .find(|l| l.entity == s.entity && l.serial == g.serial)
                    .and_then(|log| smoke_volume_json(log, c.position, file_prefix)),
            })),
        })
    })
    .collect()
}

/// The HE grenades that went off while a smoke's log ran (from its first frame to its last) close enough for their
/// hole (`reach`, voxels) to touch its grid, as blasts in that grid. CS2 sends no record of them in the smoke's log
/// (no hole record in any round of four demos, `examples/smoke_holes.rs`): every client carves the hole itself, as
/// OFAST does, from the detonation (`hegrenade_detonate`).
fn smoke_blasts(
    track: &RoundTrack,
    log: &SmokeLog,
    frames: &[smoke::VoxelFrame],
    reach: f32,
) -> Vec<Blast> {
    // The detonation point as the smoke's `grid_origin` has it (grenades_json: its last sample), so the holes fall
    // where the grid is drawn.
    let Some(point) = track
        .samples
        .iter()
        .flat_map(|s| &s.grenades)
        .filter(|g| g.entity == log.entity && g.serial == log.serial)
        .filter_map(|g| g.smoke.map(|c| c.position))
        .next_back()
    else {
        return Vec::new();
    };
    let (Some(first), Some(last)) = (frames.first(), frames.last()) else {
        return Vec::new();
    };
    let half = smoke::GRID as f32 * smoke::VOXEL_SIZE / 2.0;
    let span = -reach..=smoke::GRID as f32 + reach;
    track
        .events
        .iter()
        .filter(|e| e.name == "hegrenade_detonate" && (first.tick..=last.tick).contains(&e.tick))
        .filter_map(|e| {
            let at = e.position()?;
            let voxel: [f32; 3] =
                std::array::from_fn(|k| (at[k] - (point[k] - half)) / smoke::VOXEL_SIZE);
            voxel.iter().all(|v| span.contains(v)).then_some(Blast {
                tick: e.tick,
                voxel,
            })
        })
        .collect()
}

/// A smoke's texture file name: `<file_prefix>_smoke<entity>-<serial>.png`.
fn smoke_texture_name(log: &SmokeLog, file_prefix: &str) -> String {
    format!("{file_prefix}_smoke{}-{}.png", log.entity, log.serial)
}

/// A smoke's simulated cloud ([`super::smoke`]), for the volume Unreal draws: `texture` (the atlas next to the
/// replay file, written by [`write_replay`]), `frames`, `columns`, `width`, `height` (the layout), `frame_ticks` (the
/// tick each frame's step was sent: the cloud of frame k shows from then), `fading_frame` (the first frame with the
/// fading flag, if any), `grid_origin` (the grid's lowest corner, world inches: the detonation point − 320) and
/// `grid_size` (640). `None` for a log with no frames.
fn smoke_volume_json(log: &SmokeLog, position: [f32; 3], file_prefix: &str) -> Option<Json> {
    let frames = smoke::frames(log);
    if frames.is_empty() {
        return None;
    }
    let (columns, width, height) = atlas_layout(frames.len());
    let half = (smoke::GRID as f32 * smoke::VOXEL_SIZE) / 2.0;
    Some(json!({
        "texture": smoke_texture_name(log, file_prefix),
        "frames": frames.len(),
        "columns": columns,
        "width": width,
        "height": height,
        "frame_ticks": frames.iter().map(|f| f.tick).collect::<Vec<_>>(),
        "fading_frame": frames.iter().position(|f| f.fading),
        "grid_origin": position.map(|v| round(v - half, 100.0)),
        "grid_size": 2.0 * half,
    }))
}

/// The fires of molotovs and incendiary grenades: one entry per fire entity, with `incendiary`, `start_tick` (when
/// it lit), `end_tick` (`inferno_expire`: a smoke can put it out early; else the entity's last tick), `lifetime`
/// (seconds), and its flames in the order they appeared: `position`, `normal`, and the first and last tick each
/// burned.
fn fires_json(track: &RoundTrack) -> Vec<Json> {
    // (entity, serial) → (first sample, first tick, last tick, flames: position, normal, first, last burning)
    type Flames = Vec<([f32; 3], [f32; 3], Option<(u32, u32)>)>;
    let mut fires: BTreeMap<(u32, u32), (&FireSample, u32, u32, Flames)> = BTreeMap::new();
    for sample in &track.samples {
        for f in &sample.fires {
            let entry = fires.entry((f.entity, f.serial)).or_insert((
                f,
                sample.tick,
                sample.tick,
                Vec::new(),
            ));
            entry.2 = sample.tick;
            for (k, flame) in f.flames.iter().enumerate() {
                if entry.3.len() <= k {
                    entry.3.push((flame.position, flame.normal, None));
                }
                if flame.burning {
                    let span = &mut entry.3[k].2;
                    *span =
                        Some(span.map_or((sample.tick, sample.tick), |(a, _)| (a, sample.tick)));
                }
            }
        }
    }
    let mut out: Vec<(u32, Json)> = fires
        .into_iter()
        .map(|((entity, _), (f, first, last, flames))| {
            let start = f.start_tick.unwrap_or(first);
            let end = entity_event(track, "inferno_expire", entity, first, last)
                .map_or(last, |(tick, _)| tick.min(last));
            let flames: Vec<Json> = flames
                .into_iter()
                .filter_map(|(position, normal, span)| {
                    let (a, b) = span?;
                    Some(json!({
                        "position": position.map(|v| round(v, 100.0)),
                        "normal": normal.map(|v| round(v, 1000.0)),
                        "first_tick": a,
                        "last_tick": b.min(end),
                    }))
                })
                .collect();
            (
                start,
                json!({
                    "entity": entity,
                    "incendiary": f.incendiary,
                    "start_tick": start,
                    "end_tick": end,
                    "lifetime": f.lifetime,
                    "flames": flames,
                }),
            )
        })
        .collect();
    out.sort_by_key(|(start, _)| *start);
    out.into_iter().map(|(_, j)| j).collect()
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

/// The map's physics props: one entry per prop entity of the round, with its model and its keys ([`keyed_stretches`]:
/// its origin and rotation on the first tick and every tick it moved). CS2 makes every prop again where the map
/// entity puts it (at the round's start, sometimes within the round), so an entry's first key names the prop in the
/// map export, and the entries with the same first key are the same prop's.
fn props_json(track: &RoundTrack) -> Vec<Json> {
    keyed_stretches(
        track,
        |sample| {
            sample
                .props
                .iter()
                .map(|p| (p.entity, p, p.position, p.angles))
                .collect()
        },
        // An index reused by another prop starts a new stretch.
        |a: &PropSample, b: &PropSample| a.serial == b.serial && a.model == b.model,
    )
    .into_iter()
    .map(|s| {
        json!({
            "entity": s.item.entity,
            "model": format!("{:016x}", s.item.model),
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
                // Flashed from before the round (not a blinding of this round), then again at tick 101.
                flash_duration: if x < 1.0 { 3.0 } else { 2.5 },
                flash_max_alpha: 255.0,
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
                    props: vec![prop(180, -344.0), prop(181, -744.0)],
                    grenades: vec![grenade(0.0)],
                    fires: Vec::new(),
                },
                TickSample {
                    tick: 101,
                    players: vec![player(1, 1.0, true)], // player 2 has no pawn this tick
                    // The HE grenade was picked up; the AK-47 lies still.
                    dropped: vec![dropped(300, 7, 10.0)],
                    doors: vec![door(0.0)],
                    props: vec![prop(180, -344.0), prop(181, -744.0)],
                    // The HE grenade goes off here; a fire lights with one flame.
                    grenades: vec![grenade(10.0)],
                    fires: vec![fire(&[true])],
                },
                TickSample {
                    tick: 102,
                    players: vec![player(1, 2.0, true), player(2, 5.0, false)],
                    // The AK-47 is pushed; the door starts to open.
                    dropped: vec![dropped(300, 7, 12.0)],
                    doors: vec![door(6.5)],
                    // The hat falls; the other prop stays.
                    props: vec![prop(180, -400.0), prop(181, -744.0)],
                    // The HE grenade's entity lingers after going off; the fire's first flame went out, a second
                    // burns, and the fire is put out on this tick.
                    grenades: vec![grenade(10.0)],
                    fires: vec![fire(&[false, true])],
                },
            ],
            events: vec![
                GameEvent {
                    tick: 102,
                    name: "player_death".into(),
                    keys: vec![
                        ("userid".into(), EventValue::Int(1)),
                        ("attacker".into(), EventValue::Int(0)),
                        ("assister".into(), EventValue::Int(65535)),
                        ("weapon".into(), EventValue::String("ak47".into())),
                        ("headshot".into(), EventValue::Bool(true)),
                    ],
                },
                GameEvent {
                    tick: 101,
                    name: "hegrenade_detonate".into(),
                    keys: vec![
                        ("entityid".into(), EventValue::Int(400)),
                        ("x".into(), EventValue::Float(10.0)),
                        ("y".into(), EventValue::Float(0.0)),
                        ("z".into(), EventValue::Float(5.0)),
                    ],
                },
                GameEvent {
                    tick: 102,
                    name: "inferno_expire".into(),
                    keys: vec![("entityid".into(), EventValue::Int(500))],
                },
            ],
            smoke_logs: Vec::new(),
            shots: Vec::new(),
            player_hits: Vec::new(),
            earlier_shots: Vec::new(),
            earlier_hits: Vec::new(),
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
        // Player 1 was blinded once this round (the duration seen first is left from before); player 2 never.
        assert_eq!(
            replay["players"][0]["blindings"],
            json!([{ "tick": 101, "duration": 2.5, "alpha": 255.0 }])
        );
        assert_eq!(replay["players"][1]["blindings"], Json::Null);
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

        // The props: the hat where the map has it at 100 and 101, fallen at 102; the other one still all along.
        let props = replay["props"].as_array().expect("props");
        assert_eq!(props.len(), 2);
        assert_eq!(props[0]["model"], "00000000000000cc");
        assert_eq!(
            props[0]["keys"],
            json!([
                [100, 130.0, -1080.0, -344.0, 0.0, -90.0, 0.0],
                [101, 130.0, -1080.0, -344.0, 0.0, -90.0, 0.0],
                [102, 130.0, -1080.0, -400.0, 0.0, -90.0, 0.0]
            ])
        );
        assert_eq!(props[1]["keys"].as_array().map(Vec::len), Some(2));

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

        // The HE grenade: flies from x 0 to 10, gone at its detonation (101) although its entity stays a tick.
        let he = &replay["grenades"][0];
        assert_eq!(he["kind"], json!("he"));
        assert_eq!(he["thrower"], json!(1));
        assert_eq!(
            (he["first_tick"].clone(), he["last_tick"].clone()),
            (json!(100), json!(101))
        );
        assert_eq!(
            he["keys"],
            json!([
                [100, 0.0, 0.0, 5.0, 0.0, 0.0, 0.0],
                [101, 10.0, 0.0, 5.0, 0.0, 0.0, 0.0]
            ])
        );
        assert_eq!(
            he["detonation"],
            json!({ "tick": 101, "position": [10.0, 0.0, 5.0] })
        );
        assert_eq!(he["smoke"], Json::Null);

        // The fire: lit at 101, put out at 102; its first flame burned at 101 only, the second at 102.
        let fire = &replay["fires"][0];
        assert_eq!(
            (fire["start_tick"].clone(), fire["end_tick"].clone()),
            (json!(101), json!(102))
        );
        let flames = fire["flames"].as_array().unwrap();
        assert_eq!(flames.len(), 2);
        assert_eq!(
            (
                flames[0]["first_tick"].clone(),
                flames[0]["last_tick"].clone()
            ),
            (json!(101), json!(101))
        );
        assert_eq!(
            (
                flames[1]["first_tick"].clone(),
                flames[1]["last_tick"].clone()
            ),
            (json!(102), json!(102))
        );
        assert_eq!(flames[1]["position"], json!([50.0, 0.0, 0.0]));
    }

    /// An HE grenade (entity 400, thrown by player 1) at `x`.
    fn grenade(x: f32) -> GrenadeSample {
        GrenadeSample {
            entity: 400,
            serial: 7,
            kind: GrenadeKind::HighExplosive,
            model: 0xcc,
            thrower: Some(1),
            position: [x, 0.0, 5.0],
            angles: [0.0; 3],
            bounces: 0,
            smoke: None,
        }
    }

    /// A molotov's fire (entity 500, lit at tick 101) whose flames burn or not, 50 units apart.
    fn fire(burning: &[bool]) -> FireSample {
        FireSample {
            entity: 500,
            serial: 3,
            incendiary: false,
            start_tick: Some(101),
            lifetime: 7.0,
            flames: burning
                .iter()
                .enumerate()
                .map(|(k, &burning)| crate::demo::grenades::Flame {
                    position: [50.0 * k as f32, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    burning,
                })
                .collect(),
        }
    }

    /// A hard hat at (130, -1080, z).
    fn prop(entity: u32, z: f32) -> PropSample {
        PropSample {
            entity,
            serial: 1,
            model: 0xcc,
            position: [130.0, -1080.0, z],
            angles: [0.0, -90.0, 0.0],
        }
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
