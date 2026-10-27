//! Developer tool: which paint styles and shader features CS2's paint kits use (M10, `export::paint`), to see what
//! the bake must handle. Every kit's colour parameters are read as if painted on one weapon material (the features
//! do not depend on it, but `F_SEPARATE_CHANNEL_INPUTS` does), then counted per feature value.
//!
//! ```bash
//! cargo run --release --example paint_census                         # on the USP-S
//! cargo run --release --example paint_census -- <weapon material> F_OVERLAY_MASK=8   # list the kits with a value
//! ```

use std::collections::BTreeMap;

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::paint::{Param, kit_recipe, paint_kits, weapon_params};

/// The features counted.
const FEATURES: [&str; 12] = [
    "F_PAINT_STYLE",
    "F_OVERLAY_TEXTURE",
    "F_OVERLAY_MASK",
    "F_OVERLAY_BLEND_MODE",
    "g_bOverlayUsesPatternUVs",
    "g_bOverlayUsesUniqueUVs",
    "F_CASE_HARDENING",
    "F_CASE_HARDENING_TRILINEAR",
    "F_PATINA_AGE",
    "F_SPRAYPAINT_HALFTONE",
    "F_USE_ALL_MASKS",
    "F_PEARLESCENCE_MASK",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let material = args.first().map_or(
        "weapons/models/usp_silencer/materials/weapon_pist_usp_silencer.vmat",
        String::as_str,
    );
    let wanted = args.get(1).and_then(|w| w.split_once('='));
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives");
    let items = content
        .read("scripts/items/items_game.txt")
        .expect("items_game");
    let kits = paint_kits(&String::from_utf8_lossy(&items));
    let mut counts: BTreeMap<&str, BTreeMap<i64, usize>> = BTreeMap::new();
    let (mut read, mut failed) = (0, 0);
    for kit in kits.values() {
        let Ok(recipe) = kit_recipe(&content, kit) else {
            failed += 1;
            continue;
        };
        let Ok(params) = weapon_params(&content, &recipe, material, 1, 0.1) else {
            failed += 1;
            continue;
        };
        read += 1;
        let int = |name: &str| match params.color.get(name) {
            Some(Param::Int(i)) => *i,
            _ => 0,
        };
        // A feature only counts where its combo is on (the mask and blend mode of an overlay, its UV switches).
        let on = |name: &str| match name {
            "F_OVERLAY_MASK"
            | "F_OVERLAY_BLEND_MODE"
            | "g_bOverlayUsesPatternUVs"
            | "g_bOverlayUsesUniqueUVs" => int("F_OVERLAY_TEXTURE") != 0,
            _ => true,
        };
        for name in FEATURES.into_iter().filter(|n| on(n)) {
            *counts
                .entry(name)
                .or_default()
                .entry(int(name))
                .or_default() += 1;
        }
        if let Some((name, value)) = wanted
            && on(name)
            && value.parse::<i64>().ok() == Some(int(name))
        {
            println!("  {} {} (style {})", kit.id, kit.name, int("F_PAINT_STYLE"));
        }
    }
    println!("{read} kits read, {failed} without a readable recipe");
    for (name, values) in counts {
        println!("{name}: {values:?}");
    }
}
