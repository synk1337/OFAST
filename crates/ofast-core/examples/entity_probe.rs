//! Developer tool: every networked field of the entities whose class name contains a filter, by name, printed when
//! the entity first appears in the tick range and then whenever a field changes. Used to find what the demo says
//! about doors (their state and rotation) and other map entities.
//!
//! ```bash
//! cargo run --release --example entity_probe -- path/to/match.dem 29322 29400 Door [field filter]
//! ```
//!
//! Arguments: demo, first and last demo tick, class name filter, optionally a substring of the field names to show.

use std::collections::{BTreeMap, HashMap};

use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let (first, last): (u32, u32) = (
        args[1].parse().expect("first tick"),
        args[2].parse().expect("last tick"),
    );
    let class_filter = args.get(3).cloned().unwrap_or_default();
    let field_filter = args.get(4).cloned().unwrap_or_default();
    let mut parser = DemoParser::new(&data).expect("demo header");
    let mut previous: HashMap<u32, BTreeMap<String, String>> = HashMap::new();
    while let Some(frame) = parser.next_frame().expect("frame parses") {
        let Some(tick) = frame.tick else { continue };
        if tick < first || !frame.entities_changed {
            continue;
        }
        if tick > last {
            break;
        }
        let serializers = parser.serializers().expect("serializers");
        for e in parser.entities().iter() {
            let class = parser.class_name(e.class_id).unwrap_or("?");
            if !class.contains(class_filter.as_str()) {
                continue;
            }
            let stored =
                |p: &ofast_core::demo::fieldpath::FieldPath| e.get(p).and_then(FieldValue::as_u64);
            let mut now = BTreeMap::new();
            for (path, value) in e.fields() {
                let name = serializers
                    .resolve(e.serializer, path, stored)
                    .map(|r| serializers.fields[r.field].name.clone())
                    .unwrap_or_else(|_| format!("{:?}", path.indices()));
                if !name.contains(field_filter.as_str()) {
                    continue;
                }
                // Array elements share the array's name: keep their indices apart.
                let key = format!("{name} {:?}", path.indices());
                now.insert(key, format!("{value:?}"));
            }
            let before = previous.get(&e.index);
            let changed: Vec<String> = now
                .iter()
                .filter(|(k, v)| before.and_then(|b| b.get(*k)) != Some(*v))
                .map(|(k, v)| format!("{k} = {v}"))
                .collect();
            if !changed.is_empty() {
                let label = if before.is_none() { "new" } else { "changed" };
                println!("{tick:6} #{} {class} {label}:", e.index);
                for line in changed {
                    println!("         {line}");
                }
                previous.insert(e.index, now);
            }
        }
    }
}
