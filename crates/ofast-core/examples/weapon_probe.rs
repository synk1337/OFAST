//! Developer tool: what the demo networks about weapon entities, tick by tick — owner, weapon state, parent, the
//! body component's position and rotation, and a few physics fields — printed for each weapon when they change.
//! Used to find how dropped weapons move (M4.6: the guns the dead let go of).
//!
//! ```bash
//! cargo run --release --example weapon_probe -- path/to/match.dem 34380 34700 [class filter]
//! ```
//!
//! Arguments: demo, first and last demo tick, optionally a substring of the weapon classes to show (e.g. `M4A1`).

use std::collections::HashMap;

use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let (first, last): (u32, u32) = (
        args[1].parse().expect("first tick"),
        args[2].parse().expect("last tick"),
    );
    let filter = args.get(3).cloned().unwrap_or_default();
    let mut parser = DemoParser::new(&data).expect("demo header");
    let mut previous: HashMap<u32, Vec<String>> = HashMap::new();
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
            let path = |field: &str| serializers.path_of(e.serializer, field);
            // Weapons: entities with an item definition (the classes the players export reads weapons from).
            if path("m_AttributeManager.m_Item.m_iItemDefinitionIndex").is_none() {
                continue;
            }
            let class = parser.class_name(e.class_id).unwrap_or("?");
            if !class.contains(filter.as_str()) {
                continue;
            }
            let value =
                |field: &str| -> Option<&FieldValue> { path(field).and_then(|p| e.get(&p)) };
            let number = |field: &str| -> Option<f64> {
                match value(field)? {
                    FieldValue::Unsigned(u) => Some(*u as f64),
                    FieldValue::Signed(i) => Some(*i as f64),
                    FieldValue::Float(f) => Some(*f as f64),
                    _ => None,
                }
            };
            let axis = |k: &str| -> Option<f64> {
                Some(
                    number(&format!("CBodyComponent.m_cell{k}"))? * 512.0 - 16384.0
                        + number(&format!("CBodyComponent.m_vec{k}"))?,
                )
            };
            let position = match (axis("X"), axis("Y"), axis("Z")) {
                (Some(x), Some(y), Some(z)) => format!("({x:.1}, {y:.1}, {z:.1})"),
                _ => "-".into(),
            };
            let show = |field: &str| value(field).map_or("-".into(), |v| format!("{v:?}"));
            let now = vec![
                format!("owner {}", show("m_hOwnerEntity")),
                format!("state {}", show("m_iState")),
                format!("parent {}", show("CBodyComponent.m_hParent")),
                format!("at {position}"),
                format!("rot {}", show("CBodyComponent.m_angRotation")),
                format!("solid {}", show("m_Collision.m_nSolidType")),
                format!("move {}", show("m_MoveType")),
                format!(
                    "item {}",
                    show("m_AttributeManager.m_Item.m_iItemDefinitionIndex")
                ),
                format!(
                    "model {}",
                    match value("CBodyComponent.m_skeletonInstance.m_modelState.m_hModel") {
                        Some(FieldValue::Unsigned(id)) => format!("{id:016x}"),
                        other => format!("{other:?}"),
                    }
                ),
                format!(
                    "groups {}",
                    show("CBodyComponent.m_skeletonInstance.m_modelState.m_MeshGroupMask")
                ),
            ];
            if previous.get(&e.index) != Some(&now) {
                println!("{tick:6} #{:<4} {class:<24} {}", e.index, now.join(" | "));
                previous.insert(e.index, now);
            }
        }
    }
}
