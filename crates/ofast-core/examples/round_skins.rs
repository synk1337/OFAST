//! Developer tool: the finish of every weapon of a round (M10): who carried it first, its paint kit by name, seed,
//! wear, StatTrak, name tag, stickers and keychain, as the demo's econ attributes say (`demo::skins`).
//!
//! ```bash
//! cargo run --release --example round_skins -- path/to/match.dem 17
//! ```

use std::collections::HashMap;

use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::game::Cs2Install;
use ofast_core::items::{item_names, language_tokens, localized, section_values};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args[1].parse().expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives");
    let text = |path: &str| String::from_utf8_lossy(&content.read(path).expect(path)).into_owned();
    let items_game = text("scripts/items/items_game.txt");
    let english = language_tokens(&text("resource/csgo_english.txt"));
    let items = item_names(&items_game);
    let paint_tags = section_values(&items_game, "paint_kits", "description_tag");
    let paint_names = section_values(&items_game, "paint_kits", "name");
    let sticker_tags = section_values(&items_game, "sticker_kits", "item_name");
    let name_of = |tags: &HashMap<u32, String>, id: u32| {
        tags.get(&id)
            .and_then(|t| localized(&english, t))
            .unwrap_or("?")
            .to_owned()
    };

    let track = track_round(&data, round).expect("round");
    // Who carried each weapon first.
    let mut carriers: HashMap<u32, String> = HashMap::new();
    for sample in &track.samples {
        for player in &sample.players {
            for weapon in &player.weapons {
                carriers
                    .entry(weapon.entity)
                    .or_insert_with(|| player.name.clone());
            }
        }
    }
    println!("round {round}: {} weapons", track.skins.len());
    for (entity, skin) in &track.skins {
        let item = items.get(&skin.item).map_or("?", String::as_str);
        let carrier = carriers.get(entity).map_or("(lying)", String::as_str);
        if skin.paint_kit == 0 && skin.stickers.is_empty() && skin.name_tag.is_none() {
            println!("  #{entity:<4} {carrier:14} {item:24} plain");
            continue;
        }
        let paint = match skin.paint_kit {
            0 => "no paint".to_owned(),
            kit => format!(
                "{} ({kit} {}) seed {} wear {:.4}",
                name_of(&paint_tags, kit),
                paint_names.get(&kit).map_or("?", String::as_str),
                skin.seed,
                skin.wear
            ),
        };
        println!("  #{entity:<4} {carrier:14} {item:24} {paint}");
        if let Some(count) = skin.stattrak {
            println!("        StatTrak {count}");
        }
        if let Some(tag) = &skin.name_tag {
            println!("        name tag \"{tag}\"");
        }
        for s in &skin.stickers {
            println!(
                "        sticker slot {} {} ({}) wear {:?} scale {:?} rotation {:?} offset {:?}",
                s.slot,
                name_of(&sticker_tags, s.id),
                s.id,
                s.wear,
                s.scale,
                s.rotation,
                s.offset
            );
        }
        if let Some(k) = &skin.keychain {
            println!(
                "        keychain {} offset {:?} seed {:?}",
                k.id, k.offset, k.seed
            );
        }
    }
}
