//! Stickers on weapons (M10): baked into a weapon's colour and metallic-roughness textures as CS2's weapon shader
//! draws them (`docs/research/cs2-skins.md`, "Stickers").
//!
//! # How CS2 draws them
//!
//! Not in the paint composite: the weapon's own material (`csgo_weapon.vfx` with `F_STICKERS`, its static combo 512
//! decompiled as the paint shader was) draws five slots over whatever colour the weapon has. Each slot's sticker is
//! its material (`stickers/<sticker_kits → sticker_material>.vmat`, `csgo_weapon_sticker.vfx`), whose `…Sticker0`
//! keys `weapons/stickers/dev/_shared_sticker_generic.vcompmat` copies into the weapon's slot n (`…StickerN`). Per
//! pixel and slot:
//!
//! - **Where**: in the mesh's second UV set (`TEXCOORD 1`), `rotate(((uv₁ − 0.5) − g_vStickerNOffset) × |scale.x|,
//!   g_flStickerNRotation × 2π) + 0.5`, inside the unit square; the weapon's `g_tStickerWepInputs` (in its first UV
//!   set) masks it (R) and lends its cavity (G) to the wear. Offset, scale, rotation and wear are the material's
//!   unless the instance sets them (`exists(Sticker0Offset) ? Sticker0Offset : this`): the client takes them from
//!   the model's `StickerMarkup` (per mesh and slot; [`crate::model::StickerSlot`]) and the item's free placement,
//!   read here as a shift by the item's offset in stickers (÷ the slot's scale) and a turn by its degrees (the only
//!   reading that puts FalleN's USP-S stickers where CS2 shows them: `docs/research/cs2-skins.md`).
//! - **Alpha**: the sticker texture's A × 12.75, clamped, × the mask; the slot blends by its square (the program
//!   multiplies the two copies it keeps).
//! - **Colour** (`g_bAutomaticPBRColorFitting…`, every sticker material read): the sticker's colour tinted and
//!   desaturated by its metalness, then fitted to albedo levels (floor, exponent, ceiling: `g_vAlbedoLevels`, and
//!   `g_vMetallicAlbedoLevels` or `g_vDarkMetallicAlbedoLevels` by brightness for metallic stickers); without
//!   fitting, as stored. Holographic stickers mix in a spectrum (`g_tHoloSpectrum…`) read where their mask's G, B and
//!   the view direction say, fitted to `g_vHoloAlbedoLevels`.
//! - **Surface**: metalness 1 for metallic stickers (foil, gold), none where the holographic mask is; roughness from
//!   the sticker's normal-roughness texture (B).
//! - **Wear** (`g_flStickerNWear` > 0): scratches (`g_tStickerScratches`) and the cavity eat the alpha past a
//!   threshold set by the sticker's alpha and `g_vWearBias…`; paper stickers show a grey backing where worn, others
//!   roughen.
//!
//! # As a bake
//!
//! The weapon's textures are in its first UV set; the stickers in its second. The bake draws the mesh's triangles
//! into the texture's layout (`UvMap`) to know, at each texel, where the sticker coordinates are (spread two texels
//! past the islands' edges, so filtering at a seam does not fetch the weapon's own colour), then blends the slots
//! there. The view-dependent parts are fixed: the holographic spectrum is read as seen face on, glitter sparkles
//! and the sticker edges' parallax are left out, and the sticker's normal is not baked.

use std::collections::BTreeMap;

use super::ExportError;
use super::gltf::{MaterialInfo, decode_raw, read_material, srgb_to_linear};
use super::paint::{Address, Sampler, clamp3, halve, lerp, lerp3, luminance, rgb, smoothstep};
use crate::content::Content;
use crate::export::environment::{SRGB_TO_LINEAR, linear_to_srgb};
use crate::items::section_values;
use crate::model::{Geometry, Mesh, Model, StickerSlot};
use crate::texture::Image;

/// The weapon shader's sticker slots.
pub const SLOTS: usize = 5;

/// `g_vAlbedoLevels`, `g_vMetallicAlbedoLevels`, `g_vDarkMetallicAlbedoLevels`, `g_vHoloAlbedoLevels` as their
/// expressions make them from (−0.045, 0.4, 1.15), (−0.45, 0.4, 1.08), (−0.1, 0.4, 1.08), (−0.05, 0.35, 1.15):
/// floor, exponent (`−log2(1 − y)`), ceiling (`2 − z`).
const ALBEDO_LEVELS: [f32; 3] = [0.045, 0.736_966, 0.85];
const METALLIC_LEVELS: [f32; 3] = [0.45, 0.736_966, 0.92];
const DARK_METALLIC_LEVELS: [f32; 3] = [0.1, 0.736_966, 0.92];
const HOLO_LEVELS: [f32; 3] = [0.05, 0.621_488, 0.85];

/// `g_fColorBoostFactor` (its expression, `64 − 1`).
const COLOR_BOOST_FACTOR: f32 = 63.0;

/// How far past the UV islands' edges the sticker coordinates spread, in texels.
const DILATE: usize = 2;

/// A sticker kit of `items_game.txt` (`sticker_kits`).
#[derive(Debug, Clone, PartialEq)]
pub struct StickerKit {
    /// Kit id, as a weapon's sticker attributes name it.
    pub id: u32,
    /// Internal name: `eslcologne2015_team_virtuspro_foil`.
    pub name: String,
    /// Its material: `stickers/cologne2015/virtuspro_foil.vmat`.
    pub material: String,
}

/// The sticker kits of `items_game.txt`'s text that name a material.
pub fn sticker_kits(items_game: &str) -> BTreeMap<u32, StickerKit> {
    let materials = section_values(items_game, "sticker_kits", "sticker_material");
    section_values(items_game, "sticker_kits", "name")
        .into_iter()
        .filter_map(|(id, name)| {
            let material = format!("stickers/{}.vmat", materials.get(&id)?);
            Some((id, StickerKit { id, name, material }))
        })
        .collect()
}

/// A sticker on a weapon: the slot, the kit's material, and what the item sets (`None`: the slot's own default).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// Slot, 0 – 4: the shader's, which draws them in that order.
    pub slot: usize,
    /// The model's slot it sits on (`StickerMarkup`'s index, and the material's defaults for it): the item's
    /// `sticker slot n schema`; `None`: `slot`. Aurora vs FURIA's Island Floral AK has its HellRaisers in slots 2,
    /// 3, 4 on places 3, 2, 1 (the AK marks up 0 – 3: slot 4 alone has no place of its own) and both Sea Swallows
    /// freely placed from place 0, as CS2 shows them.
    pub place: Option<usize>,
    /// The sticker's material.
    pub material: String,
    /// CS2's free placement, in the slot's offset units.
    pub offset: Option<[f32; 2]>,
    /// Size (1 = the slot's).
    pub scale: Option<f32>,
    /// Degrees.
    pub rotation: Option<f32>,
    /// Scraped, 0 – 1.
    pub wear: Option<f32>,
}

/// A slot of the weapon's material, with the item's values applied.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    offset: [f32; 2],
    scale: f32,
    /// Turns.
    rotation: f32,
    wear: f32,
}

impl Slot {
    /// Where `placed` is drawn: at its place (`Placed::place`, else its own slot) as the model's `markup` has it.
    fn placed(weapon: &MaterialInfo, markup: &[StickerSlot], placed: &Placed) -> Self {
        let place = placed.place.unwrap_or(placed.slot);
        Self::of(
            weapon,
            markup.iter().find(|s| s.index == place),
            place,
            placed,
        )
    }

    /// Slot `n` as the model marks it up (`markup`; else the material's own defaults, `weapon`'s
    /// `g_vStickerNOffset`…), moved as `placed` says: the offset by the item's, in stickers (÷ the slot's scale),
    /// turned by the item's degrees, the scale divided by the item's (a larger sticker spans more of the UV set).
    fn of(weapon: &MaterialInfo, markup: Option<&StickerSlot>, n: usize, placed: &Placed) -> Self {
        let vector = |name: &str| {
            weapon
                .vectors
                .get(&format!("g_vSticker{n}{name}"))
                .map_or([0.0; 2], |v| [v[0] as f32, v[1] as f32])
        };
        let float = |name: &str| {
            weapon
                .floats
                .get(&format!("g_flSticker{n}{name}"))
                .map_or(0.0, |v| *v as f32)
        };
        let (offset, scale, rotation) = match markup {
            Some(m) => (m.offset, m.scale.abs(), m.rotation),
            None => (
                vector("Offset"),
                vector("Scale")[0].abs(),
                float("Rotation"),
            ),
        };
        let shift = placed
            .offset
            .map_or([0.0; 2], |o| o.map(|x| x / scale.max(1e-3)));
        Self {
            offset: [offset[0] + shift[0], offset[1] + shift[1]],
            scale: placed.scale.map_or(scale, |s| scale / s.max(1e-3)),
            rotation: rotation + placed.rotation.map_or(0.0, |r| r / 360.0),
            wear: placed.wear.unwrap_or(0.0).clamp(0.0, 1.0),
        }
    }

    /// Where `uv1` (the second UV set) falls on the sticker, or `None` off it. A slot of scale 0 draws nothing (the
    /// shader's test).
    fn sticker_uv(&self, uv1: [f32; 2]) -> Option<[f32; 2]> {
        if self.scale == 0.0 {
            return None;
        }
        let x = (uv1[0] - 0.5 - self.offset[0]) * self.scale;
        let y = (uv1[1] - 0.5 - self.offset[1]) * self.scale;
        let (s, c) = (self.rotation * std::f32::consts::TAU).sin_cos();
        let st = [x * c - y * s + 0.5, x * s + y * c + 0.5];
        st.iter().all(|t| (0.0..=1.0).contains(t)).then_some(st)
    }
}

/// A texture and its smaller copies, to read it at the size it is drawn.
struct Mips(Vec<Sampler>);

impl Mips {
    fn load(content: &Content, path: &str) -> Result<Self, ExportError> {
        let mut levels = vec![Sampler {
            image: decode_raw(content, path)?.1,
        }];
        while levels.len() < 10 {
            let last = &levels[levels.len() - 1].image;
            if last.width.min(last.height) < 2 {
                break;
            }
            let half = halve(last);
            levels.push(Sampler { image: half });
        }
        Ok(Self(levels))
    }

    fn size(&self) -> f32 {
        let image = &self.0[0].image;
        image.width.max(image.height) as f32
    }

    /// Channels at `st` (clamped), from the level `texels` (sticker texels per output texel) asks for.
    fn at(&self, st: [f32; 2], texels: f32) -> [f32; 4] {
        let level = if texels > 1.0 {
            (texels.log2().round() as usize).min(self.0.len() - 1)
        } else {
            0
        };
        self.0[level].at_with(st[0], st[1], [Address::Clamp; 2])
    }
}

/// A sticker's look: its material's `…Sticker0` keys.
struct Look {
    color: Mips,
    normal_roughness: Option<Mips>,
    sfx_mask: Option<Mips>,
    holo_spectrum: Option<Sampler>,
    scratches: Option<Mips>,
    fitting: bool,
    legacy_tint: bool,
    metallic: bool,
    holographic: bool,
    glitter: bool,
    paper: bool,
    preserve_roughness: bool,
    clamp_spectrum: bool,
    /// `g_vColorTintSticker0`, linear (its expression).
    tint: [f32; 3],
    color_boost: f32,
    sfx_color_boost: f32,
    /// `1 − g_flTintSaturateSticker0` (its expression).
    tint_saturate: f32,
    wear_scratches: f32,
    wear_bias: [f32; 2],
}

impl Look {
    fn read(content: &Content, path: &str) -> Result<Self, ExportError> {
        let info = read_material(content, path)?;
        let flag = |name: &str, default: bool| {
            info.ints
                .get(&format!("g_b{name}Sticker0"))
                .map_or(default, |v| *v != 0)
        };
        let float = |name: &str, default: f32| info.floats.get(name).map_or(default, |v| *v as f32);
        let vector = |name: &str, default: [f32; 3]| {
            info.vectors
                .get(name)
                .map_or(default, |v| [v[0] as f32, v[1] as f32, v[2] as f32])
        };
        // An optional texture that cannot be read is left out (the effect it drives is then off).
        let texture = |name: &str| {
            info.textures
                .get(name)
                .and_then(|path| Mips::load(content, path).ok())
        };
        let color_path = info
            .textures
            .get("g_tSticker0")
            .ok_or_else(|| ExportError::Sticker(format!("{path}: no g_tSticker0")))?;
        let [bias_x, bias_y, _] = vector("g_vWearBiasSticker0", [1.0, 1.0, 0.0]);
        Ok(Self {
            color: Mips::load(content, color_path)?,
            normal_roughness: texture("g_tNormalRoughnessSticker0"),
            sfx_mask: texture("g_tSfxMaskSticker0"),
            holo_spectrum: info
                .textures
                .get("g_tHoloSpectrumSticker0")
                .and_then(|path| {
                    Some(Sampler {
                        image: decode_raw(content, path).ok()?.1,
                    })
                }),
            scratches: texture("g_tStickerScratches"),
            fitting: flag("AutomaticPBRColorFitting", true),
            legacy_tint: flag("LegacyTintMultiply", false),
            metallic: flag("Metallic", false),
            holographic: flag("Holographic", false),
            glitter: flag("Glitter", false),
            paper: flag("PaperBacking", false),
            preserve_roughness: flag("PreserveRoughness", false),
            clamp_spectrum: flag("ClampSpectrumV", true),
            tint: vector("g_vColorTintSticker0", [1.0; 3]).map(srgb_to_linear),
            color_boost: float("g_flColorBoostSticker0", 1.0),
            sfx_color_boost: float("g_flSfxColorBoostSticker0", 1.0),
            tint_saturate: 1.0 - float("g_flTintSaturateSticker0", 1.0),
            wear_scratches: float("g_fWearScratchesSticker0", 1.0),
            wear_bias: [bias_x, bias_y],
        })
    }
}

/// The weapon's second UV set at each texel of a texture in its first: where its triangles cover the texel (and two
/// texels around), `None` elsewhere.
pub(crate) struct UvMap {
    width: usize,
    height: usize,
    uv: Vec<Option<[f32; 2]>>,
}

impl UvMap {
    /// Draws `triangles` (each corner `[u₀, v₀, u₁, v₁]`) into a `width` × `height` texture's layout. A triangle is
    /// moved by whole tiles to where its first UV set's middle falls in [0, 1].
    pub(crate) fn draw(triangles: &[[[f32; 4]; 3]], width: usize, height: usize) -> Self {
        let mut uv = vec![None; width * height];
        for corners in triangles {
            let middle = |i: usize| (corners[0][i] + corners[1][i] + corners[2][i]) / 3.0;
            let (tile_u, tile_v) = (middle(0).floor(), middle(1).floor());
            let p = corners.map(|c| {
                [
                    (c[0] - tile_u) * width as f32,
                    (c[1] - tile_v) * height as f32,
                ]
            });
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
            if area.abs() < 1e-12 {
                continue;
            }
            let lo = |i: usize| p.iter().map(|q| q[i]).fold(f32::MAX, f32::min);
            let hi = |i: usize| p.iter().map(|q| q[i]).fold(f32::MIN, f32::max);
            let x0 = (lo(0) - 0.5).floor().max(0.0) as usize;
            let y0 = (lo(1) - 0.5).floor().max(0.0) as usize;
            let x1 = ((hi(0) - 0.5).ceil().max(0.0) as usize).min(width - 1);
            let y1 = ((hi(1) - 0.5).ceil().max(0.0) as usize).min(height - 1);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let edge = |a: [f32; 2], b: [f32; 2]| {
                        ((b[0] - a[0]) * (py - a[1]) - (px - a[0]) * (b[1] - a[1])) / area
                    };
                    let w = [edge(p[1], p[2]), edge(p[2], p[0]), edge(p[0], p[1])];
                    if w.iter().all(|&w| w >= -1e-4) {
                        let at = |i: usize| {
                            w[0] * corners[0][i] + w[1] * corners[1][i] + w[2] * corners[2][i]
                        };
                        uv[y * width + x] = Some([at(2), at(3)]);
                    }
                }
            }
        }
        let mut map = Self { width, height, uv };
        for _ in 0..DILATE {
            map.dilate();
        }
        map
    }

    /// Fills the empty texels next to filled ones with a neighbour's coordinates.
    fn dilate(&mut self) {
        let (w, h) = (self.width, self.height);
        let mut out = self.uv.clone();
        for y in 0..h {
            for x in 0..w {
                if self.uv[y * w + x].is_some() {
                    continue;
                }
                let neighbours = [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ];
                out[y * w + x] = neighbours
                    .iter()
                    .filter(|&&(nx, ny)| nx < w && ny < h)
                    .find_map(|&(nx, ny)| self.uv[ny * w + nx]);
            }
        }
        self.uv = out;
    }

    fn at(&self, x: usize, y: usize) -> Option<[f32; 2]> {
        (x < self.width && y < self.height)
            .then(|| self.uv[y * self.width + x])
            .flatten()
    }
}

/// A sticker ready to draw: its slot number, the slot's values and the sticker's look.
struct Drawn {
    index: usize,
    slot: Slot,
    look: Look,
}

/// What a slot leaves at a texel: colour (linear), roughness, metalness, and how much of the texel it covers.
struct Layer {
    color: [f32; 3],
    roughness: f32,
    metalness: f32,
    alpha: f32,
}

impl Drawn {
    /// Slot's sticker at sticker coordinates `st` (`texels` sticker texels per output texel), on a weapon texel
    /// whose sticker mask is `mask` and cavity `cavity`.
    fn layer(&self, st: [f32; 2], texels: f32, mask: f32, cavity: f32) -> Option<Layer> {
        let look = &self.look;
        let sample = look.color.at(st, texels);
        let raw_alpha = sample[3];
        let alpha = (raw_alpha * 12.75).clamp(0.0, 1.0) * mask;
        if alpha <= 0.0 {
            return None;
        }
        let stored = rgb(sample).map(srgb_to_linear);
        let sfx = look
            .sfx_mask
            .as_ref()
            .map_or([0.0, 0.0, 0.0, 1.0], |m| m.at(st, texels));
        // Glitter makes the sticker half metallic where its mask (A, inverted) is.
        let mut metal = if look.metallic { 1.0 } else { 0.0 };
        if look.glitter && 1.0 - sfx[3] != 0.0 {
            metal = f32::max(metal, (1.0 - sfx[3]) * 0.5);
        }
        let mut color = if look.fitting {
            fit(stored, look, metal)
        } else if look.metallic {
            let grey = luminance(stored);
            let c =
                std::array::from_fn(|i| lerp(stored[i], grey, look.tint_saturate) * look.tint[i]);
            c.map(|x| {
                x * if look.legacy_tint {
                    look.color_boost
                } else {
                    1.0
                }
            })
        } else {
            stored
        };
        if look.holographic
            && sfx[0] > 0.0
            && let Some(spectrum) = &look.holo_spectrum
        {
            // Seen face on: the view direction's dot with the normal is 1, with the shader's second vector 0.
            let address = [
                Address::Wrap,
                if look.clamp_spectrum {
                    Address::Clamp
                } else {
                    Address::Wrap
                },
            ];
            let s = spectrum.at_with(sfx[1] + 1.0, sfx[2], address);
            let s = rgb(s).map(srgb_to_linear);
            let holo = levels(s, s, HOLO_LEVELS, HOLO_LEVELS[0], look.sfx_color_boost);
            color = lerp3(color, holo, sfx[0]);
            metal *= 1.0 - sfx[0];
        }
        let mut roughness = look
            .normal_roughness
            .as_ref()
            .map_or(0.5, |t| t.at(st, texels)[2]);
        let mut cover = 1.0;
        let wear = self.slot.wear;
        if wear > 0.0 {
            let scratch = 1.0
                - look
                    .scratches
                    .as_ref()
                    .map_or(0.0, |t| t.at(st, texels)[0])
                    .min(look.wear_scratches);
            let scratched = lerp(scratch, scratch * 0.5, wear);
            let worn = lerp(
                wear,
                wear + 2.0 * (cavity - 0.5) * smoothstep(1.0, 0.85, wear),
                wear,
            )
            .clamp(0.0, 1.0);
            let [bias_x, bias_y] = look.wear_bias;
            let edge_bias = if look.paper {
                0.5
            } else {
                (bias_y * if bias_y > 0.0 { 0.5 } else { 0.25 } + 0.5) * 0.5
            };
            let through = (worn
                - ((raw_alpha - 0.078_431) * 1.085_106)
                    .clamp(0.0, 1.0)
                    .powf(bias_x * bias_x))
            .clamp(0.0, 1.0);
            if look.paper {
                let start = (through * 2.0).clamp(0.0, 1.0) + bias_y * worn;
                let backing = lerp(0.7, 0.2 + 0.4 * scratched, worn);
                color = lerp3(
                    [backing; 3],
                    color,
                    smoothstep(start, start + 0.1, scratched),
                );
            }
            let edge = (through * (1.0 + edge_bias) - edge_bias).clamp(0.0, 1.0);
            if !(look.metallic || look.preserve_roughness)
                && smoothstep(0.0, 0.35, worn) >= scratched
            {
                roughness = 0.8;
            }
            color = lerp3(color, color.map(|c| c * scratched), worn * 0.3);
            cover = smoothstep(edge, edge + 0.1, scratched);
        }
        Some(Layer {
            color: clamp3(color),
            roughness,
            metalness: metal,
            alpha: alpha * alpha * cover,
        })
    }
}

/// `g_bAutomaticPBRColorFitting…`: a sticker's stored colour `c` tinted by its metalness `metal` and fitted to the
/// albedo levels.
fn fit(c: [f32; 3], look: &Look, metal: f32) -> [f32; 3] {
    let tint: [f32; 3] = std::array::from_fn(|i| lerp(1.0, look.tint[i], metal));
    let saturate = lerp(0.0, look.tint_saturate, metal);
    // The legacy tint multiplies before fitting; the other tints the hue the fitting keeps.
    let (hue, source) = if look.legacy_tint {
        let grey = luminance(c);
        let t: [f32; 3] = clamp3(std::array::from_fn(|i| {
            lerp(c[i], grey, saturate) * tint[i]
        }));
        (t, t)
    } else {
        (
            std::array::from_fn(|i| lerp(tint[i] * c[i], tint[i], saturate)),
            c,
        )
    };
    let bright = luminance(clamp3(source.map(|x| x * look.color_boost)));
    let dark = smoothstep(0.55, 0.015_6, bright);
    let level: [f32; 3] = std::array::from_fn(|i| {
        lerp(
            ALBEDO_LEVELS[i],
            lerp(METALLIC_LEVELS[i], DARK_METALLIC_LEVELS[i], dark),
            metal,
        )
    });
    let floor = lerp(ALBEDO_LEVELS[0], DARK_METALLIC_LEVELS[0], metal);
    levels(hue, source, level, floor, look.color_boost)
}

/// The fitting the shader writes out for stickers and holograms: `hue`'s direction brought between `floor` and the
/// `levels` ceiling by `source`'s brightest channel (× `boost`) to the levels' exponent, lifted by the boost's excess.
fn levels(hue: [f32; 3], source: [f32; 3], levels: [f32; 3], floor: f32, boost: f32) -> [f32; 3] {
    let n = {
        let m = hue.map(|x| x.max(0.000_3));
        let length = m.iter().map(|x| x * x).sum::<f32>().sqrt();
        m.map(|x| x / length * 1.06)
    };
    let length = n.iter().map(|x| x * x).sum::<f32>().sqrt();
    let bright = luminance(clamp3(source.map(|x| x * boost)));
    let top = source[0].max(source[1]).max(source[2]);
    let scale = lerp(
        levels[0],
        levels[2],
        (top * boost).powf(levels[1]).clamp(0.0, 1.0),
    );
    let fitted: [f32; 3] = std::array::from_fn(|i| {
        f32::max(n[i] * floor * 1.732 / length / luminance(n), n[i] * scale)
    });
    let lifted: [f32; 3] = std::array::from_fn(|i| levels[2].min(fitted[i] + bright));
    let boosted = lerp3(fitted, lifted, (boost - 1.0).max(0.0) / COLOR_BOOST_FACTOR);
    lerp3(
        [floor; 3],
        boosted,
        smoothstep(0.000_3, floor, bright).powf(0.5),
    )
}

/// Where a weapon material's stickers go: the triangles drawn with it, each corner with both UV sets
/// (`[u₀, v₀, u₁, v₁]`), and the slots its model marks up on those meshes.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// The triangles.
    pub triangles: Vec<[[f32; 4]; 3]>,
    /// The model's slots (`StickerMarkup`); empty: the material's own defaults.
    pub slots: Vec<StickerSlot>,
}

impl Places {
    /// Adds the triangles of `geometry`, decoded from one of `mesh`'s draw calls. A draw call without a second UV
    /// set has no sticker places and is left out.
    pub(crate) fn add(&mut self, mesh: &Mesh, geometry: &Geometry) {
        let (Some(uv0), Some(uv1)) = (geometry.uvs.first(), geometry.uvs.get(1)) else {
            return;
        };
        for corners in geometry.indices.as_chunks::<3>().0 {
            self.triangles.push(corners.map(|i| {
                let i = i as usize;
                [uv0[i][0], uv0[i][1], uv1[i][0], uv1[i][1]]
            }));
        }
        if self.slots.is_empty() {
            self.slots = mesh.sticker_slots.clone();
        }
    }
}

/// Where `model` draws stickers with `material`, in its most detailed meshes.
pub fn places(model: &Model, material: &str) -> Places {
    let mut out = Places::default();
    for mesh in model.meshes_in_groups(u64::MAX) {
        for draw in mesh.draw_calls.iter().filter(|d| d.material == material) {
            if let Ok(geometry) = mesh.geometry(draw) {
                out.add(mesh, &geometry);
            }
        }
    }
    out
}

/// Draws `stickers` over one of a weapon material's baked textures (`target`), at the material's `places`
/// ([`places`]). Returns a warning per sticker it could not read; the others are drawn.
pub fn apply(
    content: &Content,
    weapon_material: &str,
    stickers: &[Placed],
    places: &Places,
    image: &mut Image,
    target: Target,
) -> Result<Vec<String>, ExportError> {
    let triangles = &places.triangles;
    let weapon = read_material(content, weapon_material)?;
    let inputs = weapon.textures.get("g_tStickerWepInputs").and_then(|path| {
        Some(Sampler {
            image: decode_raw(content, path).ok()?.1,
        })
    });
    let mut warnings = Vec::new();
    let mut drawn: Vec<Drawn> = Vec::new();
    for placed in stickers {
        if placed.slot >= SLOTS {
            warnings.push(format!(
                "sticker slot {} not drawn (the shader has {SLOTS})",
                placed.slot
            ));
            continue;
        }
        match Look::read(content, &placed.material) {
            Ok(look) => drawn.push(Drawn {
                index: placed.slot,
                slot: Slot::placed(&weapon, &places.slots, placed),
                look,
            }),
            Err(e) => warnings.push(format!("sticker {}: {e}", placed.material)),
        }
    }
    // The shader draws slot 0 first, each next one over it.
    drawn.sort_by_key(|d| d.index);
    if drawn.is_empty() {
        return Ok(warnings);
    }
    match target {
        Target::Color => paint_image(image, &drawn, triangles, inputs.as_ref(), |px, layer| {
            let base = [px[0], px[1], px[2]].map(|c| SRGB_TO_LINEAR[usize::from(c)]);
            let mixed = lerp3(base, layer.color, layer.alpha);
            px[..3].copy_from_slice(&mixed.map(linear_to_srgb));
        }),
        Target::Surface => paint_image(image, &drawn, triangles, inputs.as_ref(), |px, layer| {
            let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            let roughness = lerp(f32::from(px[1]) / 255.0, layer.roughness, layer.alpha);
            let metalness = lerp(f32::from(px[2]) / 255.0, layer.metalness, layer.alpha);
            px[1] = byte(roughness);
            px[2] = byte(metalness);
        }),
    }
    Ok(warnings)
}

/// Which of a weapon material's textures the stickers are drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// The colour (sRGB).
    Color,
    /// The glTF metallic-roughness (G roughness, B metalness).
    Surface,
}

/// Stickers to draw over one of a weapon material's textures, in an export ([`apply`] with the triangles the
/// writer collected for the material). Two jobs are the same when their material, label and target are.
#[derive(Debug, Clone)]
pub(crate) struct StickerJob {
    pub(crate) material: String,
    /// The paint's label, which names its stickers ([`super::paint::Paint::label`]).
    pub(crate) label: String,
    pub(crate) stickers: std::sync::Arc<Vec<Placed>>,
    pub(crate) target: Target,
}

impl StickerJob {
    fn key(&self) -> (&str, &str, Target) {
        (&self.material, &self.label, self.target)
    }
}

impl PartialEq for StickerJob {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for StickerJob {}

impl std::hash::Hash for StickerJob {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl PartialOrd for StickerJob {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StickerJob {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

/// Blends the slots, in order, over every texel of `image` the second UV set reaches: `blend` mixes one slot's
/// [`Layer`] into the texel's bytes.
fn paint_image(
    image: &mut Image,
    drawn: &[Drawn],
    triangles: &[[[f32; 4]; 3]],
    inputs: Option<&Sampler>,
    blend: impl Fn(&mut [u8], &Layer) + Sync,
) {
    use rayon::prelude::*;
    let map = UvMap::draw(triangles, image.width, image.height);
    let width = image.width;
    image
        .rgba
        .par_chunks_mut(width * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let Some(uv1) = map.at(x, y) else {
                    continue;
                };
                if uv1[0] <= -1.0 {
                    continue;
                }
                let (u, v) = (
                    (x as f32 + 0.5) / map.width as f32,
                    (y as f32 + 0.5) / map.height as f32,
                );
                let [mask, cavity, ..] = inputs.map_or([1.0, 0.5, 0.0, 0.0], |s| s.at(u, v));
                if mask <= 0.0 {
                    continue;
                }
                // How far the sticker coordinates move to the next texel: the size the sticker is read at.
                let next = map.at(x + 1, y).or_else(|| map.at(x.wrapping_sub(1), y));
                let below = map.at(x, y + 1).or_else(|| map.at(x, y.wrapping_sub(1)));
                for d in drawn {
                    let Some(st) = d.slot.sticker_uv(uv1) else {
                        continue;
                    };
                    let step = |n: Option<[f32; 2]>| {
                        n.map_or(0.0, |n| {
                            let (du, dv) = (n[0] - uv1[0], n[1] - uv1[1]);
                            (du * du + dv * dv).sqrt() * d.slot.scale
                        })
                    };
                    let texels = step(next).max(step(below)) * d.look.color.size();
                    if let Some(layer) = d.layer(st, texels, mask, cavity) {
                        blend(px, &layer);
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(offset: [f32; 2], scale: f32, rotation: f32) -> Slot {
        Slot {
            offset,
            scale,
            rotation,
            wear: 0.0,
        }
    }

    #[test]
    fn a_slot_places_its_sticker_around_its_offset() {
        // Scale 10 around (0.6, 0.4): the sticker spans 0.1 of the second UV set, centred there.
        let s = slot([0.1, -0.1], 10.0, 0.0);
        let st = s.sticker_uv([0.6, 0.4]).expect("centre on the sticker");
        assert!(
            (st[0] - 0.5).abs() < 1e-5 && (st[1] - 0.5).abs() < 1e-5,
            "{st:?}"
        );
        assert!(s.sticker_uv([0.6 + 0.06, 0.4]).is_none());
        let st = s.sticker_uv([0.6 + 0.04, 0.4]).expect("on the sticker");
        assert!((st[0] - 0.9).abs() < 1e-4, "{st:?}");
        // A quarter turn moves +u on the weapon to +v on the sticker.
        let st = slot([0.1, -0.1], 10.0, 0.25)
            .sticker_uv([0.64, 0.4])
            .expect("on the sticker");
        assert!(
            (st[0] - 0.5).abs() < 1e-4 && (st[1] - 0.9).abs() < 1e-4,
            "{st:?}"
        );
    }

    #[test]
    fn free_placements_shift_the_marked_up_slot_in_stickers() {
        let markup = StickerSlot {
            index: 2,
            offset: [0.2, -0.4],
            scale: 10.0,
            rotation: 0.25,
        };
        let placed = |offset, rotation| Placed {
            slot: 2,
            place: None,
            material: String::new(),
            offset,
            scale: None,
            rotation,
            wear: None,
        };
        let weapon = MaterialInfo::default();
        // Nothing set: the markup's slot.
        let s = Slot::of(&weapon, Some(&markup), 2, &placed(None, None));
        assert_eq!((s.offset, s.scale, s.rotation), ([0.2, -0.4], 10.0, 0.25));
        // An offset of one sticker moves it by 1 / scale; degrees add turns.
        let s = Slot::of(
            &weapon,
            Some(&markup),
            2,
            &placed(Some([1.0, -0.5]), Some(90.0)),
        );
        assert!(
            (s.offset[0] - 0.3).abs() < 1e-6 && (s.offset[1] + 0.45).abs() < 1e-6,
            "{s:?}"
        );
        assert!((s.rotation - 0.5).abs() < 1e-6);
        // Without markup, the material's defaults (none here: scale 0, never drawn).
        let unset = Slot::of(&weapon, None, 2, &placed(None, None));
        assert_eq!(unset.scale, 0.0);
        assert!(unset.sticker_uv([0.5, 0.5]).is_none());
    }

    #[test]
    fn a_sticker_sits_on_the_place_its_schema_names() {
        // The AK marks up places 0 – 3; Aurora vs FURIA's Island Floral AK has a sticker in slot 4 on place 1.
        let markup: Vec<StickerSlot> = [0.148, 0.061, -0.025, -0.164]
            .iter()
            .enumerate()
            .map(|(index, &u)| StickerSlot {
                index,
                offset: [u, -0.434],
                scale: 14.1,
                rotation: 0.0,
            })
            .collect();
        let weapon = MaterialInfo::default();
        let sticker = |slot, place| Placed {
            slot,
            place,
            material: String::new(),
            offset: None,
            scale: None,
            rotation: None,
            wear: None,
        };
        assert_eq!(
            Slot::placed(&weapon, &markup, &sticker(4, Some(1))).offset,
            [0.061, -0.434]
        );
        assert_eq!(
            Slot::placed(&weapon, &markup, &sticker(2, Some(3))).offset,
            [-0.164, -0.434]
        );
        // Without a schema, its own slot; slot 4 has no place of its own on the AK (scale 0: not drawn).
        assert_eq!(
            Slot::placed(&weapon, &markup, &sticker(2, None)).offset,
            [-0.025, -0.434]
        );
        assert_eq!(Slot::placed(&weapon, &markup, &sticker(4, None)).scale, 0.0);
    }

    #[test]
    fn the_uv_map_interpolates_and_spreads_past_the_edges() {
        // One triangle over the left half of a 4 × 4 texture, its second UV set the first doubled.
        let corner = |u: f32, v: f32| [u, v, 2.0 * u, 2.0 * v];
        let map = UvMap::draw(
            &[[corner(0.0, 0.0), corner(0.5, 0.0), corner(0.0, 1.0)]],
            4,
            4,
        );
        let at = map.at(0, 0).expect("covered");
        assert!(
            (at[0] - 0.25).abs() < 1e-5 && (at[1] - 0.25).abs() < 1e-5,
            "{at:?}"
        );
        // Texel (3, 3) is past the edge by more than the spread; (2, 1) is within it.
        assert!(map.at(3, 3).is_none());
        assert!(map.at(2, 1).is_some());
    }

    #[test]
    fn fitting_keeps_a_grey_sticker_grey_and_within_the_levels() {
        let look_grey = |c: f32| {
            let out = levels([c; 3], [c; 3], ALBEDO_LEVELS, ALBEDO_LEVELS[0], 1.0);
            assert!(
                (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
                "{out:?}"
            );
            out[0]
        };
        // Black lifts to the floor, white stays under the ceiling, and brighter stays brighter.
        assert!((look_grey(0.0) - ALBEDO_LEVELS[0]).abs() < 1e-3);
        assert!(look_grey(1.0) <= ALBEDO_LEVELS[2] + 1e-3);
        assert!(look_grey(0.6) > look_grey(0.2));
    }
}
