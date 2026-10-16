//! Exporter tests against the developer's real CS2 install: the texture folder rules of `export::gltf` ("Texture
//! files" in its module docs). Run with:
//! `cargo test --release --features game-data --test cs2_export -- --nocapture`.

#![cfg(feature = "game-data")]

use std::collections::BTreeSet;
use std::fs;

use ofast_core::content::Content;
use ofast_core::export::gltf::export_model;
use ofast_core::game::Cs2Install;

const AK47: &str = "weapons/models/ak47/weapon_rif_ak47.vmdl_c";

/// The `textures/…` URIs of a written glTF.
fn image_uris(gltf: &std::path::Path) -> Vec<String> {
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(gltf).expect("glTF reads")).expect("glTF parses");
    document["images"]
        .as_array()
        .expect("images")
        .iter()
        .map(|image| image["uri"].as_str().expect("uri").to_owned())
        .collect()
}

#[test]
fn re_exports_leave_only_the_files_the_gltf_uses() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("pak01 opens");
    let out = std::env::temp_dir().join(format!("ofast-export-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);

    let first = export_model(&content, AK47, &out).expect("AK-47 exports");
    // What an older export or an interrupted one would have left behind (a `.part` number this export does not
    // use itself: it writes `<texture index>.part` while it works).
    fs::write(out.join("textures/weapon_rif_ak47_old_color.png"), b"x").expect("write");
    fs::write(out.join("textures/999.part"), b"x").expect("write");
    let second = export_model(&content, AK47, &out).expect("AK-47 exports again");
    println!("{first:?}\n{second:?}");
    assert_eq!(second.removed_textures, 2);
    assert_eq!(first.textures, second.textures);

    let uris = image_uris(&second.gltf);
    let unique: BTreeSet<&String> = uris.iter().collect();
    assert_eq!(unique.len(), uris.len(), "one image per file");
    let on_disk: BTreeSet<String> = fs::read_dir(out.join("textures"))
        .expect("textures folder")
        .map(|e| {
            format!(
                "textures/{}",
                e.expect("entry").file_name().to_string_lossy()
            )
        })
        .collect();
    assert_eq!(
        on_disk,
        uris.iter().cloned().collect(),
        "exactly the referenced files"
    );
    let _ = fs::remove_dir_all(&out);
}
