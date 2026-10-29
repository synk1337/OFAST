//! Developer tool: bake a weapon skin with stickers as the export does (M10, `export::stickers`), to look at where
//! and how the stickers land without a demo round's export.
//!
//! ```bash
//! cargo run --release --example sticker_bake -- out/ weapons/models/mp9/weapon_smg_mp9.vmdl \
//!     weapons/models/mp9/materials/weapon_smg_mp9.vmat 33 606 0.0092 0:1030 1:1070 2:1173:0.1,0.2:45
//! ```
//!
//! After the model and material: the paint kit, seed and wear (kit 0: the weapon's own textures are not baked, so
//! one is needed), then one sticker per argument: `slot:kit[:offset x,y][:rotation degrees][:scale][:wear]`, `-`
//! for a value the item leaves to the slot. Writes `<label>.png` and `<label>_orm.png`.

use std::path::Path;
use std::sync::Arc;

use ofast_core::content::Content;
use ofast_core::export::gltf::{load_model, write_png};
use ofast_core::export::paint::{Paint, bake};
use ofast_core::export::stickers::{Placed, Target, apply, places, sticker_kits};
use ofast_core::game::Cs2Install;
use ofast_core::paint::{kit_recipe, paint_kits};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [out, model, material, kit, seed, wear, stickers @ ..] = args.as_slice() else {
        panic!(
            "usage: sticker_bake <out dir> <model> <material> <kit> <seed> <wear> <slot:kit[:x,y][:deg][:scale][:wear]>..."
        );
    };
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives");
    let items = content
        .read("scripts/items/items_game.txt")
        .expect("items_game");
    let items = String::from_utf8_lossy(&items);
    let kits = paint_kits(&items);
    let sticker_kits = sticker_kits(&items);
    let kit = &kits[&kit.parse::<u32>().expect("kit")];
    let (seed, wear) = (
        seed.parse::<u32>().expect("seed"),
        wear.parse::<f32>().expect("wear"),
    );
    let paint = Paint {
        label: Paint::label_of(&kit.name, seed, wear),
        recipe: Some(Arc::new(kit_recipe(&content, kit).expect("recipe"))),
        seed,
        wear,
        stickers: Vec::new(),
        name_tag: None,
    };
    let (mut color, mut surface) = bake(&content, material, &paint).expect("painted");
    let placed: Vec<Placed> = stickers
        .iter()
        .map(|s| {
            let parts: Vec<&str> = s.split(':').collect();
            let value = |i: usize| parts.get(i).filter(|v| **v != "-");
            let id: u32 = parts[1].parse().expect("sticker kit");
            Placed {
                slot: parts[0].parse().expect("slot"),
                material: sticker_kits[&id].material.clone(),
                offset: value(2).map(|v| {
                    let (x, y) = v.split_once(',').expect("x,y");
                    [x.parse().expect("x"), y.parse().expect("y")]
                }),
                rotation: value(3).map(|v| v.parse().expect("degrees")),
                scale: value(4).map(|v| v.parse().expect("scale")),
                wear: value(5).map(|v| v.parse().expect("wear")),
            }
        })
        .collect();
    let model = load_model(&content, model).expect("model");
    let places = places(&model, material);
    println!(
        "{} triangles with both UV sets, {} marked-up slots",
        places.triangles.len(),
        places.slots.len()
    );
    let mut warnings = apply(
        &content,
        material,
        &placed,
        &places,
        &mut color,
        Target::Color,
    )
    .expect("stickers drawn");
    warnings.extend(
        apply(
            &content,
            material,
            &placed,
            &places,
            &mut surface,
            Target::Surface,
        )
        .expect("stickers drawn"),
    );
    for w in warnings {
        println!("warning: {w}");
    }
    std::fs::create_dir_all(out).expect("out dir");
    let name = format!("{}_stickers", paint.label);
    write_png(&Path::new(out).join(format!("{name}.png")), &color).expect("colour");
    write_png(&Path::new(out).join(format!("{name}_orm.png")), &surface).expect("surface");
    println!("wrote {name}.png and {name}_orm.png");
}
