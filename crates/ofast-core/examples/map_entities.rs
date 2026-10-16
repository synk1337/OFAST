//! Developer tool: print every key/value of a map's entities of the given classes (or all class names with counts).
//!
//! ```bash
//! cargo run --release --example map_entities -- de_mirage                     # class names with counts
//! cargo run --release --example map_entities -- de_mirage env_sky light_environment
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::Map;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().cloned().unwrap_or_else(|| "de_mirage".into());
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &name).expect("archives open");
    let map = Map::load(&content, &name).expect("map loads");

    if args.len() < 2 {
        let mut counts = std::collections::BTreeMap::new();
        for entity in &map.entities {
            *counts
                .entry(entity.class_name().unwrap_or("?"))
                .or_insert(0) += 1;
        }
        for (class, count) in counts {
            println!("{count:>6}  {class}");
        }
        return;
    }
    for class in &args[1..] {
        for entity in map.entities_of(class) {
            println!("--- {class}");
            let mut properties: Vec<_> = entity.properties().collect();
            properties.sort_by_key(|(key, _)| *key);
            for (key, value) in properties {
                println!("  {key} = {value:?}");
            }
        }
    }
}
