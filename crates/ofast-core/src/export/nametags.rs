//! Name tags on weapons (M10): the data plate CS2 shows a weapon's custom name on (`docs/research/cs2-skins.md`,
//! "Name tags").
//!
//! # How CS2 shows them
//!
//! A named weapon carries the name tag module (`weapons/models/shared/nametag/nametag_module.vmdl`, a small plate)
//! on its model's `nametag` attachment (`nametag_legacy` on the CS:GO-era meshes). The plate's material
//! (`nametag_default.vmat`) is a composite (`nametag_default.vcompmat`): its colour regenerated at 512 from the
//! plate's own (`nametag_color`, a dark panel above "INVENTORY UID DATAPLATE SERIAL NO") with the name drawn on it
//! (`COMP_MAT_PROPERTY_MUTATOR_DRAW_TEXT`: at (0.04, 0.038) of the texture, colour (241, 241, 241), font
//! "Arial Narrow"; the composite names no size).
//!
//! # As exported
//!
//! The plate joins the weapon's meshes (a merged part), rigid on the attachment's bone. The text is drawn with the
//! Windows fonts CS2 asks for: Arial Narrow (`ARIALN.TTF`), each character it lacks from the first font that has it
//! (Arial, Microsoft YaHei, Segoe UI Symbol: the names in a round are often Chinese, or hearts), 46 texels tall
//! (`TEXT_HEIGHT`) to fill the panel; past the panel's right edge it is cut. Without the fonts, the plate stays
//! blank.
//!
//! # In a left hand
//!
//! CS2 mirrors the whole viewmodel of a player who holds it in the left hand (`cl_righthand 0`), stickers
//! included, but keeps the plate's text readable: yuurih's M4A4 (round 17 of the reference demo, tick 152705) reads
//! "LOVE from Jimzor v3" from the stock towards the muzzle, the plate's logo at the stock's end. That is the plate
//! mirrored along its length, which the view's mirror turns back ([`mirrored`]). Unreal mirrors the viewmodel the
//! same way (`import_demo.py`), so the viewmodel carries both plates and shows the one for the hand
//! (`viewmodel`).

use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};

use crate::content::Content;
use crate::export::ExportError;
use crate::export::environment::{SRGB_TO_LINEAR, linear_to_srgb};
use crate::export::gltf::{decode_raw, load_model};
use crate::export::loadout::{Finish, module_mount};
use crate::model::{Affine, Model, compose};
use crate::texture::Image;

/// The plate's model.
pub const MODULE: &str = "weapons/models/shared/nametag/nametag_module.vmdl";
/// The plate's material, the one the name is drawn on.
pub const MATERIAL: &str = "weapons/models/shared/nametag/materials/nametag_default.vmat";

/// Where the composite draws the name: its top left, in the texture's 0 – 1, and its colour (sRGB).
const TEXT_AT: [f32; 2] = [0.04, 0.038];
const TEXT_COLOR: [u8; 3] = [241, 241, 241];
/// The text's height (the font's ascent to descent) in texels of the 512 composite: the dark panel's.
const TEXT_HEIGHT: f32 = 46.0;
/// The panel's right edge, in the texture's 0 – 1: the text is cut past it.
const PANEL_RIGHT: f32 = 0.965;
/// The size the composite generates the plate's colour at.
const SIZE: usize = 512;

/// The fonts tried for each character, in order (`%WINDIR%\Fonts`), and the face index in a collection.
const FONTS: [(&str, u32); 4] = [
    ("ARIALN.TTF", 0),
    ("arial.ttf", 0),
    ("msyh.ttc", 0),
    ("seguisym.ttf", 0),
];

/// Where the plate sits on `weapon` with `mesh_groups` shown: the bone it follows and the plate's space → the
/// weapon model's space (the `nametag` attachment's frame). `None` without the attachment.
pub fn mount(weapon: &Model, mesh_groups: u64) -> Option<(usize, Affine)> {
    module_mount(weapon, mesh_groups, "nametag")
}

/// `mount` ([`mount`]'s placement) mirrored along the plate's length (its x): the plate as a mirrored viewmodel
/// carries it (module docs, "In a left hand"). The plate is symmetric about its origin along x, so it covers the
/// same place.
pub fn mirrored(mount: &Affine) -> Affine {
    compose(
        mount,
        &[
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
    )
}

/// The plate's model, loaded when one of `finishes` has a name tag (`None` otherwise, or with a warning when it
/// cannot be read).
pub(crate) fn load_plate(
    content: &Content,
    finishes: impl IntoIterator<Item = Option<Finish>>,
    warnings: &mut Vec<String>,
) -> Option<Model> {
    if !finishes
        .into_iter()
        .any(|f| f.is_some_and(|f| f.name_tag.is_some()))
    {
        return None;
    }
    load_model(content, MODULE)
        .map_err(|e| warnings.push(format!("name tag plate {MODULE}: {e}: not shown")))
        .ok()
}

/// The Windows fonts folder.
fn fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("Fonts")
}

/// The fonts of [`FONTS`] found in `dir`, in order.
fn load_fonts(dir: &Path) -> Vec<FontVec> {
    FONTS
        .iter()
        .filter_map(|(file, index)| {
            let data = std::fs::read(dir.join(file)).ok()?;
            FontVec::try_from_vec_and_index(data, *index).ok()
        })
        .collect()
}

/// The plate's colour (`base`, its `g_tColor`) at 512 with `text` drawn on it as the composite draws it.
pub(crate) fn bake_color(content: &Content, base: &str, text: &str) -> Result<Image, ExportError> {
    let (_, image) = decode_raw(content, base)?;
    let mut image = resize(&image, SIZE);
    let fonts = load_fonts(&fonts_dir());
    draw_text(&mut image, &fonts, text);
    Ok(image)
}

/// `image` resampled to `size` × `size` (nearest; the plate's colour is stored at 512 already).
fn resize(image: &Image, size: usize) -> Image {
    if image.width == size && image.height == size {
        return image.clone();
    }
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (sx, sy) = (x * image.width / size, y * image.height / size);
            rgba.extend_from_slice(&image.rgba[(sy * image.width + sx) * 4..][..4]);
        }
    }
    Image::new(size, size, rgba)
}

/// Draws `text` onto `image` at [`TEXT_AT`], each character in the first of `fonts` that has it.
fn draw_text(image: &mut Image, fonts: &[FontVec], text: &str) {
    let Some(first) = fonts.first() else {
        return;
    };
    let scale = PxScale::from(TEXT_HEIGHT);
    let size = image.width as f32;
    let right = PANEL_RIGHT * size;
    let mut x = TEXT_AT[0] * size;
    let baseline = TEXT_AT[1] * size + first.as_scaled(scale).ascent();
    let color = TEXT_COLOR.map(|c| SRGB_TO_LINEAR[usize::from(c)]);
    for c in text.chars() {
        let font = fonts.iter().find(|f| f.glyph_id(c).0 != 0).unwrap_or(first);
        let scaled = font.as_scaled(scale);
        let glyph = scaled
            .glyph_id(c)
            .with_scale_and_position(scale, point(x, baseline));
        x += scaled.h_advance(glyph.id);
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let (px, py) = (bounds.min.x + gx as f32, bounds.min.y + gy as f32);
            if px < 0.0 || py < 0.0 || px >= right || py >= image.height as f32 {
                return;
            }
            let at = (py as usize * image.width + px as usize) * 4;
            for k in 0..3 {
                let base = SRGB_TO_LINEAR[usize::from(image.rgba[at + k])];
                let mixed = base + (color[k] - base) * coverage.clamp(0.0, 1.0);
                image.rgba[at + k] = linear_to_srgb(mixed);
            }
        });
        if x >= right {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lands_on_the_panel_and_stops_at_its_edge() {
        let fonts = load_fonts(&fonts_dir());
        if fonts.is_empty() {
            return; // no Windows fonts here (CI's Linux runner)
        }
        let mut image = Image::new(SIZE, SIZE, [10, 10, 10, 255].repeat(SIZE * SIZE));
        draw_text(&mut image, &fonts, "VICTORIA SIEMPRE");
        let lit = |x0: usize, x1: usize, y0: usize, y1: usize| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| image.rgba[(y * SIZE + x) * 4] > 128)
                .count()
        };
        // Light texels in the panel's top left, none below it or past its edge.
        assert!(lit(20, 260, 19, 70) > 500);
        assert_eq!(lit(0, SIZE, 80, SIZE), 0);
        assert_eq!(
            lit((PANEL_RIGHT * SIZE as f32) as usize + 1, SIZE, 0, SIZE),
            0
        );
    }
}
