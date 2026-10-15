//! Parsing of the VPK directory tree: the nested extension → directory → file name lists.
//!
//! See `docs/formats/vpk.md` → *Directory tree*. Each of the three nested loops ends with an empty string, and a
//! single space `" "` stands for "no extension" or "root directory".

use super::{TERMINATOR, VpkEntry, VpkError};
use crate::bytes::ByteReader;

/// Placeholder Valve uses for an empty directory or a missing extension.
const EMPTY_PLACEHOLDER: &str = " ";

/// Size of the fixed part of an entry record (CRC, preload count, archive index, offset, length, terminator).
const ENTRY_RECORD_SIZE: usize = 18;

/// Parses the tree occupying `dir[tree_start..tree_end]` into a flat list of entries.
///
/// The parse must end *exactly* at `tree_end`; anything else means the header's tree size and the tree disagree,
/// which would make every embedded-data offset wrong.
pub(super) fn parse(
    dir: &[u8],
    tree_start: usize,
    tree_end: usize,
) -> Result<Vec<VpkEntry>, VpkError> {
    let bounded = dir.get(..tree_end).ok_or(VpkError::TreeOutOfBounds {
        tree_end,
        file_len: dir.len(),
    })?;
    let mut r = ByteReader::new(bounded);
    r.seek(tree_start, "VPK directory tree")?;

    let mut entries = Vec::new();
    loop {
        let extension = read_name(&mut r, "VPK tree extension")?;
        if extension.is_empty() {
            break;
        }
        loop {
            let directory = read_name(&mut r, "VPK tree directory")?;
            if directory.is_empty() {
                break;
            }
            loop {
                let name = read_name(&mut r, "VPK tree file name")?;
                if name.is_empty() {
                    break;
                }
                entries.push(read_entry(&mut r, join_path(directory, name, extension))?);
            }
        }
    }

    if r.position() != tree_end {
        return Err(VpkError::TreeSizeMismatch {
            expected_end: tree_end,
            actual_end: r.position(),
        });
    }
    Ok(entries)
}

/// Reads one NUL-terminated name and checks it is UTF-8.
fn read_name<'a>(r: &mut ByteReader<'a>, what: &'static str) -> Result<&'a str, VpkError> {
    let offset = r.position();
    let bytes = r.cstr(what)?;
    std::str::from_utf8(bytes).map_err(|_| VpkError::InvalidUtf8 { offset })
}

/// Reads the 18-byte entry record and the preload bytes that follow it.
fn read_entry(r: &mut ByteReader<'_>, path: String) -> Result<VpkEntry, VpkError> {
    debug_assert_eq!(ENTRY_RECORD_SIZE, 4 + 2 + 2 + 4 + 4 + 2);
    let crc32 = r.u32("VPK entry CRC32")?;
    let preload_len = r.u16("VPK entry preload size")?;
    let archive_index = r.u16("VPK entry archive index")?;
    let offset = r.u32("VPK entry offset")?;
    let length = r.u32("VPK entry length")?;
    let terminator = r.u16("VPK entry terminator")?;
    if terminator != TERMINATOR {
        return Err(VpkError::BadTerminator {
            path,
            found: terminator,
        });
    }
    let preload_start = r.position();
    r.skip(preload_len as usize, "VPK entry preload data")?;
    Ok(VpkEntry {
        path,
        crc32,
        preload: preload_start..preload_start + preload_len as usize,
        archive_index,
        offset,
        length,
    })
}

/// Builds `directory/name.extension`, dropping the `" "` placeholders.
fn join_path(directory: &str, name: &str, extension: &str) -> String {
    let mut path = String::with_capacity(directory.len() + name.len() + extension.len() + 2);
    if directory != EMPTY_PLACEHOLDER {
        path.push_str(directory);
        path.push('/');
    }
    path.push_str(name);
    if extension != EMPTY_PLACEHOLDER {
        path.push('.');
        path.push_str(extension);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::join_path;

    #[test]
    fn placeholders_are_dropped_from_paths() {
        assert_eq!(
            join_path("materials/dev", "grid", "vmat_c"),
            "materials/dev/grid.vmat_c"
        );
        assert_eq!(join_path(" ", "readme", "txt"), "readme.txt");
        assert_eq!(join_path("scripts", "Makefile", " "), "scripts/Makefile");
        assert_eq!(join_path(" ", "LICENSE", " "), "LICENSE");
    }
}
