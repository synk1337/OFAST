//! Developer tool: bake skins onto weapon materials as an export does (M10, `export::paint::bake`), to look at the
//! painted colour and metallic-roughness textures of any kit without a demo that carries it.
//!
//! ```bash
//! cargo run --release --example paint_bake -- out/ 1217 700 0.0513 \
//!     weapons/models/usp_silencer/materials/weapon_pist_usp_silencer.vmat
//! ```
//!
//! Arguments after the output folder come in fours: kit, seed, wear, weapon material. Each writes
//! `<kit name>_<seed>_<wear>.png` and `…_orm.png`, or prints why the skin is not painted.

use std::path::Path;
use std::sync::Arc;

use ofast_core::content::Content;
use ofast_core::export::gltf::write_png;
use ofast_core::export::paint::{Paint, bake};
use ofast_core::game::Cs2Install;
use ofast_core::paint::{kit_recipe, paint_kits};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, jobs)) = args.split_first() else {
        panic!("usage: paint_bake <out dir> (<kit> <seed> <wear> <weapon material>)...");
    };
    assert!(
        !jobs.is_empty() && jobs.len().is_multiple_of(4),
        "kit, seed, wear, material"
    );
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives");
    let items = content
        .read("scripts/items/items_game.txt")
        .expect("items_game");
    let kits = paint_kits(&String::from_utf8_lossy(&items));
    std::fs::create_dir_all(out).expect("out dir");
    for job in jobs.chunks(4) {
        let [kit, seed, wear, material] = job else {
            unreachable!()
        };
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
            stattrak: None,
        };
        match bake(&content, material, &paint) {
            Ok((color, surface)) => {
                let path = Path::new(out).join(format!("{}.png", paint.label));
                write_png(&path, &color).expect("colour written");
                write_png(
                    &Path::new(out).join(format!("{}_orm.png", paint.label)),
                    &surface,
                )
                .expect("surface written");
                println!("{}: {}", paint.label, path.display());
            }
            Err(why) => println!("{}: {why}", paint.label),
        }
    }
}
