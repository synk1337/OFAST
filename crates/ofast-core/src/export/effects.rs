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
use crate::demo::players::RoundTrack;
use crate::physics::ragdoll::WorldCollision;
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
    // particles/explosions_fx/explosion_hegrenade.vpcf: the flare CS2's blasts and flashbangs go off with (a molotov's
    // glow, molotov_child_glow01a, is its light instead).
    (
        "flare",
        "materials/particle/particle_flares/particle_flare_001.vtex_c",
    ),
    // particles/explosions_fx/explosion_hegrenade_sparktrails_sparks.vpcf: the streaks a blast's sparks leave, and the
    // glow they start with (…_sparks_dots.vpcf).
    ("sparks", "materials/particle/sparks/sparks.vtex_c"),
    (
        "spark_glow",
        "materials/effects/glows/simple_glow_001.vtex_c",
    ),
    // particles/inferno_fx/molotov_groundfire_endcap_groundsmoke.vpcf and _endcap_wall.vpcf: the near-black wisps a
    // fire leaves for 5 s when it goes out.
    (
        "fire_smoke",
        "materials/particle/simulated/steam/wispy_steam_set.vtex_c",
    ),
    // particles/inferno_fx/molotov_groundfire_remnant.vpcf: the soot a fire leaves where it burned
    // (materials/effects/scortch_mod_projected_cheap.vmat's colour texture).
    (
        "fire_scorch",
        "materials/particle/particle_smokegrenade_trans_tga_9bc0185f.vtex_c",
    ),
    // particles/explosions_fx/explosion_hegrenade_debris_small.vpcf: the flecks a blast throws.
    ("debris", "materials/particle/impact/fleks.vtex_c"),
    // particles/explosions_fx/explosion_hegrenade_decal.vpcf: its scorch (materials/effects/scortch_projected_cheap.vmat's
    // colour texture; the material darkens it to near black).
    (
        "scorch",
        "materials/effects/fluidsplat_a_noisy_trans_tga_c46d6803.vtex_c",
    ),
];

/// How far below an HE grenade's detonation [`blasts_json`] looks for the ground, units.
pub const GROUND_REACH: f32 = 128.0;

/// The sphere [`blasts_json`] moves down to find the ground: its radius and step, units. The step is half the radius,
/// so the sphere first touches a floor from above, less than half its radius deep (a step as long as its diameter
/// could put its centre under a thin floor's surface, where the contact is the floor's underside).
pub const GROUND_PROBE: (f32, f32) = (1.0, 0.5);

/// A contact counts as ground when its normal points at least this much up (not a wall, a stair riser or an edge).
pub const GROUND_UP: f32 = 0.5;

/// Each HE grenade's blast, for what it leaves on the ground in Unreal (its scorch, where its debris lands): `entity`,
/// `tick` and `position` of its `hegrenade_detonate`, and `ground`: the first solid surface facing up straight below
/// the point within [`GROUND_REACH`] units, as `{z, normal}` (z where the sphere touches it), found by moving a small
/// sphere down the map's collision ([`GROUND_PROBE`]); `null` without collision or with nothing that close (a grenade
/// going off in the air above a drop).
pub fn blasts_json(track: &RoundTrack, collision: Option<&WorldCollision>) -> Vec<Json> {
    let (radius, step) = GROUND_PROBE;
    track
        .events
        .iter()
        .filter(|e| e.name == "hegrenade_detonate")
        .filter_map(|e| {
            let at = e.position()?;
            let ground = collision.and_then(|world| {
                (0..=(GROUND_REACH / step) as usize).find_map(|k| {
                    let centre = [at[0], at[1], at[2] - step * k as f32];
                    let (depth, normal) = world.sphere_contact(centre, radius)?;
                    // The contact point: from the centre against the normal, to the surface.
                    (normal[2] >= GROUND_UP).then(|| {
                        json!({
                            "z": ((centre[2] - normal[2] * (radius - depth)) * 100.0).round() / 100.0,
                            "normal": normal.map(|n| (n * 1000.0).round() / 1000.0),
                        })
                    })
                })
            });
            Some(json!({
                "entity": e.int("entityid"),
                "tick": e.tick,
                "position": at,
                "ground": ground,
            }))
        })
        .collect()
}

/// How far from a flame [`fire_walls_json`] looks for a wall, units. CS2 traces its fire's outline points along their
/// outward normals as far as a control point the game sets (its particle systems preview it at 80); OFAST traces from
/// every flame, so half that, from the flame rather than the outline.
pub const WALL_REACH: f32 = 40.0;

/// Directions [`fire_walls_json`] looks in, evenly around a flame.
pub const WALL_RAYS: usize = 8;

/// The sphere [`fire_walls_json`] moves out to find a wall: radius and step (half the radius: see [`GROUND_PROBE`]),
/// and the height above the flame it starts at, units.
pub const WALL_PROBE: (f32, f32, f32) = (2.0, 1.0, 10.0);

/// Wall points closer than this to one already found, on a wall facing the same way, are the same place, units.
pub const WALL_SPACING: f32 = 16.0;

/// How much higher [`fire_walls_json`] checks that a wall it met goes on (a curb, a step or a crate's edge at the
/// probe's height is not a wall to climb), units.
pub const WALL_CONFIRM: f32 = 20.0;

/// A fire as [`fire_walls_json`] gathers it: (entity, serial), its start tick, its flames' first positions in order.
type FireFlames = ((u32, u32), Option<u32>, Vec<[f32; 3]>);

/// A wall point as [`fire_walls_json`] gathers it: the flames whose rays met it, the point, the wall's normal.
type WallPoint = (Vec<[f32; 3]>, [f32; 3], [f32; 3]);

/// The first wall a ray from `flame` along (`dx`, `dy`) meets within [`WALL_REACH`] ([`fire_walls_json`]): the point
/// on it at the flame's height and its normal, or `None`. The sphere ([`WALL_PROBE`]) starts at the flame itself, so
/// it never begins behind a wall's face. A surface facing up or down (normal z at least [`GROUND_UP`] either way: a
/// floor, a step, a ceiling) ends the ray; so does one facing along the ray (the back of a face the sphere is already
/// past), and one that is not there [`WALL_CONFIRM`] higher (a lip, not a wall).
fn wall_hit(
    world: &WorldCollision,
    flame: [f32; 3],
    dx: f32,
    dy: f32,
) -> Option<([f32; 3], [f32; 3])> {
    let (radius, step, lift) = WALL_PROBE;
    (0..=(WALL_REACH / step) as usize)
        .find_map(|k| {
            let d = step * k as f32;
            let centre = [flame[0] + dx * d, flame[1] + dy * d, flame[2] + lift];
            let (depth, normal) = world.sphere_contact(centre, radius)?;
            let facing_back = normal[0] * dx + normal[1] * dy < -0.3;
            let higher = [centre[0], centre[1], centre[2] + WALL_CONFIRM];
            let goes_on = world
                .sphere_contact(higher, radius)
                .is_some_and(|(_, n)| n[2].abs() < GROUND_UP);
            if normal[2].abs() >= GROUND_UP || !facing_back || !goes_on {
                return Some(None);
            }
            // The contact point: from the centre against the normal, to the surface; at the flame's height.
            let at = |i: usize| centre[i] - normal[i] * (radius - depth);
            Some(Some(([at(0), at(1), flame[2]], normal)))
        })
        .flatten()
}

/// Where each fire's flames meet walls, for the flames that climb them in Unreal: per fire (`entity`, `start_tick`),
/// `walls`: `flames` (the positions of the flames whose rays met it, as the replay's `fires` give them), `position`
/// (the point on the wall, at the first such flame's height) and `normal` (the wall's, horizontal-ish). CS2's client
/// builds these itself from the networked flames by tracing against the world (`molotov_groundfire_climbingset`
/// reads them as a point set; the demo holds none), so OFAST traces too (`wall_hit`): from each flame, 10 units up,
/// in [`WALL_RAYS`] directions up to [`WALL_REACH`]. A point within [`WALL_SPACING`] of an earlier one on a wall
/// facing the same way is that one (its flames gather), so an inside corner keeps both walls. Without collision, none.
pub fn fire_walls_json(track: &RoundTrack, collision: Option<&WorldCollision>) -> Vec<Json> {
    let Some(world) = collision else {
        return Vec::new();
    };
    // Each fire's flames once, by their first position (as the replay's `fires` have them), in order.
    let mut fires: Vec<FireFlames> = Vec::new();
    for sample in &track.samples {
        for f in &sample.fires {
            let at = match fires
                .iter()
                .position(|(key, _, _)| *key == (f.entity, f.serial))
            {
                Some(at) => at,
                None => {
                    fires.push((
                        (f.entity, f.serial),
                        f.start_tick.or(Some(sample.tick)),
                        Vec::new(),
                    ));
                    fires.len() - 1
                }
            };
            for (k, flame) in f.flames.iter().enumerate() {
                if fires[at].2.len() <= k {
                    fires[at].2.push(flame.position);
                }
            }
        }
    }
    fires
        .into_iter()
        .map(|((entity, _), start, flames)| {
            let mut walls: Vec<WallPoint> = Vec::new();
            for flame in &flames {
                for ray in 0..WALL_RAYS {
                    let angle = ray as f32 * std::f32::consts::TAU / WALL_RAYS as f32;
                    let Some((point, normal)) = wall_hit(world, *flame, angle.cos(), angle.sin())
                    else {
                        continue;
                    };
                    let same = |w: &&mut WallPoint| {
                        (0..3).map(|i| (w.1[i] - point[i]).powi(2)).sum::<f32>()
                            < WALL_SPACING.powi(2)
                            && (0..3).map(|i| w.2[i] * normal[i]).sum::<f32>() > 0.7
                    };
                    match walls.iter_mut().find(same) {
                        Some(wall) if !wall.0.contains(flame) => wall.0.push(*flame),
                        Some(_) => {}
                        None => walls.push((vec![*flame], point, normal)),
                    }
                }
            }
            let round = |v: [f32; 3], scale: f32| v.map(|x| (x * scale).round() / scale);
            json!({
                "entity": entity,
                "start_tick": start,
                "walls": walls.iter().map(|(by, point, normal)| json!({
                    "flames": by.iter().map(|f| round(*f, 100.0)).collect::<Vec<_>>(),
                    "position": round(*point, 100.0),
                    "normal": round(*normal, 1000.0),
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

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
/// of cells (`stride`), so a material plays a sequence by counting cells: frame k of sequence n is cell n × stride + k.
#[derive(Debug, Clone)]
pub struct Flipbook {
    /// The cells, [`FLIPBOOK_COLUMNS`] per row; each sequence starts `stride` cells after the one before.
    pub image: Image,
    /// A cell's size, pixels: the largest uncropped frame of the sheet.
    pub cell: (usize, usize),
    /// Cells per row.
    pub columns: usize,
    /// Cells from one sequence's first frame to the next's: as many as the longest sequence needs, rounded up to a
    /// power of two up to a row (1, 2, 4, 8) and to whole rows above.
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
    // Every sequence gets as many cells as the longest one needs, so sequence n starts at cell n × stride: up to a
    // row's worth, a power of two of them (sequences then tile the rows: a sheet of 64 one-frame flecks is 8 rows,
    // not 64), else whole rows.
    let longest = sheet
        .iter()
        .map(|s| s.frames.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let stride = if longest <= FLIPBOOK_COLUMNS {
        longest.next_power_of_two()
    } else {
        longest.div_ceil(FLIPBOOK_COLUMNS) * FLIPBOOK_COLUMNS
    };
    let rows = (sheet.len() * stride).div_ceil(FLIPBOOK_COLUMNS);
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
    use crate::demo::events::{EventValue, GameEvent};
    use crate::demo::grenades::{FireSample, Flame};
    use crate::demo::players::TickSample;
    use crate::texture::{SheetFrame, SheetImage};

    #[test]
    fn walls_and_ground_are_found_in_the_collision() {
        // A floor with its top at z 0, a wall whose face is at x 20, and a curb 12 units tall at x -85 to -75.
        let mut world = WorldCollision::default();
        world.add_box([-200.0, -200.0, -10.0], [200.0, 200.0, 0.0]);
        world.add_box([20.0, -200.0, 0.0], [40.0, 200.0, 200.0]);
        world.add_box([-85.0, -200.0, 0.0], [-75.0, 200.0, 12.0]);
        let flame = |x: f32| Flame {
            position: [x, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            burning: true,
        };
        let fire = FireSample {
            entity: 500,
            serial: 3,
            incendiary: false,
            start_tick: Some(10),
            lifetime: 7.0,
            flames: vec![flame(0.0), flame(-100.0)],
        };
        let float = |v: f32| EventValue::Float(v);
        let track = RoundTrack {
            round: 1,
            start_tick: 0,
            end_tick: 20,
            tick_interval: 1.0 / 64.0,
            map_name: "de_test".into(),
            samples: vec![TickSample {
                tick: 10,
                players: Vec::new(),
                dropped: Vec::new(),
                doors: Vec::new(),
                grenades: Vec::new(),
                fires: vec![fire],
            }],
            // An HE grenade going off 3.5 units above the floor.
            events: vec![GameEvent {
                tick: 12,
                name: "hegrenade_detonate".into(),
                keys: vec![
                    ("entityid".into(), EventValue::Int(7)),
                    ("x".into(), float(-50.0)),
                    ("y".into(), float(0.0)),
                    ("z".into(), float(3.5)),
                ],
            }],
            smoke_logs: Vec::new(),
        };

        // The flame 20 units from the wall meets it straight ahead and on the two diagonals (18 units to each side,
        // further apart than WALL_SPACING): three points on its face, at the flame's height, facing back. The one by
        // the curb gets none: the curb is not there 20 units higher.
        let fires = fire_walls_json(&track, Some(&world));
        let walls = fires[0]["walls"].as_array().unwrap();
        assert_eq!(walls.len(), 3, "{walls:?}");
        for wall in walls {
            assert_eq!(wall["flames"], json!([[0.0, 0.0, 0.0]]));
            assert_eq!(wall["normal"], json!([-1.0, 0.0, 0.0]));
            assert!((wall["position"][0].as_f64().unwrap() - 20.0).abs() < 0.01);
            assert_eq!(wall["position"][2], json!(0.0));
        }
        assert_eq!(walls[0]["position"][1], json!(0.0));

        // The blast's ground: the floor's top, facing up (half-unit steps: the sphere meets it from above).
        let blasts = blasts_json(&track, Some(&world));
        assert!(
            blasts[0]["ground"]["z"].as_f64().unwrap().abs() < 0.01,
            "{}",
            blasts[0]
        );
        assert_eq!(blasts[0]["ground"]["normal"], json!([0.0, 0.0, 1.0]));
        // Without collision, neither.
        assert!(fire_walls_json(&track, None).is_empty());
        assert_eq!(blasts_json(&track, None)[0]["ground"], Json::Null);
    }

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
        // Three frames: a stride of four cells, half a row.
        assert_eq!(book.stride, 4);
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

        // Nine one-frame sequences: one cell each, nine cells in two rows.
        let single: Vec<SheetSequence> = (0..9)
            .map(|id| SheetSequence {
                id,
                frames: vec![frame(rect(4.0, 0.0, 7.0, 3.0), rect(4.0, 0.0, 7.0, 3.0))],
                ..sheet[0].clone()
            })
            .collect();
        let book = flipbook(&image, &single);
        assert_eq!(book.stride, 1);
        assert_eq!(book.sequences[8].first_cell, 8);
        assert_eq!(book.image.height, 2 * 4);
    }
}
