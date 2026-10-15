//! Where the bytes of non-embedded VPK entries come from.
//!
//! A directory file (`pak01_dir.vpk`) only holds the table of contents; file contents usually live in numbered data
//! archives (`pak01_000.vpk`, `pak01_001.vpk`, …). [`ArchiveSource`] abstracts "read N bytes at offset X of archive
//! K" so the same [`Vpk`](super::Vpk) code works on real files on disk ([`FileArchives`]) and on in-memory archives
//! built by tests ([`MemoryArchives`]).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Something that can read byte ranges out of numbered VPK data archives.
pub trait ArchiveSource: Send + Sync {
    /// Fills `buf` with the bytes starting at `offset` in archive number `archive_index`.
    fn read_at(&self, archive_index: u16, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    /// Total size of archive `archive_index` in bytes, if it exists.
    fn archive_len(&self, archive_index: u16) -> io::Result<u64>;
}

/// Data archives stored next to the directory file on disk: `{base}_{index:03}.vpk`.
///
/// Archive files are opened lazily the first time they are needed and then kept open.
#[derive(Debug)]
pub struct FileArchives {
    folder: PathBuf,
    base: String,
    open: Mutex<HashMap<u16, File>>,
}

impl FileArchives {
    /// Archives for the directory file at `dir_path` (e.g. `.../pak01_dir.vpk` → `.../pak01_NNN.vpk`).
    ///
    /// Returns `None` when the file name does not end in `_dir.vpk`, meaning it is a single self-contained VPK.
    pub fn for_dir_file(dir_path: &Path) -> Option<Self> {
        let stem = dir_path.file_stem()?.to_str()?;
        // Valve names are lowercase, but match case-insensitively like the reference implementation.
        let base_len = stem.len().checked_sub(4)?;
        if !stem.is_char_boundary(base_len) || !stem[base_len..].eq_ignore_ascii_case("_dir") {
            return None;
        }
        Some(Self {
            folder: dir_path.parent().unwrap_or(Path::new("")).to_path_buf(),
            base: stem[..base_len].to_owned(),
            open: Mutex::new(HashMap::new()),
        })
    }

    /// Path of data archive number `index`.
    pub fn archive_path(&self, index: u16) -> PathBuf {
        self.folder.join(format!("{}_{:03}.vpk", self.base, index))
    }

    /// Runs `f` with archive `index` opened (opening it on first use).
    fn with_archive<T>(
        &self,
        index: u16,
        f: impl FnOnce(&mut File) -> io::Result<T>,
    ) -> io::Result<T> {
        // A poisoned lock only means another thread panicked mid-read; the map of open files is still usable.
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file = match open.entry(index) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let path = self.archive_path(index);
                let file = File::open(&path).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("cannot open VPK archive {}: {e}", path.display()),
                    )
                })?;
                entry.insert(file)
            }
        };
        f(file)
    }
}

impl ArchiveSource for FileArchives {
    fn read_at(&self, archive_index: u16, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.with_archive(archive_index, |file| {
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(buf)
        })
    }

    fn archive_len(&self, archive_index: u16) -> io::Result<u64> {
        self.with_archive(archive_index, |file| Ok(file.metadata()?.len()))
    }
}

/// Data archives held in memory, keyed by archive index. Used by tests and for single-file VPKs (empty).
#[derive(Debug, Default, Clone)]
pub struct MemoryArchives {
    archives: HashMap<u16, Vec<u8>>,
}

impl MemoryArchives {
    /// No archives at all — correct for a single-file VPK where every entry is embedded.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Adds (or replaces) archive number `index`.
    pub fn insert(&mut self, index: u16, bytes: Vec<u8>) {
        self.archives.insert(index, bytes);
    }

    /// Mutable access to an archive's bytes (tests use this to simulate corruption).
    pub fn get_mut(&mut self, index: u16) -> Option<&mut Vec<u8>> {
        self.archives.get_mut(&index)
    }

    fn get(&self, index: u16) -> io::Result<&[u8]> {
        self.archives.get(&index).map(Vec::as_slice).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no VPK archive {index:03}"),
            )
        })
    }
}

impl ArchiveSource for MemoryArchives {
    fn read_at(&self, archive_index: u16, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let data = self.get(archive_index)?;
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let slice = data.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(slice);
        Ok(())
    }

    fn archive_len(&self, archive_index: u16) -> io::Result<u64> {
        Ok(self.get(archive_index)?.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_file_names_map_to_numbered_archives() {
        let archives = FileArchives::for_dir_file(Path::new("game/csgo/pak01_dir.vpk")).unwrap();
        assert_eq!(
            archives.archive_path(0),
            Path::new("game/csgo").join("pak01_000.vpk")
        );
        assert_eq!(
            archives.archive_path(123),
            Path::new("game/csgo").join("pak01_123.vpk")
        );
        assert_eq!(
            archives.archive_path(1234),
            Path::new("game/csgo").join("pak01_1234.vpk")
        );
    }

    #[test]
    fn single_file_vpks_have_no_archives() {
        assert!(FileArchives::for_dir_file(Path::new("maps/de_dust2.vpk")).is_none());
        assert!(FileArchives::for_dir_file(Path::new("dir.vpk")).is_none());
    }
}
