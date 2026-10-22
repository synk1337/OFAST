//! Developer tool: which model every entity of a demo shows at one tick — its `m_hModel` (a resource id, see
//! `ofast_core::resource::resource_id`) resolved to a path from CS2's archive — plus each player pawn's gloves.
//!
//! ```bash
//! # round 5, 20 s after its start
//! cargo run --release --example demo_models -- path/to/match.dem 5 20
//! # also write CS2's items_game.txt to a file
//! OFAST_ITEMS_OUT=items_game.txt cargo run --release --example demo_models -- path/to/match.dem 5 20
//! ```

use std::collections::{BTreeMap, HashMap};

use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::players::list_rounds;
use ofast_core::game::Cs2Install;
use ofast_core::resource::resource_id;

const MODEL: &str = "CBodyComponent.m_skeletonInstance.m_modelState.m_hModel";
const MESH_GROUPS: &str = "CBodyComponent.m_skeletonInstance.m_modelState.m_MeshGroupMask";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let seconds: f32 = args.get(2).map_or(10.0, |s| s.parse().expect("seconds"));

    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    // Every model's id → its path (without `_c`).
    let mut models: HashMap<u64, String> = HashMap::new();
    for vpk in content.archives() {
        for entry in vpk.entries() {
            if let Some(path) = entry.path().strip_suffix(".vmdl_c") {
                let path = format!("{path}.vmdl");
                models.insert(resource_id(&path), path);
            }
        }
    }
    println!("{} models in the archive", models.len());
    let items = content
        .read("scripts/items/items_game.txt")
        .expect("items_game");
    if let Some(out) = std::env::var_os("OFAST_ITEMS_OUT") {
        std::fs::write(out, &items).expect("items_game written");
    }
    let names = ofast_core::items::item_names(&String::from_utf8_lossy(&items));

    let info = list_rounds(&data)
        .expect("rounds")
        .into_iter()
        .find(|r| r.round == round)
        .expect("round in the demo");
    let mut parser = DemoParser::new(&data).expect("demo header");
    let interval = 1.0 / 64.0;
    let target = info.start_tick + (seconds / interval) as u32;
    while let Some(frame) = parser.next_frame().expect("frame") {
        if frame.tick.is_some_and(|t| t >= target) && frame.entities_changed {
            break;
        }
    }
    println!("round {round}, tick {target}");
    let serializers = parser.serializers().expect("send tables");
    let classes = parser.classes();
    let field = |class_id: u32, name: &str| {
        classes
            .get(class_id as usize)
            .and_then(Option::as_ref)
            .and_then(|c| serializers.path_of(c.serializer, name))
    };
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    for entity in parser.entities().iter() {
        let class = parser.class_name(entity.class_id).unwrap_or("?").to_owned();
        let Some(id) = field(entity.class_id, MODEL)
            .and_then(|p| entity.get(&p))
            .and_then(FieldValue::as_u64)
        else {
            continue;
        };
        let path = models
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("? {id:016x}"));
        let interesting = class.contains("Player")
            || class.starts_with("CWeapon")
            || class.starts_with("CKnife")
            || class.starts_with("CAK47")
            || class.contains("Wearable")
            || class.contains("Grenade")
            || class == "CC4";
        if interesting {
            let groups = field(entity.class_id, MESH_GROUPS)
                .and_then(|p| entity.get(&p))
                .and_then(FieldValue::as_u64);
            let item = field(
                entity.class_id,
                "m_AttributeManager.m_Item.m_iItemDefinitionIndex",
            )
            .and_then(|p| entity.get(&p))
            .and_then(FieldValue::as_u64);
            let owner = field(entity.class_id, "m_hOwnerEntity")
                .and_then(|p| entity.get(&p))
                .and_then(FieldValue::as_u64)
                .map(|h| h & 0x3fff);
            let gloves = field(entity.class_id, "m_EconGloves.m_iItemDefinitionIndex")
                .and_then(|p| entity.get(&p))
                .and_then(FieldValue::as_u64);
            println!(
                "{:5} {class:28} {path}  groups {groups:?} item {item:?} {} owner {owner:?} gloves {gloves:?} {}",
                entity.index,
                item.and_then(|i| names.get(&(i as u32)))
                    .map_or("", String::as_str),
                gloves
                    .and_then(|i| names.get(&(i as u32)))
                    .map_or("", String::as_str),
            );
        }
        *census.entry(format!("{class} {path}")).or_default() += 1;
    }
    println!("\nall models:");
    for (key, count) in census {
        println!("{count:4}  {key}");
    }
}
