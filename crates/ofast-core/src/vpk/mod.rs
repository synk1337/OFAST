//! Reader for Valve Pak (VPK) archives — the containers CS2 ships almost all of its content in.
//!
//! A VPK is a **directory file** (`pak01_dir.vpk`, the table of contents) plus numbered **data archives**
//! (`pak01_000.vpk`, …) holding the raw bytes. Opening a VPK reads the whole directory file into memory (≈ 8 MB for
//! CS2) and parses its tree; file contents are only read on demand.
//!
//! The byte layout is documented in `docs/formats/vpk.md`. Knowledge comes from ValvePak (MIT) and the Valve
//! Developer Community wiki; the code here is an independent implementation.
//!
//! ```no_run
//! use ofast_core::vpk::Vpk;
//!
//! let vpk = Vpk::open(r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\csgo\pak01_dir.vpk")?;
//! let entry = vpk.find("scripts/items/items_game.txt").expect("file exists");
//! let bytes = vpk.read(entry)?; // CRC32-checked
//! # Ok::<(), ofast_core::vpk::VpkError>(())
//! ```

mod archives;
mod header;
mod tree;
mod verify;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

pub use archives::{ArchiveSource, FileArchives, MemoryArchives};
pub use header::{Header, SignatureInfo, SignatureLayout};
pub use verify::{ChunkHash, HashType, OtherChecksums};

use crate::bytes::{ByteReader, UnexpectedEof};

/// Magic number at the start of every VPK directory file (bytes `34 12 AA 55`).
pub const SIGNATURE: u32 = 0x55AA_1234;

/// Archive index meaning "this entry's data is embedded in the directory file, after the tree".
pub const EMBEDDED_ARCHIVE: u16 = 0x7FFF;

/// Value that must end every entry record in the tree.
const TERMINATOR: u16 = 0xFFFF;

/// Everything that can go wrong while reading a VPK.
#[derive(Debug, thiserror::Error)]
pub enum VpkError {
    /// Reading a file from disk failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The data ended in the middle of a structure.
    #[error(transparent)]
    UnexpectedEof(#[from] UnexpectedEof),
    /// The file does not start with the VPK magic number.
    #[error("not a VPK file (signature 0x{0:08X}, expected 0x55AA1234)")]
    BadSignature(u32),
    /// The header declares a version this reader does not support.
    #[error("unsupported VPK version 0x{0:X} (only 1 and 2 are supported)")]
    UnsupportedVersion(u32),
    /// The header's tree size points past the end of the file.
    #[error("VPK tree ends at byte {tree_end} but the file is only {file_len} bytes")]
    TreeOutOfBounds {
        /// Where the header says the tree ends.
        tree_end: usize,
        /// Actual size of the directory file.
        file_len: usize,
    },
    /// Parsing the tree did not end where the header said it would.
    #[error("VPK tree should end at byte {expected_end} but parsing ended at {actual_end}")]
    TreeSizeMismatch {
        /// End according to the header.
        expected_end: usize,
        /// End according to the parse.
        actual_end: usize,
    },
    /// An entry record did not end with `0xFFFF`.
    #[error("VPK entry `{path}` ends with 0x{found:04X} instead of 0xFFFF")]
    BadTerminator {
        /// Path of the broken entry.
        path: String,
        /// The value found instead of the terminator.
        found: u16,
    },
    /// A name in the tree is not valid UTF-8.
    #[error("VPK tree name at byte {offset} is not valid UTF-8")]
    InvalidUtf8 {
        /// Offset of the name in the directory file.
        offset: usize,
    },
    /// A v2 section has an impossible size.
    #[error("VPK {section} section has an invalid size of {size} bytes")]
    BadSectionSize {
        /// Which section.
        section: &'static str,
        /// Its declared size.
        size: usize,
    },
    /// An embedded entry or chunk points outside the directory file.
    #[error("VPK entry `{path}` points outside the directory file's embedded data")]
    OutOfBounds {
        /// Path of the entry (or a description of the chunk).
        path: String,
    },
    /// A file's contents do not match the CRC32 stored in the tree.
    #[error(
        "VPK file `{path}` failed its CRC32 check (expected 0x{expected:08X}, got 0x{actual:08X})"
    )]
    CrcMismatch {
        /// Path of the file.
        path: String,
        /// CRC stored in the tree.
        expected: u32,
        /// CRC of the bytes actually read.
        actual: u32,
    },
    /// One of the v2 MD5 checksums does not match.
    #[error("VPK {section} checksum mismatch (expected {expected:02x?}, got {actual:02x?})")]
    ChecksumMismatch {
        /// Which checksum failed.
        section: &'static str,
        /// Stored value.
        expected: [u8; 16],
        /// Computed value.
        actual: [u8; 16],
    },
    /// The requested hash type cannot be verified by this reader.
    #[error("VPK chunk uses hash type {0:?}, which OFAST cannot verify yet")]
    UnsupportedHash(HashType),
    /// No file with this path exists in the VPK.
    #[error("file `{0}` is not in this VPK")]
    NotFound(String),
}

/// One file listed in a VPK's directory tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpkEntry {
    path: String,
    crc32: u32,
    /// Where the preload bytes sit inside the directory file.
    preload: Range<usize>,
    archive_index: u16,
    offset: u32,
    length: u32,
}

impl VpkEntry {
    /// Full path inside the archive, e.g. `materials/dev/grid.vmat_c`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// CRC32 of the complete file contents, as stored in the tree.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Data archive holding the bulk of the file, or [`EMBEDDED_ARCHIVE`].
    pub fn archive_index(&self) -> u16 {
        self.archive_index
    }

    /// `true` when the bulk of the file is embedded in the directory file itself.
    pub fn is_embedded(&self) -> bool {
        self.archive_index == EMBEDDED_ARCHIVE
    }

    /// Offset of the bulk data inside its archive (or inside the embedded data section).
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Number of bytes stored in the archive (excluding preload bytes).
    pub fn archive_length(&self) -> u32 {
        self.length
    }

    /// Number of bytes stored inline in the tree.
    pub fn preload_length(&self) -> usize {
        self.preload.len()
    }

    /// Total file size: preload bytes + archive bytes.
    pub fn size(&self) -> u64 {
        self.preload.len() as u64 + u64::from(self.length)
    }
}

/// An opened VPK archive set.
pub struct Vpk {
    /// Complete contents of the directory file.
    dir: Vec<u8>,
    header: Header,
    entries: Vec<VpkEntry>,
    by_path: HashMap<String, usize>,
    archives: Box<dyn ArchiveSource>,
    chunk_hashes: Vec<ChunkHash>,
    other_checksums: Option<OtherChecksums>,
    signature: Option<SignatureInfo>,
}

impl std::fmt::Debug for Vpk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vpk")
            .field("header", &self.header)
            .field("entries", &self.entries.len())
            .field("chunk_hashes", &self.chunk_hashes.len())
            .field("signature", &self.signature)
            .finish_non_exhaustive()
    }
}

impl Vpk {
    /// Opens a VPK from disk.
    ///
    /// For `X_dir.vpk`, data archives `X_NNN.vpk` are read from the same folder when needed. Any other name is
    /// treated as a single self-contained VPK.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VpkError> {
        let path = path.as_ref();
        let dir = std::fs::read(path)?;
        match FileArchives::for_dir_file(path) {
            Some(archives) => Self::from_bytes(dir, archives),
            None => Self::from_bytes(dir, MemoryArchives::empty()),
        }
    }

    /// Parses a directory file already in memory, reading data archives from `archives`.
    pub fn from_bytes(
        dir: Vec<u8>,
        archives: impl ArchiveSource + 'static,
    ) -> Result<Self, VpkError> {
        let mut r = ByteReader::new(&dir);
        let header = Header::parse(&mut r)?;
        let entries = tree::parse(&dir, header.size(), header.tree_end())?;

        let mut vpk = Self {
            by_path: HashMap::with_capacity(entries.len()),
            dir,
            header,
            entries,
            archives: Box::new(archives),
            chunk_hashes: Vec::new(),
            other_checksums: None,
            signature: None,
        };
        for (index, entry) in vpk.entries.iter().enumerate() {
            vpk.by_path.insert(entry.path.clone(), index);
        }
        if header.version == 2 {
            vpk.parse_v2_sections()?;
        }
        Ok(vpk)
    }

    /// Reads the chunk hashes, other checksums and signature summary that follow the embedded data in v2 files.
    fn parse_v2_sections(&mut self) -> Result<(), VpkError> {
        let h = self.header;
        let chunk_start = h.tree_end() + h.file_data_size as usize;
        let chunk_end = chunk_start + h.archive_md5_size as usize;
        let other_end = chunk_end + h.other_md5_size as usize;
        let signature_end = other_end + h.signature_size as usize;
        if signature_end > self.dir.len() {
            return Err(VpkError::OutOfBounds {
                path: "<v2 checksum sections>".to_owned(),
            });
        }

        self.chunk_hashes = ChunkHash::parse_all(&self.dir[chunk_start..chunk_end])?;
        // Only the 48-byte layout is known; other sizes are left unparsed (as the reference reader does).
        if h.other_md5_size as usize == OtherChecksums::SIZE {
            self.other_checksums = Some(OtherChecksums::parse(&self.dir[chunk_end..other_end])?);
        }
        if h.signature_size > 0 {
            // CS2 keeps key + signature bytes after the declared section, so pass everything to the end of file.
            self.signature = Some(SignatureInfo::parse(
                &self.dir[other_end..],
                h.signature_size,
            )?);
        }
        Ok(())
    }

    /// The parsed header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Every file in the archive, in tree order.
    pub fn entries(&self) -> &[VpkEntry] {
        &self.entries
    }

    /// Number of files in the archive.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the archive lists no files.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up a file by path. Backslashes are accepted and a leading `/` is ignored; matching is case-sensitive.
    pub fn find(&self, path: &str) -> Option<&VpkEntry> {
        let normalized = path.replace('\\', "/");
        let normalized = normalized.trim_start_matches('/');
        self.by_path
            .get(normalized)
            .map(|&index| &self.entries[index])
    }

    /// Reads a file's complete contents and checks its CRC32.
    pub fn read(&self, entry: &VpkEntry) -> Result<Vec<u8>, VpkError> {
        let size = usize::try_from(entry.size()).map_err(|_| VpkError::OutOfBounds {
            path: entry.path.clone(),
        })?;
        let mut data = Vec::with_capacity(size);
        data.extend_from_slice(&self.dir[entry.preload.clone()]);

        if entry.length > 0 {
            if entry.is_embedded() {
                let start = self.header.tree_end() + entry.offset as usize;
                let bytes = self
                    .dir
                    .get(start..start + entry.length as usize)
                    .ok_or_else(|| VpkError::OutOfBounds {
                        path: entry.path.clone(),
                    })?;
                data.extend_from_slice(bytes);
            } else {
                let preload_len = data.len();
                data.resize(size, 0);
                self.archives.read_at(
                    entry.archive_index,
                    u64::from(entry.offset),
                    &mut data[preload_len..],
                )?;
            }
        }

        let actual = crc32fast::hash(&data);
        if actual != entry.crc32 {
            return Err(VpkError::CrcMismatch {
                path: entry.path.clone(),
                expected: entry.crc32,
                actual,
            });
        }
        Ok(data)
    }

    /// Finds and reads a file by path.
    pub fn read_path(&self, path: &str) -> Result<Vec<u8>, VpkError> {
        let entry = self
            .find(path)
            .ok_or_else(|| VpkError::NotFound(path.to_owned()))?;
        self.read(entry)
    }

    /// The archive MD5 section (v2 only; empty for v1).
    pub fn chunk_hashes(&self) -> &[ChunkHash] {
        &self.chunk_hashes
    }

    /// The tree / chunk-section / whole-file checksums (v2 only).
    pub fn other_checksums(&self) -> Option<&OtherChecksums> {
        self.other_checksums.as_ref()
    }

    /// Summary of the signature section (v2 only, when present).
    pub fn signature(&self) -> Option<&SignatureInfo> {
        self.signature.as_ref()
    }

    /// Verifies the three MD5 checksums of the directory file (tree, chunk section, whole file).
    ///
    /// This only reads the directory file, so it is fast. Returns `Ok(false)` when the VPK has no checksums
    /// (version 1, or an unknown layout).
    pub fn verify_checksums(&self) -> Result<bool, VpkError> {
        let Some(other) = self.other_checksums else {
            return Ok(false);
        };
        let h = self.header;
        let chunk_start = h.tree_end() + h.file_data_size as usize;
        let chunk_end = chunk_start + h.archive_md5_size as usize;

        verify::check(
            "tree",
            other.tree,
            verify::md5(&self.dir[h.size()..h.tree_end()]),
        )?;
        verify::check(
            "archive MD5 section",
            other.archive_md5_section,
            verify::md5(&self.dir[chunk_start..chunk_end]),
        )?;
        // Whole-file checksum covers everything before itself: up to the end of the first two checksums.
        verify::check(
            "whole file",
            other.whole_file,
            verify::md5(&self.dir[..chunk_end + 32]),
        )?;
        Ok(true)
    }

    /// Verifies one chunk hash by reading its byte range from the matching archive.
    pub fn verify_chunk(&self, chunk: &ChunkHash) -> Result<(), VpkError> {
        let hash: fn(&[u8]) -> [u8; 16] = match chunk.hash_type {
            HashType::Md5 => verify::md5,
            HashType::Blake3 => verify::blake3_16,
            HashType::Unknown(_) => return Err(VpkError::UnsupportedHash(chunk.hash_type)),
        };
        let data = if chunk.archive_index == EMBEDDED_ARCHIVE {
            let start = self.header.tree_end() + chunk.offset as usize;
            self.dir
                .get(start..start + chunk.length as usize)
                .ok_or_else(|| VpkError::OutOfBounds {
                    path: format!("<embedded chunk at {}>", chunk.offset),
                })?
                .to_vec()
        } else {
            let mut data = vec![0; chunk.length as usize];
            self.archives
                .read_at(chunk.archive_index, u64::from(chunk.offset), &mut data)?;
            data
        };
        verify::check("archive chunk", chunk.hash, hash(&data))
    }
}
