//! Developer tool: what a skin is painted with (M10, `ofast_core::paint`): the paint kit, its composite material's
//! recipe (paint material, loose variables, rolls) and the parameters a weapon material's skin gets for a seed and a
//! wear.
//!
//! ```bash
//! cargo run --release --example paint_params -- 180 245 0.1434 materials/models/weapons/v_models/rif_ak47/ak47.vmat
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::paint::{kit_recipe, paint_kits, weapon_params};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [kit, seed, wear, material] = args.as_slice() else {
        panic!("usage: paint_params <paint kit> <seed> <wear> <weapon material>");
    };
    let (kit, seed, wear): (u32, u32, f32) = (
        kit.parse().expect("kit"),
        seed.parse().expect("seed"),
        wear.parse().expect("wear"),
    );
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives");
    let items = content
        .read("scripts/items/items_game.txt")
        .expect("items_game");
    let kits = paint_kits(&String::from_utf8_lossy(&items));
    let kit = kits.get(&kit).expect("paint kit");
    println!("{kit:?}");
    let recipe = kit_recipe(&content, kit).expect("recipe");
    println!("paint material: {:?}", recipe.paint_material);
    for v in &recipe.variables {
        println!(
            "  {}.{} = {:?} bounds {:?}",
            v.container, v.name, v.value, v.bounds
        );
    }
    for (container, name, n) in &recipe.rolls {
        println!("  roll {container}.{name} ×{n}");
    }
    println!("rolled with seed {seed}: {:?}", recipe.roll(seed));
    let params = weapon_params(&content, &recipe, material, seed, wear).expect("params");
    for (name, value) in params {
        println!("  {name} = {value:?}");
    }
}
