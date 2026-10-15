//! VPK v2 integrity data: the archive "chunk hashes" and the three MD5 checksums of the directory file.
//!
//! These sections make the format self-verifying. OFAST uses them to prove its reader is correct against real CS2
//! files without needing any third-party tool. See `docs/formats/vpk.md` → *Version 2 checksum sections*.

use md5::{Digest, Md5};

use super::{EMBEDDED_ARCHIVE, VpkError};
use crate::bytes::ByteReader;

/// Size of one record in the archive MD5 section.
const CHUNK_RECORD_SIZE: usize = 28;

/// Hash-type field value that, together with archive index 0, marks the directory file's embedded data (CS2).
const EMBEDDED_HASH_MARKER: u16 = 0x8000;

/// Which hash function a chunk hash uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashType {
    /// MD5 (16 bytes).
    Md5,
    /// BLAKE3, truncated to its first 16 bytes.
    Blake3,
    /// A hash type this reader does not know.
    Unknown(u16),
}

/// One record of the archive MD5 section: the hash of a byte range of one data archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHash {
    /// Archive number, or [`EMBEDDED_ARCHIVE`] for the directory file's embedded data.
    pub archive_index: u16,
    /// Hash function used.
    pub hash_type: HashType,
    /// Start of the range in that archive (for embedded data: relative to the end of the tree).
    pub offset: u32,
    /// Length of the range.
    pub length: u32,
    /// The expected hash (first 16 bytes).
    pub hash: [u8; 16],
}

impl ChunkHash {
    /// Parses every record of the archive MD5 section.
    pub(super) fn parse_all(section: &[u8]) -> Result<Vec<Self>, VpkError> {
        if !section.len().is_multiple_of(CHUNK_RECORD_SIZE) {
            return Err(VpkError::BadSectionSize {
                section: "archive MD5",
                size: section.len(),
            });
        }
        let mut r = ByteReader::new(section);
        let mut chunks = Vec::with_capacity(section.len() / CHUNK_RECORD_SIZE);
        while r.remaining() > 0 {
            let archive_index = r.u16("VPK chunk archive index")?;
            let raw_type = r.u16("VPK chunk hash type")?;
            let offset = r.u32("VPK chunk offset")?;
            let length = r.u32("VPK chunk length")?;
            let hash = r.array::<16>("VPK chunk hash")?;
            let (archive_index, hash_type) = match (archive_index, raw_type) {
                (0, EMBEDDED_HASH_MARKER) => (EMBEDDED_ARCHIVE, HashType::Md5),
                (index, 0) => (index, HashType::Md5),
                (index, 1) => (index, HashType::Blake3),
                (index, other) => (index, HashType::Unknown(other)),
            };
            chunks.push(Self {
                archive_index,
                hash_type,
                offset,
                length,
                hash,
            });
        }
        Ok(chunks)
    }
}

/// The 48-byte "other MD5" section: checksums of the tree, of the archive MD5 section, and of the whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtherChecksums {
    /// MD5 of the directory tree bytes.
    pub tree: [u8; 16],
    /// MD5 of the archive MD5 section bytes.
    pub archive_md5_section: [u8; 16],
    /// MD5 of everything from the start of the file up to (not including) this checksum.
    pub whole_file: [u8; 16],
}

impl OtherChecksums {
    /// Size of the section in bytes.
    pub const SIZE: usize = 48;

    /// Parses the section (which must be exactly [`Self::SIZE`] bytes).
    pub(super) fn parse(section: &[u8]) -> Result<Self, VpkError> {
        let mut r = ByteReader::new(section);
        Ok(Self {
            tree: r.array("VPK tree checksum")?,
            archive_md5_section: r.array("VPK archive MD5 section checksum")?,
            whole_file: r.array("VPK whole-file checksum")?,
        })
    }
}

/// MD5 of `data` as a plain array.
pub(super) fn md5(data: &[u8]) -> [u8; 16] {
    Md5::digest(data).into()
}

/// BLAKE3 of `data`, truncated to the 16 bytes a VPK chunk record has room for.
pub(super) fn blake3_16(data: &[u8]) -> [u8; 16] {
    let full = blake3::hash(data);
    full.as_bytes()[..16]
        .try_into()
        .expect("BLAKE3 output is 32 bytes")
}

/// Compares an expected and computed hash, naming the section on mismatch.
pub(super) fn check(
    section: &'static str,
    expected: [u8; 16],
    actual: [u8; 16],
) -> Result<(), VpkError> {
    if expected == actual {
        Ok(())
    } else {
        Err(VpkError::ChecksumMismatch {
            section,
            expected,
            actual,
        })
    }
}
