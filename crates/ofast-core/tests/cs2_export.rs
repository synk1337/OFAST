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

/// Each gun's muzzle flash as its model's shoot sequences start it (`export::flashes`), against what the particle
/// systems' files said on 2026-11-05: the flash family per gun and the M4A1-S's silencer tip.
#[test]
fn guns_fire_their_own_muzzle_flashes() {
    use ofast_core::export::shots::ShotData;

    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("pak01 opens");
    let items = content
        .read("scripts/items/items_game.txt")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let mut warnings = Vec::new();
    let mut data = ShotData::load(&content, items.as_deref(), &mut warnings);
    data.load_flashes(&content);
    for (name, flashes) in &data.flashes {
        println!("{name}: {flashes:?}");
    }
    let fire = |weapon: &str| {
        let flashes = &data.flashes[weapon];
        let event = flashes.fire.as_ref().expect("a flash");
        (event.system.as_str(), event.attachment.as_str())
    };
    assert_eq!(
        fire("weapon_ak47"),
        ("uweapon_muzflsh_ak47", "muzzle_flash")
    );
    assert_eq!(
        fire("weapon_m4a1"),
        ("uweapon_muzflsh_riffle", "muzzle_flash")
    );
    assert_eq!(fire("weapon_awp").0, "weapon_muzzleflash_snip");
    assert_eq!(fire("weapon_negev").0, "uweapon_muzflsh_mach");
    assert_eq!(fire("weapon_nova").0, "uweapon_muzflsh_shot");
    assert_eq!(fire("weapon_mac10").0, "uweapon_muzzleflash_subm");
    assert_eq!(fire("weapon_glock").0, "uweapon_muzzleflash_pist");
    assert_eq!(
        fire("weapon_deagle"),
        ("uweapon_muzflsh_deagle", "muzzle_flash")
    );
    assert_eq!(
        fire("weapon_m4a1_silencer"),
        ("uweapon_muzsilenced_rif", "muzzle_flash2")
    );
    let m4 = &data.flashes["weapon_m4a1_silencer"];
    assert_eq!(
        m4.unsilenced.as_ref().expect("unsilenced").system,
        "uweapon_muzflsh_ak47"
    );
    assert!(
        !data.flashes.contains_key("weapon_taser"),
        "the Zeus fires no flash"
    );
}
