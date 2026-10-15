//! Oracle tests for the VPK reader against the developer's real CS2 install.
//!
//! The VPK format is self-verifying (MD5 checksums of the tree and of 1 MiB archive chunks, CRC32 per file), so the
//! game's own data is the oracle: if our reader disagrees with Valve's checksums, our reader is wrong.
//!
//! Only runs with `cargo test --features game-data`. Never runs in CI (no CS2 there).

#![cfg(feature = "game-data")]

use ofast_core::game::Cs2Install;
use ofast_core::vpk::{HashType, Vpk};

fn open_main_vpk() -> Vpk {
    let install = Cs2Install::locate()
        .expect("CS2 install found (set OFAST_CS2_DIR if it is not in the default place)");
    Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens")
}

#[test]
fn directory_checksums_match_valve() {
    let vpk = open_main_vpk();
    assert_eq!(vpk.header().version, 2);
    assert!(vpk.len() > 10_000, "CS2 has many files, got {}", vpk.len());
    assert!(
        vpk.verify_checksums().expect("checksums verify"),
        "CS2 VPK has v2 checksums"
    );
}

#[test]
fn sampled_files_pass_their_crc() {
    let vpk = open_main_vpk();
    // Every 500th entry: spread across many archives, a few hundred files total.
    let mut bytes_read = 0u64;
    for entry in vpk.entries().iter().step_by(500) {
        let data = vpk
            .read(entry)
            .unwrap_or_else(|e| panic!("{}: {e}", entry.path()));
        bytes_read += data.len() as u64;
    }
    assert!(bytes_read > 0);
}

#[test]
fn well_known_files_are_present() {
    let vpk = open_main_vpk();
    let items = vpk
        .read_path("scripts/items/items_game.txt")
        .expect("items_game.txt reads");
    assert!(
        items.starts_with(b"\"items_game\""),
        "items_game.txt starts with its root key"
    );
}

#[test]
fn sampled_archive_chunks_match_their_md5() {
    let vpk = open_main_vpk();
    let chunks = vpk.chunk_hashes();
    assert!(!chunks.is_empty());
    let mut by_type = std::collections::BTreeMap::new();
    for chunk in chunks {
        *by_type
            .entry(format!("{:?}", chunk.hash_type))
            .or_insert(0usize) += 1;
    }
    println!("CS2 chunk hash types: {by_type:?}");
    assert!(
        chunks
            .iter()
            .all(|c| matches!(c.hash_type, HashType::Md5 | HashType::Blake3)),
        "only MD5 and BLAKE3 are expected: {by_type:?}"
    );
    // ~25 MiB of reads per hash type, spread over the whole set.
    for hash_type in [HashType::Md5, HashType::Blake3] {
        let of_type: Vec<_> = chunks.iter().filter(|c| c.hash_type == hash_type).collect();
        for chunk in of_type.iter().step_by((of_type.len() / 25).max(1)) {
            vpk.verify_chunk(chunk).unwrap_or_else(|e| {
                panic!(
                    "{hash_type:?} archive {:03} @ {}: {e}",
                    chunk.archive_index, chunk.offset
                )
            });
        }
    }
}
