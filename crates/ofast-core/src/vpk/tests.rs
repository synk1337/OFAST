//! Synthetic-fixture tests for the VPK reader.
//!
//! These tests never touch Valve files. [`Builder`] writes VPK directory files byte by byte, following
//! `docs/formats/vpk.md` exactly, so passing tests also prove the format doc is complete. Tests against the real CS2
//! archives live in `tests/cs2_vpk.rs` behind the `game-data` feature.

use std::collections::BTreeMap;

use super::*;

/// Where a test file's non-preload bytes go.
#[derive(Clone, Copy)]
enum Location {
    /// Into data archive N.
    Archive(u16),
    /// Into the directory file's embedded data section.
    Embedded,
}

/// One file to put into a synthetic VPK.
struct TestFile {
    path: &'static str,
    data: Vec<u8>,
    preload: usize,
    location: Location,
}

impl TestFile {
    fn new(path: &'static str, data: &[u8], preload: usize, location: Location) -> Self {
        Self {
            path,
            data: data.to_vec(),
            preload,
            location,
        }
    }
}

/// Directory path → list of `(file name, encoded entry record)` inside one extension of the tree.
type DirMap = BTreeMap<String, Vec<(String, Vec<u8>)>>;

/// Writes VPK directory files (and their data archives) for tests.
struct Builder {
    version: u32,
    files: Vec<TestFile>,
}

/// Splits `dir/name.ext` into the three tree components, using `" "` for empty parts like Valve does.
fn split(path: &str) -> (String, String, String) {
    let (dir, file) = path.rsplit_once('/').unwrap_or((" ", path));
    let (name, ext) = file.rsplit_once('.').unwrap_or((file, " "));
    (dir.to_owned(), name.to_owned(), ext.to_owned())
}

fn push_cstr(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

impl Builder {
    fn v2(files: Vec<TestFile>) -> Self {
        Self { version: 2, files }
    }

    fn v1(files: Vec<TestFile>) -> Self {
        Self { version: 1, files }
    }

    /// Produces `(directory file bytes, data archives)`.
    fn build(&self) -> (Vec<u8>, MemoryArchives) {
        let mut archives: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
        let mut embedded = Vec::new();

        // ext -> dir -> [(name, record bytes)], sorted for deterministic output.
        let mut tree: BTreeMap<String, DirMap> = BTreeMap::new();
        for file in &self.files {
            let (preload, rest) = file.data.split_at(file.preload);
            let (archive_index, offset) = if rest.is_empty() {
                (EMBEDDED_ARCHIVE, 0)
            } else {
                match file.location {
                    Location::Embedded => {
                        let offset = embedded.len();
                        embedded.extend_from_slice(rest);
                        (EMBEDDED_ARCHIVE, offset)
                    }
                    Location::Archive(index) => {
                        let archive = archives.entry(index).or_default();
                        let offset = archive.len();
                        archive.extend_from_slice(rest);
                        (index, offset)
                    }
                }
            };
            let mut record = Vec::new();
            record.extend_from_slice(&crc32fast::hash(&file.data).to_le_bytes());
            record.extend_from_slice(&(preload.len() as u16).to_le_bytes());
            record.extend_from_slice(&archive_index.to_le_bytes());
            record.extend_from_slice(&(offset as u32).to_le_bytes());
            record.extend_from_slice(&(rest.len() as u32).to_le_bytes());
            record.extend_from_slice(&TERMINATOR.to_le_bytes());
            record.extend_from_slice(preload);

            let (dir, name, ext) = split(file.path);
            tree.entry(ext)
                .or_default()
                .entry(dir)
                .or_default()
                .push((name, record));
        }

        let mut tree_bytes = Vec::new();
        for (ext, dirs) in &tree {
            push_cstr(&mut tree_bytes, ext);
            for (dir, names) in dirs {
                push_cstr(&mut tree_bytes, dir);
                for (name, record) in names {
                    push_cstr(&mut tree_bytes, name);
                    tree_bytes.extend_from_slice(record);
                }
                tree_bytes.push(0); // end of names
            }
            tree_bytes.push(0); // end of directories
        }
        tree_bytes.push(0); // end of extensions

        let mut out = Vec::new();
        out.extend_from_slice(&SIGNATURE.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(tree_bytes.len() as u32).to_le_bytes());

        if self.version == 1 {
            out.extend_from_slice(&tree_bytes);
            out.extend_from_slice(&embedded);
        } else {
            // One chunk hash per archive, plus the CS2-style marker entry for embedded data. Like real CS2 files,
            // both hash types appear: odd-numbered archives use BLAKE3, the rest MD5.
            let mut chunks = Vec::new();
            let mut push_chunk = |index: u16, kind: u16, data: &[u8], hash: [u8; 16]| {
                chunks.extend_from_slice(&index.to_le_bytes());
                chunks.extend_from_slice(&kind.to_le_bytes());
                chunks.extend_from_slice(&0u32.to_le_bytes());
                chunks.extend_from_slice(&(data.len() as u32).to_le_bytes());
                chunks.extend_from_slice(&hash);
            };
            if !embedded.is_empty() {
                push_chunk(0, 0x8000, &embedded, verify::md5(&embedded));
            }
            for (&index, data) in &archives {
                if index % 2 == 1 {
                    push_chunk(index, 1, data, verify::blake3_16(data));
                } else {
                    push_chunk(index, 0, data, verify::md5(data));
                }
            }

            out.extend_from_slice(&(embedded.len() as u32).to_le_bytes());
            out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
            out.extend_from_slice(&48u32.to_le_bytes());
            out.extend_from_slice(&20u32.to_le_bytes());
            out.extend_from_slice(&tree_bytes);
            out.extend_from_slice(&embedded);
            out.extend_from_slice(&chunks);
            out.extend_from_slice(&verify::md5(&tree_bytes));
            out.extend_from_slice(&verify::md5(&chunks));
            let whole = verify::md5(&out);
            out.extend_from_slice(&whole);
            // CS2-style unsigned signature section: magic, type 1, key size 0, signature size 0, reserved.
            for value in [SIGNATURE, 1, 0, 0, 0] {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }

        let mut memory = MemoryArchives::empty();
        for (index, data) in archives {
            memory.insert(index, data);
        }
        (out, memory)
    }
}

/// A VPK exercising every storage case: archives, embedded data, preload-only, mixed, root files, no extension.
fn sample_files() -> Vec<TestFile> {
    vec![
        TestFile::new(
            "materials/dev/grid.vmat_c",
            b"grid material bytes",
            0,
            Location::Archive(0),
        ),
        TestFile::new(
            "materials/dev/white.vmat_c",
            b"white",
            0,
            Location::Archive(0),
        ),
        TestFile::new(
            "models/player/ct.vmdl_c",
            &[7u8; 300],
            0,
            Location::Archive(1),
        ),
        TestFile::new(
            "scripts/items/items_game.txt",
            b"\"items_game\" {}",
            0,
            Location::Embedded,
        ),
        TestFile::new(
            "cfg/preload_only.cfg",
            b"bind mouse1 +attack",
            19,
            Location::Embedded,
        ),
        TestFile::new(
            "sounds/mixed.vsnd_c",
            b"PRELOADrest-in-archive",
            7,
            Location::Archive(1),
        ),
        TestFile::new("readme.txt", b"root file", 0, Location::Embedded),
        TestFile::new("scripts/Makefile", b"no extension", 0, Location::Archive(0)),
        TestFile::new("empty.txt", b"", 0, Location::Embedded),
    ]
}

fn open(builder: &Builder) -> Vpk {
    let (dir, archives) = builder.build();
    Vpk::from_bytes(dir, archives).expect("synthetic VPK parses")
}

#[test]
fn reads_every_file_back_exactly() {
    let files = sample_files();
    let vpk = open(&Builder::v2(sample_files()));
    assert_eq!(vpk.len(), files.len());
    for file in &files {
        let entry = vpk
            .find(file.path)
            .unwrap_or_else(|| panic!("{} is listed", file.path));
        assert_eq!(entry.size(), file.data.len() as u64, "{}", file.path);
        assert_eq!(vpk.read(entry).unwrap(), file.data, "{}", file.path);
    }
}

#[test]
fn entry_metadata_matches_storage() {
    let vpk = open(&Builder::v2(sample_files()));
    let preload_only = vpk.find("cfg/preload_only.cfg").unwrap();
    assert_eq!(preload_only.preload_length(), 19);
    assert_eq!(preload_only.archive_length(), 0);

    let mixed = vpk.find("sounds/mixed.vsnd_c").unwrap();
    assert_eq!(mixed.preload_length(), 7);
    assert_eq!(mixed.archive_index(), 1);

    assert!(
        vpk.find("scripts/items/items_game.txt")
            .unwrap()
            .is_embedded()
    );
}

#[test]
fn lookup_normalizes_separators() {
    let vpk = open(&Builder::v2(sample_files()));
    assert!(vpk.find(r"materials\dev\grid.vmat_c").is_some());
    assert!(vpk.find("/materials/dev/grid.vmat_c").is_some());
    assert!(
        vpk.find("MATERIALS/dev/grid.vmat_c").is_none(),
        "lookup is case-sensitive"
    );
    assert!(matches!(
        vpk.read_path("nope.txt"),
        Err(VpkError::NotFound(_))
    ));
}

#[test]
fn v2_checksums_and_chunks_verify() {
    let vpk = open(&Builder::v2(sample_files()));
    assert!(vpk.verify_checksums().unwrap());
    // Embedded marker + archives 0 and 1.
    assert_eq!(vpk.chunk_hashes().len(), 3);
    assert_eq!(vpk.chunk_hashes()[0].archive_index, EMBEDDED_ARCHIVE);
    assert_eq!(vpk.chunk_hashes()[2].hash_type, HashType::Blake3);
    for chunk in vpk.chunk_hashes() {
        vpk.verify_chunk(chunk).unwrap();
    }
    let signature = vpk.signature().unwrap();
    assert_eq!(signature.layout, SignatureLayout::Cs2 { signature_type: 1 });
    assert!(!signature.is_signed());
}

#[test]
fn corrupted_archive_bytes_are_detected() {
    let (dir, mut archives) = Builder::v2(sample_files()).build();
    archives.get_mut(0).unwrap()[0] ^= 0xFF;
    let vpk = Vpk::from_bytes(dir, archives).unwrap();

    let err = vpk.read_path("materials/dev/grid.vmat_c").unwrap_err();
    assert!(matches!(err, VpkError::CrcMismatch { .. }), "{err}");
    let chunk = vpk
        .chunk_hashes()
        .iter()
        .find(|c| c.archive_index == 0)
        .unwrap();
    assert!(matches!(
        vpk.verify_chunk(chunk),
        Err(VpkError::ChecksumMismatch { .. })
    ));
}

/// Offset of the 18-byte entry record that follows the file name `name` in a built directory file.
fn record_offset(dir: &[u8], name: &str) -> usize {
    let needle = [name.as_bytes(), &[0]].concat();
    let start = dir
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("name is in the tree");
    start + needle.len()
}

#[test]
fn corrupted_tree_fails_the_tree_checksum() {
    let (mut dir, archives) = Builder::v2(sample_files()).build();
    // Flip a bit in the CRC field of one entry record: the tree still parses, but its MD5 no longer matches.
    let record = record_offset(&dir, "grid");
    dir[record] ^= 0xFF;
    let vpk = Vpk::from_bytes(dir, archives).unwrap();
    let err = vpk.verify_checksums().unwrap_err();
    assert!(
        matches!(
            err,
            VpkError::ChecksumMismatch {
                section: "tree",
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn version_1_has_a_short_header_and_no_checksums() {
    let files = vec![
        TestFile::new("a/b.txt", b"hello", 0, Location::Embedded),
        TestFile::new("c.bin", b"archived", 2, Location::Archive(0)),
    ];
    let vpk = open(&Builder::v1(files));
    assert_eq!(vpk.header().size(), 12);
    assert_eq!(vpk.read_path("a/b.txt").unwrap(), b"hello");
    assert_eq!(vpk.read_path("c.bin").unwrap(), b"archived");
    assert!(!vpk.verify_checksums().unwrap());
}

#[test]
fn rejects_wrong_magic_and_version() {
    let (mut dir, _) = Builder::v2(sample_files()).build();
    dir[0] = 0;
    assert!(matches!(
        Vpk::from_bytes(dir, MemoryArchives::empty()),
        Err(VpkError::BadSignature(_))
    ));

    let (mut dir, _) = Builder::v2(sample_files()).build();
    dir[4..8].copy_from_slice(&0x0003_0002u32.to_le_bytes()); // Respawn's VPK variant
    assert!(matches!(
        Vpk::from_bytes(dir, MemoryArchives::empty()),
        Err(VpkError::UnsupportedVersion(0x0003_0002))
    ));
}

#[test]
fn rejects_a_broken_terminator() {
    let (mut dir, archives) = Builder::v2(sample_files()).build();
    // The terminator is the last 2 bytes of the 18-byte record.
    let terminator = record_offset(&dir, "white") + 16;
    dir[terminator] = 0;
    let err = Vpk::from_bytes(dir, archives).unwrap_err();
    assert!(
        matches!(err, VpkError::BadTerminator { ref path, .. } if path == "materials/dev/white.vmat_c"),
        "{err}"
    );
}

#[test]
fn truncated_files_report_what_was_missing() {
    let (dir, archives) = Builder::v2(sample_files()).build();
    let err = Vpk::from_bytes(dir[..20].to_vec(), archives).unwrap_err();
    assert!(matches!(err, VpkError::UnexpectedEof(_)), "{err}");
}
