//! A smoke's cloud as a texture for Unreal: OFAST's simulation of the cloud ([`crate::demo::smoke`]) run through
//! the smoke's voxel log, every step's 32³ density grid laid out in one 8-bit grayscale atlas (M6,
//! `docs/decisions/017-grenades-and-effects.md`).
//!
//! Layout (`docs/formats/replay.md`, "Smoke volumes"):
//!
//! - a **frame** (one simulation step) is a tile of 8 × 4 slices, 256 × 128 texels; slice `z` (0 at the bottom of the
//!   grid) is at column `z % 8`, row `z / 8`;
//! - a **slice** is 32 × 32 texels: texel column = the grid's x, texel row = the grid's y (CS2's axes, not
//!   Unreal's: the importer flips y);
//! - frames follow each other left to right, then top to bottom, `columns` per row.
//!
//! A texel holds `sqrt(density / full)` (clamped to 1) × 255, where `full` is the emitters' cap
//! ([`SmokeSettings::cap`]): the square root keeps more steps for the thin edges, where the cloud fades into the
//! air. The volume material squares it back.

use std::path::Path;

use super::ExportError;
use super::gltf::write_gray_png;
use crate::demo::smoke::{GRID, Simulation, SmokeSettings, VoxelFrame};

/// Slices per row of a frame's tile.
pub const SLICE_COLUMNS: usize = 8;
/// Rows of slices in a frame's tile.
pub const SLICE_ROWS: usize = GRID / SLICE_COLUMNS;
/// A frame's tile width, texels.
pub const FRAME_WIDTH: usize = SLICE_COLUMNS * GRID;
/// A frame's tile height, texels.
pub const FRAME_HEIGHT: usize = SLICE_ROWS * GRID;

/// One smoke's simulated cloud, every step, as an atlas.
#[derive(Debug, Clone, PartialEq)]
pub struct SmokeAtlas {
    /// Frames (simulation steps) in the atlas.
    pub frames: usize,
    /// Frames per row of the atlas.
    pub columns: usize,
    /// Width, texels.
    pub width: usize,
    /// Height, texels.
    pub height: usize,
    /// The density the texels' 255 stands for ([`SmokeSettings::cap`]).
    pub full: f32,
    /// Row-major grayscale texels.
    pub texels: Vec<u8>,
}

impl SmokeAtlas {
    /// The texel of voxel `[x, y, z]` in frame `frame`.
    pub fn texel(&self, frame: usize, voxel: [usize; 3]) -> u8 {
        let [x, y, z] = voxel;
        let column = (frame % self.columns) * FRAME_WIDTH + (z % SLICE_COLUMNS) * GRID + x;
        let row = (frame / self.columns) * FRAME_HEIGHT + (z / SLICE_COLUMNS) * GRID + y;
        self.texels[row * self.width + column]
    }
}

/// The layout of an atlas of `frames` frames: frames per row, width and height in texels. The frames per row are
/// chosen so the atlas comes out about square: 196 frames (a smoke's 22 s) make 10 per row, 2560 × 2560 texels.
pub fn atlas_layout(frames: usize) -> (usize, usize, usize) {
    // FRAME_WIDTH is twice FRAME_HEIGHT: half as many columns as rows is square.
    let columns = ((frames as f64 / 2.0).sqrt().ceil() as usize).max(1);
    let rows = frames.div_ceil(columns).max(1);
    (columns, columns * FRAME_WIDTH, rows * FRAME_HEIGHT)
}

/// An HE grenade going off in or near a smoke's cloud: when (demo tick) and where, in the smoke's grid (voxels from its
/// corner).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blast {
    /// The detonation's demo tick.
    pub tick: u32,
    /// The detonation point in the grid, voxels.
    pub voxel: [f32; 3],
}

/// Runs OFAST's simulation through `frames` (a smoke's log, [`crate::demo::smoke::frames`]) and lays out every
/// step's cloud ([`atlas_layout`]). Each of `blasts` carves its hole ([`Simulation::carve`]) after every step whose
/// frame was sent while the hole lasts, as big as [`SmokeSettings::hole_radius_at`] says for the frame's age since the
/// blast (`tick_interval`: seconds per demo tick).
pub fn smoke_atlas(
    frames: &[VoxelFrame],
    settings: SmokeSettings,
    blasts: &[Blast],
    tick_interval: f32,
) -> SmokeAtlas {
    let count = frames.len();
    let (columns, width, height) = atlas_layout(count);
    let mut atlas = SmokeAtlas {
        frames: count,
        columns,
        width,
        height,
        full: settings.cap,
        texels: vec![0; width * height],
    };
    let mut sim = Simulation::new(settings);
    for (k, frame) in frames.iter().enumerate() {
        sim.step(frame);
        // After the step, so what it spread or fed into a hole is gone again from the frame drawn.
        for blast in blasts {
            let age = (frame.tick as f32 - blast.tick as f32) * tick_interval;
            let radius = settings.hole_radius_at(age);
            if radius > 0.0 {
                sim.carve(blast.voxel, radius, settings.hole_edge);
            }
        }
        let grid = sim.render_grid();
        let (tile_x, tile_y) = ((k % columns) * FRAME_WIDTH, (k / columns) * FRAME_HEIGHT);
        for z in 0..GRID {
            let (slice_x, slice_y) = (
                tile_x + (z % SLICE_COLUMNS) * GRID,
                tile_y + (z / SLICE_COLUMNS) * GRID,
            );
            for y in 0..GRID {
                let row = (slice_y + y) * width + slice_x;
                for x in 0..GRID {
                    let d = grid[x + GRID * (y + GRID * z)];
                    atlas.texels[row + x] =
                        ((d / settings.cap).clamp(0.0, 1.0).sqrt() * 255.0).round() as u8;
                }
            }
        }
    }
    atlas
}

/// Writes an atlas as an 8-bit grayscale PNG.
pub fn write_smoke_atlas(path: &Path, atlas: &SmokeAtlas) -> Result<(), ExportError> {
    write_gray_png(path, atlas.width, atlas.height, &atlas.texels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::smoke::{Brick, Emitter};

    #[test]
    fn atlas_lays_out_frames_and_slices() {
        // An open grid, one emitter in the middle: the cloud grows over three steps.
        let bricks: Vec<Brick> = (0..512).map(|index| Brick { index, solid: 0 }).collect();
        let mut frames = vec![VoxelFrame {
            number: 0,
            emitters: vec![Emitter {
                voxel: [16, 16, 16],
                level: 5,
                hold: 0.0,
            }],
            bricks,
            ..VoxelFrame::default()
        }];
        for number in 1..3 {
            frames.push(VoxelFrame {
                number,
                ..VoxelFrame::default()
            });
        }
        let settings = SmokeSettings::default();
        let atlas = smoke_atlas(&frames, settings, &[], 1.0 / 64.0);
        assert_eq!((atlas.frames, atlas.columns), (3, 2));
        assert_eq!((atlas.width, atlas.height), (512, 256));
        assert_eq!(atlas.texels.len(), 512 * 256);

        // The atlas holds what the simulation computed, voxel by voxel, at its place.
        let mut sim = Simulation::new(settings);
        for (k, frame) in frames.iter().enumerate() {
            sim.step(frame);
            for voxel in [
                [16, 16, 16],
                [17, 16, 16],
                [16, 16, 15],
                [16, 18, 17],
                [0, 0, 0],
            ] {
                let d = sim.density(voxel);
                let expected = ((d / settings.cap).clamp(0.0, 1.0).sqrt() * 255.0).round() as u8;
                assert_eq!(atlas.texel(k, voxel), expected, "frame {k} voxel {voxel:?}");
            }
        }
        // The emitter's voxel is full; the cloud reached its neighbours, not the corner.
        assert_eq!(atlas.texel(2, [16, 16, 16]), 255);
        assert!(atlas.texel(2, [17, 16, 16]) > 0);
        assert_eq!(atlas.texel(2, [0, 0, 0]), 0);
        // Slice 16 of frame 1 starts at column 256 + 0, row 2 × 32 of the first tile row.
        let (column, row) = (FRAME_WIDTH + 16, 2 * GRID + 16);
        assert_eq!(
            atlas.texels[row * atlas.width + column],
            atlas.texel(1, [16, 16, 16])
        );

        // An HE grenade going off at the emitter on tick 10, frames sent 7 ticks apart: frames 0 and 1 (ticks 0 and 7)
        // keep their cloud; from frame 2 (tick 14) the hole holds it empty, the emitter's feed too.
        for (k, frame) in frames.iter_mut().enumerate() {
            frame.tick = 7 * k as u32;
        }
        frames.push(VoxelFrame {
            number: 3,
            tick: 21,
            ..VoxelFrame::default()
        });
        let blast = Blast {
            tick: 10,
            voxel: [16.5, 16.5, 16.5],
        };
        let atlas = smoke_atlas(&frames, settings, &[blast], 1.0 / 64.0);
        assert_eq!(atlas.texel(0, [16, 16, 16]), 255);
        assert_eq!(atlas.texel(1, [16, 16, 16]), 255);
        for k in [2, 3] {
            assert_eq!(atlas.texel(k, [16, 16, 16]), 0, "frame {k}");
            assert_eq!(atlas.texel(k, [17, 16, 16]), 0, "frame {k}");
        }
    }
}
