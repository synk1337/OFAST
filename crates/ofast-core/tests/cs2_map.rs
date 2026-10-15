//! Oracle tests for the map reader against the developer's real CS2 install (de_mirage).
//!
//! Checks the counts the format research found in de_mirage (2026-10-15 build) and that every model the world
//! references exists in the map archive or pak01. Run with:
//! `cargo test --release --features game-data --test cs2_map -- --nocapture`.

#![cfg(feature = "game-data")]

use std::collections::BTreeMap;

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::{Map, object_flags};

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
