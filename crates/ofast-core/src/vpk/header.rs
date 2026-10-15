//! The fixed-size header at the start of a VPK directory file, and the optional signature section header.
//!
//! See `docs/formats/vpk.md` → *Header* and *Signature section*.

use super::{SIGNATURE, VpkError};
use crate::bytes::ByteReader;

/// The VPK header (12 bytes for version 1, 28 bytes for version 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Format version: 1 or 2. CS2 uses 2.
    pub version: u32,
    /// Size in bytes of the directory tree that follows the header.
    pub tree_size: u32,
    /// v2: size of the file data embedded in the directory file after the tree (archive index `0x7FFF`).
    pub file_data_size: u32,
    /// v2: size of the archive MD5 ("chunk hashes") section.
    pub archive_md5_size: u32,
    /// v2: size of the "other MD5" section (48 when present).
    pub other_md5_size: u32,
    /// v2: size of the signature section.
    pub signature_size: u32,
}

impl Header {
    /// Size of the header itself, which is where the directory tree starts.
    pub fn size(&self) -> usize {
        if self.version == 1 { 12 } else { 28 }
    }

    /// Offset of the first byte after the directory tree, where embedded file data starts.
    pub fn tree_end(&self) -> usize {
        self.size() + self.tree_size as usize
    }

    /// Reads and validates the header at the start of `r`.
    pub(super) fn parse(r: &mut ByteReader<'_>) -> Result<Self, VpkError> {
        let signature = r.u32("VPK signature")?;
        if signature != SIGNATURE {
            return Err(VpkError::BadSignature(signature));
        }
        let version = r.u32("VPK version")?;
        let tree_size = r.u32("VPK tree size")?;
        let mut header = Self {
            version,
            tree_size,
            file_data_size: 0,
            archive_md5_size: 0,
            other_md5_size: 0,
            signature_size: 0,
        };
        match version {
            1 => {}
            2 => {
                header.file_data_size = r.u32("VPK file data size")?;
                header.archive_md5_size = r.u32("VPK archive MD5 section size")?;
                header.other_md5_size = r.u32("VPK other MD5 section size")?;
                header.signature_size = r.u32("VPK signature section size")?;
            }
            other => return Err(VpkError::UnsupportedVersion(other)),
        }
        Ok(header)
    }
}

/// How the signature section of a v2 VPK is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureLayout {
    /// HL2/TF2 era: key size, key, signature size, signature, all inside the section.
    Legacy,
    /// CS2: a 20-byte descriptor starting with the VPK magic; key and signature bytes follow it.
    Cs2 {
        /// What is signed. `1` means only the whole-file MD5 checksum.
        signature_type: u32,
    },
}

/// Summary of a VPK's signature section. OFAST records it but does not verify RSA signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureInfo {
    /// Which layout the section uses.
    pub layout: SignatureLayout,
    /// Size of the public key in bytes (0 when unsigned).
    pub key_size: u32,
    /// Size of the signature in bytes (0 when unsigned).
    pub signature_size: u32,
}

impl SignatureInfo {
    /// `true` when the file actually carries a signature.
    pub fn is_signed(&self) -> bool {
        self.signature_size > 0
    }

    /// Reads the signature section header. `section` starts at the section and may extend to the end of the file
    /// (CS2 stores the key and signature after the declared 20 bytes).
    pub(super) fn parse(section: &[u8], declared_size: u32) -> Result<Self, VpkError> {
        let mut r = ByteReader::new(section);
        if declared_size == 20 && section.len() >= 4 && section[..4] == SIGNATURE.to_le_bytes() {
            r.skip(4, "VPK signature magic")?;
            let signature_type = r.u32("VPK signature type")?;
            let key_size = r.u32("VPK signature key size")?;
            let signature_size = r.u32("VPK signature size")?;
            Ok(Self {
                layout: SignatureLayout::Cs2 { signature_type },
                key_size,
                signature_size,
            })
        } else {
            let key_size = r.u32("VPK public key size")?;
            r.skip(key_size as usize, "VPK public key")?;
            let signature_size = r.u32("VPK signature size")?;
            Ok(Self {
                layout: SignatureLayout::Legacy,
                key_size,
                signature_size,
            })
        }
    }
}
