//! CS2's own effect textures for the grenades' effects in Unreal (M6, `docs/decisions/017-grenades-and-effects.md`).
//!
//! CS2 draws a molotov's flames, an HE blast and a flash with particle systems whose sprites come from sprite sheets:
//! one texture with several animated sequences packed in it, each frame a rectangle (the texture's `SHEET` data,
//! [`crate::texture::SheetSequence`]). OFAST does not convert the particle systems (decision 017); it draws its own
//! sprites with CS2's sheets, so each sheet used is written once next to the replay as an RGBA PNG,
//! `fx_<texture stem>.png`, repacked as a regular grid ([`flipbook`]: CS2 packs frames as tightly as it can, each a
//! rectangle of its own size), and listed with its sequences in the agents manifest's `effects`
//! (`docs/formats/replay.md`).

use std::path::Path;

use serde_json::{Value as Json, json};

use super::ExportError;
use super::gltf::write_png;
use crate::content::Content;
use crate::resource::Resource;
use crate::texture::{Image, SheetSequence, Texture};

/// The effect textures written, by the role the Unreal side gives them, with the particle systems they come from.
pub const EFFECT_TEXTURES: &[(&str, &str)] = &[
    // particles/inferno_fx/molotov_child_flame01a.vpcf: the molotov's main flames (4 sequences of 32 – 33 frames).
    (
        "fire",
        "materials/particle/fire_small_sim/fire_small_sim_b.vtex_c",
    ),
    // particles/inferno_fx/molotov_child_flame03a.vpcf: taller tongues of burning gas among them (7 sequences of
    // 16 – 19 frames).
    (
        "gas",
        "materials/particle/fire_gas/fire_gas_batch_a_v2.vtex_c",
    ),
    // particles/explosions_fx/explosion_hegrenade_smoke_core.vpcf: an HE blast's fireball (grey: CS2's shader tints
    // it) and its smoke (3 sequences each).
    (
        "blast_flame",
        "materials/particle/explosion_fireballs/flame/explosion_fireball_01_flame.vtex_c",
    ),
    (
        "blast_smoke",
        "materials/particle/explosion_fireballs/smoke/explosion_fireball_01_smoke.vtex_c",
    ),
    // particles/explosions_fx/explosion_hegrenade.vpcf: the flare CS2's blasts and flashbangs go off with. OFAST draws
    // it for flashbangs only (an HE blast's fireball is bright on its own); a molotov's glow (molotov_child_glow01a)
    // is its light instead.
    (
        "flare",
        "materials/particle/particle_flares/particle_flare_001.vtex_c",
    ),
];

/// Writes [`EFFECT_TEXTURES`] into `out_dir` as flipbooks and returns their manifest entries: `role`, `texture`
/// (CS2 path), `file`, `width`, `height`, `cell` (a frame's size, pixels), `columns` and `rows` (cells across and
/// down), `stride` (cells from one sequence to the next) and `sequences` (`id`, `first_cell`, `frames`, `clamp`). A
/// texture that is missing or does not decode is left out with a warning.
pub fn export_effect_textures(
    content: &Content,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Vec<Json> {
    let mut out = Vec::new();
    for &(role, path) in EFFECT_TEXTURES {
        match export_effect_texture(content, path, out_dir) {
            Ok(mut entry) => {
                entry["role"] = json!(role);
                out.push(entry);
            }
            Err(e) => warnings.push(format!("effect texture {path}: {e}")),
        }
    }
    out
}

fn export_effect_texture(
    content: &Content,
    path: &str,
    out_dir: &Path,
) -> Result<Json, ExportError> {
    let data = content.read(path)?;
    let texture = Texture::parse(&Resource::parse(&data)?)?;
    let image = texture.decode_rgba8(&data)?;
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".vtex_c");
    let file = format!("fx_{stem}.png");
    let book = flipbook(&image, &texture.sheet);
    write_png(&out_dir.join(&file), &book.image)?;
    Ok(json!({
        "texture": path.trim_end_matches("_c"),
        "file": file,
        "width": book.image.width,
        "height": book.image.height,
        "cell": [book.cell.0, book.cell.1],
        "columns": book.columns,
        "rows": book.image.height / book.cell.1.max(1),
        "stride": book.stride,
        "sequences": book.sequences.iter().map(|s| json!({
            "id": s.id,
            "first_cell": s.first_cell,
            "frames": s.frames,
            "clamp": s.clamp,
        })).collect::<Vec<_>>(),
    }))
}

/// Frames per row of a [`Flipbook`].
pub const FLIPBOOK_COLUMNS: usize = 8;

/// A sprite sheet repacked as a regular grid: every frame in a cell of the same size, every sequence the same number
/// of rows, so a material plays a sequence by counting cells: frame k of sequence n is cell n × stride + k.
#[derive(Debug, Clone)]
pub struct Flipbook {
    /// The cells, [`FLIPBOOK_COLUMNS`] per row; each sequence starts a new row.
    pub image: Image,
    /// A cell's size, pixels: the largest uncropped frame of the sheet.
    pub cell: (usize, usize),
    /// Cells per row.
    pub columns: usize,
    /// Cells from one sequence's first frame to the next's: whole rows, as many as the longest sequence needs.
    pub stride: usize,
    /// Where each sequence's frames are.
    pub sequences: Vec<FlipbookSequence>,
}

/// One sequence of a [`Flipbook`].
#[derive(Debug, Clone, PartialEq)]
pub struct FlipbookSequence {
    /// The sheet's sequence id.
    pub id: u32,
    /// Its first frame's cell (row-major).
    pub first_cell: usize,
    /// Its frames, in consecutive cells.
    pub frames: usize,
    /// It holds its last frame.
    pub clamp: bool,
}

/// A UV rectangle `[min, max]` as pixels `(x0, y0, x1, y1)`, end exclusive. The corners sit inside the texels they
/// cover (`fire_small_sim_b`'s first frame: 0.25 to 184.75), so the start is the min's texel and the end the max's
/// texel + 1 (as ValveResourceFormat reads it); an empty rectangle (max not above min) is `None`.
fn pixel_rect(
    rect: [[f32; 2]; 2],
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let [[u0, v0], [u1, v1]] = rect;
    if u1 <= u0 || v1 <= v0 {
        return None;
    }
    let x0 = (u0 * width as f32) as usize;
    let y0 = (v0 * height as f32) as usize;
    let x1 = ((u1 * width as f32) as usize + 1).min(width);
    let y1 = ((v1 * height as f32) as usize + 1).min(height);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Repacks `image`'s sheet into a [`Flipbook`]: each frame's uncropped rectangle becomes a cell (top-left aligned),
/// with its cropped image copied at its place inside; empty frames stay transparent. A texture without a sheet is one
/// cell, the whole image.
pub fn flipbook(image: &Image, sheet: &[SheetSequence]) -> Flipbook {
    if sheet.is_empty() {
        return Flipbook {
            image: image.clone(),
            cell: (image.width, image.height),
            columns: 1,
            stride: 1,
            sequences: vec![FlipbookSequence {
                id: 0,
                first_cell: 0,
                frames: 1,
                clamp: false,
            }],
        };
    }
    let (w, h) = (image.width, image.height);
    let frames = || {
        sheet
            .iter()
            .flat_map(|s| s.frames.iter())
            .filter_map(|f| f.images.first())
    };
    let (mut cell_w, mut cell_h) = (1, 1);
    for i in frames() {
        if let Some((x0, y0, x1, y1)) = pixel_rect(i.uncropped, w, h) {
            cell_w = cell_w.max(x1 - x0);
            cell_h = cell_h.max(y1 - y0);
        }
    }
    // Every sequence gets as many rows as the longest one needs: sequence n starts at cell n × stride.
    let rows_per_sequence = sheet
        .iter()
        .map(|s| s.frames.len().div_ceil(FLIPBOOK_COLUMNS).max(1))
        .max()
        .unwrap_or(1);
    let stride = rows_per_sequence * FLIPBOOK_COLUMNS;
    let rows = rows_per_sequence * sheet.len();
    let mut out = Image {
        width: FLIPBOOK_COLUMNS * cell_w,
        height: rows * cell_h,
        rgba: vec![0; FLIPBOOK_COLUMNS * cell_w * rows * cell_h * 4],
    };
    let mut sequences = Vec::new();
    for (n, s) in sheet.iter().enumerate() {
        let first_cell = n * stride;
        for (k, frame) in s.frames.iter().enumerate() {
            let Some(i) = frame.images.first() else {
                continue;
            };
            let (Some(full), Some(crop)) =
                (pixel_rect(i.uncropped, w, h), pixel_rect(i.cropped, w, h))
            else {
                continue;
            };
            let cell = first_cell + k;
            let (cx, cy) = (
                (cell % FLIPBOOK_COLUMNS) * cell_w,
                (cell / FLIPBOOK_COLUMNS) * cell_h,
            );
            for y in crop.1..crop.3 {
                // The cropped image's place inside the uncropped frame, which fills the cell from its corner.
                let (dx, dy) = (crop.0.saturating_sub(full.0), y.saturating_sub(full.1));
                if dy >= cell_h {
                    break;
                }
                let n = (crop.2 - crop.0).min(cell_w.saturating_sub(dx));
                let src = (y * w + crop.0) * 4;
                let dst = ((cy + dy) * out.width + cx + dx) * 4;
                out.rgba[dst..dst + n * 4].copy_from_slice(&image.rgba[src..src + n * 4]);
            }
        }
        sequences.push(FlipbookSequence {
            id: s.id,
            first_cell,
            frames: s.frames.len(),
            clamp: s.clamp,
        });
    }
    Flipbook {
        image: out,
        cell: (cell_w, cell_h),
        columns: FLIPBOOK_COLUMNS,
        stride,
        sequences,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::{SheetFrame, SheetImage};

    #[test]
    fn flipbooks_put_each_frame_in_its_cell() {
        // An 8 × 4 image: frame A is the 4 × 4 left half, cropped to its 2 × 2 middle; frame B the right half,
        // uncropped; a third frame is empty.
        let mut image = Image {
            width: 8,
            height: 4,
            rgba: vec![0; 8 * 4 * 4],
        };
        for y in 0..4 {
            for x in 0..8 {
                image.rgba[(y * 8 + x) * 4] = (y * 8 + x) as u8;
            }
        }
        // Texel centres: a rectangle covering texels 1..=2 is u 1/8 ..= 2/8.
        let rect =
            |x0: f32, y0: f32, x1: f32, y1: f32| [[x0 / 8.0, y0 / 4.0], [x1 / 8.0, y1 / 4.0]];
        let frame = |cropped, uncropped| SheetFrame {
            display_time: 1.0,
            images: vec![SheetImage { cropped, uncropped }],
        };
        let sheet = vec![SheetSequence {
            id: 7,
            name: String::new(),
            clamp: true,
            alpha_crop: false,
            no_color: false,
            no_alpha: false,
            total_time: 3.0,
            params: Vec::new(),
            frames: vec![
                frame(rect(1.0, 1.0, 2.0, 2.0), rect(0.0, 0.0, 3.0, 3.0)),
                frame(rect(4.0, 0.0, 7.0, 3.0), rect(4.0, 0.0, 7.0, 3.0)),
                frame(rect(0.0, 0.0, 0.0, 0.0), rect(0.0, 0.0, 0.0, 0.0)),
            ],
        }];
        let book = flipbook(&image, &sheet);
        assert_eq!(book.cell, (4, 4));
        assert_eq!(
            (book.image.width, book.image.height),
            (FLIPBOOK_COLUMNS * 4, 4)
        );
        assert_eq!(
            book.sequences,
            vec![FlipbookSequence {
                id: 7,
                first_cell: 0,
                frames: 3,
                clamp: true
            }]
        );
        let at = |x: usize, y: usize| book.image.rgba[(y * book.image.width + x) * 4];
        // Frame A: only the cropped middle copied, at its place; the rest of the cell transparent.
        assert_eq!((at(1, 1), at(2, 2), at(0, 0), at(3, 3)), (9, 18, 0, 0));
        // Frame B: the whole right half in cell 1.
        assert_eq!((at(4, 0), at(7, 3)), (4, 31));
        // The empty frame's cell stays empty.
        assert!((8..12).all(|x| (0..4).all(|y| at(x, y) == 0)));

        // Sequences of 3 and 10 frames: each gets the 2 rows the longer needs, so the second starts at cell 16.
        let mut two = sheet.clone();
        two.push(SheetSequence {
            id: 8,
            frames: (0..10)
                .map(|_| frame(rect(4.0, 0.0, 7.0, 3.0), rect(4.0, 0.0, 7.0, 3.0)))
                .collect(),
            ..sheet[0].clone()
        });
        let book = flipbook(&image, &two);
        assert_eq!(book.stride, 2 * FLIPBOOK_COLUMNS);
        assert_eq!(
            book.sequences
                .iter()
                .map(|s| s.first_cell)
                .collect::<Vec<_>>(),
            vec![0, 16]
        );
        assert_eq!(book.image.height, 4 * 4);
    }
}
