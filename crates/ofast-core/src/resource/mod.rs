//! The Source 2 **resource container**: the outer wrapper of every compiled file (`*.vmat_c`, `*.vmdl_c`,
//! `*.vtex_c`, `*.vmap_c`, …).
//!
//! A resource is a small header followed by a table of **blocks**, each tagged with a four-character code:
//!
//! | Block | Holds |
//! |---|---|
//! | `RED2` / `REDI` | editor info: where the source came from, dependencies (RED2 is KV3) |
//! | `RERL` | external references: other resources this one points to |
//! | `DATA` | the actual content — usually KV3, sometimes custom binary (textures, sounds) |
//! | `MBUF`, `VBIB`, `MDAT`, `ANIM`, `PHYS`, `CTRL`, … | type-specific data (mesh buffers, animation, physics…) |
//!
//! This module only splits a resource into its blocks; interpreting each block is the job of the type-specific
//! modules (e.g. [`crate::kv3`] for KV3 blocks). Layout details: `docs/formats/resource.md`.

use crate::bytes::{ByteReader, UnexpectedEof};
use crate::kv3::{self, Kv3File, binary::Kv3Error};

/// The only header version this reader accepts (all CS2 resources use it).
pub const HEADER_VERSION: u16 = 12;

/// Everything that can go wrong while reading a resource container.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    /// The data ended in the middle of a structure.
    #[error(transparent)]
    UnexpectedEof(#[from] UnexpectedEof),
    /// The header version is not 12.
    #[error("unsupported resource header version {0} (expected 12)")]
    UnsupportedHeaderVersion(u16),
    /// A block points outside the file.
    #[error("resource block {kind} at {offset} (+{size}) is outside the {file_len}-byte file")]
    BlockOutOfBounds {
        /// Block type.
        kind: BlockKind,
        /// Absolute start offset.
        offset: usize,
        /// Size in bytes.
        size: usize,
        /// File size.
        file_len: usize,
    },
    /// A requested block is missing.
    #[error("resource has no {0} block")]
    MissingBlock(BlockKind),
    /// A block that should be KV3 failed to decode.
    #[error("{kind} block: {source}")]
    Kv3 {
        /// Which block.
        kind: BlockKind,
        /// The decoding error.
        source: Kv3Error,
    },
}

/// A four-character block type code such as `DATA` or `RED2`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockKind(pub [u8; 4]);

impl BlockKind {
    /// The main content block.
    pub const DATA: BlockKind = BlockKind(*b"DATA");
    /// Editor info, KV3 encoded.
    pub const RED2: BlockKind = BlockKind(*b"RED2");
    /// Editor info, legacy binary encoding.
    pub const REDI: BlockKind = BlockKind(*b"REDI");
    /// External resource references.
    pub const RERL: BlockKind = BlockKind(*b"RERL");
    /// Introspection manifest (marks `DATA` as the legacy NTRO encoding instead of KV3).
    pub const NTRO: BlockKind = BlockKind(*b"NTRO");

    /// The code as text (non-ASCII bytes shown as `?`).
    pub fn as_str(&self) -> String {
        self.0
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
            .collect()
    }
}

impl std::fmt::Display for BlockKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl std::fmt::Debug for BlockKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlockKind({self})")
    }
}

/// One entry of the block table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Four-character type code.
    pub kind: BlockKind,
    /// Absolute offset of the block data in the file.
    pub offset: usize,
    /// Size of the block data in bytes.
    pub size: usize,
}

/// A parsed resource container borrowing the file bytes.
#[derive(Debug, Clone)]
pub struct Resource<'a> {
    data: &'a [u8],
    /// File size as recorded in the header (texture pixel data may follow it).
    pub file_size: u32,
    /// Resource-type-specific version.
    pub version: u16,
    blocks: Vec<Block>,
}

impl<'a> Resource<'a> {
    /// Parses the header and block table of a compiled resource.
    pub fn parse(data: &'a [u8]) -> Result<Self, ResourceError> {
        let mut r = ByteReader::new(data);
        let file_size = r.u32("resource file size")?;
        let header_version = r.u16("resource header version")?;
        if header_version != HEADER_VERSION {
            return Err(ResourceError::UnsupportedHeaderVersion(header_version));
        }
        let version = r.u16("resource version")?;
        // Offsets in this format are relative to the position of the offset field itself.
        let table_field = r.position();
        let table_offset = r.u32("resource block table offset")? as usize;
        let block_count = r.u32("resource block count")? as usize;

        r.seek(table_field + table_offset, "resource block table")?;
        let mut blocks = Vec::with_capacity(block_count.min(64));
        for _ in 0..block_count {
            let kind = BlockKind(r.array("resource block type")?);
            let offset_field = r.position();
            let offset = offset_field + r.u32("resource block offset")? as usize;
            let size = r.u32("resource block size")? as usize;
            if offset.checked_add(size).is_none_or(|end| end > data.len()) {
                return Err(ResourceError::BlockOutOfBounds {
                    kind,
                    offset,
                    size,
                    file_len: data.len(),
                });
            }
            blocks.push(Block { kind, offset, size });
        }
        Ok(Self {
            data,
            file_size,
            version,
            blocks,
        })
    }

    /// All blocks in table order.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The first block of the given kind.
    pub fn block(&self, kind: BlockKind) -> Option<&Block> {
        self.blocks.iter().find(|b| b.kind == kind)
    }

    /// The bytes of a block.
    pub fn block_data(&self, block: &Block) -> &'a [u8] {
        &self.data[block.offset..block.offset + block.size]
    }

    /// `true` when the `DATA` block uses the legacy NTRO encoding (the file has an `NTRO` block).
    pub fn uses_ntro(&self) -> bool {
        self.block(BlockKind::NTRO).is_some()
    }

    /// Decodes a block as binary KV3.
    pub fn kv3(&self, kind: BlockKind) -> Result<Kv3File, ResourceError> {
        let block = self.block(kind).ok_or(ResourceError::MissingBlock(kind))?;
        kv3::binary::decode(self.block_data(block))
            .map_err(|source| ResourceError::Kv3 { kind, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a resource container with the given blocks, laid out like Valve's compiler does.
    pub(crate) fn build(blocks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let table_start = 16;
        let mut data_start = table_start + 12 * blocks.len();
        let mut out = Vec::new();
        out.extend_from_slice(&0u32.to_le_bytes()); // file size, patched below
        out.extend_from_slice(&HEADER_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes()); // table is 8 bytes after this field (at offset 16)
        out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        let mut payload = Vec::new();
        for (index, (kind, data)) in blocks.iter().enumerate() {
            let offset_field = table_start + 12 * index + 4;
            out.extend_from_slice(*kind);
            out.extend_from_slice(&((data_start - offset_field) as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            payload.extend_from_slice(data);
            data_start += data.len();
        }
        out.extend_from_slice(&payload);
        let len = out.len() as u32;
        out[..4].copy_from_slice(&len.to_le_bytes());
        out
    }

    #[test]
    fn splits_blocks() {
        let file = build(&[(b"RED2", b"editor"), (b"DATA", b"content!")]);
        let resource = Resource::parse(&file).unwrap();
        assert_eq!(resource.file_size as usize, file.len());
        assert_eq!(resource.blocks().len(), 2);
        let data = resource.block(BlockKind::DATA).unwrap();
        assert_eq!(resource.block_data(data), b"content!");
        assert!(!resource.uses_ntro());
        assert!(matches!(
            resource.kv3(BlockKind::RERL),
            Err(ResourceError::MissingBlock(BlockKind::RERL))
        ));
    }

    #[test]
    fn rejects_blocks_outside_the_file() {
        let mut file = build(&[(b"DATA", b"content!")]);
        let size_field = 16 + 8;
        file[size_field..size_field + 4].copy_from_slice(&1000u32.to_le_bytes());
        assert!(matches!(
            Resource::parse(&file),
            Err(ResourceError::BlockOutOfBounds { .. })
        ));
    }
}
