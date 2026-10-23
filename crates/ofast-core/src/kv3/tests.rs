//! Round-trip tests for binary KV3: a test-only encoder writes every version (1–5) × compression (none, LZ4,
//! ZSTD) straight from `docs/formats/kv3.md`, and the decoder must give back the exact same value tree while
//! consuming every byte. Real CS2 files are tested in `tests/cs2_kv3.rs`.

use std::collections::HashMap;

use super::binary::{DecodeReport, Kv3Error, decode_with_report};
use super::{Flag, Guid, Kv3File, Object, Value, text};

const NONE: u32 = 0;
const LZ4: u32 = 1;
const ZSTD: u32 = 2;
const MARKER: u32 = 0xFFEE_DD00;
const FRAME: usize = 16384;

/// Lane contents of one buffer while encoding.
#[derive(Default)]
struct Lanes {
    b1: Vec<u8>,
    b2: Vec<u8>,
    b4: Vec<u8>,
    b8: Vec<u8>,
}

/// Test-only binary KV3 encoder. Mirrors the decoder's reading order exactly.
struct Encoder {
    version: u8,
    main: Lanes,
    aux: Lanes,
    types: Vec<u8>,
    strings: Vec<String>,
    string_ids: HashMap<String, i32>,
    object_counts: Vec<u8>,
    blobs: Vec<Vec<u8>>,
}

impl Encoder {
    fn new(version: u8) -> Self {
        Self {
            version,
            main: Lanes::default(),
            aux: Lanes::default(),
            types: Vec::new(),
            strings: Vec::new(),
            string_ids: HashMap::new(),
            object_counts: Vec::new(),
            blobs: Vec::new(),
        }
    }

    fn string_id(&mut self, s: &str) -> i32 {
        if let Some(&id) = self.string_ids.get(s) {
            return id;
        }
        let id = self.strings.len() as i32;
        self.strings.push(s.to_owned());
        self.string_ids.insert(s.to_owned(), id);
        id
    }

    fn type_byte(&mut self, ty: u8, flag: Option<Flag>) {
        match flag {
            None => self.types.push(ty),
            Some(flag) if self.version >= 3 => {
                self.types.push(ty | 0x80);
                self.types.push(match flag {
                    Flag::Resource => 1,
                    Flag::ResourceName => 2,
                    Flag::Panorama => 3,
                    Flag::SoundEvent => 4,
                    Flag::SubClass => 5,
                    Flag::EntityName => 6,
                    Flag::Localize => 7,
                    Flag::Unknown(v) => v,
                });
            }
            Some(flag) => {
                self.types.push(ty | 0x80);
                self.types.push(match flag {
                    Flag::Resource => 1,
                    Flag::ResourceName => 2,
                    Flag::Panorama => 8,
                    Flag::SoundEvent => 16,
                    Flag::SubClass => 32,
                    other => panic!("{other:?} has no bit in KV3 v{}", self.version),
                });
            }
        }
    }

    fn write_value(&mut self, value: &Value) {
        let (flag, value) = match value {
            Value::Flagged(flag, inner) => (Some(*flag), inner.as_ref()),
            other => (None, other),
        };
        match value {
            Value::Null => self.type_byte(1, flag),
            Value::Bool(b) => {
                self.type_byte(2, flag);
                self.main.b1.push(u8::from(*b));
            }
            Value::Int(0) => self.type_byte(15, flag),
            Value::Int(1) => self.type_byte(16, flag),
            Value::Int(i) if self.version >= 4 && i16::try_from(*i).is_ok() => {
                self.type_byte(20, flag);
                self.main.b2.extend_from_slice(&(*i as i16).to_le_bytes());
            }
            Value::Int(i) if i32::try_from(*i).is_ok() => {
                self.type_byte(11, flag);
                self.main.b4.extend_from_slice(&(*i as i32).to_le_bytes());
            }
            Value::Int(i) => {
                self.type_byte(3, flag);
                self.main.b8.extend_from_slice(&i.to_le_bytes());
            }
            Value::UInt(u) if u32::try_from(*u).is_ok() => {
                self.type_byte(12, flag);
                self.main.b4.extend_from_slice(&(*u as u32).to_le_bytes());
            }
            Value::UInt(u) => {
                self.type_byte(4, flag);
                self.main.b8.extend_from_slice(&u.to_le_bytes());
            }
            Value::Float(f) => {
                self.type_byte(5, flag);
                self.main.b8.extend_from_slice(&f.to_le_bytes());
            }
            Value::String(s) => {
                self.type_byte(6, flag);
                let id = self.string_id(s);
                self.main.b4.extend_from_slice(&id.to_le_bytes());
            }
            Value::Blob(bytes) => {
                self.type_byte(7, flag);
                if self.version == 1 {
                    self.main
                        .b4
                        .extend_from_slice(&(bytes.len() as i32).to_le_bytes());
                    self.main.b1.extend_from_slice(bytes);
                } else {
                    self.blobs.push(bytes.clone());
                }
            }
            Value::Array(items) => self.write_array(items, flag),
            Value::Object(object) => {
                self.type_byte(9, flag);
                let len = (object.len() as i32).to_le_bytes();
                if self.version >= 5 {
                    self.object_counts.extend_from_slice(&len);
                } else {
                    self.main.b4.extend_from_slice(&len);
                }
                for (key, member) in object.iter() {
                    let id = self.string_id(key);
                    self.main.b4.extend_from_slice(&id.to_le_bytes());
                    self.write_value(member);
                }
            }
            Value::Flagged(..) => unreachable!("flags were unwrapped above"),
        }
    }

    /// Arrays of floats become typed arrays; in v5, short int arrays use the auxiliary buffer.
    fn write_array(&mut self, items: &[Value], flag: Option<Flag>) {
        let all_floats = !items.is_empty() && items.iter().all(|v| matches!(v, Value::Float(_)));
        let all_small_ints = !items.is_empty()
            && items.len() <= 255
            && items
                .iter()
                .all(|v| matches!(v, Value::Int(i) if i32::try_from(*i).is_ok()));

        if self.version >= 5 && all_small_ints {
            self.type_byte(25, flag);
            self.main.b1.push(items.len() as u8);
            self.type_byte(11, None);
            for item in items {
                let Value::Int(i) = item else { unreachable!() };
                self.aux.b4.extend_from_slice(&(*i as i32).to_le_bytes());
            }
        } else if self.version >= 2 && all_floats {
            self.type_byte(10, flag);
            self.main
                .b4
                .extend_from_slice(&(items.len() as i32).to_le_bytes());
            self.type_byte(5, None);
            for item in items {
                let Value::Float(f) = item else {
                    unreachable!()
                };
                self.main.b8.extend_from_slice(&f.to_le_bytes());
            }
        } else {
            self.type_byte(8, flag);
            self.main
                .b4
                .extend_from_slice(&(items.len() as i32).to_le_bytes());
            for item in items {
                self.write_value(item);
            }
        }
    }

    /// Produces the complete block for `file` with the given compression.
    fn encode(version: u8, compression: u32, file: &Kv3File) -> Vec<u8> {
        let mut enc = Encoder::new(version);
        enc.write_value(&file.root);
        enc.finish(compression, file.format)
    }

    fn finish(self, compression: u32, format: Guid) -> Vec<u8> {
        let mut strings = Vec::new();
        for s in &self.strings {
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
        }
        let string_count = (self.strings.len() as i32).to_le_bytes();
        let blob_bytes: Vec<u8> = self.blobs.concat();

        // LZ4 blob chunks are compressed first: their sizes go at the end of the main buffer.
        let mut chunk_sizes = Vec::new();
        let mut lz4_blob_data = Vec::new();
        if compression == LZ4 {
            for blob in &self.blobs {
                for chunk in blob.chunks(FRAME) {
                    let compressed = lz4_flex::block::compress(chunk);
                    chunk_sizes.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
                    lz4_blob_data.extend_from_slice(&compressed);
                }
            }
        }
        let mut tail = Vec::new();
        for blob in &self.blobs {
            tail.extend_from_slice(&(blob.len() as i32).to_le_bytes());
        }
        tail.extend_from_slice(&MARKER.to_le_bytes());
        tail.extend_from_slice(&chunk_sizes);

        let pad = |buf: &mut Vec<u8>, align: usize, lane_len: usize, pad_empty: bool| {
            if lane_len > 0 || pad_empty {
                buf.resize(buf.len().next_multiple_of(align), 0);
            }
        };

        let mut out = Vec::new();
        out.extend_from_slice(&(0x4B56_3300u32 | u32::from(self.version)).to_le_bytes());
        out.extend_from_slice(&format.0);
        out.extend_from_slice(&compression.to_le_bytes());
        let i32s = |out: &mut Vec<u8>, values: &[usize]| {
            for v in values {
                out.extend_from_slice(&(*v as i32).to_le_bytes());
            }
        };

        if self.version < 5 {
            // Single buffer: lanes, strings, types, tail. The string count leads the 4-byte lane.
            let b4 = [string_count.as_slice(), &self.main.b4].concat();
            let mut buf = self.main.b1.clone();
            pad(&mut buf, 2, self.main.b2.len(), true);
            buf.extend_from_slice(&self.main.b2);
            pad(&mut buf, 4, b4.len(), true);
            buf.extend_from_slice(&b4);
            pad(&mut buf, 8, self.main.b8.len(), true);
            buf.extend_from_slice(&self.main.b8);
            buf.extend_from_slice(&strings);
            buf.extend_from_slice(&self.types);
            if self.version == 1 {
                buf.extend_from_slice(&MARKER.to_le_bytes());
                i32s(
                    &mut out,
                    &[
                        self.main.b1.len(),
                        b4.len() / 4,
                        self.main.b8.len() / 8,
                        buf.len(),
                    ],
                );
                out.extend_from_slice(&buf);
                return out;
            }
            buf.extend_from_slice(&tail);

            let (payload, blobs_after): (Vec<u8>, Vec<u8>) = match compression {
                NONE => (buf.clone(), blob_bytes.clone()),
                LZ4 => (lz4_flex::block::compress(&buf), lz4_blob_data.clone()),
                _ => {
                    let mut frames = ruzstd::encoding::compress_to_vec(
                        buf.as_slice(),
                        ruzstd::encoding::CompressionLevel::Fastest,
                    );
                    if !blob_bytes.is_empty() {
                        frames.extend(ruzstd::encoding::compress_to_vec(
                            blob_bytes.as_slice(),
                            ruzstd::encoding::CompressionLevel::Fastest,
                        ));
                    }
                    (frames, Vec::new())
                }
            };
            out.extend_from_slice(&0u16.to_le_bytes()); // dictionary
            out.extend_from_slice(
                &(if compression == LZ4 { FRAME as u16 } else { 0 }).to_le_bytes(),
            );
            i32s(
                &mut out,
                &[self.main.b1.len(), b4.len() / 4, self.main.b8.len() / 8],
            );
            i32s(&mut out, &[strings.len() + self.types.len()]);
            out.extend_from_slice(&0u16.to_le_bytes()); // object count hint
            out.extend_from_slice(&0u16.to_le_bytes()); // array count hint
            i32s(
                &mut out,
                &[buf.len(), payload.len(), self.blobs.len(), blob_bytes.len()],
            );
            if self.version >= 4 {
                i32s(&mut out, &[self.main.b2.len() / 2, chunk_sizes.len()]);
            }
            out.extend_from_slice(&payload);
            out.extend_from_slice(&blobs_after);
        } else {
            // Buffer 1 (auxiliary): strings lead the 1-byte lane, the string count leads the 4-byte lane.
            let aux_b1 = [strings.as_slice(), &self.aux.b1].concat();
            let aux_b4 = [string_count.as_slice(), &self.aux.b4].concat();
            let mut aux = aux_b1.clone();
            pad(&mut aux, 2, self.aux.b2.len(), false);
            aux.extend_from_slice(&self.aux.b2);
            pad(&mut aux, 4, aux_b4.len(), false);
            aux.extend_from_slice(&aux_b4);
            pad(&mut aux, 8, self.aux.b8.len(), false);
            aux.extend_from_slice(&self.aux.b8);

            // Buffer 2 (main): object counts, lanes, types, tail.
            let mut main = self.object_counts.clone();
            main.extend_from_slice(&self.main.b1);
            pad(&mut main, 2, self.main.b2.len(), false);
            main.extend_from_slice(&self.main.b2);
            pad(&mut main, 4, self.main.b4.len(), false);
            main.extend_from_slice(&self.main.b4);
            pad(&mut main, 8, self.main.b8.len(), false);
            main.extend_from_slice(&self.main.b8);
            main.extend_from_slice(&self.types);
            main.append(&mut tail);

            let zstd = |data: &[u8]| {
                ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
            };
            let (c_aux, c_main, c_blobs) = match compression {
                NONE => (aux.clone(), main.clone(), blob_bytes.clone()),
                LZ4 => (
                    lz4_flex::block::compress(&aux),
                    lz4_flex::block::compress(&main),
                    lz4_blob_data.clone(),
                ),
                _ => (
                    zstd(&aux),
                    zstd(&main),
                    if blob_bytes.is_empty() {
                        Vec::new()
                    } else {
                        zstd(&blob_bytes)
                    },
                ),
            };
            let (size_c1, size_c2) = if compression == NONE {
                (0, 0)
            } else {
                (c_aux.len(), c_main.len())
            };

            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(
                &(if compression == LZ4 { FRAME as u16 } else { 0 }).to_le_bytes(),
            );
            i32s(
                &mut out,
                &[
                    aux_b1.len(),
                    aux_b4.len() / 4,
                    self.aux.b8.len() / 8,
                    self.types.len(),
                ],
            );
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            i32s(
                &mut out,
                &[
                    aux.len() + main.len(),
                    c_aux.len() + c_main.len() + c_blobs.len(),
                    self.blobs.len(),
                    blob_bytes.len(),
                ],
            );
            i32s(&mut out, &[self.aux.b2.len() / 2, chunk_sizes.len()]);
            i32s(&mut out, &[aux.len(), size_c1, main.len(), size_c2]);
            i32s(
                &mut out,
                &[
                    self.main.b1.len(),
                    self.main.b2.len() / 2,
                    self.main.b4.len() / 4,
                    self.main.b8.len() / 8,
                ],
            );
            i32s(&mut out, &[0, self.object_counts.len() / 4, 0, 0]);
            out.extend_from_slice(&c_aux);
            out.extend_from_slice(&c_main);
            out.extend_from_slice(&c_blobs);
        }
        if !self.blobs.is_empty() {
            out.extend_from_slice(&MARKER.to_le_bytes());
        }
        out
    }
}

/// A value tree exercising every kind the encoder can write for `version`.
fn sample(version: u8) -> Kv3File {
    let mut nested = Object::new();
    nested.push("m_name", Value::String("g_tColor".into()));
    nested.push(
        "m_pValue",
        Value::Flagged(
            Flag::Resource,
            Box::new(Value::String("materials/dev/grid_color.vtex".into())),
        ),
    );

    let mut root = Object::new();
    root.push("m_shaderName", Value::String("csgo_environment.vfx".into()));
    root.push("nothing", Value::Null);
    root.push("enabled", Value::Bool(true));
    root.push("disabled", Value::Bool(false));
    root.push("zero", Value::Int(0));
    root.push("one", Value::Int(1));
    root.push("small", Value::Int(-5));
    root.push("medium", Value::Int(70_000));
    root.push("large", Value::Int(-9_000_000_000));
    root.push("unsigned", Value::UInt(4_000_000_000));
    root.push("huge_unsigned", Value::UInt(u64::MAX));
    root.push("ratio", Value::Float(0.5));
    root.push("unicode", Value::String("caméra → ✓".into()));
    root.push("reused key", Value::String("csgo_environment.vfx".into()));
    root.push("m_textureParams", Value::Array(vec![Value::Object(nested)]));
    root.push(
        "color",
        Value::Array(vec![
            Value::Float(1.0),
            Value::Float(0.25),
            Value::Float(-3.5),
        ]),
    );
    root.push(
        "ids",
        Value::Array(vec![Value::Int(10), Value::Int(-20), Value::Int(30)]),
    );
    root.push(
        "mixed",
        Value::Array(vec![Value::Int(7), Value::String("x".into()), Value::Null]),
    );
    root.push("empty_array", Value::Array(Vec::new()));
    root.push("empty_object", Value::Object(Object::new()));
    root.push(
        "sound",
        Value::Flagged(
            Flag::SoundEvent,
            Box::new(Value::String("Weapon_AK47.Single".into())),
        ),
    );
    if version >= 3 {
        root.push(
            "entity",
            Value::Flagged(Flag::EntityName, Box::new(Value::String("spawn_ct".into()))),
        );
    }
    root.push("small_blob", Value::Blob(vec![1, 2, 3, 0xFF]));
    if version >= 2 {
        // Bigger than one LZ4 frame, and repetitive so chunks compress (and exercise chained decoding).
        let big: Vec<u8> = (0..40_000u32)
            .map(|i| (i % 251) as u8 ^ (i / 1000) as u8)
            .collect();
        root.push("big_blob", Value::Blob(big));
    }
    Kv3File {
        format: Guid::FORMAT_GENERIC,
        root: Value::Object(root),
    }
}

fn round_trip(version: u8, compression: u32) -> DecodeReport {
    let file = sample(version);
    let block = Encoder::encode(version, compression, &file);
    let (decoded, report) = decode_with_report(&block)
        .unwrap_or_else(|e| panic!("v{version} compression {compression}: {e}"));
    assert_eq!(decoded, file, "v{version} compression {compression}");
    assert!(
        report.is_exact(),
        "v{version} compression {compression}: {report:?}"
    );
    assert_eq!(
        report.trailing_bytes, 0,
        "v{version} compression {compression}"
    );
    assert_eq!(report.version, version);
    report
}

#[test]
fn version_1_round_trips() {
    round_trip(1, NONE);
}

#[test]
fn versions_2_to_5_round_trip_with_every_compression() {
    for version in 2..=5 {
        for compression in [NONE, LZ4, ZSTD] {
            let report = round_trip(version, compression);
            assert_eq!(
                report.blob_marker_ok,
                Some(true),
                "v{version} compression {compression}"
            );
        }
    }
}

#[test]
fn rejects_unknown_magic_and_versions() {
    use super::binary::{Kv3Error, decode};
    assert!(matches!(decode(b"NOPE...."), Err(Kv3Error::BadMagic(_))));
    assert!(matches!(
        decode(&0x4B56_3306u32.to_le_bytes()),
        Err(Kv3Error::UnsupportedVersion(6))
    ));
    assert!(matches!(
        decode(b"VKV\x03"),
        Err(Kv3Error::LegacyUnsupported)
    ));
}

#[test]
fn truncated_blocks_fail_cleanly() {
    let block = Encoder::encode(5, LZ4, &sample(5));
    for cut in [3, 20, 100, block.len() / 2] {
        assert!(decode_with_report(&block[..cut]).is_err(), "cut at {cut}");
    }
}

#[test]
fn text_keeps_floats_and_keys_unambiguous() {
    assert_eq!(text::value_to_string(&Value::Float(1e20)), "1e20");
    assert_eq!(text::value_to_string(&Value::Float(3.0)), "3.0");
    let object: Object = [("two\nlines", Value::Null)].into_iter().collect();
    assert_eq!(
        text::value_to_string(&Value::Object(object)),
        "{\n\t\"two\\nlines\" = null\n}"
    );
}

#[test]
fn guid_round_trips_through_text() {
    assert_eq!(
        Guid::FORMAT_GENERIC.to_string(),
        "7412167c-06e9-4698-aff2-e63eb59037e7"
    );
    // On-disk bytes as documented in docs/formats/kv3.md.
    assert_eq!(
        Guid::FORMAT_GENERIC.0,
        [
            0x7C, 0x16, 0x12, 0x74, 0xE9, 0x06, 0x98, 0x46, 0xAF, 0xF2, 0xE6, 0x3E, 0xB5, 0x90,
            0x37, 0xE7
        ]
    );
}

#[test]
fn text_output_is_readable() {
    let mut root = Object::new();
    root.push("m_shaderName", Value::String("csgo_environment.vfx".into()));
    root.push(
        "m_vColor",
        Value::Array(vec![Value::Float(1.0), Value::Float(0.5)]),
    );
    root.push(
        "m_pTexture",
        Value::Flagged(
            Flag::Resource,
            Box::new(Value::String("materials/a.vtex".into())),
        ),
    );
    root.push("has space", Value::Bool(true));
    let text = text::to_string(&Kv3File {
        format: Guid::FORMAT_GENERIC,
        root: Value::Object(root),
    });
    let expected = "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} \
                    format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->\n\
                    {\n\
                    \tm_shaderName = \"csgo_environment.vfx\"\n\
                    \tm_vColor =\n\
                    \t[\n\
                    \t\t1.0,\n\
                    \t\t0.5,\n\
                    \t]\n\
                    \tm_pTexture = resource:\"materials/a.vtex\"\n\
                    \t\"has space\" = true\n\
                    }\n";
    assert_eq!(text, expected);
}

/// Every kind of value the text encoding tells apart, written and read back unchanged.
#[test]
fn text_round_trips() {
    let mut inner = Object::new();
    inner.push(
        "m_name",
        Value::String("two\nlines \"quoted\" \\ tab\t".into()),
    );
    inner.push("m_block", Value::String("first line\nsecond line".into()));
    inner.push(
        "m_edges",
        Value::String("\nstarts and ends with a line break\n".into()),
    );
    inner.push("m_crlf", Value::String("windows\r\nline end\r".into()));
    inner.push("m_empty", Value::Array(Vec::new()));
    inner.push("m_none", Value::Object(Object::new()));
    let mut root = Object::new();
    root.push("m_int", Value::Int(-42));
    root.push("m_big", Value::UInt(u64::MAX));
    root.push("m_float", Value::Float(0.15107809007167816));
    root.push("m_whole", Value::Float(-3.0));
    root.push("m_tiny", Value::Float(1.2e-38));
    root.push("m_inf", Value::Float(f64::NEG_INFINITY));
    root.push(
        "m_bools",
        Value::Array(vec![Value::Bool(true), Value::Bool(false), Value::Null]),
    );
    root.push("m_blob", Value::Blob(vec![0x00, 0x0A, 0xFF]));
    root.push(
        "m_sound",
        Value::Flagged(
            Flag::SoundEvent,
            Box::new(Value::String("Weapon.Fire".into())),
        ),
    );
    root.push("m_inner", Value::Array(vec![Value::Object(inner)]));
    root.push("dotted.key", Value::Int(1));
    root.push("needs quotes", Value::Int(2));
    let file = Kv3File {
        format: Guid::FORMAT_GENERIC,
        root: Value::Object(root),
    };
    assert_eq!(text::parse(&text::to_string(&file)).expect("parses"), file);
}

/// The style Valve's compiler writes into a model's `m_keyValueText`: a `vmdlkeys4` header, `key = ` with the block
/// on the next line, inline arrays with spaces, trailing commas, and comments in hand-written files.
#[test]
fn text_reads_valve_style() {
    let text = "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} \
                format:vmdlkeys4:version{9f5c1002-f19f-470c-bd42-c3c26c063d33} -->\n\
                {\n\
                \tBoneConstraintList = \n\
                \t[\n\
                \t\t{\n\
                \t\t\t_class = \"CTiltTwistConstraint\" // a comment\n\
                \t\t\tm_vUpVector = [ 0.0, 0.0, -1.0 ]\n\
                \t\t\tm_nBoneHash = 2957772116\n\
                \t\t\t/* a block\n comment */ m_bIsAttachment = false\n\
                \t\t},\n\
                \t]\n\
                }\n";
    let file = text::parse(text).expect("parses");
    assert_eq!(
        file.format.to_string(),
        "9f5c1002-f19f-470c-bd42-c3c26c063d33"
    );
    let constraint = &file
        .root
        .get("BoneConstraintList")
        .and_then(Value::as_array)
        .expect("list")[0];
    assert_eq!(
        constraint.get("_class").and_then(Value::as_str),
        Some("CTiltTwistConstraint")
    );
    assert_eq!(
        constraint.get("m_nBoneHash").and_then(Value::as_i64),
        Some(2957772116)
    );
    assert_eq!(
        constraint.get("m_bIsAttachment").and_then(Value::as_bool),
        Some(false)
    );
    let up: Vec<f64> = constraint
        .get("m_vUpVector")
        .and_then(Value::as_array)
        .expect("up")
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    assert_eq!(up, [0.0, 0.0, -1.0]);
    // No header: the generic format.
    assert_eq!(
        text::parse("{ a = 1 }").expect("parses").format,
        Guid::FORMAT_GENERIC
    );
}

#[test]
fn text_errors_name_the_line() {
    for (bad, line) in [
        ("{\n a = 1\n b 2\n}", 3),
        ("{ a = [ 1 2 ] }", 1),
        ("{\n a = \"open\n}", 3),
        ("{ a = #[ 0G ] }", 1),
        ("{ a = #[ +F ] }", 1),
        ("{ a = 1 } extra", 1),
        ("{\n a = wrong\n}", 2),
    ] {
        match text::parse(bad) {
            Err(Kv3Error::Text { line: found, .. }) => assert_eq!(found, line, "{bad:?}"),
            other => panic!("{bad:?}: {other:?}"),
        }
    }
    let deep = "[".repeat(600) + &"]".repeat(600);
    assert!(matches!(text::parse(&deep), Err(Kv3Error::TooDeep)));
    // A chain of flags nests too: an error, not a stack overflow.
    let flags = "a:".repeat(200_000) + "1";
    assert!(matches!(text::parse(&flags), Err(Kv3Error::TooDeep)));
    // `-nan` reads (Rust writes NaN without a sign, other tools with one).
    assert!(matches!(text::parse("-nan").map(|f| f.root), Ok(Value::Float(f)) if f.is_nan()));
}
