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

    /// The resources this one refers to: the `RERL` block, empty when there is none.
    ///
    /// The block is `u32` offset to the entries (relative to the field) and `u32` count; each entry is 16 bytes:
    /// the referenced resource's [`resource_id`] (`u64`), an offset to its NUL-terminated path (`u32`, relative to
    /// the field) and 4 unused bytes.
    pub fn external_refs(&self) -> Result<Vec<ExternalRef>, ResourceError> {
        let Some(block) = self.block(BlockKind::RERL) else {
            return Ok(Vec::new());
        };
        let mut r = ByteReader::new(self.block_data(block));
        let entries = r.u32("RERL entries offset")? as usize;
        let count = r.u32("RERL entry count")? as usize;
        let mut refs = Vec::with_capacity(count.min(4096));
        for index in 0..count {
            let entry = entries + 16 * index;
            r.seek(entry, "RERL entry")?;
            let id = r.u64("RERL id")?;
            let name_field = r.position();
            let name_offset = r.u32("RERL name offset")? as usize;
            r.seek(name_field + name_offset, "RERL name")?;
            let path = String::from_utf8_lossy(r.cstr("RERL name")?).into_owned();
            refs.push(ExternalRef { id, path });
        }
        Ok(refs)
    }
}

/// One resource another refers to (see [`Resource::external_refs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRef {
    /// The resource's id: [`resource_id`] of its path.
    pub id: u64,
    /// Its path without the `_c` suffix (`materials/models/weapons/v_models/rif_ak47/ak47.vmat`).
    pub path: String,
}

/// Seed of the hash that names resources ([`resource_id`]).
pub const RESOURCE_ID_SEED: u32 = 0xEDAB_CDEF;

/// The 64-bit id Source 2 names a resource by: MurmurHash64B (Austin Appleby's 64-bit hash for 32-bit machines) of
/// its path without the `_c` suffix, seed [`RESOURCE_ID_SEED`].
///
/// Resources store these ids next to the paths they refer to ([`Resource::external_refs`]), and entities send them
/// instead of paths: a demo's `m_hModel` is the id of the entity's `.vmdl`. The path is used as written; CS2's
/// paths are lowercase with forward slashes, so pass them that way. Checked against every reference of every
/// resource in CS2's archive (the `resource_ids_match_references` oracle test).
pub fn resource_id(path: &str) -> u64 {
    const M: u32 = 0x5bd1_e995;
    let mix = |k: u32| {
        let k = k.wrapping_mul(M);
        (k ^ (k >> 24)).wrapping_mul(M)
    };
    let data = path.as_bytes();
    let mut h1 = RESOURCE_ID_SEED ^ data.len() as u32;
    let mut h2 = 0u32; // the high half of the 64-bit seed
    let (words, tail) = data.as_chunks::<4>();
    // Whole 4-byte words go alternately into the two halves.
    for (i, word) in words.iter().enumerate() {
        let k = mix(u32::from_le_bytes(*word));
        if i % 2 == 0 {
            h1 = h1.wrapping_mul(M) ^ k;
        } else {
            h2 = h2.wrapping_mul(M) ^ k;
        }
    }
    // The last 1 – 3 bytes go into the second half.
    if !tail.is_empty() {
        for (i, &byte) in tail.iter().enumerate() {
            h2 ^= u32::from(byte) << (8 * i);
        }
        h2 = h2.wrapping_mul(M);
    }
    h1 = (h1 ^ (h2 >> 18)).wrapping_mul(M);
    h2 = (h2 ^ (h1 >> 22)).wrapping_mul(M);
    h1 = (h1 ^ (h2 >> 17)).wrapping_mul(M);
    h2 = (h2 ^ (h1 >> 19)).wrapping_mul(M);
    (u64::from(h1) << 32) | u64::from(h2)
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

    /// Ids CS2 stores next to these paths (the AK-47 model's references), covering paths of an even and an odd
    /// number of 4-byte words and with a 2-byte tail.
    #[test]
    fn resource_ids_match_cs2() {
        for (path, id) in [
            (
                "animation/skeletons/weapons/ak47.vnmskel",
                0xb437_c30c_3ebc_bc99,
            ),
            (
                "materials/models/weapons/v_models/rif_ak47/ak47.vmat",
                0x027b_33f3_ca99_2e1f,
            ),
            (
                "weapons/models/ak47/weapon_rif_ak47.vgcxdata",
                0xd665_7de7_e590_2d55,
            ),
            (
                "weapons/models/shared/materials/sticker_gaps/sticker_gaps.vmat",
                0x11d7_7a8d_5f1a_ed61,
            ),
        ] {
            assert_eq!(resource_id(path), id, "{path}");
        }
    }

    #[test]
    fn reads_external_refs() {
        // Header (entries 8 bytes after the field, 2 entries), two entries, then the names.
        let mut rerl = Vec::new();
        rerl.extend_from_slice(&8u32.to_le_bytes());
        rerl.extend_from_slice(&2u32.to_le_bytes());
        let names_at = 8 + 2 * 16;
        let names = [&b"a.vmat\0"[..], b"bb.vtex\0"];
        let mut name_offset = names_at;
        for (i, name) in names.iter().enumerate() {
            let entry = 8 + 16 * i;
            rerl.extend_from_slice(&(i as u64 + 10).to_le_bytes());
            rerl.extend_from_slice(&((name_offset - (entry + 8)) as u32).to_le_bytes());
            rerl.extend_from_slice(&0u32.to_le_bytes());
            name_offset += name.len();
        }
        for name in names {
            rerl.extend_from_slice(name);
        }
        let file = build(&[(b"RERL", &rerl)]);
        let refs = Resource::parse(&file).unwrap().external_refs().unwrap();
        assert_eq!(
            refs,
            [
                ExternalRef {
                    id: 10,
                    path: "a.vmat".into()
                },
                ExternalRef {
                    id: 11,
                    path: "bb.vtex".into()
                },
            ]
        );
        let none = build(&[(b"DATA", b"x")]);
        assert!(
            Resource::parse(&none)
                .unwrap()
                .external_refs()
                .unwrap()
                .is_empty()
        );
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
