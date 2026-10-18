//! Reading a demo from start to end and keeping the game state up to date.
//!
//! [`DemoParser`] walks the frames (see `frame.rs`) and handles the ones that build up the game state:
//!
//! | Frame | What it gives us |
//! |---|---|
//! | `DEM_FileHeader` | map, server, build ([`DemoParser::header`]) |
//! | `DEM_SendTables` | the serializers (`serializer.rs`) |
//! | `DEM_ClassInfo` | class id → class name → serializer |
//! | `DEM_StringTables` | a snapshot of the string tables |
//! | `DEM_SignonPacket`, `DEM_Packet` | network messages: server info, string tables, entity updates |
//! | `DEM_FullPacket` | string-table snapshot + a packet with a full entity snapshot (only the first one is used) |
//!
//! Packets contain a sequence of network messages: `type` (ubitvar), `size` (varint), `size` bytes of protobuf,
//! all inside a bit stream (not byte-aligned). Within one packet, string-table and server-info messages are applied
//! before entity updates, because an entity created in a packet may need a baseline that arrives in that same
//! packet (demoinfocs-golang orders them the same way).

use std::collections::HashMap;

use prost::Message;

use super::bits::{BitReader, BitsEof};
use super::entity::{Entities, EntityContext, EntityError, ServerClass};
use super::events::{EventDescriptors, GameEvent};
use super::frame::{FrameError, Frames};
use super::proto::{
    CDemoClassInfo, CDemoFileHeader, CDemoFullPacket, CDemoPacket, CDemoSendTables,
    CDemoStringTables, CMsgSource1LegacyGameEvent, CMsgSource1LegacyGameEventList, CnetMsgTick,
    CsvcMsgCreateStringTable, CsvcMsgPacketEntities, CsvcMsgServerInfo, CsvcMsgUpdateStringTable,
    EBaseGameEvents, EDemoCommands, NetMessages, SvcMessages,
};
use super::serializer::{SerializerError, Serializers};
use super::stringtable::{StringTableError, StringTables};

/// Name of the string table that holds entity baselines.
const BASELINE_TABLE: &str = "instancebaseline";

/// Upper bound for class ids (CS2 has ~220 classes); protects the class list from absurd ids in corrupt files.
const MAX_CLASSES: usize = 1 << 16;

/// Warnings kept by [`DemoParser::warnings`] (later ones are only counted).
const MAX_WARNINGS: usize = 100;

/// Any error while reading a demo.
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    /// Frame layer (magic, truncation, decompression).
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// A protobuf payload did not decode.
    #[error("{what}: {source}")]
    Protobuf {
        /// Which message.
        what: &'static str,
        /// The decoder's error.
        source: prost::DecodeError,
    },
    /// Packet bit stream ended early.
    #[error("packet: {0}")]
    Bits(#[from] BitsEof),
    /// Send tables could not be turned into serializers.
    #[error(transparent)]
    Serializer(#[from] SerializerError),
    /// A string table could not be read.
    #[error(transparent)]
    StringTable(#[from] StringTableError),
    /// Entity updates could not be applied.
    #[error(transparent)]
    Entity(#[from] EntityError),
    /// Entity data arrived before the send tables and class list it needs.
    #[error("entity data before send tables / class info")]
    MissingSendTables,
    /// A class in the class list has no serializer.
    #[error("class {0:?} has no serializer")]
    MissingSerializer(String),
    /// A class id is negative or implausibly large.
    #[error("class id {0} out of range")]
    BadClassId(i32),
    /// The first frame is not a file header.
    #[error("demo does not start with a file header")]
    MissingHeader,
    /// Send tables or class info arrived again after entities exist. Entities refer to classes by number, so
    /// changing the classes under them is not supported (never seen in CS2 demos).
    #[error("send tables / class info changed after entities were created")]
    ClassesChanged,
}

fn decode<M: Message + Default>(what: &'static str, bytes: &[u8]) -> Result<M, DemoError> {
    M::decode(bytes).map_err(|source| DemoError::Protobuf { what, source })
}

/// What [`DemoParser::next_frame`] just processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// The frame's tick (`None` before the game starts).
    pub tick: Option<u32>,
    /// The frame's command (without the compression flag).
    pub raw_command: u32,
    /// Entity state changed in this frame.
    pub entities_changed: bool,
}

/// Reads a demo frame by frame and maintains the game state (entities, string tables, classes).
#[derive(Debug)]
pub struct DemoParser<'a> {
    frames: Frames<'a>,
    header: CDemoFileHeader,
    tick_interval: Option<f32>,
    max_classes: Option<u32>,
    class_info: Option<CDemoClassInfo>,
    classes: Vec<Option<ServerClass>>,
    serializers: Option<Serializers>,
    string_tables: StringTables,
    baselines: HashMap<u32, Vec<u8>>,
    entities: Entities,
    tick: Option<u32>,
    /// The server's game tick from the latest `CNETMsg_Tick`.
    server_tick: Option<u32>,
    event_descriptors: EventDescriptors,
    /// Game events read since the last [`DemoParser::take_events`].
    events: Vec<GameEvent>,
    warnings: Vec<String>,
    warning_count: usize,
}

impl<'a> DemoParser<'a> {
    /// Starts reading a demo held in memory. Reads the file header right away.
    pub fn new(data: &'a [u8]) -> Result<Self, DemoError> {
        let mut frames = Frames::new(data)?;
        let first = frames.next().ok_or(DemoError::MissingHeader)??;
        if first.command() != Some(EDemoCommands::DemFileHeader) {
            return Err(DemoError::MissingHeader);
        }
        let header = decode("CDemoFileHeader", &first.payload)?;
        Ok(Self {
            frames,
            header,
            tick_interval: None,
            max_classes: None,
            class_info: None,
            classes: Vec::new(),
            serializers: None,
            string_tables: StringTables::default(),
            baselines: HashMap::new(),
            entities: Entities::default(),
            tick: None,
            server_tick: None,
            event_descriptors: EventDescriptors::default(),
            events: Vec::new(),
            warnings: Vec::new(),
            warning_count: 0,
        })
    }

    /// Problems that did not stop the parse, e.g. a string table OFAST does not use that could not be read.
    /// At most 100 are kept; [`DemoParser::warning_count`] counts all of them.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Number of warnings, including those not kept.
    pub fn warning_count(&self) -> usize {
        self.warning_count
    }

    fn warn(&mut self, message: String) {
        self.warning_count += 1;
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings
                .push(format!("tick {:?}: {message}", self.tick));
        }
    }

    /// String table problems are fatal only for the table entity decoding depends on (baselines); other tables
    /// are not used yet, so a problem there becomes a warning (demoinfocs-golang also reads only a few tables).
    fn string_table_problem(
        &mut self,
        table: Option<&str>,
        error: StringTableError,
    ) -> Result<(), DemoError> {
        if table == Some(BASELINE_TABLE) {
            return Err(error.into());
        }
        self.warn(error.to_string());
        Ok(())
    }

    /// The demo's file header: map, server name, build number…
    pub fn header(&self) -> &CDemoFileHeader {
        &self.header
    }

    /// Seconds per tick (1/64 for CS2), once the server info has been read.
    pub fn tick_interval(&self) -> Option<f32> {
        self.tick_interval
    }

    /// Tick of the most recently processed frame.
    pub fn tick(&self) -> Option<u32> {
        self.tick
    }

    /// The server's game tick, from the latest `CNETMsg_Tick` (`None` before the first one). Game-time fields count in
    /// it (`GameTick_t`, such as a pawn's last jump tick); it runs a constant distance ahead of the demo's own
    /// [`tick`](Self::tick) (2403 ticks on the local match demo), so `demo tick = game tick − (server tick − tick)`.
    pub fn server_tick(&self) -> Option<u32> {
        self.server_tick
    }

    /// A game tick (`GameTick_t` field) as a demo tick, using the current distance between the two clocks. `None`
    /// before the first `CNETMsg_Tick`, or for a game tick before the demo started.
    pub fn game_tick_to_demo(&self, game_tick: u32) -> Option<u32> {
        let offset = i64::from(self.server_tick?) - i64::from(self.tick?);
        u32::try_from(i64::from(game_tick) - offset).ok()
    }

    /// The serializers, once the send tables have been read.
    pub fn serializers(&self) -> Option<&Serializers> {
        self.serializers.as_ref()
    }

    /// Server classes by class id, once the class info has been read.
    pub fn classes(&self) -> &[Option<ServerClass>] {
        &self.classes
    }

    /// Current entities.
    pub fn entities(&self) -> &Entities {
        &self.entities
    }

    /// Current string tables.
    pub fn string_tables(&self) -> &StringTables {
        &self.string_tables
    }

    /// The game events (deaths, round ends, …) read since the last call, oldest first. Events of a tick are read
    /// before that tick's entity updates. See [`super::events`].
    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }

    /// Class name of an entity.
    pub fn class_name(&self, class_id: u32) -> Option<&str> {
        self.classes
            .get(class_id as usize)
            .and_then(Option::as_ref)
            .map(|c| c.name.as_str())
    }

    /// Processes the next frame. Returns `None` at the end of the demo.
    pub fn next_frame(&mut self) -> Result<Option<FrameInfo>, DemoError> {
        let Some(frame) = self.frames.next() else {
            return Ok(None);
        };
        let frame = frame?;
        self.tick = frame.tick;
        let mut entities_changed = false;
        match frame.command() {
            Some(EDemoCommands::DemSendTables) => {
                if self.entities.iter().next().is_some() {
                    return Err(DemoError::ClassesChanged);
                }
                let msg: CDemoSendTables = decode("CDemoSendTables", &frame.payload)?;
                self.serializers = Some(Serializers::from_send_tables(msg.data())?);
                self.link_classes()?;
            }
            Some(EDemoCommands::DemClassInfo) => {
                if self.entities.iter().next().is_some() {
                    return Err(DemoError::ClassesChanged);
                }
                self.class_info = Some(decode("CDemoClassInfo", &frame.payload)?);
                self.link_classes()?;
            }
            Some(EDemoCommands::DemStringTables) => {
                let msg: CDemoStringTables = decode("CDemoStringTables", &frame.payload)?;
                self.string_tables.apply_snapshot(&msg);
                self.refresh_baselines();
            }
            Some(EDemoCommands::DemPacket | EDemoCommands::DemSignonPacket) => {
                let msg: CDemoPacket = decode("CDemoPacket", &frame.payload)?;
                entities_changed = self.read_packet(msg.data(), false)?;
            }
            Some(EDemoCommands::DemFullPacket) => {
                let msg: CDemoFullPacket = decode("CDemoFullPacket", &frame.payload)?;
                if let Some(tables) = &msg.string_table {
                    self.string_tables.apply_snapshot(tables);
                    self.refresh_baselines();
                }
                if let Some(packet) = &msg.packet {
                    entities_changed = self.read_packet(packet.data(), true)?;
                }
            }
            // Everything else (console commands, user commands, animation data, file info, …) is not needed yet.
            _ => {}
        }
        Ok(Some(FrameInfo {
            tick: frame.tick,
            raw_command: frame.raw_command,
            entities_changed,
        }))
    }

    /// Connects the class list with the serializers once both are known.
    fn link_classes(&mut self) -> Result<(), DemoError> {
        let (Some(info), Some(serializers)) = (&self.class_info, &self.serializers) else {
            return Ok(());
        };
        self.classes.clear();
        for c in &info.classes {
            let id = usize::try_from(c.class_id())
                .ok()
                .filter(|&id| id < MAX_CLASSES)
                .ok_or(DemoError::BadClassId(c.class_id()))?;
            let name = c.network_name().to_owned();
            let serializer = serializers
                .by_name(&name)
                .ok_or_else(|| DemoError::MissingSerializer(name.clone()))?;
            if self.classes.len() <= id {
                self.classes.resize(id + 1, None);
            }
            self.classes[id] = Some(ServerClass {
                id: id as u32,
                name,
                serializer,
            });
        }
        Ok(())
    }

    /// Rebuilds the class-id → baseline map from the `instancebaseline` string table.
    fn refresh_baselines(&mut self) {
        self.baselines.clear();
        if let Some(table) = self.string_tables.by_name(BASELINE_TABLE) {
            for entry in &table.entries {
                // Keys are class ids in decimal. (Keys like "12:3" belong to alternate baselines, not used here.)
                if let (Ok(class_id), Some(value)) = (entry.key.parse::<u32>(), &entry.value) {
                    self.baselines.insert(class_id, value.clone());
                }
            }
        }
    }

    /// Reads the network messages of one packet. Returns whether entities changed.
    fn read_packet(&mut self, data: &[u8], full_packet: bool) -> Result<bool, DemoError> {
        let mut r = BitReader::new(data);
        let mut messages = Vec::new();
        // Fewer than 8 bits left = padding at the end of the last byte.
        while r.remaining_bits() > 7 {
            let kind = r.ubitvar("message type")?;
            let size = r.varint_u32("message size")? as usize;
            messages.push((kind, r.bytes(size, "message body")?));
        }

        // Pass 1: everything except entities (string tables must be up to date before entities are created).
        for (kind, body) in &messages {
            let kind = *kind as i32;
            if kind == SvcMessages::SvcServerInfo as i32 {
                let msg: CsvcMsgServerInfo = decode("CSVCMsg_ServerInfo", body)?;
                self.tick_interval = msg.tick_interval;
                self.max_classes = msg.max_classes.map(|m| m.max(0) as u32);
            } else if kind == SvcMessages::SvcCreateStringTable as i32 {
                let msg: CsvcMsgCreateStringTable = decode("CSVCMsg_CreateStringTable", body)?;
                let (_, entries) = self.string_tables.create(&msg)?;
                if let Err(error) = entries {
                    self.string_table_problem(msg.name.as_deref(), error)?;
                }
                if msg.name() == BASELINE_TABLE {
                    self.refresh_baselines();
                }
            } else if kind == SvcMessages::SvcUpdateStringTable as i32 {
                let msg: CsvcMsgUpdateStringTable = decode("CSVCMsg_UpdateStringTable", body)?;
                let count = msg.num_changed_entries().max(0) as usize;
                let name = usize::try_from(msg.table_id())
                    .ok()
                    .and_then(|id| self.string_tables.get(id))
                    .map(|t| t.name.clone());
                if let Err(error) =
                    self.string_tables
                        .update(msg.table_id(), msg.string_data(), count)
                {
                    self.string_table_problem(name.as_deref(), error)?;
                }
                if name.as_deref() == Some(BASELINE_TABLE) {
                    self.refresh_baselines();
                }
            } else if kind == SvcMessages::SvcClearAllStringTables as i32 {
                self.string_tables.clear();
                self.baselines.clear();
            } else if kind == NetMessages::NetTick as i32 {
                // The server tick (frame tick + a constant offset); the frame tick is what we report, the server
                // tick converts game-tick fields (`game_tick_to_demo`).
                let msg: CnetMsgTick = decode("CNETMsg_Tick", body)?;
                if let Some(tick) = msg.tick {
                    self.server_tick = Some(tick);
                }
            } else if kind == EBaseGameEvents::GeSource1LegacyGameEventList as i32 {
                let list: CMsgSource1LegacyGameEventList =
                    decode("CMsgSource1LegacyGameEventList", body)?;
                self.event_descriptors.load(&list);
            } else if kind == EBaseGameEvents::GeSource1LegacyGameEvent as i32 && !full_packet {
                // Full packets are snapshots for seeking; events happen in ordinary packets only.
                let event: CMsgSource1LegacyGameEvent = decode("CMsgSource1LegacyGameEvent", body)?;
                match self
                    .event_descriptors
                    .decode(self.tick.unwrap_or(0), &event)
                {
                    Some(decoded) => self.events.push(decoded),
                    None => self.warn(format!(
                        "game event id {} has no descriptor",
                        event.eventid()
                    )),
                }
            }
        }

        // Pass 2: entities.
        let mut changed = false;
        for (kind, body) in &messages {
            if *kind as i32 == SvcMessages::SvcPacketEntities as i32 {
                let msg: CsvcMsgPacketEntities = decode("CSVCMsg_PacketEntities", body)?;
                let serializers = self
                    .serializers
                    .as_ref()
                    .ok_or(DemoError::MissingSendTables)?;
                if self.classes.is_empty() {
                    return Err(DemoError::MissingSendTables);
                }
                // A class id is written with as many bits as the class count needs (218 classes → 8 bits).
                let class_count = self.max_classes.unwrap_or(self.classes.len() as u32);
                let ctx = EntityContext {
                    serializers,
                    classes: &self.classes,
                    class_id_bits: u32::BITS - class_count.leading_zeros(),
                    baselines: &self.baselines,
                };
                changed |= self.entities.apply(&msg, ctx, full_packet)?;
            }
        }
        Ok(changed)
    }
}

/// Reads only the file header of a demo (fast: the first frame).
pub fn read_header(data: &[u8]) -> Result<CDemoFileHeader, DemoError> {
    Ok(DemoParser::new(data)?.header)
}
