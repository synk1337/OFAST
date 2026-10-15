//! The outer layer of a `.dem` file: the 16-byte file header and the stream of **frames**.
//!
//! ```text
//! "PBDEMS2\0"  file-info offset (u32)  spawn-groups offset (u32)
//! frame: command (varint) · tick (varint) · size (varint) · payload [size bytes]
//! frame: …
//! ```
//!
//! Each frame's command is an `EDemoCommands` value (see `proto/demo.proto`); if bit 64 (`DEM_IsCompressed`) is set,
//! the payload is Snappy-compressed (raw block format, no framing). The payload is the protobuf message that matches
//! the command: `CDemoFileHeader` for `DEM_FileHeader`, `CDemoPacket` for `DEM_Packet`, and so on.
//!
//! See `docs/formats/demo.md` for the full description and sources.

use std::borrow::Cow;

use crate::bytes::{ByteReader, UnexpectedEof};
use crate::demo::proto::EDemoCommands;

/// The 8 magic bytes every CS2 (Source 2) demo starts with.
pub const MAGIC: &[u8; 8] = b"PBDEMS2\0";

/// Size of the file header: magic + two `u32` offsets.
pub const FILE_HEADER_SIZE: usize = 16;

/// Bit set in a frame's command when its payload is Snappy-compressed.
const COMPRESSED_FLAG: u32 = EDemoCommands::DemIsCompressed as u32;

/// Tick value written for frames that happen before the game simulation starts (the "signon" phase).
const SIGNON_TICK: u32 = u32::MAX;

/// Errors from the frame layer.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The file does not start with [`MAGIC`] (it may be a CS:GO demo, `HL2DEMO`, or not a demo at all).
    #[error("not a CS2 demo: expected magic \"PBDEMS2\\0\", found {found:?}")]
    BadMagic {
        /// The first bytes of the file.
        found: Vec<u8>,
    },
    /// The file ended in the middle of a frame.
    #[error(transparent)]
    Truncated(#[from] UnexpectedEof),
    /// A compressed payload is not valid Snappy data.
    #[error("frame at offset {offset}: snappy decompression failed: {source}")]
    Snappy {
        /// File offset of the frame.
        offset: usize,
        /// The decoder's error.
        source: snap::Error,
    },
}

/// Largest output one byte of Snappy data can produce: a 3-byte copy element writes at most 64 bytes.
const SNAPPY_MAX_RATIO: usize = 22;

/// Decompresses raw (unframed) Snappy data.
///
/// Snappy data starts with the decompressed length, and the decoder allocates that much up front. A demo is untrusted
/// input, so a length that the compressed bytes could not possibly produce (more than ~21× their size) is rejected
/// before allocating, instead of letting a 5-byte header ask for 4 GB.
pub(crate) fn decompress_snappy(input: &[u8]) -> Result<Vec<u8>, snap::Error> {
    let claimed = snap::raw::decompress_len(input)?;
    let max = input.len().saturating_mul(SNAPPY_MAX_RATIO);
    if claimed > max {
        return Err(snap::Error::TooBig {
            given: claimed as u64,
            max: max as u64,
        });
    }
    snap::raw::Decoder::new().decompress_vec(input)
}

/// The fixed 16 bytes at the start of a demo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHeader {
    /// File offset of the `DEM_FileInfo` frame near the end of the file (summary: duration, round start ticks).
    /// Zero if the recording was cut short and never finalized.
    pub file_info_offset: u32,
    /// File offset of the `DEM_SpawnGroups` frame (zero when absent).
    pub spawn_groups_offset: u32,
}

impl FileHeader {
    /// Checks the magic and reads the two offsets.
    pub fn parse(data: &[u8]) -> Result<Self, FrameError> {
        let mut r = ByteReader::new(data);
        let magic = r.take(MAGIC.len().min(data.len()), "demo magic")?;
        if magic != MAGIC {
            return Err(FrameError::BadMagic {
                found: magic.to_vec(),
            });
        }
        Ok(Self {
            file_info_offset: r.u32("file info offset")?,
            spawn_groups_offset: r.u32("spawn groups offset")?,
        })
    }
}

/// One frame of the demo.
#[derive(Debug, Clone)]
pub struct Frame<'a> {
    /// File offset of the frame's first byte (useful in error messages).
    pub offset: usize,
    /// The command with the compression flag removed, as a raw number (newer CS2 builds may add commands that
    /// `proto/demo.proto` does not know yet). Use [`Frame::command`] for the typed value.
    pub raw_command: u32,
    /// Game tick the frame belongs to; `None` for frames recorded before the simulation started (signon).
    pub tick: Option<u32>,
    /// The protobuf payload, already decompressed.
    pub payload: Cow<'a, [u8]>,
}

impl Frame<'_> {
    /// The command as a known `EDemoCommands` value, or `None` for commands this build of OFAST does not know.
    pub fn command(&self) -> Option<EDemoCommands> {
        i32::try_from(self.raw_command)
            .ok()
            .and_then(|c| EDemoCommands::try_from(c).ok())
    }
}

/// Iterator over the frames of a demo, starting right after the file header.
///
/// Stops after the `DEM_Stop` frame or at the end of the data, whichever comes first. A truncated last frame is
/// reported as an error (demos of matches that crashed are often cut off) and ends the iteration.
#[derive(Debug, Clone)]
pub struct Frames<'a> {
    reader: ByteReader<'a>,
    done: bool,
}

impl<'a> Frames<'a> {
    /// Iterates over the frames of a whole demo file (checks the magic first).
    pub fn new(data: &'a [u8]) -> Result<Self, FrameError> {
        FileHeader::parse(data)?;
        let mut reader = ByteReader::new(data);
        reader.seek(FILE_HEADER_SIZE, "first frame")?;
        Ok(Self {
            reader,
            done: false,
        })
    }

    /// Iterates over frames starting at an absolute file offset (e.g. [`FileHeader::file_info_offset`]).
    pub fn at(data: &'a [u8], offset: usize) -> Result<Self, FrameError> {
        FileHeader::parse(data)?;
        let mut reader = ByteReader::new(data);
        reader.seek(offset, "frame offset")?;
        Ok(Self {
            reader,
            done: false,
        })
    }

    fn read_frame(&mut self) -> Result<Frame<'a>, FrameError> {
        let offset = self.reader.position();
        let command = self.reader.varint_u32("frame command")?;
        let tick = self.reader.varint_u32("frame tick")?;
        let size = self.reader.varint_u32("frame size")? as usize;
        let raw = self.reader.take(size, "frame payload")?;
        let payload = if command & COMPRESSED_FLAG != 0 {
            let decompressed =
                decompress_snappy(raw).map_err(|source| FrameError::Snappy { offset, source })?;
            Cow::Owned(decompressed)
        } else {
            Cow::Borrowed(raw)
        };
        Ok(Frame {
            offset,
            raw_command: command & !COMPRESSED_FLAG,
            tick: (tick != SIGNON_TICK).then_some(tick),
            payload,
        })
    }
}

impl<'a> Iterator for Frames<'a> {
    type Item = Result<Frame<'a>, FrameError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.reader.remaining() == 0 {
            return None;
        }
        let frame = self.read_frame();
        match &frame {
            Ok(f) if f.raw_command == EDemoCommands::DemStop as u32 => self.done = true,
            Err(_) => self.done = true,
            Ok(_) => {}
        }
        Some(frame)
    }
}

/// Builds demo files byte by byte for tests, following the layout in the module docs.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Appends an unsigned varint to `out`.
    pub(crate) fn push_varint(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    /// A demo under construction: file header followed by frames.
    pub(crate) struct DemoBuilder {
        pub(crate) bytes: Vec<u8>,
    }

    impl DemoBuilder {
        pub(crate) fn new() -> Self {
            let mut bytes = MAGIC.to_vec();
            bytes.extend_from_slice(&[0; 8]);
            Self { bytes }
        }

        /// Appends a frame; `tick = None` writes the signon tick. `compress` Snappy-compresses the payload.
        pub(crate) fn frame(
            &mut self,
            command: EDemoCommands,
            tick: Option<u32>,
            payload: &[u8],
            compress: bool,
        ) {
            let mut cmd = command as u32;
            let body = if compress {
                cmd |= COMPRESSED_FLAG;
                snap::raw::Encoder::new()
                    .compress_vec(payload)
                    .expect("snappy compress")
            } else {
                payload.to_vec()
            };
            push_varint(&mut self.bytes, u64::from(cmd));
            push_varint(&mut self.bytes, u64::from(tick.unwrap_or(SIGNON_TICK)));
            push_varint(&mut self.bytes, body.len() as u64);
            self.bytes.extend_from_slice(&body);
        }

        /// Writes the offset of the next frame into the header's file-info slot.
        pub(crate) fn mark_file_info(&mut self) {
            let offset = self.bytes.len() as u32;
            self.bytes[8..12].copy_from_slice(&offset.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::DemoBuilder;
    use super::*;

    #[test]
    fn rejects_wrong_magic() {
        let err = FileHeader::parse(b"HL2DEMO\0\0\0\0\0\0\0\0\0").unwrap_err();
        assert!(matches!(err, FrameError::BadMagic { .. }), "{err}");
        assert!(FileHeader::parse(b"PBD").is_err());
    }

    #[test]
    fn reads_plain_and_compressed_frames() {
        let mut demo = DemoBuilder::new();
        demo.frame(EDemoCommands::DemFileHeader, None, b"header", false);
        demo.frame(EDemoCommands::DemPacket, Some(42), &[7u8; 300], true);
        demo.mark_file_info();
        demo.frame(EDemoCommands::DemStop, Some(43), b"", false);
        demo.frame(EDemoCommands::DemPacket, Some(44), b"after stop", false);

        let header = FileHeader::parse(&demo.bytes).unwrap();
        let frames: Vec<_> = Frames::new(&demo.bytes)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(frames.len(), 3, "iteration ends at DEM_Stop");
        assert_eq!(frames[0].command(), Some(EDemoCommands::DemFileHeader));
        assert_eq!(frames[0].tick, None);
        assert_eq!(&*frames[0].payload, b"header");
        assert_eq!(frames[1].command(), Some(EDemoCommands::DemPacket));
        assert_eq!(frames[1].tick, Some(42));
        assert_eq!(&*frames[1].payload, &[7u8; 300]);
        assert_eq!(frames[2].offset, header.file_info_offset as usize);
    }

    #[test]
    fn snappy_length_bomb_is_rejected() {
        // Header claims 4 GB (varint 0xFFFF_FFFF) but only a few bytes follow.
        let bomb = [0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x00];
        assert!(matches!(
            decompress_snappy(&bomb),
            Err(snap::Error::TooBig { .. })
        ));
        let ok = snap::raw::Encoder::new()
            .compress_vec(&[0u8; 10_000])
            .unwrap();
        assert_eq!(decompress_snappy(&ok).unwrap(), vec![0u8; 10_000]);
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let mut demo = DemoBuilder::new();
        demo.frame(EDemoCommands::DemPacket, Some(1), b"0123456789", false);
        demo.bytes.truncate(demo.bytes.len() - 3);
        let mut frames = Frames::new(&demo.bytes).unwrap();
        assert!(matches!(frames.next(), Some(Err(FrameError::Truncated(_)))));
        assert!(frames.next().is_none());
    }
}
