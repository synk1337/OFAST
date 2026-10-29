//! StatTrak counters on weapons (M10): the module CS2 shows a StatTrak weapon's kill count on
//! (`docs/research/cs2-skins.md`, "StatTrak").
//!
//! # How CS2 shows them
//!
//! A StatTrak weapon carries the StatTrak module (`weapons/models/shared/stattrak/stattrak_module.vmdl`, a small box
//! with a display; on knives `stattrak_module_knife.vmdl`, the display alone) on its model's `stattrak` attachment
//! (`stattrak_legacy` on the CS:GO-era meshes). The display (`stattrak_module_display.vmat`: `csgo_weapon.vfx` with
//! `F_STATTRAK`, self-illuminated) draws six digits from a digit atlas, picked in the vertex shader (static combo
//! `S_STATTRAK`) from `g_nStatTrakValue`, which the game sets to the weapon's count (the demo's attribute 80, counted
//! on during the round: [`crate::demo::players::RoundTrack::stattrak_at`]):
//!
//! - a digit's vertices carry a negative v in their first UV set and a u in [d + 1, d + 2) for digit d (0 the
//!   hundred-thousands … 5 the units), its fraction over the atlas's cell d (the cells are 1/16 wide, in the
//!   atlas's top row);
//! - the shader moves them to cell 1 + the digit, or to cell 0 (a dim zero) for a leading zero: d = ⌊u⌋ − 1,
//!   n = count / 10^(5 − d) (whole), u += (cell − d) / 16 with cell = 1 + n mod 10 when n ≠ 0 or d = 5, else 0.
//!
//! # As exported
//!
//! The module joins the weapon's meshes, rigid on the attachment's bone, its digits' UVs set for a count as they
//! are written ([`digit_uv`], through [`super::paint::Paint::stattrak`]). A weapon whose count changes during the
//! round carries one module per count, each on a bone of its own, and the animation scales the others to 0
//! (`viewmodel`, `players`).

use std::collections::BTreeMap;

use crate::content::Content;
use crate::export::gltf::load_model;
use crate::export::loadout::{Finish, module_mount};
use crate::model::{Affine, Model};

/// The module on guns.
pub const MODULE: &str = "weapons/models/shared/stattrak/stattrak_module.vmdl";
/// The module on knives: the display alone.
pub const KNIFE_MODULE: &str = "weapons/models/shared/stattrak/stattrak_module_knife.vmdl";

/// The width of a digit atlas cell, in the texture's 0 – 1.
const CELL: f32 = 1.0 / 16.0;
/// What each digit position divides the count by.
const PLACES: [u32; 6] = [100_000, 10_000, 1_000, 100, 10, 1];

/// A StatTrak display vertex's first-UV-set coordinate `uv` as CS2's vertex shader shows `count` with it (module
/// docs), wrapped into 0 – 1 (the texture repeats); a vertex that is not a digit's (v ≥ 0) keeps its own.
pub fn digit_uv(uv: [f32; 2], count: u32) -> [f32; 2] {
    if uv[1] >= 0.0 {
        return uv;
    }
    // The shader moves u by the place as it is and divides by the place clamped to the six digits.
    let place = uv[0].floor() - 1.0;
    let digit = (place as i32).clamp(0, 5) as usize;
    let n = count / PLACES[digit];
    let cell = if n != 0 || digit == 5 { 1 + n % 10 } else { 0 };
    let u = uv[0] + (cell as f32 - place) * CELL;
    [u - u.floor(), uv[1] - uv[1].floor()]
}

/// The module `weapon_model` carries.
pub fn module_for(weapon_model: &str) -> &'static str {
    if weapon_model.contains("/knife/") {
        KNIFE_MODULE
    } else {
        MODULE
    }
}

/// Where the module sits on `weapon` with `mesh_groups` shown: the bone it follows and the module's space → the
/// weapon model's space. `None` without the `stattrak` attachment.
pub fn mount(weapon: &Model, mesh_groups: u64) -> Option<(usize, Affine)> {
    module_mount(weapon, mesh_groups, "stattrak")
}

/// The modules the StatTrak weapons among `weapons` (model path, finish) carry, by path: `None` for one that cannot
/// be read (with a warning).
pub(crate) fn load_modules<'a>(
    content: &Content,
    weapons: impl IntoIterator<Item = (&'a str, Option<Finish>)>,
    warnings: &mut Vec<String>,
) -> BTreeMap<&'static str, Option<Model>> {
    let mut modules = BTreeMap::new();
    for (model, finish) in weapons {
        if finish.and_then(|f| f.stattrak).is_none() {
            continue;
        }
        modules.entry(module_for(model)).or_insert_with_key(|path| {
            load_model(content, path)
                .map_err(|e| warnings.push(format!("StatTrak module {path}: {e}: not shown")))
                .ok()
        });
    }
    modules
}

/// The StatTrak module `weapon_model` carries with `finish`, from `modules` ([`load_modules`]).
pub(crate) fn module<'m>(
    modules: &'m BTreeMap<&'static str, Option<Model>>,
    weapon_model: &str,
    finish: Option<Finish>,
) -> Option<&'m Model> {
    finish?.stattrak?;
    modules.get(module_for(weapon_model))?.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A digit vertex of place `digit` at the left (`side` 0) or right (1) of its cell, as the module stores it.
    fn vertex(digit: usize, side: f32) -> [f32; 2] {
        [
            digit as f32 + 1.0 + digit as f32 * CELL + side * 0.0547,
            -1.0,
        ]
    }

    /// The atlas cell a vertex shows.
    fn cell(uv: [f32; 2]) -> u32 {
        (uv[0] / CELL).floor() as u32
    }

    #[test]
    fn digits_show_the_count_with_dim_leading_zeros() {
        // 1471: places 0 and 1 are leading zeros (cell 0), then 1, 4, 7, 1 (cells 2, 5, 8, 2).
        let cells: Vec<u32> = (0..6)
            .map(|d| cell(digit_uv(vertex(d, 0.0), 1471)))
            .collect();
        assert_eq!(cells, [0, 0, 2, 5, 8, 2]);
        // The units show even at 0; the right edge stays in its cell.
        assert_eq!(cell(digit_uv(vertex(5, 0.0), 0)), 1);
        assert_eq!(cell(digit_uv(vertex(5, 1.0), 0)), 1);
        assert_eq!(cell(digit_uv(vertex(0, 1.0), 654_321)), 7);
        // v wraps into the atlas's top row; the module's other vertices keep their own.
        assert!((digit_uv(vertex(3, 0.0), 9)[1]).abs() < 1e-6);
        assert_eq!(digit_uv([0.25, 0.5], 9), [0.25, 0.5]);
    }

    #[test]
    fn knives_carry_their_own_module() {
        assert_eq!(
            module_for("weapons/models/knife/knife_karambit/weapon_knife_karambit.vmdl"),
            KNIFE_MODULE
        );
        assert_eq!(
            module_for("weapons/models/m4a4/weapon_rif_m4a4.vmdl"),
            MODULE
        );
    }
}
