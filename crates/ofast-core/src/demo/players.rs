//! Per-tick player positions and eye angles for one round — the M0 demo spike.
//!
//! Players are two linked entities:
//!
//! - a **controller** (`CCSPlayerController`, one per connected player, entity index = player slot + 1) with the
//!   name (`m_iszPlayerName`), Steam id (`m_steamID`), team (`m_iTeamNum`) and a handle to the pawn
//!   (`m_hPlayerPawn`);
//! - a **pawn** (`CCSPlayerPawn`, the body in the world) with the position, the view offset and the eye angles.
//!
//! The position is split into a coarse grid cell and an offset inside it, both on the pawn's `CBodyComponent`:
//! `x = m_cellX × 512 − 16384 + m_vecX` (cells are 512 units wide and the world spans ±16384 units), same for y and
//! z. That is the feet. The first-person camera sits `m_vecViewOffset` above them (63.96875 units standing,
//! 45.96875 crouched, smoothed over steps, so it changes from tick to tick), and CS2 aims it along `m_angEyeAngles`
//! = (pitch, yaw, roll) in degrees. Both were checked against the camera CS2 itself shows when spectating a player.
//!
//! Rounds come from the game rules entity (`CCSGameRulesProxy` → `m_pGameRules`). A round **starts** when the
//! freeze time begins (`m_bFreezePeriod` turns on) outside warmup and before the match has ended (`m_gamePhase` =
//! 5). It lasts until the tick before the next round starts, the match ends, or the demo ends — so it includes the
//! few seconds after the round is decided. Its number is `m_totalRoundsPlayed + 1` at the start. See
//! `docs/formats/demo.md`.

use super::decoder::FieldValue;
use super::entity::{Entity, handle_to_index};
use super::fieldpath::FieldPath;
use super::parser::{DemoError, DemoParser};
use super::serializer::Serializers;

/// Width of one position grid cell, in world units.
pub const CELL_WIDTH: f32 = 512.0;
/// Half the size of the world: cell coordinates are offset by this so they are never negative.
pub const MAX_COORD: f32 = 16384.0;

/// One player at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSample {
    /// Controller entity index (player slot + 1).
    pub controller: u32,
    /// Player name.
    pub name: String,
    /// 64-bit Steam id (0 for bots).
    pub steam_id: u64,
    /// Team number: 2 = Terrorists, 3 = Counter-Terrorists (1 = spectators).
    pub team: u32,
    /// World position of the pawn's origin (feet), in Source units (1 unit = 1 inch).
    pub position: [f32; 3],
    /// From the feet to the first-person camera: the pawn's networked `m_vecViewOffset`, in world units.
    ///
    /// z is the eye height. It is not a constant: 63.96875 standing (not 64), 45.96875 fully crouched, values in
    /// between while crouching or standing up, other values in special poses (37.96875 was seen crouched with the C4
    /// in hand), and when the feet go up or down a step it moves the other way for a few ticks so the camera glides.
    /// It comes in steps of 1/64, at most 64. x and y are networked too but were 0 in every sample of the checked
    /// match; they are kept because the engine adds the whole vector (so does demoinfocs-golang's `PositionEyes`),
    /// so [`eye_position`](Self::eye_position) stays right even if a CS2 update starts using them.
    pub view_offset: [f32; 3],
    /// Looking up/down in degrees (negative = up).
    pub pitch: f32,
    /// Looking left/right in degrees (-180..180, 0 = +x axis, counter-clockwise).
    pub yaw: f32,
    /// The pawn is alive (`m_lifeState` = 0).
    pub alive: bool,
}

impl PlayerSample {
    /// Where the first-person camera is: [`position`](Self::position) + [`view_offset`](Self::view_offset).
    ///
    /// This is exactly the camera position CS2 prints with `getpos` while spectating the player in first person
    /// (checked to 6 decimals, standing, crouched and running — "Checked in the game" in `docs/formats/demo.md`).
    /// Point the camera along [`pitch`](Self::pitch) and [`yaw`](Self::yaw) to get the player's view.
    pub fn eye_position(&self) -> [f32; 3] {
        let [x, y, z] = self.position;
        let [dx, dy, dz] = self.view_offset;
        [x + dx, y + dy, z + dz]
    }
}

/// Every player at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct TickSample {
    /// Demo tick.
    pub tick: u32,
    /// Players with a pawn, in controller order.
    pub players: Vec<PlayerSample>,
}

/// The players of one round, tick by tick.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTrack {
    /// Round number (1 = first round after warmup).
    pub round: u32,
    /// First tick of the round (freeze time starts).
    pub start_tick: u32,
    /// Last tick that belongs to the round.
    pub end_tick: u32,
    /// Seconds per tick.
    pub tick_interval: f32,
    /// Map name from the demo header.
    pub map_name: String,
    /// One sample per tick that had entity updates.
    pub samples: Vec<TickSample>,
}

/// Where one round is in the demo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundInfo {
    /// Round number (1 = first round after warmup).
    pub round: u32,
    /// First tick (freeze time starts).
    pub start_tick: u32,
    /// Last tick that belongs to the round.
    pub end_tick: u32,
}

/// Errors specific to round tracking.
#[derive(Debug, thiserror::Error)]
pub enum TrackError {
    /// The demo itself could not be read.
    #[error(transparent)]
    Demo(#[from] DemoError),
    /// A class or field the tracker needs is missing (a CS2 update may have renamed it).
    #[error("demo has no {0}")]
    MissingField(String),
    /// The demo ended before the requested round started.
    #[error("round {0} not found in the demo")]
    RoundNotFound(u32),
}

/// Field paths the tracker reads, looked up once by name.
struct Paths {
    controller_class: String,
    pawn_class: String,
    rules_class: String,
    pawn_handle: FieldPath,
    name: FieldPath,
    steam_id: FieldPath,
    team: FieldPath,
    cell: [FieldPath; 3],
    offset: [FieldPath; 3],
    view_offset: [FieldPath; 3],
    eye_angles: FieldPath,
    life_state: FieldPath,
    rounds_played: FieldPath,
    freeze_period: FieldPath,
    warmup_period: FieldPath,
    game_phase: FieldPath,
}

impl Paths {
    fn find(serializers: &Serializers) -> Result<Self, TrackError> {
        let path = |class: &str, field: &str| {
            serializers
                .by_name(class)
                .and_then(|s| serializers.path_of(s, field))
                .ok_or_else(|| TrackError::MissingField(format!("{class}.{field}")))
        };
        let controller = "CCSPlayerController";
        let pawn = "CCSPlayerPawn";
        let rules = "CCSGameRulesProxy";
        // The x, y and z members of a vector field on the pawn, e.g. `CBodyComponent.m_cellX` / `Y` / `Z`.
        let axes = |prefix: &str| -> Result<[FieldPath; 3], TrackError> {
            Ok([
                path(pawn, &format!("{prefix}X"))?,
                path(pawn, &format!("{prefix}Y"))?,
                path(pawn, &format!("{prefix}Z"))?,
            ])
        };
        Ok(Self {
            controller_class: controller.into(),
            pawn_class: pawn.into(),
            rules_class: rules.into(),
            pawn_handle: path(controller, "m_hPlayerPawn")?,
            name: path(controller, "m_iszPlayerName")?,
            steam_id: path(controller, "m_steamID")?,
            team: path(controller, "m_iTeamNum")?,
            cell: axes("CBodyComponent.m_cell")?,
            offset: axes("CBodyComponent.m_vec")?,
            // The full name is needed: the pawn also has `m_vecVelocity.m_vecX` etc. (see `Serializers::path_of`).
            view_offset: axes("m_vecViewOffset.m_vec")?,
            eye_angles: path(pawn, "m_angEyeAngles")?,
            life_state: path(pawn, "m_lifeState")?,
            rounds_played: path(rules, "m_pGameRules.m_totalRoundsPlayed")?,
            freeze_period: path(rules, "m_pGameRules.m_bFreezePeriod")?,
            warmup_period: path(rules, "m_pGameRules.m_bWarmupPeriod")?,
            game_phase: path(rules, "m_pGameRules.m_gamePhase")?,
        })
    }
}

/// `m_gamePhase` value once the match is over (Valve's `GAMEPHASE_MATCH_ENDED`).
const GAME_PHASE_MATCH_ENDED: i64 = 5;

/// Round state read from the game rules entity.
#[derive(Debug, Clone, Copy, Default)]
struct RulesState {
    rounds_played: u32,
    freeze: bool,
    warmup: bool,
    match_ended: bool,
}

/// What happened at a tick, according to [`RoundDetector`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundEvent {
    /// A round ended on the previous sampled tick (its info is complete).
    Ended(RoundInfo),
    /// A round started at this tick (`Ended` for the previous round comes first if one was running).
    Started(u32),
}

/// Turns the game rules state of successive ticks into round start / end events.
#[derive(Debug, Default)]
struct RoundDetector {
    was_freeze: bool,
    current: Option<RoundInfo>,
}

impl RoundDetector {
    /// Feeds the state of one tick; returns up to two events (an end, then a start).
    fn update(&mut self, tick: u32, rules: RulesState) -> Vec<RoundEvent> {
        let mut events = Vec::new();
        let starts = rules.freeze && !self.was_freeze && !rules.warmup && !rules.match_ended;
        self.was_freeze = rules.freeze;
        if (starts || rules.match_ended)
            && let Some(done) = self.current.take()
        {
            events.push(RoundEvent::Ended(done));
        }
        if starts {
            let round = rules.rounds_played + 1;
            self.current = Some(RoundInfo {
                round,
                start_tick: tick,
                end_tick: tick,
            });
            events.push(RoundEvent::Started(round));
        } else if let Some(current) = self.current.as_mut() {
            current.end_tick = tick;
        }
        events
    }

    /// The round still running when the demo ended.
    fn finish(self) -> Option<RoundInfo> {
        self.current
    }
}

/// Calls `on_tick` with the parser and the detected round events after every tick that changed entities.
fn walk_rounds(
    data: &[u8],
    mut on_tick: impl FnMut(&DemoParser<'_>, &Paths, u32, &[RoundEvent], Option<RoundInfo>) -> bool,
) -> Result<Option<RoundInfo>, TrackError> {
    let mut parser = DemoParser::new(data)?;
    let mut paths: Option<Paths> = None;
    let mut detector = RoundDetector::default();
    while let Some(frame) = parser.next_frame()? {
        let (Some(tick), true) = (frame.tick, frame.entities_changed) else {
            continue;
        };
        if paths.is_none() {
            let serializers = parser.serializers().ok_or(DemoError::MissingSendTables)?;
            paths = Some(Paths::find(serializers)?);
        }
        let paths = paths.as_ref().expect("set above");
        let Some(rules) = read_rules(&parser, paths) else {
            continue;
        };
        let events = detector.update(tick, rules);
        if !on_tick(&parser, paths, tick, &events, detector.current) {
            return Ok(None);
        }
    }
    Ok(detector.finish())
}

/// Lists every round in the demo with its tick range (reads the whole demo).
pub fn list_rounds(data: &[u8]) -> Result<Vec<RoundInfo>, TrackError> {
    let mut rounds = Vec::new();
    let last = walk_rounds(data, |_, _, _, events, _| {
        rounds.extend(events.iter().filter_map(|e| match e {
            RoundEvent::Ended(info) => Some(*info),
            RoundEvent::Started(_) => None,
        }));
        true
    })?;
    rounds.extend(last);
    Ok(rounds)
}

/// Reads the demo up to the end of `round` and returns every player's position, view offset and eye angles for each
/// tick of it.
pub fn track_round(data: &[u8], round: u32) -> Result<RoundTrack, TrackError> {
    let mut track: Option<RoundTrack> = None;
    walk_rounds(data, |parser, paths, tick, events, current| {
        if events
            .iter()
            .any(|e| matches!(e, RoundEvent::Ended(info) if info.round == round))
        {
            return false; // our round is complete
        }
        if current.is_some_and(|c| c.round == round) {
            let track = track.get_or_insert_with(|| RoundTrack {
                round,
                start_tick: tick,
                end_tick: tick,
                tick_interval: parser.tick_interval().unwrap_or(1.0 / 64.0),
                map_name: parser.header().map_name().to_owned(),
                samples: Vec::new(),
            });
            let sample = TickSample {
                tick,
                players: read_players(parser, paths),
            };
            // Several frames can share a tick; the last state of the tick wins.
            match track.samples.last_mut() {
                Some(last) if last.tick == tick => *last = sample,
                _ => track.samples.push(sample),
            }
            track.end_tick = tick;
        }
        true
    })?;
    track.ok_or(TrackError::RoundNotFound(round))
}

fn read_rules(parser: &DemoParser<'_>, paths: &Paths) -> Option<RulesState> {
    let rules = parser
        .entities()
        .iter()
        .find(|e| parser.class_name(e.class_id) == Some(paths.rules_class.as_str()))?;
    Some(RulesState {
        rounds_played: rules
            .get(&paths.rounds_played)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0) as u32,
        freeze: rules
            .get(&paths.freeze_period)
            .and_then(FieldValue::as_bool)
            .unwrap_or(false),
        warmup: rules
            .get(&paths.warmup_period)
            .and_then(FieldValue::as_bool)
            .unwrap_or(false),
        match_ended: rules.get(&paths.game_phase).and_then(FieldValue::as_i64)
            == Some(GAME_PHASE_MATCH_ENDED),
    })
}

fn read_players(parser: &DemoParser<'_>, paths: &Paths) -> Vec<PlayerSample> {
    let entities = parser.entities();
    let mut players = Vec::new();
    for controller in entities.iter() {
        if parser.class_name(controller.class_id) != Some(paths.controller_class.as_str()) {
            continue;
        }
        let Some(pawn) = controller
            .get(&paths.pawn_handle)
            .and_then(FieldValue::as_u64)
            .and_then(handle_to_index)
            .and_then(|i| entities.get(i))
            .filter(|p| parser.class_name(p.class_id) == Some(paths.pawn_class.as_str()))
        else {
            continue;
        };
        let Some(position) = position(pawn, paths) else {
            continue;
        };
        // A field the server never sent still holds its C++ default, which is 0 for a vector.
        let view_offset = paths
            .view_offset
            .each_ref()
            .map(|p| pawn.get(p).and_then(FieldValue::as_f32).unwrap_or(0.0));
        let angles = pawn
            .get(&paths.eye_angles)
            .and_then(FieldValue::as_vector3)
            .unwrap_or_default();
        players.push(PlayerSample {
            controller: controller.index,
            name: controller
                .get(&paths.name)
                .and_then(FieldValue::as_str)
                .unwrap_or_default()
                .to_owned(),
            steam_id: controller
                .get(&paths.steam_id)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0),
            team: controller
                .get(&paths.team)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0) as u32,
            position,
            view_offset,
            pitch: angles[0],
            yaw: angles[1],
            alive: pawn
                .get(&paths.life_state)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0)
                == 0,
        });
    }
    players
}

/// `cell × 512 − 16384 + offset` for each axis.
fn position(pawn: &Entity, paths: &Paths) -> Option<[f32; 3]> {
    let mut out = [0.0; 3];
    for ((value, cell), offset) in out.iter_mut().zip(&paths.cell).zip(&paths.offset) {
        let cell = pawn.get(cell)?.as_u64()? as f32;
        let offset = pawn.get(offset)?.as_f32()?;
        *value = cell * CELL_WIDTH - MAX_COORD + offset;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    //! End-to-end test on a tiny demo built byte by byte from the format description in `docs/formats/demo.md`:
    //! file header → server info + string table → send tables → class info → entity packets. It runs in CI, where
    //! no real demo is available.

    use prost::Message;

    use super::*;
    use crate::demo::bits::BitWriter;
    use crate::demo::fieldpath::write_paths;
    use crate::demo::frame::testing::{DemoBuilder, push_varint};
    use crate::demo::proto::{
        CDemoClassInfo, CDemoFileHeader, CDemoPacket, CDemoSendTables, CsvcMsgCreateStringTable,
        CsvcMsgPacketEntities, CsvcMsgServerInfo, CsvcMsgUpdateStringTable, EDemoCommands,
        SvcMessages, c_demo_class_info,
    };
    use crate::demo::serializer::testing::{F, message};

    // Class ids (3 classes → 2 bits) and entity indices used by the fixture.
    const RULES_CLASS: u32 = 0;
    const CONTROLLER_CLASS: u32 = 1;
    const PAWN_CLASS: u32 = 2;
    const RULES: u32 = 3;
    const CONTROLLER: u32 = 1;
    const PAWN: u32 = 10;

    fn send_tables() -> Vec<u8> {
        let f = |name, ty| F {
            name,
            ty,
            ..Default::default()
        };
        let noscale = |name| F {
            bits: Some(32),
            ..f(name, "CNetworkedQuantizedFloat")
        };
        // The view offset as CS2 sends it (`demo_inspect … fields CCSPlayerPawn`): x and y 10 bits in −64..64 with
        // the encode-zero flag, z 12 bits up to 64 with the round-up flag and no low value — which the decoder turns
        // into 12 plain bits, value = (raw + 1) / 64.
        let view_offset = |name, bits, low, flags| F {
            bits: Some(bits),
            low,
            high: Some(64.0),
            flags: Some(flags),
            send_node: Some("m_vecViewOffset"),
            ..f(name, "CNetworkedQuantizedFloat")
        };
        let msg = message(&[
            (
                "CBodyComponentBaseAnimGraph",
                vec![
                    f("m_cellX", "uint16"),
                    f("m_cellY", "uint16"),
                    f("m_cellZ", "uint16"),
                    noscale("m_vecX"),
                    noscale("m_vecY"),
                    noscale("m_vecZ"),
                ],
            ),
            (
                "CCSPlayerPawn",
                vec![
                    F {
                        serializer: Some("CBodyComponentBaseAnimGraph"),
                        ..f("CBodyComponent", "CBodyComponent")
                    },
                    F {
                        encoder: Some("qangle_precise"),
                        bits: Some(0),
                        ..f("m_angEyeAngles", "QAngle")
                    },
                    f("m_lifeState", "uint8"),
                    view_offset("m_vecX", 10, Some(-64.0), 4),
                    view_offset("m_vecY", 10, Some(-64.0), 4),
                    view_offset("m_vecZ", 12, None, 2),
                ],
            ),
            (
                "CCSPlayerController",
                vec![
                    f("m_iTeamNum", "uint8"),
                    f("m_iszPlayerName", "char[128]"),
                    F {
                        encoder: Some("fixed64"),
                        ..f("m_steamID", "uint64")
                    },
                    f("m_hPlayerPawn", "CHandle< CCSPlayerPawn >"),
                ],
            ),
            (
                "CCSGameRules",
                vec![
                    f("m_bFreezePeriod", "bool"),
                    f("m_bWarmupPeriod", "bool"),
                    f("m_gamePhase", "int32"),
                    f("m_totalRoundsPlayed", "int32"),
                ],
            ),
            (
                "CCSGameRulesProxy",
                vec![F {
                    serializer: Some("CCSGameRules"),
                    ..f("m_pGameRules", "CCSGameRules*")
                }],
            ),
        ]);
        let body = msg.encode_to_vec();
        let mut data = Vec::new();
        push_varint(&mut data, body.len() as u64);
        data.extend_from_slice(&body);
        CDemoSendTables { data: Some(data) }.encode_to_vec()
    }

    fn class_info() -> Vec<u8> {
        let class = |id: u32, name: &str| c_demo_class_info::ClassT {
            class_id: Some(id as i32),
            network_name: Some(name.into()),
            table_name: None,
        };
        CDemoClassInfo {
            classes: vec![
                class(RULES_CLASS, "CCSGameRulesProxy"),
                class(CONTROLLER_CLASS, "CCSPlayerController"),
                class(PAWN_CLASS, "CCSPlayerPawn"),
            ],
        }
        .encode_to_vec()
    }

    /// A packet frame payload holding the given (message type, protobuf) pairs.
    fn packet(messages: &[(SvcMessages, Vec<u8>)]) -> Vec<u8> {
        let mut w = BitWriter::default();
        for (kind, body) in messages {
            w.ubitvar(*kind as u32)
                .varint(body.len() as u64)
                .bytes(body);
        }
        CDemoPacket {
            data: Some(w.finish()),
        }
        .encode_to_vec()
    }

    fn entities(updated: i32, delta: bool, w: &BitWriter) -> (SvcMessages, Vec<u8>) {
        let msg = CsvcMsgPacketEntities {
            updated_entries: Some(updated),
            legacy_is_delta: Some(delta),
            entity_data: Some(w.finish()),
            ..Default::default()
        };
        (SvcMessages::SvcPacketEntities, msg.encode_to_vec())
    }

    /// Writes "update entity `index`" (`previous` = index of the entity written before, or -1).
    fn update_header(w: &mut BitWriter, previous: i64, index: u32) {
        w.ubitvar((i64::from(index) - previous - 1) as u32)
            .bits(0b00, 2);
    }

    /// Writes "create entity `index` of class `class`".
    fn create_header(w: &mut BitWriter, previous: i64, index: u32, class: u32) {
        w.ubitvar((i64::from(index) - previous - 1) as u32)
            .bits(0b10, 2)
            .bits(u64::from(class), 2)
            .bits(1, 17)
            .varint(0);
    }

    /// Game rules values: pointer present, freeze, no warmup, phase 2 (first half), rounds played.
    fn rules_fields(w: &mut BitWriter, freeze: bool, rounds: i64) {
        write_paths(w, &[&[0], &[0, 0], &[0, 1], &[0, 2], &[0, 3]]);
        w.bool(true)
            .bool(freeze)
            .bool(false)
            .varint_signed(2)
            .varint_signed(rounds);
    }

    /// Pawn position (cell 32 on each axis, so world position = offset), eye angles (20-bit precise angles) and the
    /// view offset's z (12 bits, value = (raw + 1) / 64). The view offset's x and y are never sent, so they keep
    /// their default 0 (CS2's are 0 too).
    fn pawn_fields(
        w: &mut BitWriter,
        with_pointer: bool,
        pos: [f32; 3],
        pitch_raw: u64,
        yaw_raw: u64,
        eye_raw: u64,
    ) {
        let mut paths: Vec<&[i32]> = Vec::new();
        if with_pointer {
            paths.extend([&[0][..], &[0, 0], &[0, 1], &[0, 2]]);
        }
        paths.extend([&[0, 3][..], &[0, 4], &[0, 5], &[1], &[5]]);
        write_paths(w, &paths);
        if with_pointer {
            // Present, then cells: 32 × 512 − 16384 = 0.
            w.bool(true).varint(32).varint(32).varint(32);
        }
        for v in pos {
            w.bits(u64::from(v.to_bits()), 32);
        }
        w.bool(true)
            .bool(true)
            .bool(false)
            .bits(pitch_raw, 20)
            .bits(yaw_raw, 20)
            .bits(eye_raw, 12);
    }

    fn synthetic_demo() -> Vec<u8> {
        let mut demo = DemoBuilder::new();
        let header = CDemoFileHeader {
            demo_file_stamp: "PBDEMS2\0".into(),
            map_name: Some("de_synthetic".into()),
            ..Default::default()
        };
        demo.frame(
            EDemoCommands::DemFileHeader,
            None,
            &header.encode_to_vec(),
            false,
        );

        let server_info = CsvcMsgServerInfo {
            max_classes: Some(3),
            tick_interval: Some(1.0 / 64.0),
            ..Default::default()
        };
        let baselines = CsvcMsgCreateStringTable {
            name: Some("instancebaseline".into()),
            num_entries: Some(0),
            flags: Some(1),
            using_varint_bitcounts: Some(true),
            string_data: Some(Vec::new()),
            ..Default::default()
        };
        let signon = packet(&[
            (SvcMessages::SvcServerInfo, server_info.encode_to_vec()),
            (SvcMessages::SvcCreateStringTable, baselines.encode_to_vec()),
        ]);
        demo.frame(EDemoCommands::DemSignonPacket, None, &signon, false);
        demo.frame(EDemoCommands::DemSendTables, None, &send_tables(), true);
        demo.frame(EDemoCommands::DemClassInfo, None, &class_info(), false);

        // Tick 10: full snapshot. Controller ("synk", Steam id, pawn handle), rules (no freeze), pawn.
        let mut w = BitWriter::default();
        create_header(&mut w, -1, CONTROLLER, CONTROLLER_CLASS);
        write_paths(&mut w, &[&[1], &[2], &[3]]);
        w.bytes(b"synk\0")
            .bits(76561197960287930, 64)
            .varint((7 << 14) | u64::from(PAWN));
        create_header(&mut w, i64::from(CONTROLLER), RULES, RULES_CLASS);
        rules_fields(&mut w, false, 0);
        create_header(&mut w, i64::from(RULES), PAWN, PAWN_CLASS);
        // Standing: eye height (4093 + 1) / 64 = 63.96875.
        pawn_fields(&mut w, true, [100.0, 200.0, -50.0], 1 << 19, 1 << 19, 4093);
        // The controller's team (3) only comes from its class baseline, which this same packet adds to the
        // `instancebaseline` table *after* the entity message: string tables must be applied first.
        let mut baseline = BitWriter::default();
        write_paths(&mut baseline, &[&[0]]);
        baseline.varint(3);
        let baseline = baseline.finish();
        let mut entry = BitWriter::default();
        entry
            .bool(true)
            .bool(true)
            .bool(false)
            .bytes(format!("{CONTROLLER_CLASS}\0").as_bytes());
        entry
            .bool(true)
            .bool(false)
            .ubitvar(baseline.len() as u32)
            .bytes(&baseline);
        let update = CsvcMsgUpdateStringTable {
            table_id: Some(0),
            num_changed_entries: Some(1),
            string_data: Some(entry.finish()),
        };
        let messages = [
            entities(3, false, &w),
            (SvcMessages::SvcUpdateStringTable, update.encode_to_vec()),
        ];
        demo.frame(
            EDemoCommands::DemPacket,
            Some(10),
            &packet(&messages),
            false,
        );

        // Tick 11: freeze time starts → round 1 starts. Ticks 12-13: the pawn moves; tick 13 also ends freeze time.
        let mut w = BitWriter::default();
        update_header(&mut w, -1, RULES);
        rules_fields(&mut w, true, 0);
        demo.frame(
            EDemoCommands::DemPacket,
            Some(11),
            &packet(&[entities(1, true, &w)]),
            false,
        );
        for (tick, x) in [(12, 104.0), (13, 108.0)] {
            let mut w = BitWriter::default();
            let mut count = 1;
            let mut previous = -1;
            if tick == 13 {
                update_header(&mut w, -1, RULES);
                rules_fields(&mut w, false, 0);
                previous = i64::from(RULES);
                count = 2;
            }
            update_header(&mut w, previous, PAWN);
            // pitch: 3/8 turn − 180° = −45°; yaw: 5/8 turn − 180° = 45°; crouched: (2941 + 1) / 64 = 45.96875.
            pawn_fields(&mut w, false, [x, 200.0, -50.0], 3 << 17, 5 << 17, 2941);
            demo.frame(
                EDemoCommands::DemPacket,
                Some(tick),
                &packet(&[entities(count, true, &w)]),
                false,
            );
        }

        // Tick 14: round over (1 round played) and the next freeze time starts → round 2.
        let mut w = BitWriter::default();
        update_header(&mut w, -1, RULES);
        rules_fields(&mut w, true, 1);
        demo.frame(
            EDemoCommands::DemPacket,
            Some(14),
            &packet(&[entities(1, true, &w)]),
            false,
        );
        demo.frame(EDemoCommands::DemStop, Some(15), &[], false);
        demo.bytes
    }

    #[test]
    fn tracks_players_through_a_synthetic_demo() {
        let data = synthetic_demo();
        let track = track_round(&data, 1).unwrap();
        assert_eq!(track.map_name, "de_synthetic");
        assert_eq!((track.start_tick, track.end_tick), (11, 13));
        assert_eq!(track.tick_interval, 1.0 / 64.0);
        let ticks: Vec<u32> = track.samples.iter().map(|s| s.tick).collect();
        assert_eq!(ticks, [11, 12, 13]);

        let first = &track.samples[0].players[0];
        assert_eq!(first.name, "synk");
        assert_eq!(first.steam_id, 76561197960287930);
        assert_eq!(first.team, 3);
        assert_eq!(first.controller, CONTROLLER);
        assert!(first.alive);
        assert_eq!(first.position, [100.0, 200.0, -50.0]);
        // x and y were never sent, so they keep their default 0. z is (raw + 1) / 64 in exact arithmetic; the
        // quantized float formula (Valve's, also used by demoparser2) can land one float step (~4e-6) off.
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5);
        assert!(close(first.view_offset, [0.0, 0.0, 63.96875]));
        assert!(close(first.eye_position(), [100.0, 200.0, 13.96875]));
        assert_eq!((first.pitch, first.yaw), (0.0, 0.0));

        let last = &track.samples[2].players[0];
        assert_eq!(last.position, [108.0, 200.0, -50.0]);
        assert!(close(last.view_offset, [0.0, 0.0, 45.96875]));
        assert!(close(last.eye_position(), [108.0, 200.0, -4.03125]));
        assert_eq!((last.pitch, last.yaw), (-45.0, 45.0));

        let rounds = list_rounds(&data).unwrap();
        let round = |round, start_tick, end_tick| RoundInfo {
            round,
            start_tick,
            end_tick,
        };
        assert_eq!(rounds, [round(1, 11, 13), round(2, 14, 14)]);
        assert!(matches!(
            track_round(&data, 3),
            Err(TrackError::RoundNotFound(3))
        ));
    }
}
