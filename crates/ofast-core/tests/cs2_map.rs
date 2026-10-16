//! Oracle tests for the map reader against the developer's real CS2 install (de_mirage).
//!
//! Checks the counts the format research found in de_mirage (2026-10-15 build) and that every model the world
//! references exists in the map archive or pak01. Run with:
//! `cargo test --release --features game-data --test cs2_map -- --nocapture`.

#![cfg(feature = "game-data")]

use std::collections::BTreeMap;

use ofast_core::content::Content;
use ofast_core::export::sky::export_sky;
use ofast_core::game::Cs2Install;
use ofast_core::map::{Map, object_flags, sky_to_world};
use ofast_core::vpk::Vpk;

#[test]
fn de_mirage_loads_with_its_world_and_entities() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, "de_mirage").expect("map archive opens");
    let map = Map::load(&content, "de_mirage").expect("map loads");

    let objects: usize = map.nodes.iter().map(|n| n.scene_objects.len()).sum();
    let aggregates: usize = map.nodes.iter().map(|n| n.aggregates.len()).sum();
    let fragments: usize = map
        .nodes
        .iter()
        .flat_map(|n| &n.aggregates)
        .map(|a| a.fragments.len())
        .sum();
    let placed: usize = map
        .nodes
        .iter()
        .flat_map(|n| &n.aggregates)
        .flat_map(|a| &a.fragments)
        .filter(|f| f.transform.is_some())
        .count();
    let overlays = map
        .nodes
        .iter()
        .flat_map(|n| &n.scene_objects)
        .filter(|o| o.flags & object_flags::OVERLAY != 0)
        .count();
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    for entity in &map.entities {
        *classes
            .entry(entity.class_name().unwrap_or("?"))
            .or_default() += 1;
    }
    println!(
        "de_mirage: {} nodes, {objects} scene objects ({overlays} overlays), {aggregates} aggregates, \
         {fragments} fragments ({placed} with own transform), {} entities",
        map.nodes.len(),
        map.entities.len()
    );
    println!("entity classes: {classes:?}");

    assert!(objects > 100 && aggregates > 100 && fragments > 1000);
    assert!(map.entities.len() > 100);
    assert_eq!(map.entities_of("light_environment").count(), 1);
    assert!(map.entities_of("info_player_counterterrorist").count() >= 5);

    for node in &map.nodes {
        for model in node
            .scene_objects
            .iter()
            .map(|o| &o.model)
            .chain(node.aggregates.iter().map(|a| &a.model))
        {
            assert!(
                content.exists(&format!("{}_c", model.replace('\\', "/"))),
                "missing {model}"
            );
        }
    }
}

#[test]
fn de_mirage_3d_skybox_and_sky_load() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let mut content = Content::for_map(&install, "de_mirage").expect("map archive opens");
    let map = Map::load(&content, "de_mirage").expect("map loads");

    let reference = map.skybox_reference().expect("de_mirage has a 3D skybox");
    assert_eq!(reference.world, "prefabs/de_mirage/3dskybox_mirage_legacy");
    content.insert(
        1,
        Vpk::open(install.csgo_dir().join(&reference.archive)).expect("skybox archive opens"),
    );
    let sky = Map::load(&content, &reference.world).expect("skybox world loads");
    let (camera, scale) = sky.sky_camera().expect("skybox has a sky_camera");
    println!(
        "sky camera {camera:?} scale {scale}, {} nodes",
        sky.nodes.len()
    );
    assert_eq!(scale, 16.0);
    assert!(
        sky.nodes
            .iter()
            .map(|n| n.scene_objects.len() + n.aggregates.len())
            .sum::<usize>()
            > 50
    );

    // The sky drawn behind everything is the skybox world's env_sky: an HDR cube map.
    let env_sky = sky
        .entities_of("env_sky")
        .next()
        .expect("skybox has an env_sky");
    let out = std::env::temp_dir().join("ofast_test_sky.hdr");
    let frame = sky_to_world(camera, scale, &reference);
    let exported = export_sky(&content, env_sky, &frame, &out)
        .expect("sky exports")
        .expect("sky is a cube map");
    println!("sky {exported:?}");
    assert_eq!((exported.width, exported.height), (2048, 1024));
    // Average radiance close to the texture's stored reflectivity (0.742, 0.788, 0.844).
    assert!(
        (exported.mean[2] - 0.844).abs() < 0.05,
        "mean {:?}",
        exported.mean
    );
    std::fs::remove_file(out).ok();
}
