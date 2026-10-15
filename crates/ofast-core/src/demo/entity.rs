//! Entities: the live game objects (players, weapons, grenades, the game rules…) and their networked fields.
//!
//! `svc_PacketEntities` messages carry entity changes as one bit stream. For each changed entity:
//!
//! ```text
//! index delta (ubitvar)  → index = previous index + delta + 1   (previous starts at -1)
//! command (2 bits)       → bit 0: leaves the update set, bit 1 (with bit 0): deleted / (without): created
//! create: class id (N bits), serial (17 bits), one unused varint, then field changes
//! update: field changes
//! ```
//!
//! "Field changes" = a list of [`FieldPath`]s (see `fieldpath.rs`) followed by one value per path, each decoded
//! with the decoder its path resolves to (see `serializer.rs`). A new entity first gets its class **baseline** —
//! the default values from the `instancebaseline` string table, stored in the same field-change format — and then
//! the changes from the packet.
//!
//! Knowledge source: demoinfocs-golang `sendtables/entity.go` and demoparser2 `second_pass/entities.rs` (MIT).
//! See `docs/formats/demo.md`.

use std::collections::HashMap;

use super::bits::{BitReader, BitsEof};
use super::decoder::FieldValue;
use super::fieldpath::{FieldPath, FieldPathError, read_field_paths};
use super::proto::CsvcMsgPacketEntities;
use super::serializer::{ResolveError, SerializerId, Serializers};

/// Bits of an entity index inside an entity handle (`CHandle`, `CEntityHandle`): handles are `serial << 14 | index`.
pub const HANDLE_INDEX_BITS: u32 = 14;
/// Mask that extracts the entity index from a handle.
pub const HANDLE_INDEX_MASK: u64 = (1 << HANDLE_INDEX_BITS) - 1;
/// Handle value meaning "no entity".
pub const INVALID_HANDLE: u64 = 0xFF_FFFF;

/// Bits of the serial number sent when an entity is created.
const CREATE_SERIAL_BITS: u32 = 17;

/// Most fields one entity may hold. The largest CS2 entity has about 3,200; the limit (like the next one) stops a
/// corrupt or hostile demo from using unbounded memory.
pub const MAX_FIELDS_PER_ENTITY: usize = 1 << 16;
/// Most fields all entities together may hold. A real match peaks around 56,000 (about 4 MB).
pub const MAX_TOTAL_FIELDS: usize = 1 << 22;

/// Returns the entity index a handle points to, or `None` for the invalid handle.
pub fn handle_to_index(handle: u64) -> Option<u32> {
    (handle != INVALID_HANDLE && handle != u64::from(u32::MAX))
        .then_some((handle & HANDLE_INDEX_MASK) as u32)
}

/// Errors while applying entity updates. All of them mean OFAST and the demo disagree about the format.
#[derive(Debug, thiserror::Error)]
pub enum EntityError {
    /// Ran out of bits.
    #[error("entity data: {0}")]
    Eof(#[from] BitsEof),
    /// Field path list was invalid.
    #[error("entity {index}: {source}")]
    FieldPath {
        /// Entity index.
        index: u32,
        /// The problem.
        source: FieldPathError,
    },
    /// A field path did not match the entity's serializer.
    #[error("entity {index} ({class}): {source}")]
    Resolve {
        /// Entity index.
        index: u32,
        /// Class name.
        class: String,
        /// The problem.
        source: ResolveError,
    },
    /// A created entity's class id is unknown, or the class has no serializer.
    #[error("entity {index}: unknown class id {class_id}")]
    UnknownClass {
        /// Entity index.
        index: u32,
        /// The class id that was read.
        class_id: u32,
    },
    /// An update targeted an entity that does not exist.
    #[error("update for entity {0}, which does not exist")]
    MissingEntity(u32),
    /// The index went out of the valid range.
    #[error("entity index {0} out of range")]
    BadIndex(i64),
    /// An entity, or all entities together, would hold more fields than any real demo needs.
    #[error(
        "entity {0}: too many fields (limits: {MAX_FIELDS_PER_ENTITY} per entity, {MAX_TOTAL_FIELDS} in total)"
    )]
    TooManyFields(u32),
}

/// One networked class, from the `DEM_ClassInfo` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerClass {
    /// Class id used in entity creation.
    pub id: u32,
    /// Network name, e.g. `CCSPlayerPawn` (also the serializer name).
    pub name: String,
    /// The class's serializer.
    pub serializer: SerializerId,
}

/// Everything entity decoding needs besides the entity list itself.
#[derive(Debug, Clone, Copy)]
pub struct EntityContext<'a> {
    /// Serializers from the send tables.
    pub serializers: &'a Serializers,
    /// Server classes by class id.
    pub classes: &'a [Option<ServerClass>],
    /// Bits of a class id in entity creation: the bit length of the class count.
    pub class_id_bits: u32,
    /// Baseline field data per class id (from the `instancebaseline` string table).
    pub baselines: &'a HashMap<u32, Vec<u8>>,
}

/// One entity and its current field values.
#[derive(Debug, Clone)]
pub struct Entity {
    /// Entity index (slot).
    pub index: u32,
    /// Server class id.
    pub class_id: u32,
    /// Serial number from the creation message (distinguishes reuses of the same slot).
    pub serial: u32,
    /// The class's serializer.
    pub serializer: SerializerId,
    /// Field values by path. Variable-length arrays store their length at the array's own path; when an array
    /// shrinks, its elements past the new length are removed.
    fields: HashMap<FieldPath, FieldValue>,
}

impl Entity {
    /// The current value at a field path.
    pub fn get(&self, path: &FieldPath) -> Option<&FieldValue> {
        self.fields.get(path)
    }

    /// All current field values (in no particular order).
    pub fn fields(&self) -> impl Iterator<Item = (&FieldPath, &FieldValue)> {
        self.fields.iter()
    }

    /// Reads one list of field changes (paths, then values) and applies it.
    fn read_fields(
        &mut self,
        r: &mut BitReader<'_>,
        serializers: &Serializers,
        paths: &mut Vec<FieldPath>,
    ) -> Result<(), EntityError> {
        read_field_paths(r, paths).map_err(|source| EntityError::FieldPath {
            index: self.index,
            source,
        })?;
        for path in paths.iter() {
            let resolved = serializers
                .resolve(self.serializer, path, |prefix| {
                    match self.fields.get(prefix) {
                        Some(FieldValue::Pointer(Some(i))) => Some(u64::from(*i)),
                        Some(FieldValue::Unsigned(n)) => Some(*n),
                        _ => None,
                    }
                })
                .map_err(|source| EntityError::Resolve {
                    index: self.index,
                    class: serializers
                        .serializers
                        .get(self.serializer)
                        .map_or_else(String::new, |s| s.name.clone()),
                    source,
                })?;
            let value = resolved.decoder.read(r)?;
            if resolved.is_length {
                self.shrink_array(path, &value);
            }
            self.fields.insert(*path, value);
            if self.fields.len() > MAX_FIELDS_PER_ENTITY {
                return Err(EntityError::TooManyFields(self.index));
            }
        }
        Ok(())
    }

    /// When a variable-length array at `array` gets a shorter `new_length`, removes its elements past the end.
    /// Otherwise a later regrowth would show stale values: Valve does not resend elements equal to their default.
    fn shrink_array(&mut self, array: &FieldPath, new_length: &FieldValue) {
        let (Some(old), Some(new)) = (
            self.fields.get(array).and_then(FieldValue::as_u64),
            new_length.as_u64(),
        ) else {
            return;
        };
        if new >= old {
            return;
        }
        let prefix = array.indices();
        let depth = prefix.len();
        self.fields.retain(|path, _| {
            let p = path.indices();
            !(p.len() > depth
                && &p[..depth] == prefix
                && u64::try_from(p[depth]).is_ok_and(|i| i >= new))
        });
    }
}

/// All entities of the game, by index.
#[derive(Debug, Clone, Default)]
pub struct Entities {
    slots: Vec<Option<Entity>>,
    /// Scratch buffer for field paths, reused between updates.
    paths: Vec<FieldPath>,
    /// Whether a full entity snapshot (from a `DEM_FullPacket`) has been applied yet.
    have_full_snapshot: bool,
    /// Number of fields over all entities (bounded by [`MAX_TOTAL_FIELDS`]).
    total_fields: usize,
}

/// Highest entity index + 1 (indices have 14 bits).
const MAX_ENTITIES: i64 = 1 << HANDLE_INDEX_BITS;

impl Entities {
    /// The entity at `index`, if it exists.
    pub fn get(&self, index: u32) -> Option<&Entity> {
        self.slots.get(index as usize).and_then(Option::as_ref)
    }

    /// All existing entities.
    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.slots.iter().flatten()
    }

    /// Applies one `svc_PacketEntities` message. Returns `false` if the message was skipped.
    ///
    /// `from_full_packet` says the message came from a `DEM_FullPacket` frame: a complete snapshot that lets a player
    /// seek. When reading a demo from the start only the first one is needed (it is the only complete snapshot, and
    /// the deltas after it build on it); later ones restate what the deltas already built and are skipped, like
    /// demoinfocs-golang does. (The message's own `legacy_is_delta` flag says the same today, but its name suggests
    /// Valve may stop filling it, so the frame type decides.)
    pub fn apply(
        &mut self,
        msg: &CsvcMsgPacketEntities,
        ctx: EntityContext<'_>,
        from_full_packet: bool,
    ) -> Result<bool, EntityError> {
        if from_full_packet {
            if self.have_full_snapshot {
                return Ok(false);
            }
            self.have_full_snapshot = true;
        }

        let mut r = BitReader::new(msg.entity_data());
        let has_vis_bits = msg.has_pvs_vis_bits_deprecated() != 0;
        let mut index: i64 = -1;
        for _ in 0..msg.updated_entries() {
            index += i64::from(r.ubitvar("entity index delta")?) + 1;
            if !(0..MAX_ENTITIES).contains(&index) {
                return Err(EntityError::BadIndex(index));
            }
            let slot = index as usize;
            if self.slots.len() <= slot {
                self.slots.resize_with(slot + 1, || None);
            }
            let command = r.bits(2, "entity command")?;
            match command {
                // Update.
                0b00 => {
                    if has_vis_bits && r.bits(2, "entity visibility")? & 1 == 1 {
                        continue;
                    }
                    let entity = self.slots[slot]
                        .as_mut()
                        .ok_or(EntityError::MissingEntity(slot as u32))?;
                    let before = entity.fields.len();
                    entity.read_fields(&mut r, ctx.serializers, &mut self.paths)?;
                    let after = entity.fields.len();
                    self.count_fields(before, after, slot)?;
                }
                // Create (enter the update set): class, serial, baseline, then changes.
                0b10 => {
                    let class_id = r.bits(ctx.class_id_bits, "entity class id")?;
                    let serial = r.bits(CREATE_SERIAL_BITS, "entity serial")?;
                    r.varint_u32("entity create (unused)")?;
                    let class = ctx
                        .classes
                        .get(class_id as usize)
                        .and_then(Option::as_ref)
                        .ok_or(EntityError::UnknownClass {
                            index: slot as u32,
                            class_id,
                        })?;
                    let mut entity = Entity {
                        index: slot as u32,
                        class_id,
                        serial,
                        serializer: class.serializer,
                        fields: HashMap::new(),
                    };
                    if let Some(baseline) = ctx.baselines.get(&class_id) {
                        entity.read_fields(
                            &mut BitReader::new(baseline),
                            ctx.serializers,
                            &mut self.paths,
                        )?;
                    }
                    entity.read_fields(&mut r, ctx.serializers, &mut self.paths)?;
                    let replaced = self.slots[slot].as_ref().map_or(0, |old| old.fields.len());
                    let added = entity.fields.len();
                    self.slots[slot] = Some(entity);
                    self.count_fields(replaced, added, slot)?;
                }
                // Delete.
                0b11 => {
                    if let Some(old) = self.slots[slot].take() {
                        self.total_fields -= old.fields.len();
                    }
                }
                // 0b01: leaves the update set (out of view) but still exists. Demos recorded by SourceTV see
                // everything, so this is rare; the entity keeps its last state.
                _ => {}
            }
        }
        Ok(true)
    }

    /// Updates the total field count after an entity went from `before` to `after` fields.
    fn count_fields(
        &mut self,
        before: usize,
        after: usize,
        slot: usize,
    ) -> Result<(), EntityError> {
        self.total_fields = self.total_fields - before + after;
        if self.total_fields > MAX_TOTAL_FIELDS {
            return Err(EntityError::TooManyFields(slot as u32));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::bits::BitWriter;
    use crate::demo::fieldpath::{write_op, write_paths};
    use crate::demo::serializer::testing::{F, message};

    /// Field path operation indices used below (see `fieldpath::OPS`).
    const PLUS_ONE: u8 = 0;
    const PUSH_ONE_RIGHT_ZERO: u8 = 7; // [i] → [i+1, 0]
    const FINISH: u8 = 39;

    fn context() -> (Serializers, Vec<Option<ServerClass>>) {
        let simple = |name, ty| F {
            name,
            ty,
            ..Default::default()
        };
        let msg = message(&[
            ("CBody", vec![simple("m_cellX", "uint16")]),
            (
                "CThing",
                vec![
                    simple("m_iHealth", "int32"),
                    F {
                        serializer: Some("CBody"),
                        ..simple("CBodyComponent", "CBodyComponent")
                    },
                ],
            ),
        ]);
        let serializers = Serializers::from_message(&msg).unwrap();
        let class = ServerClass {
            id: 1,
            name: "CThing".into(),
            serializer: serializers.by_name("CThing").unwrap(),
        };
        (serializers, vec![None, Some(class)])
    }

    /// Field changes: m_iHealth (path [0]) = health, CBodyComponent.m_cellX (path [1, 0]) = cell. The pointer's own
    /// "present" path [1] is not sent: the baseline already set it.
    fn changes(w: &mut BitWriter, health: i64, cell: u64) {
        write_op(w, PLUS_ONE); // [-1] → [0]
        write_op(w, PUSH_ONE_RIGHT_ZERO); // [0] → [1] → push → [1, 0]
        write_op(w, FINISH);
        w.varint_signed(health).varint(cell);
    }

    #[test]
    fn create_update_delete_with_baseline() {
        let (serializers, classes) = context();

        // Baseline for class 1: CBodyComponent present ([1]), health 100 ([0]).
        let mut b = BitWriter::default();
        write_op(&mut b, PLUS_ONE); // [0]
        write_op(&mut b, PLUS_ONE); // [1]
        write_op(&mut b, FINISH);
        b.varint_signed(100).bool(true);
        let baselines = HashMap::from([(1u32, b.finish())]);
        let ctx = EntityContext {
            serializers: &serializers,
            classes: &classes,
            class_id_bits: 2,
            baselines: &baselines,
        };

        // Packet 1 (full snapshot): create entity 5 (delta 5 from -1 → ubitvar 5), class 1, serial 77.
        let mut w = BitWriter::default();
        w.ubitvar(5).bits(0b10, 2).bits(1, 2).bits(77, 17).varint(0);
        changes(&mut w, 90, 12);
        let msg = CsvcMsgPacketEntities {
            updated_entries: Some(1),
            legacy_is_delta: Some(false),
            entity_data: Some(w.finish()),
            ..Default::default()
        };
        let mut entities = Entities::default();
        assert!(entities.apply(&msg, ctx, true).unwrap());
        let e = entities.get(5).unwrap();
        assert_eq!(e.serial, 77);
        assert_eq!(e.get(&FieldPath::new(&[0])), Some(&FieldValue::Signed(90)));
        assert_eq!(
            e.get(&FieldPath::new(&[1])),
            Some(&FieldValue::Pointer(Some(0)))
        );
        assert_eq!(
            e.get(&FieldPath::new(&[1, 0])),
            Some(&FieldValue::Unsigned(12))
        );

        // Packet 2 (delta): update entity 5, then delete entity 7.
        let mut w = BitWriter::default();
        w.ubitvar(5).bits(0b00, 2);
        changes(&mut w, 50, 13);
        w.ubitvar(1).bits(0b11, 2);
        let msg = CsvcMsgPacketEntities {
            updated_entries: Some(2),
            legacy_is_delta: Some(true),
            entity_data: Some(w.finish()),
            ..Default::default()
        };
        assert!(entities.apply(&msg, ctx, false).unwrap());
        let e = entities.get(5).unwrap();
        assert_eq!(e.get(&FieldPath::new(&[0])), Some(&FieldValue::Signed(50)));
        assert_eq!(
            e.get(&FieldPath::new(&[1, 0])),
            Some(&FieldValue::Unsigned(13))
        );
        assert!(entities.get(7).is_none());

        // A second full snapshot is ignored entirely (its bits are not even read).
        let msg = CsvcMsgPacketEntities {
            updated_entries: Some(1),
            legacy_is_delta: Some(false),
            entity_data: Some(vec![]),
            ..Default::default()
        };
        assert!(!entities.apply(&msg, ctx, true).unwrap());

        // Updating a missing entity is an error.
        let mut w = BitWriter::default();
        w.ubitvar(0).bits(0b00, 2);
        let msg = CsvcMsgPacketEntities {
            updated_entries: Some(1),
            legacy_is_delta: Some(true),
            entity_data: Some(w.finish()),
            ..Default::default()
        };
        assert!(matches!(
            entities.apply(&msg, ctx, false),
            Err(EntityError::MissingEntity(0))
        ));
    }

    #[test]
    fn handles() {
        assert_eq!(handle_to_index((3 << 14) | 42), Some(42));
        assert_eq!(handle_to_index(INVALID_HANDLE), None);
    }

    #[test]
    fn arrays_shrink_and_reject_elements_past_the_end() {
        let msg = message(&[(
            "CList",
            vec![F {
                name: "m_list",
                ty: "CNetworkUtlVectorBase< uint32 >",
                ..Default::default()
            }],
        )]);
        let serializers = Serializers::from_message(&msg).unwrap();
        let classes = vec![Some(ServerClass {
            id: 0,
            name: "CList".into(),
            serializer: 0,
        })];
        let baselines = HashMap::new();
        let ctx = EntityContext {
            serializers: &serializers,
            classes: &classes,
            class_id_bits: 1,
            baselines: &baselines,
        };
        let packet = |w: BitWriter| CsvcMsgPacketEntities {
            updated_entries: Some(1),
            entity_data: Some(w.finish()),
            ..Default::default()
        };
        let element = |i| FieldPath::new(&[0, i]);
        let mut entities = Entities::default();

        // Create entity 0 with m_list = [7, 8, 9].
        let mut w = BitWriter::default();
        w.ubitvar(0).bits(0b10, 2).bits(0, 1).bits(1, 17).varint(0);
        write_paths(&mut w, &[&[0], &[0, 0], &[0, 1], &[0, 2]]);
        w.varint(3).varint(7).varint(8).varint(9);
        entities.apply(&packet(w), ctx, false).unwrap();
        assert_eq!(
            entities.get(0).unwrap().get(&element(2)),
            Some(&FieldValue::Unsigned(9))
        );

        // Shrink to 1 element: elements 1 and 2 are gone, element 0 stays.
        let mut w = BitWriter::default();
        w.ubitvar(0).bits(0b00, 2);
        write_paths(&mut w, &[&[0]]);
        w.varint(1);
        entities.apply(&packet(w), ctx, false).unwrap();
        let e = entities.get(0).unwrap();
        assert_eq!(e.get(&element(0)), Some(&FieldValue::Unsigned(7)));
        assert_eq!(e.get(&element(1)), None);
        assert_eq!(e.get(&element(2)), None);

        // Writing element 1 while the length is 1 means the stream is out of sync.
        let mut w = BitWriter::default();
        w.ubitvar(0).bits(0b00, 2);
        write_paths(&mut w, &[&[0, 1]]);
        w.varint(5);
        let err = entities.apply(&packet(w), ctx, false).unwrap_err();
        assert!(matches!(
            err,
            EntityError::Resolve {
                source: ResolveError::ElementOutOfRange { .. },
                ..
            }
        ));
    }
}
