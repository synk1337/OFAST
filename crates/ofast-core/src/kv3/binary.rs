//! Decoder for binary KeyValues3, the encoding used inside compiled Source 2 resources.
//!
//! # How binary KV3 is laid out (summary — full details in `docs/formats/kv3.md`)
//!
//! Instead of writing values one after another, binary KV3 sorts them into separate **lanes** by size:
//!
//! - a **1-byte lane** (bools, int8, small array lengths),
//! - a **2-byte lane** (int16; version 4+),
//! - a **4-byte lane** (int32, float, string indices, array/object counts, member name indices),
//! - an **8-byte lane** (int64, double),
//! - a **string table** (every distinct string once),
//! - a **type stream** (one byte per value saying what it is, plus optional flag bytes),
//! - **blobs** (binary data, possibly compressed separately).
//!
//! Decoding walks the type stream and pulls each value's payload from the lane matching its size. Each lane is read
//! strictly in order, so the order of values in the type stream fully determines where every payload is.
//!
//! Versions 1–4 put all lanes in one buffer. Version 5 uses two: an **auxiliary** buffer (string table and payloads
//! of "auxiliary" typed arrays) and a **main** buffer (everything else, plus a table of object member counts).
//! Buffers are stored raw, LZ4-compressed (raw blocks) or ZSTD-compressed.
//!
//! The decoder is deliberately strict: after decoding it reports any lane bytes that were *not* consumed
//! ([`DecodeReport`]). A correct understanding of the format consumes every byte, which is how the tests in
//! `tests/cs2_kv3.rs` validate this module against thousands of real CS2 files.

use std::borrow::Cow;

use super::{Flag, Guid, Kv3File, Object, Value};
use crate::bytes::{ByteReader, UnexpectedEof};

/// Magic of the legacy (pre-versioned) binary format: bytes `VKV\x03`.
pub const MAGIC_LEGACY: u32 = 0x0356_4B56;
/// Versioned magics are `0x4B5633nn` (bytes `nn 3 V K`), where `nn` is the version.
const MAGIC_VERSIONED_MASK: u32 = 0xFFFF_FF00;
const MAGIC_VERSIONED: u32 = 0x4B56_3300;
/// Newest version this decoder understands.
pub const MAX_VERSION: u8 = 5;

/// Value that ends the main buffer (and follows blob data).
const END_MARKER: u32 = 0xFFEE_DD00;

/// Largest LZ4 back-reference distance: the dictionary for chained blob chunks never needs more.
const LZ4_WINDOW: usize = 64 * 1024;

/// Everything that can go wrong while decoding binary KV3.
#[derive(Debug, thiserror::Error)]
pub enum Kv3Error {
    /// The data ended in the middle of a structure.
    #[error(transparent)]
    UnexpectedEof(#[from] UnexpectedEof),
    /// The block does not start with a known KV3 magic.
    #[error("not binary KV3 (magic 0x{0:08X})")]
    BadMagic(u32),
    /// The legacy `VKV\x03` encoding is recognized but not implemented yet.
    #[error("legacy VKV3 binary encoding is not supported yet")]
    LegacyUnsupported,
    /// A newer KV3 version than this decoder knows.
    #[error("KV3 version {0} is not supported (newest known is {MAX_VERSION})")]
    UnsupportedVersion(u8),
    /// An unknown compression method.
    #[error("unknown KV3 compression method {0}")]
    UnknownCompression(u32),
    /// A non-zero compression dictionary id (meaning unknown; the reference reader rejects it too).
    #[error("KV3 compression dictionary {0} is not supported")]
    CompressionDictionary(u16),
    /// LZ4 decompression failed.
    #[error("LZ4 decompression failed: {0}")]
    Lz4(String),
    /// ZSTD decompression failed.
    #[error("ZSTD decompression failed: {0}")]
    Zstd(String),
    /// A decompressed buffer is not the size the header announced.
    #[error("decompressed {what} is {actual} bytes, header says {expected}")]
    SizeMismatch {
        /// Which buffer.
        what: &'static str,
        /// Size from the header.
        expected: usize,
        /// Size actually produced.
        actual: usize,
    },
    /// A size or count in the header is negative or does not fit the buffer.
    #[error("invalid KV3 header field {0}")]
    BadHeader(&'static str),
    /// The end marker `0xFFEEDD00` was not where expected.
    #[error("KV3 end marker missing (found 0x{0:08X})")]
    MissingEndMarker(u32),
    /// The type stream contains a type this decoder does not know.
    #[error("unknown KV3 value type {0}")]
    UnknownType(u8),
    /// A string in the string table is not UTF-8.
    #[error("KV3 string table entry {0} is not valid UTF-8")]
    InvalidUtf8(usize),
    /// A blob was referenced but no blob data is left.
    #[error("KV3 blob {0} is missing")]
    MissingBlob(usize),
    /// Arrays nested deeper than [`MAX_DEPTH`] (protects against malicious files).
    #[error("KV3 values are nested too deeply")]
    TooDeep,
    /// The text encoding is malformed ([`super::text::parse`]).
    #[error("KV3 text, line {line}: {message}")]
    Text {
        /// 1-based line of the problem.
        line: usize,
        /// What was expected or found.
        message: String,
    },
}

/// Maximum nesting depth accepted (real files stay far below this).
pub const MAX_DEPTH: usize = 512;

/// How a KV3 block was stored, and what (if anything) was left over after decoding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodeReport {
    /// Format version (1–5).
    pub version: u8,
    /// Compression method: 0 none, 1 LZ4, 2 ZSTD.
    pub compression: u32,
    /// Unconsumed bytes per main lane `[1-byte, 2-byte, 4-byte, 8-byte]`.
    pub leftover_main: [usize; 4],
    /// Unconsumed bytes per auxiliary lane (version 5).
    pub leftover_aux: [usize; 4],
    /// Unconsumed type-stream bytes.
    pub leftover_types: usize,
    /// Unconsumed object member counts (version 5).
    pub leftover_object_counts: usize,
    /// Unconsumed blobs.
    pub leftover_blobs: usize,
    /// Unconsumed bytes of the LZ4 blob chunk-size list.
    pub leftover_chunk_sizes: usize,
    /// For blocks with blobs: whether the end marker after the blob data was found (`None` without blobs).
    pub blob_marker_ok: Option<bool>,
    /// Bytes left in the block after everything was read.
    pub trailing_bytes: usize,
}

impl DecodeReport {
    /// `true` when every byte of every lane was consumed.
    pub fn is_exact(&self) -> bool {
        self.leftover_main == [0; 4]
            && self.leftover_aux == [0; 4]
            && self.leftover_types == 0
            && self.leftover_object_counts == 0
            && self.leftover_blobs == 0
            && self.leftover_chunk_sizes == 0
    }
}

/// Decodes a binary KV3 block (starting at its magic number).
pub fn decode(block: &[u8]) -> Result<Kv3File, Kv3Error> {
    decode_with_report(block).map(|(file, _)| file)
}

/// `true` when `block` starts with a binary KV3 magic (any version, including legacy).
pub fn is_kv3(block: &[u8]) -> bool {
    block.len() >= 4 && {
        let magic = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        magic == MAGIC_LEGACY || magic & MAGIC_VERSIONED_MASK == MAGIC_VERSIONED
    }
}

/// Decodes a binary KV3 block and also returns a [`DecodeReport`].
pub fn decode_with_report(block: &[u8]) -> Result<(Kv3File, DecodeReport), Kv3Error> {
    let mut r = ByteReader::new(block);
    let magic = r.u32("KV3 magic")?;
    let version = match magic {
        MAGIC_LEGACY => return Err(Kv3Error::LegacyUnsupported),
        m if m & MAGIC_VERSIONED_MASK == MAGIC_VERSIONED => (m & 0xFF) as u8,
        other => return Err(Kv3Error::BadMagic(other)),
    };
    if version == 0 || version > MAX_VERSION {
        return Err(Kv3Error::UnsupportedVersion(version));
    }

    let header = Header::parse(&mut r, version)?;
    let buffers = Buffers::read(&mut r, &header)?;
    let mut decoder = Decoder::new(&header, &buffers)?;
    let blob_marker_ok = decoder.load_blobs(&mut r, &header)?;
    let root = decoder.read_value(0)?;

    let mut report = decoder.report();
    report.version = version;
    report.compression = header.compression;
    report.blob_marker_ok = blob_marker_ok;
    report.trailing_bytes = r.remaining();
    Ok((
        Kv3File {
            format: header.format,
            root,
        },
        report,
    ))
}

/// Reads an `i32` header count and rejects negative values.
fn count(r: &mut ByteReader<'_>, what: &'static str) -> Result<usize, Kv3Error> {
    let value = r.i32(what)?;
    usize::try_from(value).map_err(|_| Kv3Error::BadHeader(what))
}

/// Parsed binary KV3 header. Fields that do not exist in a version are 0.
#[derive(Debug, Default)]
struct Header {
    version: u8,
    format: Guid,
    compression: u32,
    frame_size: usize,
    /// Lane element counts for the main buffer (v1–v4: the only buffer; v5: buffer 1 = auxiliary).
    bytes1: usize,
    bytes2: usize,
    bytes4: usize,
    bytes8: usize,
    count_types: usize,
    size_uncompressed: usize,
    size_compressed: usize,
    count_blobs: usize,
    size_blob_bytes: usize,
    /// v4+ header field at offset 68 — meaning unknown (always 0 in CS2). Kept for debugging.
    #[allow(dead_code)]
    unknown_v4_field: usize,
    // Version 5 only.
    size_uncompressed_buf1: usize,
    size_compressed_buf1: usize,
    size_uncompressed_buf2: usize,
    size_compressed_buf2: usize,
    bytes1_buf2: usize,
    bytes2_buf2: usize,
    bytes4_buf2: usize,
    bytes8_buf2: usize,
    count_objects_buf2: usize,
}

impl Header {
    fn parse(r: &mut ByteReader<'_>, version: u8) -> Result<Self, Kv3Error> {
        let mut h = Header {
            version,
            format: Guid(r.array("KV3 format GUID")?),
            compression: r.u32("KV3 compression method")?,
            ..Header::default()
        };
        if h.compression > 2 {
            return Err(Kv3Error::UnknownCompression(h.compression));
        }

        if version == 1 {
            h.bytes1 = count(r, "KV3 1-byte count")?;
            h.bytes4 = count(r, "KV3 4-byte count")?;
            h.bytes8 = count(r, "KV3 8-byte count")?;
            h.size_uncompressed = count(r, "KV3 uncompressed size")?;
            return Ok(h);
        }

        let dictionary = r.u16("KV3 compression dictionary")?;
        if dictionary != 0 {
            return Err(Kv3Error::CompressionDictionary(dictionary));
        }
        h.frame_size = r.u16("KV3 compression frame size")? as usize;
        h.bytes1 = count(r, "KV3 1-byte count")?;
        h.bytes4 = count(r, "KV3 4-byte count")?;
        h.bytes8 = count(r, "KV3 8-byte count")?;
        h.count_types = count(r, "KV3 type count")?;
        // u16 object / array counts: allocation hints only (they saturate in v5), not needed to decode.
        r.skip(4, "KV3 object and array counts")?;
        h.size_uncompressed = count(r, "KV3 uncompressed size")?;
        h.size_compressed = count(r, "KV3 compressed size")?;
        h.count_blobs = count(r, "KV3 blob count")?;
        h.size_blob_bytes = count(r, "KV3 blob size")?;

        if version >= 4 {
            h.bytes2 = count(r, "KV3 2-byte count")?;
            // Documented elsewhere as the size of the LZ4 chunk-size list, but 0 in every real CS2 file even with
            // LZ4 blobs (see docs/formats/kv3.md → Unknowns). Read for completeness, not used to decode.
            h.unknown_v4_field = count(r, "KV3 v4 header field at offset 68")?;
        }
        if version >= 5 {
            h.size_uncompressed_buf1 = count(r, "KV3 buffer 1 uncompressed size")?;
            h.size_compressed_buf1 = count(r, "KV3 buffer 1 compressed size")?;
            h.size_uncompressed_buf2 = count(r, "KV3 buffer 2 uncompressed size")?;
            h.size_compressed_buf2 = count(r, "KV3 buffer 2 compressed size")?;
            h.bytes1_buf2 = count(r, "KV3 buffer 2 1-byte count")?;
            h.bytes2_buf2 = count(r, "KV3 buffer 2 2-byte count")?;
            h.bytes4_buf2 = count(r, "KV3 buffer 2 4-byte count")?;
            h.bytes8_buf2 = count(r, "KV3 buffer 2 8-byte count")?;
            r.skip(4, "KV3 node count (hint)")?;
            h.count_objects_buf2 = count(r, "KV3 buffer 2 object count")?;
            r.skip(8, "KV3 array and element counts (hints)")?;
        }
        Ok(h)
    }
}

/// The decompressed buffers of one KV3 block.
struct Buffers {
    /// v1–v4: the only buffer. v5: buffer 2 ("main").
    main: Vec<u8>,
    /// v5: buffer 1 ("auxiliary").
    aux: Option<Vec<u8>>,
    /// Blob bytes when they were stored raw or ZSTD-compressed alongside the buffers. LZ4 blobs are decompressed
    /// later, once the chunk sizes at the end of the main buffer are known.
    blobs: Option<Vec<u8>>,
}

impl Buffers {
    fn read(r: &mut ByteReader<'_>, h: &Header) -> Result<Self, Kv3Error> {
        if h.version == 1 {
            let rest = r.take(r.remaining(), "KV3 v1 data")?;
            let main = match h.compression {
                0 => rest
                    .get(..h.size_uncompressed)
                    .ok_or(Kv3Error::BadHeader("KV3 uncompressed size"))?
                    .to_vec(),
                1 => lz4(rest, h.size_uncompressed, "KV3 buffer")?,
                _ => zstd(rest, h.size_uncompressed, "KV3 buffer")?,
            };
            return Ok(Self {
                main,
                aux: None,
                blobs: None,
            });
        }

        let has_blobs = h.count_blobs > 0;
        if h.version < 5 {
            return match h.compression {
                0 => Ok(Self {
                    main: {
                        // Uncompressed: both sizes must agree, or every lane offset would be wrong.
                        check_size(
                            "KV3 uncompressed buffer",
                            h.size_uncompressed,
                            h.size_compressed,
                        )?;
                        r.take(h.size_compressed, "KV3 buffer")?.to_vec()
                    },
                    aux: None,
                    blobs: has_blobs
                        .then(|| r.take(h.size_blob_bytes, "KV3 blobs").map(<[u8]>::to_vec))
                        .transpose()?,
                }),
                1 => Ok(Self {
                    main: lz4(
                        r.take(h.size_compressed, "KV3 buffer")?,
                        h.size_uncompressed,
                        "KV3 buffer",
                    )?,
                    aux: None,
                    blobs: None,
                }),
                _ => {
                    // Buffer and blobs are consecutive ZSTD frames, decompressed together.
                    let total = h.size_uncompressed + h.size_blob_bytes;
                    let mut all = zstd(
                        r.take(h.size_compressed, "KV3 buffer")?,
                        total,
                        "KV3 buffer + blobs",
                    )?;
                    let blobs = all.split_off(h.size_uncompressed);
                    Ok(Self {
                        main: all,
                        aux: None,
                        blobs: has_blobs.then_some(blobs),
                    })
                }
            };
        }

        // Version 5: auxiliary buffer, then main buffer, then blobs.
        let (aux, main, blobs) = match h.compression {
            0 => (
                r.take(h.size_uncompressed_buf1, "KV3 buffer 1")?.to_vec(),
                r.take(h.size_uncompressed_buf2, "KV3 buffer 2")?.to_vec(),
                has_blobs
                    .then(|| r.take(h.size_blob_bytes, "KV3 blobs").map(<[u8]>::to_vec))
                    .transpose()?,
            ),
            1 => (
                lz4(
                    r.take(h.size_compressed_buf1, "KV3 buffer 1")?,
                    h.size_uncompressed_buf1,
                    "KV3 buffer 1",
                )?,
                lz4(
                    r.take(h.size_compressed_buf2, "KV3 buffer 2")?,
                    h.size_uncompressed_buf2,
                    "KV3 buffer 2",
                )?,
                None,
            ),
            _ => {
                let aux = zstd(
                    r.take(h.size_compressed_buf1, "KV3 buffer 1")?,
                    h.size_uncompressed_buf1,
                    "KV3 buffer 1",
                )?;
                let main = zstd(
                    r.take(h.size_compressed_buf2, "KV3 buffer 2")?,
                    h.size_uncompressed_buf2,
                    "KV3 buffer 2",
                )?;
                let blobs = if has_blobs {
                    let rest = h
                        .size_compressed
                        .checked_sub(h.size_compressed_buf1 + h.size_compressed_buf2)
                        .ok_or(Kv3Error::BadHeader("KV3 compressed size"))?;
                    Some(zstd(
                        r.take(rest, "KV3 blobs")?,
                        h.size_blob_bytes,
                        "KV3 blobs",
                    )?)
                } else {
                    None
                };
                (aux, main, blobs)
            }
        };
        Ok(Self {
            main,
            aux: Some(aux),
            blobs,
        })
    }
}

/// Decompresses one raw LZ4 block that must expand to exactly `size` bytes.
fn lz4(input: &[u8], size: usize, what: &'static str) -> Result<Vec<u8>, Kv3Error> {
    let out = lz4_flex::block::decompress(input, size).map_err(|e| Kv3Error::Lz4(e.to_string()))?;
    check_size(what, size, out.len())?;
    Ok(out)
}

/// Decompresses one or more consecutive ZSTD frames that must expand to exactly `size` bytes.
fn zstd(input: &[u8], size: usize, what: &'static str) -> Result<Vec<u8>, Kv3Error> {
    use std::io::Read;
    let mut out = Vec::with_capacity(size);
    let mut rest = input;
    // Each StreamingDecoder handles one frame; loop for consecutive frames.
    while !rest.is_empty() {
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&mut rest)
            .map_err(|e| Kv3Error::Zstd(e.to_string()))?;
        decoder
            .read_to_end(&mut out)
            .map_err(|e| Kv3Error::Zstd(e.to_string()))?;
    }
    check_size(what, size, out.len())?;
    Ok(out)
}

fn check_size(what: &'static str, expected: usize, actual: usize) -> Result<(), Kv3Error> {
    if expected == actual {
        Ok(())
    } else {
        Err(Kv3Error::SizeMismatch {
            what,
            expected,
            actual,
        })
    }
}

/// Readers for the four fixed-width lanes of one buffer.
#[derive(Debug, Clone)]
struct Lanes<'a> {
    b1: ByteReader<'a>,
    b2: ByteReader<'a>,
    b4: ByteReader<'a>,
    b8: ByteReader<'a>,
}

impl<'a> Lanes<'a> {
    /// Splits `buf[start..]` into lanes of `[count1, count2, count4, count8]` *elements*.
    ///
    /// Each lane is aligned to its element size relative to the buffer start. `pad_empty` controls whether the
    /// alignment padding is inserted even before an empty lane (true for versions 1–4, false for version 5).
    /// Returns the lanes and the offset just after the 8-byte lane.
    fn split(
        buf: &'a [u8],
        start: usize,
        counts: [usize; 4],
        pad_empty: bool,
    ) -> Result<(Self, usize), Kv3Error> {
        let mut pos = start;
        let mut lane =
            |width: usize, count: usize, what: &'static str| -> Result<ByteReader<'a>, Kv3Error> {
                if count > 0 || pad_empty {
                    pos = pos.next_multiple_of(width);
                }
                let len = count.checked_mul(width).ok_or(Kv3Error::BadHeader(what))?;
                let bytes = buf.get(pos..pos + len).ok_or(UnexpectedEof {
                    what,
                    offset: pos,
                    needed: len,
                    available: buf.len().saturating_sub(pos),
                })?;
                pos += len;
                Ok(ByteReader::new(bytes))
            };
        let b1 = lane(1, counts[0], "KV3 1-byte lane")?;
        let b2 = lane(2, counts[1], "KV3 2-byte lane")?;
        let b4 = lane(4, counts[2], "KV3 4-byte lane")?;
        let b8 = lane(8, counts[3], "KV3 8-byte lane")?;
        Ok((Self { b1, b2, b4, b8 }, pos))
    }

    fn leftovers(&self) -> [usize; 4] {
        [
            self.b1.remaining(),
            self.b2.remaining(),
            self.b4.remaining(),
            self.b8.remaining(),
        ]
    }
}

/// Which buffer's lanes a primitive value is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Main,
    Aux,
}

/// Walks the type stream and rebuilds the value tree.
struct Decoder<'a> {
    version: u8,
    main: Lanes<'a>,
    aux: Option<Lanes<'a>>,
    types: ByteReader<'a>,
    /// Version 5: member count of each object, in traversal order.
    object_counts: Option<ByteReader<'a>>,
    strings: Vec<&'a str>,
    /// Lengths of the blobs, in traversal order (versions 2+).
    blob_lengths: Vec<usize>,
    /// Main-buffer tail after the end marker: u16 compressed sizes of LZ4 blob chunks.
    blob_chunk_sizes: ByteReader<'a>,
    /// All blob bytes, concatenated (borrowed when stored raw/ZSTD, owned when decompressed from LZ4 chunks).
    blob_data: Cow<'a, [u8]>,
    /// Number of blobs read so far.
    blobs_read: usize,
    /// Offset of the next blob in `blob_data`.
    blob_cursor: usize,
}

impl<'a> Decoder<'a> {
    fn new(h: &Header, buffers: &'a Buffers) -> Result<Self, Kv3Error> {
        let main_buf = buffers.main.as_slice();

        // --- Lanes, string table and type stream -------------------------------------------------------------
        let (main, aux, strings, types_start, types_len, object_counts) = if h.version < 5 {
            let counts = [h.bytes1, h.bytes2, h.bytes4, h.bytes8];
            let (mut main, lanes_end) = Lanes::split(main_buf, 0, counts, true)?;
            let string_count = main.b4.i32("KV3 string count")?;
            let mut table = ByteReader::new(main_buf);
            table.seek(lanes_end, "KV3 string table")?;
            let strings = read_strings(&mut table, string_count)?;
            let strings_len = table.position() - lanes_end;
            let types_len = if h.version == 1 {
                // v1: types run until the 4-byte end marker.
                main_buf
                    .len()
                    .checked_sub(table.position() + 4)
                    .ok_or(Kv3Error::BadHeader("KV3 v1 type stream size"))?
            } else {
                // v2–v4: the header's type count includes the string table bytes.
                h.count_types
                    .checked_sub(strings_len)
                    .ok_or(Kv3Error::BadHeader("KV3 type count"))?
            };
            (main, None, strings, table.position(), types_len, None)
        } else {
            let aux_buf = buffers.aux.as_deref().unwrap_or_default();
            let (mut aux, _) =
                Lanes::split(aux_buf, 0, [h.bytes1, h.bytes2, h.bytes4, h.bytes8], false)?;
            // The string table sits at the start of the auxiliary 1-byte lane; its count is the first aux int.
            let string_count = aux.b4.i32("KV3 string count")?;
            let strings = read_strings(&mut aux.b1, string_count)?;

            let counts_len = h
                .count_objects_buf2
                .checked_mul(4)
                .ok_or(Kv3Error::BadHeader("KV3 object count"))?;
            let object_counts =
                ByteReader::new(main_buf.get(..counts_len).ok_or(UnexpectedEof {
                    what: "KV3 object member counts",
                    offset: 0,
                    needed: counts_len,
                    available: main_buf.len(),
                })?);
            let counts = [h.bytes1_buf2, h.bytes2_buf2, h.bytes4_buf2, h.bytes8_buf2];
            let (main, lanes_end) = Lanes::split(main_buf, counts_len, counts, false)?;
            (
                main,
                Some(aux),
                strings,
                lanes_end,
                h.count_types,
                Some(object_counts),
            )
        };

        let mut tail = ByteReader::new(main_buf);
        tail.seek(types_start, "KV3 type stream")?;
        let types = ByteReader::new(tail.take(types_len, "KV3 type stream")?);

        // --- Tail of the main buffer: blob lengths, end marker, LZ4 chunk sizes -------------------------------
        let mut blob_lengths = Vec::new();
        if h.version >= 2 {
            for _ in 0..h.count_blobs {
                blob_lengths.push(count(&mut tail, "KV3 blob length")?);
            }
        }
        let marker = tail.u32("KV3 end marker")?;
        if marker != END_MARKER {
            return Err(Kv3Error::MissingEndMarker(marker));
        }
        // The u16 LZ4 chunk sizes run to the end of the main buffer. (Real CS2 v4 files have them there while the
        // header field `size_block_compressed_sizes` is 0, so that field is not used to find them.)
        let blob_chunk_sizes =
            ByteReader::new(tail.take(tail.remaining(), "KV3 blob chunk sizes")?);

        Ok(Self {
            version: h.version,
            main,
            aux,
            types,
            object_counts,
            strings,
            blob_lengths,
            blob_chunk_sizes,
            blob_data: Cow::Borrowed(buffers.blobs.as_deref().unwrap_or_default()),
            blobs_read: 0,
            blob_cursor: 0,
        })
    }

    /// Prepares blob data before the tree is walked, and reads the end marker that follows blob data.
    ///
    /// LZ4 blobs are stored after the buffers as chunks of at most `frame_size` uncompressed bytes. Chunk `k`'s
    /// compressed size is the `k`-th u16 after the main buffer's end marker. Chunks are decoded as a chain, so a
    /// chunk may copy bytes from earlier output: the previously decoded bytes are passed as the LZ4 dictionary.
    ///
    /// Returns whether the blob end marker was present (`None` when there are no blobs).
    fn load_blobs(&mut self, r: &mut ByteReader<'_>, h: &Header) -> Result<Option<bool>, Kv3Error> {
        if h.version < 2 || h.count_blobs == 0 {
            return Ok(None);
        }
        if h.compression == 1 {
            if h.frame_size == 0 {
                return Err(Kv3Error::BadHeader("KV3 LZ4 frame size"));
            }
            let mut out = vec![0u8; h.size_blob_bytes];
            let mut pos = 0;
            for &len in &self.blob_lengths {
                let end = pos + len;
                if end > out.len() {
                    return Err(Kv3Error::BadHeader("KV3 blob size"));
                }
                while pos < end {
                    let chunk = (end - pos).min(h.frame_size);
                    let compressed_len = self.blob_chunk_sizes.u16("KV3 blob chunk size")? as usize;
                    let input = r.take(compressed_len, "KV3 compressed blob chunk")?;
                    let (before, after) = out.split_at_mut(pos);
                    let dict = &before[pos.saturating_sub(LZ4_WINDOW)..];
                    let written = lz4_flex::block::decompress_into_with_dict(
                        input,
                        &mut after[..chunk],
                        dict,
                    )
                    .map_err(|e| Kv3Error::Lz4(e.to_string()))?;
                    check_size("KV3 blob chunk", chunk, written)?;
                    pos += chunk;
                }
            }
            self.blob_data = Cow::Owned(out);
        }
        // The blob lengths must describe the blob data exactly; anything else means the lengths were misread.
        check_size(
            "KV3 blob data",
            self.blob_lengths.iter().sum(),
            self.blob_data.len(),
        )?;
        if r.remaining() < 4 {
            return Ok(Some(false));
        }
        let marker = r.u32("KV3 blob end marker")?;
        Ok(Some(marker == END_MARKER))
    }

    /// Reads a type byte (and its optional flag) from the type stream.
    fn read_type(&mut self) -> Result<(u8, Option<Flag>), Kv3Error> {
        let byte = self.types.u8("KV3 type")?;
        if self.version >= 3 {
            let flag = if byte & 0x80 != 0 {
                Some(flag_from_enum(self.types.u8("KV3 flag")?))
            } else {
                None
            };
            if byte & 0x40 != 0 {
                // Meaning unknown; the reference reader skips one byte. No known writer sets it.
                self.types.skip(1, "KV3 extra type byte")?;
            }
            Ok((byte & 0x3F, flag))
        } else {
            let flag = if byte & 0x80 != 0 {
                flag_from_bits(self.types.u8("KV3 flag bits")?)
            } else {
                None
            };
            Ok((byte & 0x7F, flag))
        }
    }

    /// Reads one complete value: type byte, optional flag, payload.
    fn read_value(&mut self, depth: usize) -> Result<Value, Kv3Error> {
        let (ty, flag) = self.read_type()?;
        let value = self.read_payload(ty, Source::Main, depth)?;
        Ok(with_flag(flag, value))
    }

    fn lanes(&mut self, source: Source) -> &mut Lanes<'a> {
        match (source, self.aux.as_mut()) {
            (Source::Aux, Some(aux)) => aux,
            _ => &mut self.main,
        }
    }

    /// Reads the payload of a value whose type is already known.
    fn read_payload(&mut self, ty: u8, source: Source, depth: usize) -> Result<Value, Kv3Error> {
        if depth > MAX_DEPTH {
            return Err(Kv3Error::TooDeep);
        }
        Ok(match ty {
            1 => Value::Null,
            2 => Value::Bool(self.lanes(source).b1.u8("KV3 bool")? != 0),
            3 => Value::Int(self.lanes(source).b8.i64("KV3 int64")?),
            4 => Value::UInt(self.lanes(source).b8.u64("KV3 uint64")?),
            5 => Value::Float(self.lanes(source).b8.f64("KV3 double")?),
            6 => {
                let index = self.main.b4.i32("KV3 string index")?;
                Value::String(self.string(index).to_owned())
            }
            7 => self.read_blob()?,
            8 => {
                let len = self.main.b4.i32("KV3 array length")?.max(0) as usize;
                let mut items = Vec::with_capacity(len.min(4096));
                for _ in 0..len {
                    items.push(self.read_value(depth + 1)?);
                }
                Value::Array(items)
            }
            9 => {
                let len = match self.object_counts.as_mut() {
                    Some(counts) => counts.i32("KV3 object member count")?,
                    None => self.main.b4.i32("KV3 object member count")?,
                }
                .max(0) as usize;
                let mut object = Object::new();
                for _ in 0..len {
                    let name_index = self.main.b4.i32("KV3 member name index")?;
                    let name = self.string(name_index).to_owned();
                    let value = self.read_value(depth + 1)?;
                    object.push(name, value);
                }
                Value::Object(object)
            }
            10 => {
                let len = self.main.b4.i32("KV3 typed array length")?.max(0) as usize;
                self.read_typed_array(len, Source::Main, depth)?
            }
            11 => Value::Int(self.lanes(source).b4.i32("KV3 int32")?.into()),
            12 => Value::UInt(self.lanes(source).b4.u32("KV3 uint32")?.into()),
            13 => Value::Bool(true),
            14 => Value::Bool(false),
            15 => Value::Int(0),
            16 => Value::Int(1),
            17 => Value::Float(0.0),
            18 => Value::Float(1.0),
            19 => Value::Float(self.lanes(source).b4.f32("KV3 float")?.into()),
            20 => Value::Int(self.lanes(source).b2.i16("KV3 int16")?.into()),
            21 => Value::UInt(self.lanes(source).b2.u16("KV3 uint16")?.into()),
            22 => Value::Int(self.lanes(source).b1.i8("KV3 int8")?.into()),
            23 => Value::UInt(self.lanes(source).b1.u8("KV3 uint8")?.into()),
            24 => {
                let len = self.main.b1.u8("KV3 short typed array length")? as usize;
                self.read_typed_array(len, Source::Main, depth)?
            }
            25 => {
                let len = self.main.b1.u8("KV3 auxiliary typed array length")? as usize;
                self.read_typed_array(len, Source::Aux, depth)?
            }
            26 => Value::UInt(0),
            27 => Value::UInt(1),
            other => return Err(Kv3Error::UnknownType(other)),
        })
    }

    /// A typed array: one shared element type, then `len` payloads with no per-element type bytes.
    fn read_typed_array(
        &mut self,
        len: usize,
        source: Source,
        depth: usize,
    ) -> Result<Value, Kv3Error> {
        let (ty, flag) = self.read_type()?;
        let mut items = Vec::with_capacity(len.min(4096));
        for _ in 0..len {
            items.push(with_flag(flag, self.read_payload(ty, source, depth + 1)?));
        }
        Ok(Value::Array(items))
    }

    fn string(&self, index: i32) -> &'a str {
        // -1 (or any out-of-range index) means the empty string.
        usize::try_from(index)
            .ok()
            .and_then(|i| self.strings.get(i).copied())
            .unwrap_or("")
    }

    fn read_blob(&mut self) -> Result<Value, Kv3Error> {
        if self.version == 1 {
            let len = self.main.b4.i32("KV3 v1 blob length")?.max(0) as usize;
            return Ok(Value::Blob(self.main.b1.take(len, "KV3 v1 blob")?.to_vec()));
        }
        let index = self.blobs_read;
        let len = *self
            .blob_lengths
            .get(index)
            .ok_or(Kv3Error::MissingBlob(index))?;
        let start = self.blob_cursor;
        let bytes = self
            .blob_data
            .get(start..start + len)
            .ok_or(Kv3Error::MissingBlob(index))?
            .to_vec();
        self.blobs_read += 1;
        self.blob_cursor += len;
        Ok(Value::Blob(bytes))
    }

    fn report(&self) -> DecodeReport {
        DecodeReport {
            leftover_main: self.main.leftovers(),
            leftover_aux: self.aux.as_ref().map(Lanes::leftovers).unwrap_or_default(),
            leftover_types: self.types.remaining(),
            leftover_object_counts: self.object_counts.as_ref().map_or(0, ByteReader::remaining),
            leftover_blobs: self.blob_lengths.len() - self.blobs_read,
            leftover_chunk_sizes: self.blob_chunk_sizes.remaining(),
            ..DecodeReport::default()
        }
    }
}

/// Reads `count` NUL-terminated UTF-8 strings.
fn read_strings<'a>(r: &mut ByteReader<'a>, count: i32) -> Result<Vec<&'a str>, Kv3Error> {
    let count = usize::try_from(count).map_err(|_| Kv3Error::BadHeader("KV3 string count"))?;
    let mut strings = Vec::with_capacity(count.min(65536));
    for index in 0..count {
        let bytes = r.cstr("KV3 string")?;
        strings.push(std::str::from_utf8(bytes).map_err(|_| Kv3Error::InvalidUtf8(index))?);
    }
    Ok(strings)
}

/// Version 3+: the flag byte is an enum.
fn flag_from_enum(value: u8) -> Flag {
    match value {
        1 => Flag::Resource,
        2 => Flag::ResourceName,
        3 => Flag::Panorama,
        4 => Flag::SoundEvent,
        5 => Flag::SubClass,
        6 => Flag::EntityName,
        7 => Flag::Localize,
        other => Flag::Unknown(other),
    }
}

/// Versions 1–2: the flag byte is a bit field; the lowest set bit wins and "multiline" (4) is dropped.
fn flag_from_bits(bits: u8) -> Option<Flag> {
    [
        (1, Some(Flag::Resource)),
        (2, Some(Flag::ResourceName)),
        (4, None),
        (8, Some(Flag::Panorama)),
        (16, Some(Flag::SoundEvent)),
        (32, Some(Flag::SubClass)),
    ]
    .into_iter()
    .find(|(bit, _)| bits & bit != 0)
    .and_then(|(_, flag)| flag)
}

fn with_flag(flag: Option<Flag>, value: Value) -> Value {
    match flag {
        Some(flag) => Value::Flagged(flag, Box::new(value)),
        None => value,
    }
}
