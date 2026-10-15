//! Oracle tests for the resource container and binary KV3 decoder against the developer's real CS2 install.
//!
//! For every compiled file type (`*_c`) in `pak01_dir.vpk`, a spread-out sample of files is split into blocks and
//! every KV3 block is decoded. The decoder must succeed **and consume every byte of every lane**: a decoder that
//! misunderstands the format would leave bytes behind or run out early. The test prints a summary of which KV3
//! versions and compressions CS2 actually uses (copied into `docs/formats/kv3.md`).
//!
//! Only runs with `cargo test --release --features game-data -- --nocapture`.

#![cfg(feature = "game-data")]

use std::collections::BTreeMap;

use ofast_core::game::Cs2Install;
use ofast_core::kv3::binary::{decode_with_report, is_kv3};
use ofast_core::resource::{BlockKind, Resource};
use ofast_core::vpk::Vpk;

/// Files sampled per extension. Set `OFAST_FULL_SCAN=1` to decode every compiled file instead (slow).
const PER_EXTENSION: usize = 120;

fn per_extension_limit() -> usize {
    if std::env::var_os("OFAST_FULL_SCAN").is_some() {
        usize::MAX
    } else {
        PER_EXTENSION
    }
}

#[test]
fn every_sampled_kv3_block_decodes_exactly() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");

    let mut by_extension: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for entry in vpk.entries() {
        if let Some((_, ext)) = entry.path().rsplit_once('.')
            && ext.ends_with("_c")
        {
            by_extension.entry(ext).or_default().push(entry);
        }
    }

    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut inexact = Vec::new();
    let mut blocks_decoded = 0usize;
    let mut legacy = 0usize;

    let limit = per_extension_limit();
    for entries in by_extension.values() {
        let step = (entries.len() / limit).max(1);
        for entry in entries.iter().step_by(step).take(limit) {
            let data = vpk.read(entry).expect("CRC-checked read");
            let resource = match Resource::parse(&data) {
                Ok(resource) => resource,
                Err(e) => {
                    failures.push(format!("{}: container: {e}", entry.path()));
                    continue;
                }
            };
            for block in resource.blocks() {
                let bytes = resource.block_data(block);
                if !is_kv3(bytes) {
                    continue;
                }
                if bytes.starts_with(b"VKV\x03") {
                    legacy += 1;
                    continue;
                }
                match decode_with_report(bytes) {
                    Ok((_, report)) => {
                        blocks_decoded += 1;
                        let compression = ["none", "lz4", "zstd"][report.compression as usize];
                        *formats
                            .entry(format!("v{} {compression}", report.version))
                            .or_default() += 1;
                        if !report.is_exact() || report.blob_marker_ok == Some(false) {
                            inexact.push(format!("{} {}: {report:?}", entry.path(), block.kind));
                        }
                    }
                    Err(e) => failures.push(format!("{} {} block: {e}", entry.path(), block.kind)),
                }
            }
        }
    }

    println!("extensions sampled: {}", by_extension.len());
    println!("KV3 blocks decoded: {blocks_decoded} (legacy VKV3 skipped: {legacy})");
    println!("versions/compressions: {formats:?}");
    for line in failures.iter().take(20) {
        println!("FAIL {line}");
    }
    for line in inexact.iter().take(20) {
        println!("INEXACT {line}");
    }
    assert!(blocks_decoded > 1000, "decoded {blocks_decoded} blocks");
    assert!(failures.is_empty(), "{} failures", failures.len());
    assert!(inexact.is_empty(), "{} inexact decodes", inexact.len());
}

#[test]
fn a_material_has_its_shader_and_textures() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");
    let entry = vpk
        .entries()
        .iter()
        .find(|e| e.path().starts_with("materials/") && e.path().ends_with(".vmat_c"))
        .expect("CS2 ships materials");
    let data = vpk.read(entry).unwrap();
    let resource = Resource::parse(&data).unwrap();
    let material = resource.kv3(BlockKind::DATA).unwrap();
    let shader = material.root.get("m_shaderName").and_then(|v| v.as_str());
    println!("{} uses shader {shader:?}", entry.path());
    assert!(shader.is_some_and(|s| s.ends_with(".vfx")), "{shader:?}");
    assert!(
        material
            .root
            .get("m_textureParams")
            .and_then(|v| v.as_array())
            .is_some()
    );
}
