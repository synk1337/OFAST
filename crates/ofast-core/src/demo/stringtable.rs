//! String tables: named key → value lists that the server shares with clients.
//!
//! A demo carries a few dozen tables (`instancebaseline`, `userinfo`, `modelprecache`, `soundprecache`, …). OFAST
//! needs two of them so far:
//!
//! - **`instancebaseline`**: key = server class id as decimal text (`"38"`), value = an encoded entity update
//!   holding that class's default field values. Every newly created entity starts from its class baseline.
//! - **`userinfo`**: key = player slot, value = a `CMsgPlayerInfo` protobuf (name, Steam id).
//!
//! Tables are created by `svc_CreateStringTable` (all entries at once) and patched by `svc_UpdateStringTable`. Both
//! carry the entries as one bit stream, described on [`StringTable::apply_entries`]. Full packets and the
//! `DEM_StringTables` frame carry a plain protobuf copy instead (`CDemoStringTables`), applied with
//! [`StringTables::apply_snapshot`].
//!
//! Knowledge source: demoinfocs-golang `stringtables.go` (MIT); one detail (fixed-size values) was corrected after
//! checking real demos — see `docs/formats/demo.md`.

use super::bits::{BitReader, BitsEof};
use super::frame::decompress_snappy;
use super::proto::{CDemoStringTables, CsvcMsgCreateStringTable};

/// Number of recent keys kept for "key starts like an earlier key" compression.
const KEY_HISTORY: usize = 32;

/// Table flag: values may be individually Snappy-compressed (a bit before each value says whether this one is).
const FLAG_VALUES_MAY_BE_COMPRESSED: i32 = 1;

/// Most entries one table may have. Real CS2 tables have at most a few hundred; the limit keeps a corrupt index
/// from allocating huge tables.
pub const MAX_ENTRIES: i64 = 1 << 16;

/// Most tables a demo may create (CS2 creates about a dozen).
pub const MAX_TABLES: usize = 64;

/// Errors while reading string table entries.
#[derive(Debug, thiserror::Error)]
pub enum StringTableError {
    /// Ran out of bits.
    #[error(transparent)]
    Eof(#[from] BitsEof),
    /// A compressed table blob or value is not valid Snappy data.
    #[error("string table {table:?}: snappy decompression failed: {source}")]
    Snappy {
        /// Table name.
        table: String,
        /// The decoder's error.
        source: snap::Error,
    },
    /// An entry index went negative or absurdly high: the stream is out of sync.
    #[error("string table {table:?}: entry index {index} out of range")]
    BadIndex {
        /// Table name.
        table: String,
        /// The index that was computed.
        index: i64,
    },
    /// `svc_UpdateStringTable` referenced a table that was never created.
    #[error("update for unknown string table id {0}")]
    UnknownTable(i32),
    /// More than [`MAX_TABLES`] tables were created.
    #[error("more than {MAX_TABLES} string tables")]
    TooManyTables,
}

/// One entry of a string table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StringTableEntry {
    /// The key (often a file path, a number or a player slot).
    pub key: String,
    /// The value, if the entry has one.
    pub value: Option<Vec<u8>>,
}

/// One string table and the settings that tell how its entries are encoded.
#[derive(Debug, Clone, Default)]
pub struct StringTable {
    /// Table name, e.g. `instancebaseline`.
    pub name: String,
    /// Every value has exactly `user_data_size_bits` bits.
    pub user_data_fixed_size: bool,
    /// Bit size of fixed-size values.
    pub user_data_size_bits: u32,
    /// Table flags (bit 0: values may be compressed).
    pub flags: i32,
    /// Value lengths are ubitvars instead of 17-bit numbers.
    pub using_varint_bitcounts: bool,
    /// The entries, by index.
    pub entries: Vec<StringTableEntry>,
}

impl StringTable {
    /// Creates a table from `svc_CreateStringTable` and reads its initial entries. The table is returned even when
    /// its entries could not all be read (with the entries read so far), so table ids stay in step with the server.
    pub fn create(msg: &CsvcMsgCreateStringTable) -> (Self, Result<(), StringTableError>) {
        let mut table = Self {
            name: msg.name().to_owned(),
            user_data_fixed_size: msg.user_data_fixed_size(),
            user_data_size_bits: msg.user_data_size_bits().max(0) as u32,
            flags: msg.flags(),
            using_varint_bitcounts: msg.using_varint_bitcounts(),
            entries: Vec::new(),
        };
        let data = if msg.data_compressed() {
            match decompress_snappy(msg.string_data()) {
                Ok(data) => data,
                Err(source) => {
                    let name = table.name.clone();
                    return (
                        table,
                        Err(StringTableError::Snappy {
                            table: name,
                            source,
                        }),
                    );
                }
            }
        } else {
            msg.string_data().to_vec()
        };
        let result = table.apply_entries(&data, msg.num_entries().max(0) as usize);
        (table, result.map(|_| ()))
    }

    /// Reads `count` entry changes from `data` and applies them. Returns the indices that changed.
    ///
    /// Bit layout of each entry:
    ///
    /// 1. Index: a `1` bit means "previous index + 1"; a `0` bit is followed by a varint `n` meaning "previous index
    ///    + n + 2". The previous index starts at -1.
    /// 2. `has_key` bit. If set: a `use_history` bit; with history, 5 bits pick one of the last 32 keys and 5 bits
    ///    say how many of its leading bytes to reuse; then a NUL-terminated string completes the key.
    /// 3. `has_value` bit. If set: for fixed-size tables, `user_data_size_bits` bits; otherwise an `is_compressed`
    ///    bit (only if the table flags allow compression), a byte length (ubitvar or 17 bits) and that many bytes,
    ///    Snappy-compressed if `is_compressed`.
    pub fn apply_entries(
        &mut self,
        data: &[u8],
        count: usize,
    ) -> Result<Vec<usize>, StringTableError> {
        let mut r = BitReader::new(data);
        let mut index: i64 = -1;
        let mut history: Vec<String> = Vec::with_capacity(KEY_HISTORY);
        // Not `with_capacity(count)`: `count` comes from the file and could ask for gigabytes up front.
        let mut changed = Vec::new();

        for _ in 0..count {
            index += if r.bool("string table index increment")? {
                1
            } else {
                i64::from(r.varint_u32("string table index skip")?) + 2
            };
            if !(0..MAX_ENTRIES).contains(&index) {
                return Err(StringTableError::BadIndex {
                    table: self.name.clone(),
                    index,
                });
            }

            let key = if r.bool("string table has key")? {
                let key = if r.bool("string table key history")? {
                    let position = r.bits(5, "key history position")? as usize;
                    let length = r.bits(5, "key history length")? as usize;
                    let mut key = history
                        .get(position)
                        .map(|k| k.as_bytes()[..length.min(k.len())].to_vec())
                        .unwrap_or_default();
                    key.extend_from_slice(r.string("string table key suffix")?.as_bytes());
                    String::from_utf8_lossy(&key).into_owned()
                } else {
                    r.string("string table key")?
                };
                if history.len() == KEY_HISTORY {
                    history.remove(0);
                }
                history.push(key.clone());
                Some(key)
            } else {
                None
            };

            let value = if r.bool("string table has value")? {
                Some(self.read_value(&mut r)?)
            } else {
                None
            };

            let i = index as usize;
            if self.entries.len() <= i {
                self.entries.resize_with(i + 1, Default::default);
            }
            let entry = &mut self.entries[i];
            if let Some(key) = key {
                entry.key = key;
            }
            if value.is_some() {
                entry.value = value;
            }
            changed.push(i);
        }
        Ok(changed)
    }

    fn read_value(&self, r: &mut BitReader<'_>) -> Result<Vec<u8>, StringTableError> {
        if self.user_data_fixed_size {
            // Both reference parsers read `user_data_size` here; real CS2 demos only stay in sync with the *bit* size.
            return Ok(read_bits_as_bytes(r, self.user_data_size_bits)?);
        }
        let compressed =
            self.flags & FLAG_VALUES_MAY_BE_COMPRESSED != 0 && r.bool("value compressed")?;
        let len = if self.using_varint_bitcounts {
            r.ubitvar("value length")?
        } else {
            r.bits(17, "value length")?
        };
        let bytes = r.bytes(len as usize, "string table value")?;
        if compressed {
            return decompress_snappy(&bytes).map_err(|source| StringTableError::Snappy {
                table: self.name.clone(),
                source,
            });
        }
        Ok(bytes)
    }
}

/// Reads `bits` bits into bytes (the last byte holds the leftover bits in its low end).
fn read_bits_as_bytes(r: &mut BitReader<'_>, bits: u32) -> Result<Vec<u8>, BitsEof> {
    let mut out = r.bytes((bits / 8) as usize, "fixed-size value")?;
    if !bits.is_multiple_of(8) {
        out.push(r.bits(bits % 8, "fixed-size value")? as u8);
    }
    Ok(out)
}

/// All string tables of a demo, in creation order (the order is the table id used by updates).
#[derive(Debug, Clone, Default)]
pub struct StringTables {
    tables: Vec<StringTable>,
    /// Encoding settings of tables dropped by [`StringTables::clear`], so a snapshot that brings them back can
    /// restore how their later updates are encoded.
    cleared: Vec<StringTable>,
}

impl StringTables {
    /// Handles `svc_CreateStringTable`. Returns the new table's id and whether its entries were read completely
    /// (the table is added either way). Fails only when there are already [`MAX_TABLES`] tables.
    pub fn create(
        &mut self,
        msg: &CsvcMsgCreateStringTable,
    ) -> Result<(usize, Result<(), StringTableError>), StringTableError> {
        if self.tables.len() >= MAX_TABLES {
            return Err(StringTableError::TooManyTables);
        }
        let (table, entries) = StringTable::create(msg);
        self.tables.push(table);
        Ok((self.tables.len() - 1, entries))
    }

    /// Handles `svc_UpdateStringTable`; returns the changed entry indices.
    pub fn update(
        &mut self,
        table_id: i32,
        data: &[u8],
        count: usize,
    ) -> Result<Vec<usize>, StringTableError> {
        let table = usize::try_from(table_id)
            .ok()
            .and_then(|id| self.tables.get_mut(id))
            .ok_or(StringTableError::UnknownTable(table_id))?;
        table.apply_entries(data, count)
    }

    /// Handles `svc_ClearAllStringTables`.
    pub fn clear(&mut self) {
        for mut table in self.tables.drain(..) {
            table.entries = Vec::new();
            self.cleared.retain(|t| t.name != table.name);
            self.cleared.push(table);
        }
    }

    /// Replaces the entries of every table in a `CDemoStringTables` snapshot (full packets, `DEM_StringTables`).
    /// Tables in the snapshot that do not exist yet are created with the settings they had before a
    /// [`clear`](Self::clear), or default settings if they never existed.
    pub fn apply_snapshot(&mut self, snapshot: &CDemoStringTables) {
        for t in &snapshot.tables {
            let entries = t
                .items
                .iter()
                .map(|item| StringTableEntry {
                    key: item.str().to_owned(),
                    value: item.data.clone(),
                })
                .collect();
            if let Some(table) = self
                .tables
                .iter_mut()
                .find(|table| table.name == t.table_name())
            {
                table.entries = entries;
            } else if self.tables.len() < MAX_TABLES {
                let settings = self.cleared.iter().find(|c| c.name == t.table_name());
                self.tables.push(StringTable {
                    name: t.table_name().to_owned(),
                    entries,
                    ..settings.cloned().unwrap_or_default()
                });
            }
        }
    }

    /// Table by id.
    pub fn get(&self, id: usize) -> Option<&StringTable> {
        self.tables.get(id)
    }

    /// Table by name.
    pub fn by_name(&self, name: &str) -> Option<&StringTable> {
        self.tables.iter().find(|t| t.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::bits::BitWriter;

    fn table(fixed_bits: Option<u32>) -> StringTable {
        StringTable {
            name: "test".into(),
            user_data_fixed_size: fixed_bits.is_some(),
            user_data_size_bits: fixed_bits.unwrap_or(0),
            flags: FLAG_VALUES_MAY_BE_COMPRESSED,
            using_varint_bitcounts: true,
            entries: Vec::new(),
        }
    }

    #[test]
    fn reads_keys_history_skips_and_compressed_values() {
        let compressed = snap::raw::Encoder::new()
            .compress_vec(b"baseline bytes")
            .unwrap();
        let mut w = BitWriter::default();
        // Entry 0: key "models/a.vmdl", plain value "xy".
        w.bool(true)
            .bool(true)
            .bool(false)
            .bytes(b"models/a.vmdl\0");
        w.bool(true).bool(false).ubitvar(2).bytes(b"xy");
        // Entry 3 (skip: 0 + n=1 + 2): key = first 7 bytes of history[0] + "b.vmdl", no value.
        w.bool(false).varint(1);
        w.bool(true)
            .bool(true)
            .bits(0, 5)
            .bits(7, 5)
            .bytes(b"b.vmdl\0");
        w.bool(false);
        // Entry 4: no key, compressed value.
        w.bool(true).bool(false);
        w.bool(true)
            .bool(true)
            .ubitvar(compressed.len() as u32)
            .bytes(&compressed);
        let data = w.finish();

        let mut t = table(None);
        let changed = t.apply_entries(&data, 3).unwrap();
        assert_eq!(changed, vec![0, 3, 4]);
        assert_eq!(t.entries[0].key, "models/a.vmdl");
        assert_eq!(t.entries[0].value.as_deref(), Some(&b"xy"[..]));
        assert_eq!(t.entries[1], StringTableEntry::default());
        assert_eq!(t.entries[3].key, "models/b.vmdl");
        assert_eq!(t.entries[3].value, None);
        assert_eq!(t.entries[4].value.as_deref(), Some(&b"baseline bytes"[..]));
    }

    #[test]
    fn create_update_and_snapshot() {
        use crate::demo::proto::c_demo_string_tables::{ItemsT, TableT};

        // Create a table with one entry, key "38", value "a" (compressed blob, like CS2 sends it).
        let mut w = BitWriter::default();
        w.bool(true).bool(true).bool(false).bytes(b"38\0");
        w.bool(true).bool(false).ubitvar(1).bytes(b"a");
        let blob = w.finish();
        let msg = CsvcMsgCreateStringTable {
            name: Some("instancebaseline".into()),
            num_entries: Some(1),
            flags: Some(FLAG_VALUES_MAY_BE_COMPRESSED),
            using_varint_bitcounts: Some(true),
            data_compressed: Some(true),
            string_data: Some(snap::raw::Encoder::new().compress_vec(&blob).unwrap()),
            ..Default::default()
        };
        let mut tables = StringTables::default();
        let (id, entries) = tables.create(&msg).unwrap();
        entries.unwrap();
        assert_eq!(
            tables.by_name("instancebaseline").unwrap().entries[0].key,
            "38"
        );

        // Update entry 0's value through the table id; the key stays.
        let mut w = BitWriter::default();
        w.bool(true)
            .bool(false)
            .bool(true)
            .bool(false)
            .ubitvar(1)
            .bytes(b"b");
        assert_eq!(tables.update(id as i32, &w.finish(), 1).unwrap(), vec![0]);
        let entry = &tables.get(id).unwrap().entries[0];
        assert_eq!(
            (entry.key.as_str(), entry.value.as_deref()),
            ("38", Some(&b"b"[..]))
        );
        assert!(matches!(
            tables.update(7, &[], 0),
            Err(StringTableError::UnknownTable(7))
        ));

        // A snapshot replaces the entries of a known table and creates unknown ones.
        let item = |key: &str, value: &[u8]| ItemsT {
            str: Some(key.into()),
            data: Some(value.to_vec()),
        };
        let table = |name: &str, items| TableT {
            table_name: Some(name.into()),
            items,
            ..Default::default()
        };
        tables.apply_snapshot(&CDemoStringTables {
            tables: vec![
                table("instancebaseline", vec![item("40", b"c")]),
                table("userinfo", vec![item("0", b"p")]),
            ],
        });
        assert_eq!(
            tables.by_name("instancebaseline").unwrap().entries[0].key,
            "40"
        );
        assert_eq!(tables.by_name("userinfo").unwrap().entries.len(), 1);

        tables.clear();
        assert!(tables.by_name("userinfo").is_none());

        // After a clear, a snapshot brings a table back with its old encoding settings.
        tables.apply_snapshot(&CDemoStringTables {
            tables: vec![table("instancebaseline", vec![item("41", b"d")])],
        });
        let restored = tables.by_name("instancebaseline").unwrap();
        assert!(restored.using_varint_bitcounts);
        assert_eq!(restored.flags, FLAG_VALUES_MAY_BE_COMPRESSED);
        assert_eq!(restored.entries[0].key, "41");
    }

    #[test]
    fn fixed_size_values_use_the_bit_size() {
        let mut w = BitWriter::default();
        w.bool(true)
            .bool(true)
            .bool(false)
            .bytes(b"k\0")
            .bool(true)
            .bits(0b10, 2);
        w.bool(true).bool(false).bool(true).bits(0b01, 2);
        let data = w.finish();
        let mut t = table(Some(2));
        t.apply_entries(&data, 2).unwrap();
        assert_eq!(t.entries[0].value.as_deref(), Some(&[0b10u8][..]));
        assert_eq!(t.entries[1].value.as_deref(), Some(&[0b01u8][..]));
    }
}
