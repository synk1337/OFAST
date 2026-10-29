//! Skins baked onto weapon textures (M10): a weapon material's colour and metal-roughness, painted with a skin's
//! parameters ([`crate::paint`]), as CS2's composite materials generate them into the weapon's own textures
//! (`docs/research/cs2-skins.md`).
//!
//! # CS2's paint shader, as OFAST's own Rust
//!
//! CS2 renders the paint material (`csgo_customweapon.vfx`) over the weapon's UV layout twice, once for the colour
//! and once for roughness and metalness (`F_ROUGHNESS_MODE`); every input is a texture in that layout (the weapon's
//! composite inputs) or one placed on it by a transform. This bake does the same per texel (decision 024).
//!
//! Where the knowledge comes from — Valve's compiled shader in CS2's own `shaders_vulkan_dir.vpk`
//! (`csgo_customweapon_vulkan_50_ps.vcs`, format 72), each paint style's SPIR-V decompiled to GLSL with SPIRV-Cross,
//! names restored as Source 2 Viewer 20.1 restores them (MIT; `docs/research/cs2-skins.md` has the steps). Static
//! combos 0 – 8 are the nine styles (`S_PAINT_STYLE`), 288 – 296 the same with `F_SEPARATE_CHANNEL_INPUTS` (the
//! composite inputs of CS2's own weapon models; the CS:GO-era ones, knives among them, use the first). The vertex
//! stage and style 0 match SteamDatabase's GameTracking-CS2 (`csgo_customweapon.slang`). Every style shares:
//!
//! - **Placements** (vertex stage): pattern, wear and grunge each at `s·R(θ)·uv + t` (`Placement`), the scale
//!   `g_fl*TexCoordScale` times the weapon's size (`g_flUvScale1`, 1 with `g_bIgnoreWeaponSizeScale`), scale,
//!   rotation and offset rounded to hundredths, and the translation the shader derives from them. Spray paint and
//!   anodized airbrushed (2, 5) instead project the pattern from the weapon's 3D position (`g_tPosition`, 17 taps
//!   averaged, × 2) along three axes blended by the surface normal (`g_tSurface`), scaled by `g_flWeaponLength1 / 36`.
//! - **The inputs**: cavity (`g_tAmbientOcclusion` R, sRGB; `0.96 R^1.5` with separate channels), ambient occlusion
//!   (its G), the no-paint mask (its A; a texture stored without alpha counts as none), the paint masks (`g_tMasks`
//!   RGB), the wear texture (`g_tWear` R), the grunge (`g_tGrunge`, sRGB colour), the wear amount `w`.
//! - **Paint gone** (the weapon's own colour and surface show): `smoothstep(0.58 − k, 0.68 + k, (no_paint + wear ×
//!   cavity) × (6w + 1) × d)`, `d` the paint's durability (`1 − g_vPaintDurability`, by mask or pattern channel),
//!   `k = g_fWearSoftness × d`; with separate channels `0.56 – 0.74` and the cavity through a curve that sharpens
//!   with wear.
//! - **Grunge**: `lerp(1, grunge, (1 − cavity)⁴ / 4 + 3w / 4)` multiplies the paint's colour; its alpha roughens it,
//!   `+ (1 − a) × w² / 2`.
//! - **Albedo levels**: the paint's colour pulled by the wear towards a floor-and-ceiling remap
//!   (`albedo_levels`).
//!
//! Then each style its own way (`Inputs::shade`): solid colours by mask (0); hydrographic and spray paint, a
//! pattern's channels picking colours that wear off layer by layer (1, 2); anodized, the bare metal tinted where
//! `g_tMasks` R is, with a steel edge where it wears (3), its colours picked by a pattern (4, Dopplers) or a
//! projected one (5, Fades); a custom paint job's own colours (6); patina's metal (7) and gunsmith's paint over metal
//! parts (8), scratched and patinated by cavity, ambient occlusion and grunge. The paint textures
//! (`F_ROUGHNESS_TEXTURE`, `F_METALNESS_TEXTURE`) replace the roughness and metalness values, as their variants do.
//!
//! The variants, each its own static combos (576 + n, 1152 + n, 18 + 2; the halftone a switch in the spray styles'
//! code):
//!
//! - **Overlays** (`F_OVERLAY_TEXTURE`, on solid, hydrographic, spray paint, custom and gunsmith: the styles CS2's
//!   kits put one on): `g_tOverlay` (sRGB) on the weapon's UVs, the pattern's or its own placement, its alpha masked
//!   by a paint colour's region (`F_OVERLAY_MASK` 1 – 5: by the masks or the pattern's channels, as the style picks
//!   its colours), the pattern, or `g_tOverlayMask` (8). Its durability scales the paint's wear under it; with mask 0
//!   it is a layer of its own whose wear (`wear × cavity × growth × durability`) keeps the paint it covers. Blended
//!   over the paint's colour (`blend`: normal, colour, multiply, add, layer) before the grunge and the levels, its
//!   roughness and metalness taking a share; the patterned styles first mix the paint towards the weapon where gone.
//! - **Halftone** (`F_SPRAYPAINT_HALFTONE`, spray paint and airbrushed): the projected pattern darkened by cavity ×
//!   occlusion, × three more projections' dots turned by their rotation offsets, then thresholded.
//! - **All masks** (`F_USE_ALL_MASKS`, spray paint): colours 2 and 3 also by the paint masks' G and B.
//! - **Case hardening** (`F_CASE_HARDENING`, solid, patina, gunsmith): a colour ramp read across by the weapon's
//!   shape (cavity, occlusion, the normal map's flatness) and the pattern's R, down by its G; solid's colours take it
//!   by their `g_vCaseHardening` share, patina's pattern is it, gunsmith's where the masks' G is. Its alpha scales the
//!   metalness, its brightness lowers the roughness.
//!
//! **Not baked**: patina age (`F_PATINA_AGE`, 4 kits) and trilinear case hardening (1) — those weapons keep their own
//! finish (`unpainted_reason`). Pearlescence and the normal-map pass are not read.
//!
//! **Size**: the weapon's colour texture's or the pattern's, the larger, 2048 at most; the alpha is the weapon
//! colour's (alpha-tested parts keep their cut-outs). Each colour and surface job decodes its inputs on its own (the
//! weapon's composite inputs and the shared wear and grunge textures again for each skin): a round of about 30 skins
//! exports in about a minute and a quarter.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use crate::content::Content;
use crate::export::ExportError;
use crate::export::environment::{SRGB_TO_LINEAR, linear_to_srgb};
use crate::export::gltf::{decode_raw, srgb_to_linear};
use crate::export::keychains::{Charm, keychain_models};
use crate::export::loadout::Finish;
use crate::export::stickers::{Placed, StickerKit, sticker_kits};
use crate::paint::{PaintKit, Param, Recipe, WeaponParams, kit_recipe, paint_kits};
use crate::resource::Resource;
use crate::texture::{Image, ImageF32, Texture};

/// A skin to paint on a model's materials: what a weapon look carries into the glTF writer.
#[derive(Debug, Clone)]
pub struct Paint {
    /// A short name for the painted materials and textures, unique to the kit, seed, wear and stickers:
    /// `cu_fireserpent_ak47_bravo_245_1434` ([`Paint::label_of`]), `…_st3fa2c1d0` with stickers.
    pub label: String,
    /// The paint kit's recipe; `None` for stickers on the weapon's own finish.
    pub recipe: Option<Arc<Recipe>>,
    /// Pattern seed.
    pub seed: u32,
    /// Wear, 0 – 1.
    pub wear: f32,
    /// The stickers, drawn over the paint on the materials that draw them (`F_STICKERS`).
    pub stickers: Vec<Placed>,
    /// A name to draw on the name tag plate's material ([`super::nametags`]).
    pub name_tag: Option<String>,
    /// A count for the StatTrak module's digits (`F_STATTRAK` draw calls, [`super::stattrak`]).
    pub stattrak: Option<u32>,
}

impl Paint {
    /// Nothing painted yet: `label` and the defaults.
    fn bare(label: String) -> Self {
        Self {
            label,
            recipe: None,
            seed: 0,
            wear: 0.0,
            stickers: Vec::new(),
            name_tag: None,
            stattrak: None,
        }
    }

    /// What a name tag plate is painted with: its text.
    pub fn name_tag(text: String) -> Self {
        let hash = blake3::hash(text.as_bytes());
        Self {
            name_tag: Some(text),
            ..Self::bare(format!("nametag_{}", &hash.to_hex()[..8]))
        }
    }

    /// What a StatTrak module shows: its count.
    pub fn stattrak(count: u32) -> Self {
        Self {
            stattrak: Some(count),
            ..Self::bare(format!("stattrak_{count}"))
        }
    }

    /// The label of kit `kit` at `seed` and `wear` (wear to 4 digits, as CS2's inspect panel shows more).
    pub fn label_of(kit: &str, seed: u32, wear: f32) -> String {
        format!("{kit}_{seed}_{:04}", (wear * 10_000.0).round() as u32)
    }
}

/// The paint and sticker kits of `items_game.txt`, and each paint kit's recipe read once (on first use), for a
/// round's export.
#[derive(Debug, Default)]
pub struct Paints {
    kits: BTreeMap<u32, PaintKit>,
    stickers: BTreeMap<u32, StickerKit>,
    keychains: BTreeMap<u32, Charm>,
    recipes: Mutex<HashMap<u32, Option<Arc<Recipe>>>>,
}

impl Paints {
    /// The kits of `items_game.txt`'s text (none without it: weapons keep their own finish).
    pub fn new(items_game: Option<&str>) -> Self {
        Self {
            kits: items_game.map(paint_kits).unwrap_or_default(),
            stickers: items_game.map(sticker_kits).unwrap_or_default(),
            keychains: items_game.map(keychain_models).unwrap_or_default(),
            recipes: Mutex::default(),
        }
    }

    /// The charm of keychain definition `id`, if `items_game.txt` names its model.
    pub fn keychain(&self, id: u32) -> Option<&Charm> {
        self.keychains.get(&id)
    }

    /// The paint of `finish`: its kit's recipe and its stickers. A kit or recipe that cannot be read is left out, a
    /// sticker kit that is not known too, each with a warning (the first time for a paint kit; without
    /// `items_game.txt`, quietly: the export warned about it already); `None` when nothing is left.
    pub fn paint(
        &self,
        content: &Content,
        finish: Finish,
        warnings: &mut Vec<String>,
    ) -> Option<Paint> {
        if self.kits.is_empty() {
            return None;
        }
        let kit = (finish.kit != 0)
            .then(|| self.kit_recipe(content, finish.kit, warnings))
            .flatten();
        let mut stickers = Vec::new();
        for (slot, s) in finish.stickers.iter().enumerate() {
            let Some(s) = s else { continue };
            let Some(sticker) = self.stickers.get(&s.id) else {
                warnings.push(format!(
                    "sticker kit {}: not in items_game.txt, left out",
                    s.id
                ));
                continue;
            };
            stickers.push(Placed {
                slot,
                material: sticker.material.clone(),
                offset: s.offset.map(|o| o.map(f32::from_bits)),
                scale: s.scale.map(f32::from_bits),
                rotation: s.rotation.map(f32::from_bits),
                wear: s.wear.map(f32::from_bits),
            });
        }
        if kit.is_none() && stickers.is_empty() {
            return None;
        }
        let mut label = match &kit {
            Some((name, _)) => Paint::label_of(name, finish.seed, finish.wear()),
            None => "own".into(),
        };
        if !stickers.is_empty() {
            // Short and stable: the stickers' values hashed.
            let hash = blake3::hash(format!("{:?}", finish.stickers).as_bytes());
            label = format!("{label}_st{}", &hash.to_hex()[..8]);
        }
        Some(Paint {
            label,
            recipe: kit.map(|(_, recipe)| recipe),
            seed: finish.seed,
            wear: finish.wear(),
            stickers,
            name_tag: None,
            stattrak: None,
        })
    }

    /// Paint kit `id`'s name and recipe, read once; `None`, with a warning the first time, when either cannot be.
    fn kit_recipe(
        &self,
        content: &Content,
        id: u32,
        warnings: &mut Vec<String>,
    ) -> Option<(String, Arc<Recipe>)> {
        let mut recipes = self.recipes.lock().expect("lock");
        let Some(kit) = self.kits.get(&id) else {
            recipes.entry(id).or_insert_with(|| {
                warnings.push(format!(
                    "paint kit {id}: not in items_game.txt, left unpainted"
                ));
                None
            });
            return None;
        };
        let recipe = recipes
            .entry(id)
            .or_insert_with(|| match kit_recipe(content, kit) {
                Ok(recipe) => Some(Arc::new(recipe)),
                Err(e) => {
                    warnings.push(format!(
                        "paint kit {} ({}): {e}, left unpainted",
                        kit.id, kit.name
                    ));
                    None
                }
            })
            .clone()?;
        Some((kit.name.clone(), recipe))
    }
}

/// One weapon material painted with one skin: the parameters to bake from ([`crate::paint::weapon_params`]: the
/// colour's and the surface's). Two jobs are the same when their keys are (material, kit, seed, wear).
#[derive(Debug, Clone)]
pub(crate) struct PaintJob {
    pub(crate) key: String,
    pub(crate) params: Arc<WeaponParams>,
}

impl PartialEq for PaintJob {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for PaintJob {}

impl Hash for PaintJob {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl PartialOrd for PaintJob {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PaintJob {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

/// The largest side a painted texture gets (CS2 generates 4096 or 2048).
const MAX_SIZE: usize = 2048;

/// How a texture repeats past 0 – 1 (`RsTextureAddressMode_t`: wrap 0, mirror 1, clamp 2, border 3, mirror once 4;
/// border is read as clamp).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Address {
    Wrap,
    Mirror,
    Clamp,
    MirrorOnce,
}

impl Address {
    fn of(mode: i64) -> Self {
        match mode {
            1 => Self::Mirror,
            2 | 3 => Self::Clamp,
            4 => Self::MirrorOnce,
            _ => Self::Wrap,
        }
    }

    /// The two texels a bilinear sample at `t` (0 – 1 across `n` texels) blends, and the second one's weight.
    fn texels(self, t: f32, n: usize) -> (usize, usize, f32) {
        let t = match self {
            Self::Wrap => {
                let x = (t.rem_euclid(1.0) * n as f32 - 0.5).rem_euclid(n as f32);
                let x0 = x.floor() as usize % n;
                return (x0, (x0 + 1) % n, x.fract());
            }
            Self::Mirror => {
                let t = t.rem_euclid(2.0);
                if t > 1.0 { 2.0 - t } else { t }
            }
            Self::Clamp => t,
            Self::MirrorOnce => t.abs(),
        };
        let x = (t.clamp(0.0, 1.0) * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let x0 = x.floor() as usize;
        (x0, (x0 + 1).min(n - 1), x - x0 as f32)
    }
}

/// Bilinear samples of a decoded texture at UV coordinates.
pub(super) struct Sampler {
    pub(super) image: Image,
}

impl Sampler {
    pub(super) fn load(content: &Content, path: &str) -> Result<Self, ExportError> {
        Ok(Self {
            image: decode_raw(content, path)?.1,
        })
    }

    /// Channels 0 – 3 at (`u`, `v`), each 0 – 1 as stored, wrapping around.
    pub(super) fn at(&self, u: f32, v: f32) -> [f32; 4] {
        self.at_with(u, v, [Address::Wrap; 2])
    }

    /// Channels 0 – 3 at (`u`, `v`), with an address mode per axis.
    pub(super) fn at_with(&self, u: f32, v: f32, address: [Address; 2]) -> [f32; 4] {
        let (w, h) = (self.image.width, self.image.height);
        let (x0, x1, fx) = address[0].texels(u, w);
        let (y0, y1, fy) = address[1].texels(v, h);
        let px = |x: usize, y: usize| &self.image.rgba[(y * w + x) * 4..][..4];
        std::array::from_fn(|c| {
            let top = f32::from(px(x0, y0)[c]) * (1.0 - fx) + f32::from(px(x1, y0)[c]) * fx;
            let bottom = f32::from(px(x0, y1)[c]) * (1.0 - fx) + f32::from(px(x1, y1)[c]) * fx;
            (top * (1.0 - fy) + bottom * fy) / 255.0
        })
    }
}

/// A float texture's first image (`g_tPosition` is RGBA16F); `None` for one without any.
fn load_f32(content: &Content, path: &str) -> Result<Option<ImageF32>, ExportError> {
    let data = content.read_resource(path)?;
    let texture = Texture::parse(&Resource::parse(&data)?)?;
    Ok(texture.decode_slices_f32(&data)?.into_iter().next())
}

/// A texture placement as the shader builds it (`g_v*TexCoordXform0`, `1`): scale `s`, rotation `θ` (degrees) and
/// offset rounded to hundredths, then `u' = s·(cos θ·u − sin θ·v) + tu`, `v' = s·(sin θ·u + cos θ·v) + tv`. The
/// translation is the shader's own expression of the three: the offset itself at 0° and 90°, the offset plus a
/// rotation-dependent shift in between.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placement {
    rows: [[f32; 3]; 2],
}

/// `x` rounded to hundredths, as the shader's expressions do: `floor((x + 0.005) × 100) / 100`.
fn hundredths(x: f32) -> f32 {
    ((x + 0.005) * 100.0).floor() / 100.0
}

impl Placement {
    fn new(scale: f32, rotation: f32, offset: [f32; 2]) -> Self {
        Self::exact(
            hundredths(scale),
            hundredths(rotation),
            offset.map(hundredths),
        )
    }

    /// The same construction without the rounding, as the gloves' shader builds its placements.
    pub(super) fn exact(s: f32, rotation: f32, [ou, ov]: [f32; 2]) -> Self {
        let angle = rotation * std::f32::consts::PI / 180.0;
        let (sin, cos) = angle.sin_cos();
        // The expression's v6 – v10, as written.
        let half = 0.5 / if s != 0.0 { s } else { 1.0 };
        let (sin_back, cos_back) = (-angle).sin_cos();
        let a = half * cos_back - half * sin_back;
        let b = a * sin_back + half * cos_back;
        Self {
            rows: [
                [cos * s, -sin * s, s * cos * a - s * sin * b + (ou - 0.5)],
                [sin * s, cos * s, s * sin * a + s * cos * b + (ov - 0.5)],
            ],
        }
    }

    pub(super) fn apply(&self, u: f32, v: f32) -> (f32, f32) {
        let [x, y] = self.rows.map(|r| r[0] * u + r[1] * v + r[2]);
        (x, y)
    }
}

pub(super) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(super) fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|c| lerp(a[c], b[c], t))
}

pub(super) fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| a[c] * b[c])
}

pub(super) fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    a.map(|x| x * k)
}

pub(super) fn clamp3(a: [f32; 3]) -> [f32; 3] {
    a.map(|x| x.clamp(0.0, 1.0))
}

pub(super) fn rgb(a: [f32; 4]) -> [f32; 3] {
    [a[0], a[1], a[2]]
}

/// The shaders' luminance weights.
pub(super) fn luminance(c: [f32; 3]) -> f32 {
    c[0] * 0.2125 + c[1] * 0.7154 + c[2] * 0.0721
}

/// An image at half its size, each texel the mean of four: the next mip level, for textures the bakes read smaller
/// than they are stored (the gloves' layers, stickers).
pub(super) fn halve(image: &Image) -> Image {
    let (width, height) = (image.width / 2, image.height / 2);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            for c in 0..4 {
                let at = |dx: usize, dy: usize| {
                    u32::from(image.rgba[((2 * y + dy) * image.width + 2 * x + dx) * 4 + c])
                };
                rgba.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
            }
        }
    }
    Image::new(width, height, rgba)
}

/// GLSL's `smoothstep`, also with `low` above `high` (a falling edge, as the shader uses it); equal edges step.
pub(super) fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    if low == high {
        return if x >= low { 1.0 } else { 0.0 };
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A pattern alpha band that cuts the paint (0.6 – 0.9: `smoothstep(0.5, 0.6, a) × smoothstep(1, 0.9, a)`).
fn cutout(alpha: f32) -> f32 {
    smoothstep(0.5, 0.6, alpha) * smoothstep(1.0, 0.9, alpha)
}

/// `values` picked by three weights in turn: `lerp(lerp(lerp(v0, v1, t0), v2, t1), v3, t2)` — the shader's way to
/// choose per-colour values by paint masks or pattern channels.
pub(super) fn pick(values: [f32; 4], t: [f32; 3]) -> f32 {
    lerp(
        lerp(lerp(values[0], values[1], t[0]), values[2], t[1]),
        values[3],
        t[2],
    )
}

/// [`pick`], then the paint masks' G and B over values 2 and 3 (hydrographic, the anodized ones).
fn pick_masked(values: [f32; 4], t: [f32; 3], masks: [f32; 4]) -> f32 {
    lerp(
        lerp(pick(values, t), values[2], masks[1]),
        values[3],
        masks[2],
    )
}

/// The colour channel `c` of the four paint colours.
fn channel(colors: &[[f32; 3]; 4], c: usize) -> [f32; 4] {
    colors.map(|color| color[c])
}

/// Steel where anodized paint wears through.
const STEEL: [f32; 3] = [0.38, 0.37, 0.35];

/// The 17 taps averaged from the position texture (× 0.2 UV), as the decompiled shader holds them.
const POSITION_TAPS: [[f32; 2]; 17] = [
    [-0.001_072_34, -0.004_002_03],
    [0.001_953_12, -0.003_382_91],
    [0.004_002_03, -0.001_072_34],
    [-0.000_714_896, -0.002_668_02],
    [0.000_976_565, -0.001_691_46],
    [0.002_668_02, -0.000_714_896],
    [-0.003_382_91, -0.001_953_12],
    [-0.001_691_46, -0.000_976_565],
    [0.0, 0.0],
    [0.001_691_46, 0.000_976_565],
    [0.003_382_91, 0.001_953_12],
    [-0.002_668_02, 0.000_714_896],
    [-0.000_976_565, 0.001_691_46],
    [0.000_714_896, 0.002_668_02],
    [-0.004_002_03, 0.001_072_34],
    [-0.001_953_12, 0.003_382_91],
    [0.001_072_34, 0.004_002_02],
];

/// The paint's colour pulled by the wear towards its albedo levels: `lerp(painted, hue × lerp(min(floor,
/// luminance(base)), 1, saturate(max(painted)^exponent)), wear)`, the hue `normalize(painted)` over its largest
/// channel. The levels are `g_vPaintAlbedoLevels` (−0.045, 0.6, 1) and `g_vMetallicPaintAlbedoLevels`
/// (−0.08, 0.6, 1), mixed by `metal`, as their expressions make them a floor (0.045 – 0.08), an exponent
/// (`−log2(1 − 0.6)`) and a ceiling (1).
fn albedo_levels(base: [f32; 3], painted: [f32; 3], metal: f32, wear: f32) -> [f32; 3] {
    let floor = 0.045 + (0.08 - 0.045) * metal;
    let exponent = -(1.0f32 - 0.6).log2();
    let largest = |c: [f32; 3]| c[0].max(c[1]).max(c[2]);
    let clamped = painted.map(|c| c.max(0.0003));
    let length = clamped.iter().map(|c| c * c).sum::<f32>().sqrt();
    let hue = clamped.map(|c| c / length);
    let low = floor.min(luminance(base));
    let level = low + (1.0 - low) * largest(painted).powf(exponent).clamp(0.0, 1.0);
    let top = largest(hue);
    std::array::from_fn(|i| painted[i] + (hue[i] * level / top - painted[i]) * wear)
}

/// `g_vAlbedoLevels` as its expression makes it from (−0.045, 0.4, 1.15): a floor, an exponent (`−log2(1 − 0.4)`)
/// and a ceiling — the levels the overlay's colour, multiply and add blends land in.
const OVERLAY_LEVELS: [f32; 3] = [0.045, 0.736_966, 0.85];

/// The hue the overlay blends keep: `normalize(max(0.0003, c)) × 1.06`.
fn blend_hue(c: [f32; 3]) -> [f32; 3] {
    let c = c.map(|x| x.max(0.0003));
    let length = c.iter().map(|x| x * x).sum::<f32>().sqrt();
    c.map(|x| x / length * 1.06)
}

/// A blend's hue `h` brought to the levels: `max(h × floor × 1.732 / |h| / luminance(h), h × lerp(floor, ceiling,
/// saturate(top^exponent)))`.
fn blend_toned(h: [f32; 3], top: f32) -> [f32; 3] {
    let [floor, exponent, ceiling] = OVERLAY_LEVELS;
    let length = h.iter().map(|x| x * x).sum::<f32>().sqrt();
    let dim = floor * 1.732 / length / luminance(h);
    let level = lerp(floor, ceiling, top.powf(exponent).clamp(0.0, 1.0));
    h.map(|x| (x * dim).max(x * level))
}

/// From the floor grey to `target` as luminance `lum` rises past the floor: `lerp(floor, target,
/// smoothstep(0.0003, floor, lum)^0.5)`.
fn blend_settled(target: [f32; 3], lum: f32) -> [f32; 3] {
    let floor = OVERLAY_LEVELS[0];
    let t = smoothstep(0.0003, floor, lum).sqrt();
    target.map(|x| lerp(floor, x, t))
}

/// The overlay's colour `o` blended over the paint's `c` by `alpha` (`F_OVERLAY_BLEND_MODE`): normal and layer (0,
/// 4) mix it in; colour (1) takes the hue of `o × c` at `c`'s brightness, multiply (2) `o × c`'s own, add (3)
/// `o + c` lifted towards the ceiling — each brought to `g_vAlbedoLevels`.
fn blend(c: [f32; 3], o: [f32; 3], alpha: f32, mode: i64) -> [f32; 3] {
    let largest = |c: [f32; 3]| c[0].max(c[1]).max(c[2]);
    let target = match mode {
        0 | 4 => o,
        1 => blend_settled(
            blend_toned(blend_hue(mul3(o, c)), largest(c)),
            luminance(clamp3(c)),
        ),
        2 => {
            let m = mul3(c, o);
            blend_settled(blend_toned(blend_hue(m), largest(m)), luminance(clamp3(m)))
        }
        3 => {
            let s: [f32; 3] = std::array::from_fn(|i| c[i] + o[i]);
            let lum = luminance(clamp3(scale3(s, 2.0)));
            let toned = blend_toned(blend_hue(s), largest(s) * 2.0);
            let ceiling = OVERLAY_LEVELS[2];
            let lifted = toned.map(|x| lerp(x, ceiling.min(x + lum * 2.0), 1.0 / 63.0));
            blend_settled(lifted, lum)
        }
        _ => return c,
    };
    lerp3(c, target, alpha)
}

/// The share of an overlay a paint colour's region gets (`F_OVERLAY_MASK` 1 – 5: colour 0's, 1's, 2's, 3's, any but
/// 0's), from the weights that pick the colours (paint masks or pattern channels); 1 for the other modes.
fn overlay_region(mode: i64, [r, g, b]: [f32; 3]) -> f32 {
    match mode {
        1 => 1.0 - r.max(g).max(b),
        2 => r * (1.0 - g.max(b)),
        3 => g * (1.0 - b),
        4 => b,
        5 => r.max(g).max(b),
        _ => 1.0,
    }
}

/// Where an overlay sits: the weapon's UVs, the pattern's (`g_bOverlayUsesPatternUVs`), or its own placement
/// (`g_bOverlayUsesUniqueUVs`).
enum OverlayAt {
    Weapon,
    Pattern,
    Own(Placement),
}

/// An overlay over the paint (`F_OVERLAY_TEXTURE`): a decal or second layer with its own wear.
struct Overlay {
    /// `g_tOverlay`, read as sRGB.
    image: Sampler,
    /// `g_tOverlayMask` R, for mask mode 8.
    mask: Option<Sampler>,
    at: OverlayAt,
    /// `F_OVERLAY_MASK`: 0 its own layer (its wear protects the paint), 1 – 5 a paint colour's region, 6 the paint,
    /// 7 the pattern, 8 its own mask; 6 – 8 wear with the paint.
    mask_mode: i64,
    /// `F_OVERLAY_BLEND_MODE` ([`blend`]); layer (4) also fades as its own wear takes it.
    blend: i64,
    strength: f32,
    brightness: f32,
    /// `1 − g_fOverlayDurability`, as its expression makes it.
    durability: f32,
    /// How much of the paint's roughness and metalness the overlay replaces.
    material_strength: f32,
    roughness: f32,
    metalness: f32,
}

/// The overlay at a texel: colour (linear, × brightness) and alpha, masked, before the strength.
#[derive(Debug, Clone, Copy)]
struct OverlayTexel {
    color: [f32; 3],
    alpha: f32,
}

/// What the overlay does at a texel once the paint's wear is known.
#[derive(Debug, Clone, Copy, Default)]
struct Cover {
    /// Where the paint is gone, the overlay's own wear counted (mask mode 0: it protects what it covers).
    gone: f32,
    /// What is left of the overlay against its own wear (`1 −` its gone).
    kept: f32,
    color: [f32; 3],
    /// Its alpha to blend with, and its share of the roughness and metalness.
    alpha: f32,
    share: f32,
    mode: i64,
    roughness: f32,
    metalness: f32,
}

impl Cover {
    fn blend(&self, c: [f32; 3]) -> [f32; 3] {
        blend(c, self.color, self.alpha, self.mode)
    }

    fn roughness(&self, r: f32) -> f32 {
        lerp(r, self.roughness, self.share)
    }

    fn metalness(&self, m: f32) -> f32 {
        lerp(m, self.metalness, self.share)
    }
}

/// The halftone of spray paint and anodized airbrushed (`F_SPRAYPAINT_HALFTONE`): the projected pattern darkened
/// by the cavity and occlusion, then thresholded through three more projections' dots.
struct Halftone {
    /// `g_bHalftoneInCavity` and `g_fHalftoneCavityCutoff`.
    in_cavity: bool,
    cutoff: f32,
    /// `g_vHalftonePatternLevels` as its expression makes it: low edge, exponent, high edge.
    levels: [f32; 3],
    /// `g_vHalftoneThresholds`.
    thresholds: [f32; 2],
    /// The dots' placements: the pattern's, turned by `g_flHalftoneRotationOffsetA`, `B`, `C`.
    dots: [Placement; 3],
}

/// Case hardening (`F_CASE_HARDENING`): a colour ramp (`g_tCaseHardeningColorRamp`) read across by the weapon's
/// shape and the pattern's R, down by the pattern's G.
struct CaseHardening {
    /// The ramp, read as sRGB, clamped.
    ramp: Sampler,
    /// `g_tNormal`: its flat parts count as less shaped.
    normal: Option<Sampler>,
    /// `g_flCaseHardeningPatternInfluence`, `GeometricInfluence`, `RampOffset`.
    pattern: f32,
    geometric: f32,
    offset: f32,
    /// `g_vCaseHardening`: how much the ramp replaces each paint colour (solid).
    weights: [f32; 4],
}

/// The paint styles (`F_PAINT_STYLE`).
const SOLID: i64 = 0;
const HYDROGRAPHIC: i64 = 1;
const SPRAY: i64 = 2;
const ANODIZED: i64 = 3;
const ANODIZED_MULTI: i64 = 4;
const AIRBRUSHED: i64 = 5;
const CUSTOM: i64 = 6;
const PATINA: i64 = 7;
const GUNSMITH: i64 = 8;

/// What the bake reads from a job's parameters.
struct Inputs {
    style: i64,
    /// `F_SEPARATE_CHANNEL_INPUTS`: CS2's own composite inputs.
    separate: bool,
    pattern: Option<Sampler>,
    /// The pattern is a colour (custom paint jobs, patina, gunsmith), read as sRGB; the others hold masks.
    pattern_srgb: bool,
    pattern_address: [Address; 2],
    position: Option<ImageF32>,
    surface: Option<Sampler>,
    base_color: Option<Sampler>,
    base_surface: Option<Sampler>,
    occlusion: Option<Sampler>,
    /// The occlusion's alpha holds a no-paint mask (some texel below 250).
    masked: bool,
    masks: Option<Sampler>,
    wear_texture: Option<Sampler>,
    grunge: Option<Sampler>,
    paint_roughness: Option<Sampler>,
    paint_metalness: Option<Sampler>,
    roughness_on_pattern: bool,
    metalness_on_pattern: bool,
    pattern_at: Placement,
    projection_at: Placement,
    wear_at: Placement,
    grunge_at: Placement,
    wear: f32,
    brightness: f32,
    /// The paint colours, linear.
    colors: [[f32; 3]; 4],
    /// Per colour: durability (`1 − g_vPaintDurability`), metalness (`g_vPaintMetalness`).
    durability: [f32; 4],
    metalness: [f32; 4],
    /// `g_flPaintRoughness`, or per colour with `F_ROUGHNESS_PER_COLOR`.
    roughness: f32,
    roughness_per_color: Option<[f32; 4]>,
    /// `g_flPaintMetalness` (custom paint jobs, gunsmith).
    metalness_flat: f32,
    softness: f32,
    /// `g_nColorAdjustmentMode` 1: the brightness applies to all of a patina or gunsmith pattern, not only its
    /// metal.
    adjust_all: bool,
    bias_spray: bool,
    spray_blend: [f32; 2],
    overlay: Option<Overlay>,
    halftone: Option<Halftone>,
    /// `F_USE_ALL_MASKS`: spray paint's colours 2 and 3 also by the paint masks' G and B.
    all_masks: bool,
    case_hardening: Option<CaseHardening>,
}

/// What every style reads at a texel of the weapon's layout.
struct Texel {
    /// The cavity as the style uses it (sRGB-decoded; `0.96 R^1.5` with separate channels), and `R^1.5`.
    cavity: f32,
    cavity15: f32,
    /// Ambient occlusion (G, sRGB-decoded).
    occlusion: f32,
    no_paint: f32,
    masks: [f32; 4],
    /// The wear texture.
    wear: f32,
    /// The grunge texture: RGB linear, A as stored.
    grunge: [f32; 4],
    /// The weapon's own colour (linear) and surface (roughness, metalness).
    weapon: [f32; 3],
    surface: [f32; 2],
}

/// A texel painted: colour (linear), roughness, metalness.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shade {
    color: [f32; 3],
    roughness: f32,
    metalness: f32,
}

impl Inputs {
    fn read(content: &Content, params: &BTreeMap<String, Param>) -> Result<Self, ExportError> {
        let float = |name: &str, default: f32| match params.get(name) {
            Some(Param::Floats(v)) => v[0],
            Some(Param::Int(i)) => *i as f32,
            _ => default,
        };
        let floats = |name: &str, default: [f32; 4]| match params.get(name) {
            Some(Param::Floats(v)) => *v,
            _ => default,
        };
        let int = |name: &str, default: i64| match params.get(name) {
            Some(Param::Int(i)) => *i,
            _ => default,
        };
        // An input that cannot be read is left out (no mask, no wear, no grunge…); only a pattern or position the
        // style uses is needed ([`unpainted_reason`] checked that they are named).
        let texture = |name: &str| -> Result<Option<Sampler>, ExportError> {
            match params.get(name) {
                Some(Param::Texture(path)) if name == "g_tPattern" => {
                    Sampler::load(content, path).map(Some)
                }
                Some(Param::Texture(path)) => Ok(Sampler::load(content, path).ok()),
                _ => Ok(None),
            }
        };
        let style = int("F_PAINT_STYLE", SOLID);
        let ignore_size = int("g_bIgnoreWeaponSizeScale", 0) != 0;
        let size_scale = if ignore_size {
            1.0
        } else {
            float("g_flUvScale1", 1.0)
        };
        let placement = |prefix: &str, size: f32| {
            let offset = floats(&format!("g_v{prefix}TexCoordOffset"), [0.0; 4]);
            Placement::new(
                float(&format!("g_fl{prefix}TexCoordScale"), 1.0) * size,
                float(&format!("g_fl{prefix}TexCoordRotation"), 0.0),
                [offset[0], offset[1]],
            )
        };
        let projected = matches!(style, SPRAY | AIRBRUSHED);
        let position = match params.get("g_tPosition") {
            Some(Param::Texture(path)) if projected => load_f32(content, path)?,
            _ => None,
        };
        // A texture without alpha decodes to 255 there: no no-paint mask, not paint nowhere.
        let occlusion = texture("g_tAmbientOcclusion")?;
        let masked = occlusion
            .as_ref()
            .is_some_and(|s| s.image.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 250));
        // The declarations' defaults: colours 0.5, 0.59, 0.38, 0.42 grey; roughness 0.6 (0.2 for anodized
        // multicolour); metalness and durability 0.
        let colors = [
            ("g_vColor0", 0.5),
            ("g_vColor1", 0.59),
            ("g_vColor2", 0.38),
            ("g_vColor3", 0.42),
        ]
        .map(|(name, grey)| {
            let c = floats(name, [grey; 4]);
            [c[0], c[1], c[2]].map(srgb_to_linear)
        });
        let spray_blend = floats("g_vSprayBlend", [1.0; 4]);
        let pattern_at = placement("Pattern", size_scale);
        let projection_scale = if ignore_size {
            1.0
        } else {
            float("g_flWeaponLength1", 36.0) / 36.0
        };
        let overlay = match params.get("g_tOverlay") {
            Some(Param::Texture(path)) if int("F_OVERLAY_TEXTURE", 0) != 0 => Some(Overlay {
                image: Sampler::load(content, path)?,
                mask: texture("g_tOverlayMask")?,
                at: match (
                    int("g_bOverlayUsesPatternUVs", 0) != 0,
                    int("g_bOverlayUsesUniqueUVs", 0) != 0,
                ) {
                    (_, true) => OverlayAt::Own(placement("Overlay", size_scale)),
                    (true, false) => OverlayAt::Pattern,
                    (false, false) => OverlayAt::Weapon,
                },
                mask_mode: int("F_OVERLAY_MASK", 0),
                blend: int("F_OVERLAY_BLEND_MODE", 0),
                strength: float("g_fOverlayStrength", 1.0),
                brightness: float("g_fOverlayBrightness", 1.0),
                durability: 1.0 - float("g_fOverlayDurability", 0.0),
                material_strength: float("g_fOverlayMaterialStrength", 1.0),
                roughness: float("g_fOverlayRoughness", 0.0),
                metalness: float("g_fOverlayMetalness", 0.0),
            }),
            _ => None,
        };
        let halftone = (int("F_SPRAYPAINT_HALFTONE", 0) != 0 && projected).then(|| {
            let [x, y, z, _] = floats("g_vHalftonePatternLevels", [0.0, 0.5, 0.0, 0.0]);
            let [low, high, _, _] = floats("g_vHalftoneThresholds", [0.0; 4]);
            let offset = floats("g_vPatternTexCoordOffset", [0.0; 4]);
            let dot = |name: &str, default: f32| {
                Placement::new(
                    float("g_flPatternTexCoordScale", 1.0) * projection_scale,
                    float(name, default) + float("g_flPatternTexCoordRotation", 0.0),
                    [offset[0], offset[1]],
                )
            };
            Halftone {
                in_cavity: int("g_bHalftoneInCavity", 1) != 0,
                cutoff: float("g_fHalftoneCavityCutoff", 1.0),
                levels: [-x, -(1.0 - y).max(0.0001).log2(), 2.0 - z],
                thresholds: [low, high],
                dots: [
                    dot("g_flHalftoneRotationOffsetA", 0.0),
                    dot("g_flHalftoneRotationOffsetB", 4.0),
                    dot("g_flHalftoneRotationOffsetC", -4.0),
                ],
            }
        });
        let case_hardening = match params.get("g_tCaseHardeningColorRamp") {
            Some(Param::Texture(path)) if int("F_CASE_HARDENING", 0) != 0 => Some(CaseHardening {
                ramp: Sampler::load(content, path)?,
                normal: texture("g_tNormal")?,
                pattern: float("g_flCaseHardeningPatternInfluence", 0.5),
                geometric: float("g_flCaseHardeningGeometricInfluence", 1.0),
                offset: float("g_flCaseHardeningRampOffset", 0.0),
                weights: floats("g_vCaseHardening", [0.0; 4]),
            }),
            _ => None,
        };
        Ok(Self {
            style,
            separate: int("F_SEPARATE_CHANNEL_INPUTS", 0) != 0,
            pattern: if (style == SOLID && case_hardening.is_none()) || style == ANODIZED {
                None
            } else {
                texture("g_tPattern")?
            },
            pattern_srgb: matches!(style, CUSTOM | PATINA | GUNSMITH),
            pattern_address: [
                Address::of(int("g_nPatternTextureHorizontalSampling", 0)),
                Address::of(int("g_nPatternTextureVerticalSampling", 0)),
            ],
            position,
            surface: if projected {
                texture("g_tSurface")?
            } else {
                None
            },
            base_color: texture("g_tColor")?,
            base_surface: texture("g_tMetalness")?,
            occlusion,
            masked,
            masks: texture("g_tMasks")?,
            wear_texture: texture("g_tWear")?,
            grunge: texture("g_tGrunge")?,
            paint_roughness: if int("F_ROUGHNESS_TEXTURE", 0) != 0 {
                texture("g_tPaintRoughness")?
            } else {
                None
            },
            paint_metalness: if int("F_METALNESS_TEXTURE", 0) != 0 {
                texture("g_tPaintMetalness")?
            } else {
                None
            },
            roughness_on_pattern: int("g_bRoughnessUsesPatternUVs", 1) != 0,
            metalness_on_pattern: int("g_bMetalnessUsesPatternUVs", 0) != 0,
            pattern_at,
            projection_at: placement("Pattern", projection_scale),
            wear_at: placement("Wear", size_scale),
            grunge_at: placement("Grunge", size_scale),
            wear: float("g_flWearAmount", 0.0).clamp(0.0, 1.0),
            brightness: float("g_flColorBrightness", 1.0),
            colors,
            durability: floats("g_vPaintDurability", [0.0; 4]).map(|d| 1.0 - d),
            metalness: floats("g_vPaintMetalness", [0.0; 4]),
            roughness: float(
                "g_flPaintRoughness",
                if style == ANODIZED_MULTI { 0.2 } else { 0.6 },
            ),
            roughness_per_color: (int("F_ROUGHNESS_PER_COLOR", 0) != 0)
                .then(|| floats("g_vPaintRoughness", [0.6; 4])),
            metalness_flat: float("g_flPaintMetalness", 0.0),
            softness: float("g_fWearSoftness", 0.0),
            adjust_all: int("g_nColorAdjustmentMode", 1) != 0,
            bias_spray: int("g_bBiasSpray", 0) != 0,
            spray_blend: [spray_blend[0], spray_blend[1]],
            overlay,
            halftone,
            all_masks: int("F_USE_ALL_MASKS", 0) != 0,
            case_hardening,
        })
    }

    /// The output size: the larger of the weapon colour's and the pattern's (the weapon's UV layout keeps its detail
    /// under a small tiling pattern), at most [`MAX_SIZE`].
    fn size(&self) -> (usize, usize) {
        let sizes = [&self.base_color, &self.pattern].map(|s| {
            s.as_ref()
                .map_or((1, 1), |s| (s.image.width, s.image.height))
        });
        let (width, height) = sizes
            .into_iter()
            .max_by_key(|(w, h)| w * h)
            .unwrap_or((1, 1));
        (width.min(MAX_SIZE), height.min(MAX_SIZE))
    }

    /// What every style reads at (u, v); without a texture, the declarations' defaults (cavity 0.5, occlusion 1,
    /// masks 1, white colour, roughness 0.5).
    fn texel(&self, u: f32, v: f32) -> Texel {
        let decode = |c: f32| SRGB_TO_LINEAR[(c * 255.0).round() as usize];
        let (raw_cavity, occlusion, no_paint) = match &self.occlusion {
            Some(s) => {
                let o = s.at_with(u, v, [Address::Clamp; 2]);
                let no_paint = if self.masked { o[3] } else { 0.0 };
                (decode(o[0]), decode(o[1]), no_paint)
            }
            None => (srgb_to_linear(0.5), 1.0, 0.0),
        };
        let cavity15 = raw_cavity.powf(1.5);
        let grunge = self.grunge.as_ref().map_or([1.0; 4], |s| {
            let (gu, gv) = self.grunge_at.apply(u, v);
            let g = s.at(gu, gv);
            [decode(g[0]), decode(g[1]), decode(g[2]), g[3]]
        });
        let clamp = |s: &Option<Sampler>| s.as_ref().map(|s| s.at_with(u, v, [Address::Clamp; 2]));
        Texel {
            cavity: if self.separate {
                cavity15 * 0.96
            } else {
                raw_cavity
            },
            cavity15,
            occlusion,
            no_paint,
            masks: clamp(&self.masks).unwrap_or([1.0; 4]),
            wear: self.wear_texture.as_ref().map_or(0.0, |s| {
                let (wu, wv) = self.wear_at.apply(u, v);
                s.at(wu, wv)[0]
            }),
            grunge,
            weapon: clamp(&self.base_color).map_or([1.0; 3], |c| rgb(c).map(decode)),
            surface: clamp(&self.base_surface).map_or([0.5, 0.0], |s| [s[0], s[1]]),
        }
    }

    /// The pattern at the texel's pattern placement (sRGB-decoded for the colour patterns).
    fn pattern_at(&self, u: f32, v: f32) -> [f32; 4] {
        let (pu, pv) = self.pattern_at.apply(u, v);
        self.pattern_sample(pu, pv)
    }

    fn pattern_sample(&self, u: f32, v: f32) -> [f32; 4] {
        let Some(pattern) = &self.pattern else {
            return [1.0; 4];
        };
        let p = pattern.at_with(u, v, self.pattern_address);
        if self.pattern_srgb {
            [
                srgb_to_linear(p[0]),
                srgb_to_linear(p[1]),
                srgb_to_linear(p[2]),
                p[3],
            ]
        } else {
            p
        }
    }

    /// The pattern projected from the weapon's 3D position (spray paint, anodized airbrushed): the position texture
    /// averaged over 17 taps, × 2, the pattern placed on its (y, z), (x, z) and (y, x) planes, blended towards the
    /// second and third by the surface normal's |y|⁷ and |z|⁷ (× `g_vSprayBlend`). With separate channels and
    /// `g_bBiasSpray`, the normal texture is read through an sRGB encode first.
    ///
    /// With the halftone (z mirrored): the colour channels × a level of cavity × occlusion × 2 (`smoothstep` of its
    /// power; inverted when the thresholds fall, the other way out of the cavity), × the alpha of three more
    /// projections turned by the dots' rotations, then thresholded.
    fn pattern_projected(&self, t: &Texel, u: f32, v: f32) -> [f32; 4] {
        let Some(position) = &self.position else {
            return self.pattern_at(u, v);
        };
        let mut sum = [0.0f32; 4];
        for [du, dv] in POSITION_TAPS {
            let p = position.sample_bilinear(u + du * 0.2, v + dv * 0.2);
            for c in 0..4 {
                sum[c] += p[c] / 17.0;
            }
        }
        let [x, y, z, _] = sum.map(|c| c * 2.0);
        let z = if self.halftone.is_some() { -z } else { z };
        let stored = self.surface.as_ref().map_or([0.5, 0.5, 1.0], |s| {
            rgb(s.at_with(u, v, [Address::Clamp; 2]))
        });
        let stored = if self.separate && self.bias_spray {
            stored.map(|c| f32::from(linear_to_srgb(c)) / 255.0)
        } else {
            stored
        };
        let n = stored.map(|c| c * 2.0 - 1.0);
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        let n = n.map(|c| c / length);
        let to_xz = self.spray_blend[0] * n[1].abs().powi(7);
        let to_yx = self.spray_blend[1] * n[2].abs().powi(7);
        let project = |placement: &Placement| -> [f32; 4] {
            let at = |a: f32, b: f32| {
                let (pu, pv) = placement.apply(a, b);
                self.pattern_sample(pu, pv)
            };
            let (yz, xz, yx) = (at(y, z), at(x, z), at(y, x));
            std::array::from_fn(|c| lerp(lerp(yz[c], xz[c], to_xz), yx[c], to_yx))
        };
        let p = project(&self.projection_at);
        let Some(h) = &self.halftone else {
            return p;
        };
        let cavity = if h.in_cavity {
            t.cavity.min(h.cutoff)
        } else {
            t.cavity
        };
        let level = smoothstep(
            h.levels[0],
            h.levels[2],
            (cavity * t.occlusion * 2.0).powf(h.levels[1]),
        );
        let factor = if (h.thresholds[0] > h.thresholds[1]) == h.in_cavity {
            level
        } else {
            1.0 - level
        };
        let dots = h.dots.each_ref().map(|at| project(at)[3]);
        let [low, high] = h.thresholds;
        [
            smoothstep(low, high, p[0] * factor * dots[0]),
            smoothstep(low, high, p[1] * factor * dots[1]),
            smoothstep(low, high, p[2] * factor * dots[2]),
            p[3],
        ]
    }

    /// The overlay at (u, v), its alpha × `region` of its mask mode (`g_tOverlayMask` for mode 8); `None` without
    /// one.
    fn overlay_texel(
        &self,
        u: f32,
        v: f32,
        region: impl FnOnce(i64) -> f32,
    ) -> Option<OverlayTexel> {
        let overlay = self.overlay.as_ref()?;
        let (ou, ov) = match &overlay.at {
            OverlayAt::Weapon => (u, v),
            OverlayAt::Pattern => self.pattern_at.apply(u, v),
            OverlayAt::Own(placement) => placement.apply(u, v),
        };
        let s = overlay.image.at(ou, ov);
        let mask = match overlay.mask_mode {
            8 => overlay.mask.as_ref().map_or(1.0, |m| m.at(u, v)[0]),
            mode => region(mode),
        };
        Some(OverlayTexel {
            color: rgb(s).map(|c| srgb_to_linear(c) * overlay.brightness),
            alpha: s[3] * mask,
        })
    }

    /// How the overlay's durability scales the paint's wear under it: `lerp(1, durability, alpha)`.
    fn overlay_durability(&self, texel: Option<OverlayTexel>) -> f32 {
        match (&self.overlay, texel) {
            (Some(overlay), Some(texel)) => lerp(1.0, overlay.durability, texel.alpha),
            _ => 1.0,
        }
    }

    /// The overlay against its own wear (the wear texture × cavity × growth × its durability, past the paint's
    /// threshold with softness `k`), where the paint `gone` is: mask mode 0 keeps the paint it covers
    /// (`lerp(gone, min(gone, its gone), strength)`); the layer blend fades with its wear; its share of roughness and
    /// metalness is what is left × alpha × `g_fOverlayMaterialStrength`.
    fn cover(&self, texel: Option<OverlayTexel>, t: &Texel, k: f32, gone: f32) -> Cover {
        let (Some(overlay), Some(texel)) = (&self.overlay, texel) else {
            return Cover {
                gone,
                ..Cover::default()
            };
        };
        let strength = texel.alpha * overlay.strength;
        let own = self.gone(self.worn(t) * self.growth() * overlay.durability, k);
        let alpha = if overlay.blend == 4 {
            strength * (1.0 - own).clamp(0.0, 1.0)
        } else {
            strength
        };
        Cover {
            gone: if overlay.mask_mode == 0 {
                lerp(gone, gone.min(own), strength).max(0.0)
            } else {
                gone
            },
            kept: 1.0 - own,
            color: texel.color,
            alpha,
            share: (1.0 - own) * alpha * overlay.material_strength,
            mode: overlay.blend,
            roughness: overlay.roughness,
            metalness: overlay.metalness,
        }
    }

    /// The case-hardening ramp at (u, v) (sRGB-decoded; `None` without it): across by the shape — `lerp(0.5,
    /// cavity^0.85, G) × lerp(1, occlusion × 2 × flatness of g_tNormal, G)`, 2× it or itself + the pattern's R by
    /// `P` — and down by `max(pattern G × P, (1 − occlusion) × 0.2 × G) + offset`; averaged over five pattern taps
    /// (±1/2048) when `taps`.
    fn ramp(&self, t: &Texel, u: f32, v: f32, taps: bool) -> Option<[f32; 4]> {
        let ch = self.case_hardening.as_ref()?;
        let n = ch.normal.as_ref().map_or([0.5; 4], |s| s.at(u, v));
        let flat = |c: f32| smoothstep(0.85, 0.2, c.min(1.0 - c));
        let shape = lerp(0.5, t.cavity.powf(0.85), ch.geometric)
            * lerp(
                1.0,
                t.occlusion * flat(n[0]).min(flat(n[1])) * 2.0,
                ch.geometric,
            );
        let (pu, pv) = self.pattern_at.apply(u, v);
        let p = self.pattern_sample(pu, pv);
        let row = (p[1] * ch.pattern).max((1.0 - t.occlusion) * 0.2 * ch.geometric) + ch.offset;
        let at = |x: f32| {
            let s = ch.ramp.at_with(
                lerp(shape * 2.0, shape + x, ch.pattern),
                row,
                [Address::Clamp; 2],
            );
            [
                srgb_to_linear(s[0]),
                srgb_to_linear(s[1]),
                srgb_to_linear(s[2]),
                s[3],
            ]
        };
        if !taps {
            return Some(at(p[0]));
        }
        const D: f32 = 1.0 / 2048.0;
        let mut sum = at(p[0]);
        for (du, dv) in [(D, D), (-D, -D), (-D, D), (D, -D)] {
            let s = at(self.pattern_sample(pu + du, pv + dv)[0]);
            for c in 0..4 {
                sum[c] += s[c];
            }
        }
        Some(sum.map(|c| c * 0.2))
    }

    /// The paint's roughness at (u, v): its texture where it has one, else `per_color` of the per-colour values,
    /// else `g_flPaintRoughness`.
    fn paint_roughness(&self, u: f32, v: f32, per_color: impl FnOnce([f32; 4]) -> f32) -> f32 {
        match (&self.paint_roughness, self.roughness_per_color) {
            (Some(s), _) => self.paint_texture(s, self.roughness_on_pattern, u, v),
            (None, Some(values)) => per_color(values),
            (None, None) => self.roughness,
        }
    }

    /// The paint's metalness at (u, v): its texture where it has one, else `otherwise`.
    fn paint_metalness(&self, u: f32, v: f32, otherwise: f32) -> f32 {
        match &self.paint_metalness {
            Some(s) => self.paint_texture(s, self.metalness_on_pattern, u, v),
            None => otherwise,
        }
    }

    fn paint_texture(&self, s: &Sampler, on_pattern: bool, u: f32, v: f32) -> f32 {
        let (pu, pv) = if on_pattern {
            self.pattern_at.apply(u, v)
        } else {
            (u, v)
        };
        s.at(pu, pv)[0]
    }

    /// The wear texture against the cavity: `wear × cavity`, with separate channels through a curve that sharpens
    /// with wear (spray paint its own).
    fn worn(&self, t: &Texel) -> f32 {
        let (c, w) = (t.cavity, self.wear);
        let curve = if !self.separate {
            c
        } else if self.style == SPRAY {
            lerp(
                smoothstep(0.0, 0.68, c.powf(1.1)),
                smoothstep(0.0, 0.51, c.powf(0.8)),
                w.powf(0.3),
            )
        } else {
            lerp(
                smoothstep(0.0, 0.72, c.powf(1.3)),
                smoothstep(0.0, 0.4, c),
                w.powf(1.2),
            )
        };
        t.wear * curve
    }

    /// How the wear grows the paint's loss: `6w + 1`.
    fn growth(&self) -> f32 {
        self.wear * 6.0 + 1.0
    }

    /// Where the paint is gone, softness `k` either side: 0.58 – 0.68, 0.56 – 0.74 with separate channels (gunsmith
    /// keeps the first).
    fn gone(&self, worn: f32, k: f32) -> f32 {
        let (low, high) = if self.separate && self.style != GUNSMITH {
            (0.56, 0.74)
        } else {
            (0.58, 0.68)
        };
        smoothstep(low - k, high + k, worn)
    }

    /// The edge where paint starts to go (hydrographic's brightness, the anodized ones' steel): as soon as any is
    /// gone, or 0.53 – 0.72 with separate channels.
    fn edge(&self, gone: f32, worn: f32, k: f32) -> f32 {
        if self.separate {
            smoothstep(0.53 - k, 0.72 + k, worn)
        } else {
            smoothstep(0.0, 0.01, gone)
        }
    }

    /// The grunge multiplying the paint: `lerp(1, grunge, (1 − cavity)⁴ / 4 + 3w / 4)`.
    fn grime(&self, t: &Texel) -> [f32; 4] {
        let amount = (1.0 - t.cavity).powi(4) * 0.25 + 0.75 * self.wear;
        t.grunge.map(|g| lerp(1.0, g, amount))
    }

    /// The common end of the styles that paint over the weapon: roughness (plus the grunge's), metalness, the
    /// weapon's own where the paint is `gone`.
    fn painted_surface(
        &self,
        t: &Texel,
        grime: [f32; 4],
        roughness: f32,
        metal: f32,
        gone: f32,
    ) -> (f32, f32) {
        let rough = (roughness + (1.0 - grime[3]) * self.wear * self.wear * 0.5).min(1.0);
        (
            lerp(rough, t.surface[0], gone),
            lerp(metal, t.surface[1], gone),
        )
    }

    /// The texel at (u, v) as the paint style draws it (see the [module docs](self)).
    fn shade(&self, u: f32, v: f32) -> Shade {
        let t = self.texel(u, v);
        let shade = match self.style {
            HYDROGRAPHIC | SPRAY => self.layered(&t, u, v),
            ANODIZED | ANODIZED_MULTI | AIRBRUSHED => self.anodized(&t, u, v),
            CUSTOM => self.custom(&t, u, v),
            PATINA | GUNSMITH => self.metal(&t, u, v),
            _ => self.solid(&t, u, v),
        };
        Shade {
            color: clamp3(shade.color),
            roughness: shade.roughness.clamp(0.0, 1.0),
            metalness: shade.metalness.clamp(0.0, 1.0),
        }
    }

    /// Solid (0): colours by paint mask; with case hardening each colour takes the ramp by its `g_vCaseHardening`
    /// share. An overlay's regions are the masks'.
    fn solid(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let masks = rgb(t.masks);
        let over = self.overlay_texel(u, v, |mode| overlay_region(mode, masks));
        let d = pick(self.durability, masks);
        let k = self.softness * d;
        let worn = (t.no_paint + self.worn(t)) * self.growth() * self.overlay_durability(over) * d;
        let cover = self.cover(over, t, k, self.gone(worn, k));
        let grime = self.grime(t);
        let metal = self.paint_metalness(u, v, pick(self.metalness, masks));
        let colors = match (self.ramp(t, u, v, true), &self.case_hardening) {
            (Some(ramp), Some(ch)) => {
                std::array::from_fn(|i| lerp3(self.colors[i], rgb(ramp), ch.weights[i]))
            }
            _ => self.colors,
        };
        let base = std::array::from_fn(|c| pick(channel(&colors, c), masks));
        let blended = cover.blend(base);
        let painted = mul3(blended, rgb(grime));
        let roughness = cover.roughness(self.paint_roughness(u, v, |r| pick(r, masks)));
        let (roughness, metalness) =
            self.painted_surface(t, grime, roughness, cover.metalness(metal), cover.gone);
        Shade {
            color: lerp3(
                albedo_levels(blended, painted, metal, self.wear),
                t.weapon,
                cover.gone,
            ),
            roughness,
            metalness,
        }
    }

    /// Hydrographic and spray paint (1, 2): the pattern's channels pick the colours, and the wear peels them off
    /// layer by layer (R, then G, then B) before the paint goes.
    ///
    /// Spray paint with `F_USE_ALL_MASKS` also takes colours 2 and 3 by the paint masks, as hydrographic does. An
    /// overlay's regions are the pattern's channels (hydrographic's with the masks over them); its durability scales
    /// the wear (hydrographic: the reach), and the paint is mixed towards the weapon where gone before it is laid on.
    fn layered(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let w = self.wear;
        let spray = self.style == SPRAY;
        let masked = !spray || self.all_masks;
        let p = if spray {
            self.pattern_projected(t, u, v)
        } else {
            self.pattern_at(u, v)
        };
        let cut = cutout(p[3]);
        let [mg, mb] = [t.masks[1], t.masks[2]];
        let over = self.overlay_texel(u, v, |mode| match (spray, mode) {
            (true, _) => overlay_region(mode, rgb(p)),
            (false, 1) => (1.0 - p[0].max(p[1]).max(p[2])) * (1.0 - mg.max(mb)),
            (false, 2) => p[0] * (1.0 - p[1].max(p[2])) * (1.0 - mg.max(mb)),
            (false, 3) => (p[1] * (1.0 - p[2])).max(mg) * (1.0 - mb),
            (false, 4) => (p[2] * (1.0 - mg)).max(mb),
            (false, 5) => p[0].max(p[1]).max(p[2]).max(mg).max(mb),
            (false, 7) => 1.0 - mg.max(mb),
            _ => 1.0,
        });
        let held = self.overlay_durability(over);
        let d = if masked {
            pick_masked(self.durability, rgb(p), t.masks)
        } else {
            pick(self.durability, rgb(p))
        };
        // Hydrographic paints only where the masks G and B or the pattern's alpha are.
        let worn = if spray {
            (t.no_paint + self.worn(t)) * self.growth() * held * d
        } else {
            let masks = (mg + mb).clamp(0.0, 1.0);
            let alpha = smoothstep(0.0, 0.5, p[3]);
            let reach = match self.overlay.as_ref().map(|o| o.mask_mode) {
                Some(mode) if mode > 0 => masks.max(alpha * held),
                _ => masks.max(alpha) * held,
            };
            ((t.no_paint + self.worn(t)) * self.growth() + cut) * reach * d
        };
        let k = self.softness * d;
        let (first, second, third) = (0.56 - 0.06 * w, 0.54 - 0.12 * w, 0.52 - 0.18 * w);
        let layers = [
            p[0] * smoothstep(0.58 + k, first - k, worn),
            p[1] * smoothstep(first + k, second - k, worn),
            p[2] * smoothstep(second + k, third - k, worn),
        ];
        let gone_paint = self.gone(worn, k);
        let cover = self.cover(over, t, k, gone_paint);
        let gone = cover.gone;
        let grime = self.grime(t);
        let by_layer = |values: [f32; 4]| {
            if masked {
                pick_masked(values, layers, t.masks)
            } else {
                pick(values, layers)
            }
        };
        let metal = self.paint_metalness(u, v, by_layer(self.metalness));
        let base: [f32; 3] = std::array::from_fn(|c| by_layer(channel(&self.colors, c)));
        let base = if self.overlay.is_some() {
            cover.blend(lerp3(base, t.weapon, gone))
        } else {
            base
        };
        let edge = self.edge(gone_paint, worn, k) * (1.0 - cut);
        let color = clamp3(scale3(base, lerp(self.brightness, 1.0, edge)));
        let painted = mul3(color, rgb(grime));
        let roughness = cover.roughness(self.paint_roughness(u, v, by_layer));
        let (roughness, metalness) =
            self.painted_surface(t, grime, roughness, cover.metalness(metal), gone);
        Shade {
            color: lerp3(albedo_levels(color, painted, metal, w), t.weapon, gone),
            roughness,
            metalness,
        }
    }

    /// The anodized styles (3, 4, 5): the bare metal where the paint masks' R is, tinted by `g_vColor0` (3), by the
    /// colours a pattern picks (4: Dopplers) or a projected one picks (5: Fades), steel at the edge of the wear.
    fn anodized(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let m = t.masks[0];
        let (tint, cut, d, alpha) = match self.style {
            ANODIZED => (self.colors[0], 0.0, 1.0, 1.0),
            style => {
                let p = if style == AIRBRUSHED {
                    self.pattern_projected(t, u, v)
                } else {
                    self.pattern_at(u, v)
                };
                let picks = if style == AIRBRUSHED {
                    rgb(p)
                } else {
                    scale3(rgb(p), m)
                };
                let tint =
                    std::array::from_fn(|c| pick_masked(channel(&self.colors, c), picks, t.masks));
                (
                    tint,
                    cutout(p[3]),
                    pick_masked(self.durability, rgb(p), t.masks),
                    p[3],
                )
            }
        };
        let k = self.softness * d;
        // Multicolour's pattern cut-out also wears; airbrushed's only stops the steel edge.
        let cut_wear = if self.style == ANODIZED_MULTI {
            cut
        } else {
            0.0
        };
        let worn = ((t.no_paint + self.worn(t)) * self.growth() + cut_wear) * d;
        let gone_paint = self.gone(worn, k);
        let gone = (1.0 - m).max(gone_paint);
        let edge = self.edge(gone_paint, worn, k) * (1.0 - cut) * m;
        let grime = self.grime(t);
        let bright = lerp(self.brightness, 1.0, edge);
        let color = clamp3(scale3(
            clamp3(scale3(lerp3(lerp3(t.weapon, tint, m), STEEL, edge), bright)),
            bright,
        ));
        let painted = mul3(color, std::array::from_fn(|c| lerp(grime[c], 1.0, edge)));
        let roughness = if self.style == ANODIZED_MULTI {
            let r = 1.0 - (alpha * 2.0).min(1.0);
            let pattern_rough = lerp(r * r * 0.85 + 0.15, self.roughness, f32::from(alpha >= 0.5));
            lerp(
                pattern_rough,
                self.roughness,
                t.masks[1].max(t.masks[2]).max(gone),
            )
        } else {
            self.roughness
        };
        let roughness = self.paint_roughness(u, v, |_| roughness);
        let rough = (lerp(roughness, 0.35, edge) + (1.0 - luminance(rgb(grime))) * 0.2).min(1.0);
        Shade {
            color: lerp3(
                albedo_levels(color, painted, 1.0, self.wear),
                t.weapon,
                gone,
            ),
            roughness: lerp(t.surface[0], rough, (1.0 - gone).max(0.0)),
            metalness: lerp(m, t.surface[1], gone),
        }
    }

    /// Custom paint job (6): the pattern's own colours, its alpha the paint's durability (and a cut-out band). An
    /// overlay's durability scales that one, and the paint is mixed towards the weapon where gone before it is laid
    /// on.
    fn custom(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let p = self.pattern_at(u, v);
        let over = self.overlay_texel(u, v, |_| 1.0);
        let d = smoothstep(0.0, 0.5, p[3]).max(0.0) * self.overlay_durability(over);
        let k = self.softness * d;
        let gone_paint = self.gone(
            ((t.no_paint + self.worn(t)) * self.growth() + cutout(p[3])) * d,
            k,
        );
        let cover = self.cover(over, t, k, gone_paint);
        let gone = cover.gone;
        let grime = self.grime(t);
        let metal = self.paint_metalness(u, v, self.metalness_flat);
        let color = clamp3(scale3(rgb(p), self.brightness));
        let color = if self.overlay.is_some() {
            cover.blend(lerp3(color, t.weapon, gone))
        } else {
            color
        };
        let painted = mul3(color, rgb(grime));
        let roughness = cover.roughness(self.paint_roughness(u, v, |_| self.roughness));
        let (roughness, metalness) =
            self.painted_surface(t, grime, roughness, cover.metalness(metal), gone);
        Shade {
            color: lerp3(
                albedo_levels(color, painted, metal, self.wear),
                t.weapon,
                gone,
            ),
            roughness,
            metalness,
        }
    }

    /// The bare metal's wear of patina and gunsmith: scratches (bright where wear texture, occlusion and cavity
    /// meet), patina (from cavity, occlusion and grunge; 1 clean, 0 patinated) and the grunge (lifted by the
    /// scratches with separate channels).
    fn metal_wear(&self, t: &Texel) -> (f32, f32, [f32; 4]) {
        let w = self.wear;
        let grime = self.grime(t);
        let dirt = (t.grunge[0] * t.grunge[1] * t.grunge[2]).clamp(0.0, 1.0);
        if !self.separate {
            let scratch = smoothstep(0.1, 0.2, t.wear * t.occlusion * t.cavity * t.cavity * w);
            let patina = smoothstep(
                0.0,
                0.15,
                (t.cavity * t.occlusion - 0.1 * w).clamp(0.0, 1.0) - dirt * 0.23 + 0.08,
            );
            (scratch, patina, grime)
        } else {
            let scratch = smoothstep(
                0.2,
                0.6,
                t.wear * t.occlusion * (t.wear * smoothstep(0.2, 0.3, t.cavity)) * w,
            );
            let aged = smoothstep(
                0.5 * w,
                0.0,
                (t.cavity15 * 11.52 * t.occlusion - w * dirt * 2.0).clamp(0.0, 1.0),
            ) * w;
            let lift = (scratch * 2.0).clamp(0.0, 1.0);
            (scratch, 1.0 - aged, grime.map(|g| g.max(lift)))
        }
    }

    /// Patina and gunsmith (7, 8): bare metal where the paint masks' R is — the pattern tinted by colours 1 – 3 as
    /// it patinates and wears, colour 0 in scratches — and, for gunsmith, the pattern painted elsewhere.
    ///
    /// Case hardening replaces the pattern with its ramp (patina; gunsmith by the paint masks' G, over five taps):
    /// the ramp's alpha scales the metalness, its brightness lowers the roughness (× `1 − luminance / 4`). Gunsmith's
    /// overlay goes over its metal by what its own wear leaves, and over its paint, mixed towards the weapon where
    /// gone.
    fn metal(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let w = self.wear;
        let m = t.masks[0];
        let p = self.pattern_at(u, v);
        let (p, hardened) = match self.ramp(t, u, v, self.style == GUNSMITH) {
            Some(ramp) if self.style == PATINA => (ramp, 1.0),
            Some(ramp) => (
                std::array::from_fn(|c| lerp(p[c], ramp[c], t.masks[1])),
                t.masks[1],
            ),
            None => (p, 0.0),
        };
        let ramp_metal = lerp(1.0, p[3], hardened);
        let soften = |r: f32| lerp(r, r * (1.0 - 0.25 * luminance(rgb(p))), hardened);
        let (scratch, patina, grime) = self.metal_wear(t);
        let [c0, c1, c2, c3] = self.colors;
        let pattern = lerp3(
            rgb(p),
            scale3(rgb(p), self.brightness),
            m.max(f32::from(u8::from(self.adjust_all))),
        );
        let metal_color = lerp3(
            mul3(
                lerp3(lerp3(c1, c3, w.sqrt()), lerp3(c1, c2, w), patina),
                pattern,
            ),
            scale3(c0, luminance(pattern)),
            scratch,
        );
        let lum_g = luminance(rgb(grime));
        let dull = (1.0 - grime[3]) * w;
        let metalness = lerp(
            ramp_metal * lerp(1.0, (patina * grime[3] * lum_g).sqrt(), w),
            1.0,
            scratch,
        );
        let rough_with = |base: f32| {
            (base * lerp(1.0, 0.9, scratch)
                + (1.0 - lum_g) * w * 0.05
                + (1.0 - patina) * 0.15 * w
                + dull * 0.15)
                .clamp(0.0, 1.0)
        };
        if self.style == PATINA {
            let roughness = rough_with(soften(self.paint_roughness(u, v, |_| self.roughness)));
            let painted = mul3(metal_color, rgb(grime));
            let levels = albedo_levels(mul3(pattern, c1), painted, ramp_metal, w);
            let edge = if t.no_paint <= 0.996 { m } else { 0.0 };
            return Shade {
                color: lerp3(levels, t.weapon, 1.0 - m),
                roughness: lerp(
                    t.surface[0],
                    lerp((roughness + dull * w * 0.5).min(1.0), roughness, m),
                    edge,
                ),
                metalness: lerp(t.surface[1], metalness, m),
            };
        }
        // Gunsmith: the pattern's alpha is the paint's durability; the metal parts (masks R above 0.99) wear
        // without the threshold.
        let a = p[3];
        let over = self.overlay_texel(u, v, |_| 1.0);
        let d = smoothstep(0.0, 0.5, a).max(0.0) * self.overlay_durability(over);
        let k = self.softness * d;
        let worn = ((t.no_paint.min(1.0 - m) + self.worn(t)) * self.growth() + cutout(a)) * d;
        let metal_part = m > 0.99;
        let gone_paint = if metal_part { worn } else { self.gone(worn, k) };
        let cover = self.cover(over, t, k, gone_paint);
        let gone = cover.gone;
        let paint_metal = self.paint_metalness(u, v, self.metalness_flat);
        let (paint_color, metal_color) = if self.overlay.is_some() {
            let on_metal = Cover {
                alpha: cover.alpha * cover.kept,
                ..cover
            };
            (
                cover.blend(lerp3(clamp3(pattern), t.weapon, gone)),
                on_metal.blend(metal_color),
            )
        } else {
            (pattern, metal_color)
        };
        let painted = mul3(lerp3(paint_color, metal_color, m), rgb(grime));
        let levels = albedo_levels(
            lerp3(pattern, mul3(pattern, c1), m),
            painted,
            lerp(paint_metal, ramp_metal, m),
            w,
        );
        let r = 1.0 - (a * 2.0).min(1.0);
        let base_rough = soften(self.paint_roughness(u, v, |_| self.roughness));
        let roughness = rough_with(lerp(
            base_rough,
            lerp(r * r * 0.85 + 0.15, base_rough, f32::from(a >= 0.5)),
            m,
        ));
        Shade {
            color: lerp3(levels, t.weapon, gone * (1.0 - m)),
            roughness: cover.roughness(lerp(
                t.surface[0],
                lerp((roughness + dull * w * 0.5).min(1.0), roughness, m),
                if metal_part {
                    1.0
                } else {
                    (1.0 - gone).max(0.0)
                },
            )),
            metalness: cover.metalness(lerp(lerp(paint_metal, t.surface[1], gone), metalness, m)),
        }
    }
}

/// Every paint style (`F_PAINT_STYLE` 0 – 8) is baked.
pub(crate) const PAINTED_STYLES: std::ops::RangeInclusive<i64> = SOLID..=GUNSMITH;

/// The styles an overlay is baked on: those CS2's kits put one on (no kit has one on anodized or patina).
const OVERLAID_STYLES: [i64; 5] = [SOLID, HYDROGRAPHIC, SPRAY, CUSTOM, GUNSMITH];

/// Why a weapon material is not painted with these parameters (a style or variant not baked yet, an input missing),
/// or `None`.
pub(crate) fn unpainted_reason(params: &BTreeMap<String, Param>) -> Option<String> {
    let int = |name: &str| match params.get(name) {
        Some(Param::Int(i)) => *i,
        _ => 0,
    };
    let named =
        |name: &str| matches!(params.get(name), Some(Param::Texture(path)) if !path.is_empty());
    let style = int("F_PAINT_STYLE");
    if !PAINTED_STYLES.contains(&style) {
        return Some(format!("paint style {style} not baked yet"));
    }
    for (feature, what) in [
        ("F_PATINA_AGE", "patina age"),
        ("F_CASE_HARDENING_TRILINEAR", "trilinear case hardening"),
    ] {
        if int(feature) != 0 {
            return Some(format!("{what} not baked yet"));
        }
    }
    if int("F_OVERLAY_TEXTURE") != 0 {
        if !OVERLAID_STYLES.contains(&style) {
            return Some(format!("overlay on paint style {style} not baked yet"));
        }
        if !named("g_tOverlay") {
            return Some("no overlay texture".into());
        }
    }
    if int("F_CASE_HARDENING") != 0 && !named("g_tCaseHardeningColorRamp") {
        return Some("no case-hardening ramp".into());
    }
    match style {
        // Solid and anodized name their colours (a recipe without a paint material would otherwise paint the
        // defaults' greys).
        SOLID | ANODIZED => {
            let colored = (0..4).any(|i| params.contains_key(&format!("g_vColor{i}")));
            (!colored).then(|| "no paint colours".into())
        }
        _ if !named("g_tPattern") => Some("no pattern texture".into()),
        SPRAY | AIRBRUSHED if !named("g_tPosition") => Some("no position texture".into()),
        _ => None,
    }
}

/// The job that paints weapon material `material` with `paint`, or why it is not painted (its skin's parameters
/// unreadable, its style or a variant not baked yet, an input missing).
pub(crate) fn job(content: &Content, material: &str, paint: &Paint) -> Result<PaintJob, String> {
    let recipe = paint.recipe.as_ref().ok_or("no paint kit")?;
    let params = crate::paint::weapon_params(content, recipe, material, paint.seed, paint.wear)
        .map_err(|e| e.to_string())?;
    let reason = if params.glove {
        super::gloves::unpainted_reason(&params.color)
    } else {
        unpainted_reason(&params.color).or_else(|| unpainted_reason(&params.surface))
    };
    if let Some(why) = reason {
        return Err(why);
    }
    // The paint alone: two weapons with one skin and different stickers share it (the stickers are drawn over it).
    Ok(PaintJob {
        key: format!(
            "{material}|{:p}|{}|{:08x}",
            Arc::as_ptr(recipe),
            paint.seed,
            paint.wear.to_bits()
        ),
        params: Arc::new(params),
    })
}

/// Weapon material `material` painted with `paint` as an export bakes it: its colour and metallic-roughness
/// textures, or why it is not painted. A developer's check (`examples/paint_bake.rs`).
pub fn bake(content: &Content, material: &str, paint: &Paint) -> Result<(Image, Image), String> {
    let job = job(content, material, paint)?;
    let color = bake_color(content, &job).map_err(|e| e.to_string())?;
    let surface = bake_surface(content, &job).map_err(|e| e.to_string())?;
    Ok((color, surface))
}

/// The painted colour texture of `job` (sRGB; alpha the weapon colour's): see the [module docs](self).
pub(crate) fn bake_color(content: &Content, job: &PaintJob) -> Result<Image, ExportError> {
    if job.params.glove {
        return super::gloves::bake_color(content, &job.params.color);
    }
    let inputs = Inputs::read(content, &job.params.color)?;
    let (width, height) = inputs.size();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            );
            let alpha = inputs
                .base_color
                .as_ref()
                .map_or(1.0, |s| s.at_with(u, v, [Address::Clamp; 2])[3]);
            let shade = inputs.shade(u, v);
            rgba.extend(shade.color.map(linear_to_srgb));
            rgba.push((alpha * 255.0).round() as u8);
        }
    }
    Ok(Image::new(width, height, rgba))
}

/// The painted metallic-roughness texture of `job` (glTF channels: G roughness, B metalness): see the
/// [module docs](self).
pub(crate) fn bake_surface(content: &Content, job: &PaintJob) -> Result<Image, ExportError> {
    if job.params.glove {
        // The glove's roughness comes with its normal output, its metalness with the cloth: both from the colour's
        // parameters but for the output mode.
        return super::gloves::bake_surface(content, &job.params.color);
    }
    let inputs = Inputs::read(content, &job.params.surface)?;
    let (width, height) = inputs.size();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            );
            let shade = inputs.shade(u, v);
            let byte = |c: f32| (c * 255.0).round() as u8;
            rgba.extend_from_slice(&[255, byte(shade.roughness), byte(shade.metalness), 255]);
        }
    }
    Ok(Image::new(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5
    }

    #[test]
    fn placements_follow_the_shaders_transform() {
        // No rotation: scale then offset, about the layout's corner.
        let at = Placement::new(2.0, 0.0, [0.1, 0.2]);
        assert!(close(at.apply(0.5, 0.5), (1.1, 1.2)));
        assert!(close(at.apply(0.0, 0.0), (0.1, 0.2)));
        // A quarter turn: (u, v) → (−s·v, s·u), plus the offset.
        let at = Placement::new(2.0, 90.0, [0.1, 0.2]);
        assert!(
            close(at.apply(0.75, 0.5), (-0.9, 1.7)),
            "{:?}",
            at.apply(0.75, 0.5)
        );
        // Between them, the shader's translation adds a shift: at 45° and scale 1, a corner goes to
        // ((cos² + cos·sin² + sin³ − 1) / 2, …) with cos = sin = √½: about 0.1036.
        let (u, _) = Placement::new(1.0, 45.0, [0.0; 2]).apply(0.0, 0.0);
        assert!((u - 0.103_55).abs() < 1e-4, "{u}");
        // Scale, rotation and offset are rounded to hundredths first.
        assert_eq!(hundredths(321.706_33), 321.71);
        assert!(close(
            Placement::new(1.0, 0.0, [0.702_792, 0.613_506]).apply(0.0, 0.0),
            (0.7, 0.61)
        ));
    }

    #[test]
    fn samples_wrap_clamp_mirror_and_blend() {
        let sampler = Sampler {
            image: Image::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]),
        };
        // Texel centres: exact.
        assert_eq!(sampler.at(0.25, 0.5)[0], 0.0);
        assert_eq!(sampler.at(0.75, 0.5)[0], 1.0);
        // Halfway, and across the wrap.
        assert!((sampler.at(0.5, 0.5)[0] - 0.5).abs() < 1e-6);
        assert!((sampler.at(1.0, 0.5)[0] - 0.5).abs() < 1e-6);
        assert_eq!(sampler.at(1.25, 0.5), sampler.at(0.25, 0.5));
        // Clamped, the edge texel stays; mirrored, 1.25 reads 0.75.
        let clamp = [Address::Clamp; 2];
        assert_eq!(sampler.at_with(1.4, 0.5, clamp)[0], 1.0);
        assert_eq!(sampler.at_with(-0.4, 0.5, clamp)[0], 0.0);
        let mirror = [Address::Mirror; 2];
        assert_eq!(sampler.at_with(1.25, 0.5, mirror), sampler.at(0.75, 0.5));
    }

    fn pixel(rgba: [u8; 4]) -> Option<Sampler> {
        Some(Sampler {
            image: Image::new(1, 1, rgba.to_vec()),
        })
    }

    /// A one-texel weapon in `style`: wear amount `wear`, wear texture `wear_texel`, cavity `cavity` (sRGB byte),
    /// no-paint mask `no_paint`; white weapon colour, black paint colours.
    fn inputs(style: i64, wear: f32, wear_texel: u8, cavity: u8, no_paint: u8) -> Inputs {
        let at = Placement::new(1.0, 0.0, [0.0; 2]);
        Inputs {
            style,
            separate: false,
            pattern: None,
            pattern_srgb: false,
            pattern_address: [Address::Wrap; 2],
            position: None,
            surface: None,
            base_color: None,
            base_surface: None,
            occlusion: pixel([cavity, 255, 0, no_paint]),
            masked: true,
            masks: None,
            wear_texture: pixel([wear_texel, 0, 0, 255]),
            grunge: None,
            paint_roughness: None,
            paint_metalness: None,
            roughness_on_pattern: true,
            metalness_on_pattern: false,
            pattern_at: at,
            projection_at: at,
            wear_at: at,
            grunge_at: at,
            wear,
            brightness: 1.0,
            colors: [[0.0; 3]; 4],
            durability: [1.0; 4],
            metalness: [0.0; 4],
            roughness: 0.6,
            roughness_per_color: None,
            metalness_flat: 0.0,
            softness: 0.0,
            adjust_all: true,
            bias_spray: false,
            spray_blend: [1.0; 2],
            overlay: None,
            halftone: None,
            all_masks: false,
            case_hardening: None,
        }
    }

    /// An opaque overlay of colour `rgba` (sRGB bytes) on the weapon's UVs, normal blend, mask mode `mask_mode`,
    /// durability as authored `durability` (0 lasts like the paint).
    fn overlay(rgba: [u8; 4], mask_mode: i64, durability: f32) -> Option<Overlay> {
        Some(Overlay {
            image: pixel(rgba)?,
            mask: None,
            at: OverlayAt::Weapon,
            mask_mode,
            blend: 0,
            strength: 1.0,
            brightness: 1.0,
            durability: 1.0 - durability,
            material_strength: 1.0,
            roughness: 0.25,
            metalness: 0.75,
        })
    }

    #[test]
    fn an_overlay_covers_the_paint_and_its_own_wear_can_keep_it() {
        // Black solid paint under a white overlay, nothing worn: white, with the overlay's roughness and metalness.
        let mut covered = inputs(SOLID, 0.0, 0, 128, 0);
        covered.overlay = overlay([255; 4], 0, 0.0);
        let shade = covered.shade(0.5, 0.5);
        assert!(shade.color.iter().all(|c| *c > 0.99), "{:?}", shade.color);
        assert!((shade.roughness - 0.25).abs() < 1e-6 && (shade.metalness - 0.75).abs() < 1e-6);
        // Where the paint wears off (white wear texel on an edge), a durable overlay (as authored 0.9 → 0.1) keeps
        // it, red over the black paint.
        let mut durable = inputs(SOLID, 0.0, 255, 255, 0);
        durable.overlay = overlay([255, 0, 0, 255], 6, 0.9);
        let kept = durable.shade(0.5, 0.5).color;
        assert!(kept[0] > 0.99 && kept[1] < 0.01, "{kept:?}");
        // Where the weapon takes no paint, an overlay of its own layer (mask mode 0) stays; one on the paint (mode
        // 6) goes with it.
        let bare = |mask_mode: i64| {
            let mut inputs = inputs(SOLID, 0.0, 0, 255, 255);
            inputs.overlay = overlay([255, 0, 0, 255], mask_mode, 0.0);
            inputs.shade(0.5, 0.5).color
        };
        let stays = bare(0);
        assert!(stays[0] > 0.99 && stays[1] < 0.01, "{stays:?}");
        assert_eq!(bare(6), [1.0; 3]);
        // A region mask: colour 1's region (R mask) gets the overlay, colour 0's does not.
        assert_eq!(overlay_region(2, [1.0, 0.0, 0.0]), 1.0);
        assert_eq!(overlay_region(2, [0.0, 0.0, 0.0]), 0.0);
        assert_eq!(overlay_region(1, [0.0, 0.0, 0.0]), 1.0);
    }

    #[test]
    fn blends_mix_or_tone_the_overlay_into_the_paint() {
        let (c, o) = ([0.2, 0.4, 0.6], [0.9, 0.1, 0.1]);
        // No alpha: the paint as it is; normal and layer at full alpha: the overlay.
        for mode in 0..5 {
            assert_eq!(blend(c, o, 0.0, mode), c);
        }
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6);
        assert!(near(blend(c, o, 1.0, 0), o));
        assert!(near(blend(c, o, 1.0, 4), o));
        // Multiply by white keeps the hue of the paint; colour takes the overlay's hue × the paint's.
        let white = blend(c, [1.0; 3], 1.0, 2);
        assert!(white[2] > white[1] && white[1] > white[0], "{white:?}");
        let tinted = blend(c, o, 1.0, 1);
        assert!(tinted[0] > tinted[2], "{tinted:?}");
        // Add brightens.
        assert!(luminance(blend(c, o, 1.0, 3)) > luminance(c));
    }

    #[test]
    fn case_hardening_takes_colours_from_its_ramp() {
        // A blue ramp replacing colour 0 entirely: the unmasked paint is blue.
        let mut hardened = inputs(SOLID, 0.0, 0, 128, 0);
        hardened.case_hardening = Some(CaseHardening {
            ramp: pixel([0, 0, 255, 255]).expect("ramp"),
            normal: None,
            pattern: 0.5,
            geometric: 1.0,
            offset: 0.0,
            weights: [1.0, 0.0, 0.0, 0.0],
        });
        hardened.masks = pixel([0, 0, 0, 255]);
        let shade = hardened.shade(0.5, 0.5);
        assert!(
            shade.color[2] > 0.9 && shade.color[0] < 0.1,
            "{:?}",
            shade.color
        );
        // Colour 1 keeps its own (black) where the R mask picks it.
        hardened.masks = pixel([255, 0, 0, 255]);
        assert!(hardened.shade(0.5, 0.5).color[2] < 0.1);
    }

    #[test]
    fn all_masks_spray_paint_takes_colours_2_and_3_by_the_masks() {
        // A spray paint with an empty pattern paints colour 0; with all masks, the masks' B picks colour 3.
        let spray = |all_masks: bool| {
            let mut inputs = inputs(SPRAY, 0.0, 0, 128, 0);
            inputs.pattern = pixel([0, 0, 0, 255]);
            inputs.colors[3] = [1.0, 1.0, 0.0];
            inputs.masks = pixel([0, 0, 255, 255]);
            inputs.all_masks = all_masks;
            inputs.shade(0.5, 0.5).color
        };
        assert!(spray(false)[0] < 0.01);
        assert!(spray(true)[0] > 0.9);
    }

    /// How much of a black solid paint is gone over a white weapon: its colour's red.
    fn gone(inputs: Inputs) -> f32 {
        inputs.shade(0.5, 0.5).color[0]
    }

    #[test]
    fn paint_goes_where_wear_texture_and_cavity_are_bright() {
        // (wear texture × cavity) × (6 × wear + 1) past 0.58 – 0.68: a white wear texel on the brightest edge loses
        // its paint even factory new, a mid-grey one only once worn.
        assert!(gone(inputs(SOLID, 0.0, 255, 255, 0)) > 0.99);
        assert!(gone(inputs(SOLID, 0.0, 128, 255, 0)) < 0.01);
        assert!(gone(inputs(SOLID, 0.1, 128, 255, 0)) > 0.99);
        // The same wear keeps the paint in a recess (cavity 128, about 0.22 linear).
        assert!(gone(inputs(SOLID, 0.1, 128, 128, 0)) < 0.01);
        // The no-paint mask has none, worn or not.
        assert!(gone(inputs(SOLID, 0.0, 0, 0, 255)) > 0.99);
        // A fully durable paint (g_vPaintDurability 1) never wears.
        let mut durable = inputs(SOLID, 1.0, 255, 255, 0);
        durable.durability = [0.0; 4];
        assert!(gone(durable) < 0.01);
    }

    #[test]
    fn separate_channels_curve_the_cavity_and_move_the_threshold() {
        // Cavity 0.8 (sRGB byte 231): 0.96 × 0.8^1.5 ≈ 0.686; at wear 0 the curve is smoothstep(0, 0.72, c^1.3)
        // ≈ 0.94, past 0.74 with a full wear texel.
        let mut fresh = inputs(SOLID, 0.0, 255, 231, 0);
        fresh.separate = true;
        let t = fresh.texel(0.5, 0.5);
        assert!((t.cavity - 0.96 * t.cavity15).abs() < 1e-6);
        assert!((fresh.worn(&t) - 0.94).abs() < 0.01, "{}", fresh.worn(&t));
        assert!(gone(fresh) > 0.9);
        // The old threshold takes a value of 0.66 almost all; the new one about half.
        let mut separate = inputs(SOLID, 0.0, 0, 0, 0);
        separate.separate = true;
        assert!((separate.gone(0.66, 0.0) - 0.5).abs() < 0.2);
        assert!(inputs(SOLID, 0.0, 0, 0, 0).gone(0.66, 0.0) > 0.8);
    }

    #[test]
    fn grunge_grows_with_wear_and_in_recesses() {
        let grime = |wear: f32, cavity: u8| {
            let mut inputs = inputs(SOLID, wear, 0, cavity, 0);
            inputs.grunge = pixel([0, 0, 0, 0]);
            let t = inputs.texel(0.5, 0.5);
            inputs.grime(&t)
        };
        // On an edge (cavity 1) none when new, 3/4 of it fully worn; in a recess (cavity 0) 1/4 already.
        assert_eq!(grime(0.0, 255), [1.0; 4]);
        assert!((grime(1.0, 255)[0] - 0.25).abs() < 1e-6);
        assert!((grime(0.0, 0)[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn colours_take_their_levels_with_wear() {
        let grey = [0.5; 3];
        assert_eq!(albedo_levels(grey, grey, 0.0, 0.0), grey);
        // Fully worn, mid grey goes to 0.045 + 0.955 × 0.5^1.32 (0.4): about 0.427; white stays white.
        assert!((albedo_levels(grey, grey, 0.0, 1.0)[0] - 0.427).abs() < 1e-3);
        let white = [1.0; 3];
        assert!(
            albedo_levels(white, white, 0.0, 1.0)
                .iter()
                .all(|c| (c - 1.0).abs() < 1e-5)
        );
        // The masks pick the colour: R picks colour 1 over 0, then G colour 2, then B colour 3.
        assert_eq!(pick([0.0, 1.0, 2.0, 3.0], [0.0, 0.0, 0.0]), 0.0);
        assert_eq!(pick([0.0, 1.0, 2.0, 3.0], [1.0, 0.0, 0.0]), 1.0);
        assert_eq!(pick([0.0, 1.0, 2.0, 3.0], [1.0, 1.0, 1.0]), 3.0);
    }

    #[test]
    fn anodized_tints_only_the_masked_metal() {
        // Masks R 1: the weapon takes colour 0 (here red); masks R 0: the weapon's own white.
        let mut tinted = inputs(ANODIZED, 0.0, 0, 128, 0);
        tinted.colors[0] = [1.0, 0.0, 0.0];
        tinted.masks = pixel([255, 0, 0, 255]);
        let shade = tinted.shade(0.5, 0.5);
        assert!(
            shade.color[0] > 0.9 && shade.color[1] < 0.1,
            "{:?}",
            shade.color
        );
        assert_eq!(shade.metalness, 1.0);
        let mut bare = inputs(ANODIZED, 0.0, 0, 128, 0);
        bare.colors[0] = [1.0, 0.0, 0.0];
        bare.masks = pixel([0, 0, 0, 255]);
        assert_eq!(bare.shade(0.5, 0.5).color, [1.0; 3]);
    }

    #[test]
    fn custom_paint_wears_by_its_patterns_alpha() {
        // An opaque white pattern stays on a mid-grey wear texel; alpha 0 (durability 0) never wears; alpha 0.75
        // is the cut-out band: no paint there.
        let custom = |alpha: u8, wear_texel: u8| {
            let mut inputs = inputs(CUSTOM, 0.0, wear_texel, 255, 0);
            inputs.pattern = pixel([0, 0, 0, alpha]);
            gone(inputs)
        };
        assert!(custom(255, 128) < 0.01);
        assert!(custom(255, 255) > 0.99);
        assert!(custom(0, 255) < 0.01);
        assert!(custom(191, 0) > 0.99);
    }

    #[test]
    fn every_style_is_painted_unless_a_variant_or_input_is_missing() {
        let params = |style: i64, pattern: Option<&str>| {
            let mut params = BTreeMap::from([("F_PAINT_STYLE".to_owned(), Param::Int(style))]);
            if let Some(path) = pattern {
                params.insert("g_tPattern".into(), Param::Texture(path.into()));
            }
            params
        };
        assert_eq!(unpainted_reason(&params(6, Some("fireserpent.vtex"))), None);
        assert_eq!(unpainted_reason(&params(8, Some("gungnir.vtex"))), None);
        assert_eq!(unpainted_reason(&params(1, Some("webs.vtex"))), None);
        assert_eq!(unpainted_reason(&params(4, Some("doppler.vtex"))), None);
        // Solid needs its colours but no pattern (Whiteout); an overlay (Royal Guard) its texture.
        let mut solid = params(0, None);
        solid.insert("g_vColor0".into(), Param::Floats([0.86, 0.85, 0.83, 1.0]));
        assert_eq!(unpainted_reason(&solid), None);
        assert!(unpainted_reason(&BTreeMap::new()).is_some());
        let mut overlay = solid.clone();
        overlay.insert("F_OVERLAY_TEXTURE".into(), Param::Int(1));
        assert!(unpainted_reason(&overlay).is_some());
        overlay.insert(
            "g_tOverlay".into(),
            Param::Texture("usp_blue_cavity_overlay.vtex".into()),
        );
        assert_eq!(unpainted_reason(&overlay), None);
        // No kit puts one on an anodized or patina finish; patina age is not baked yet.
        let mut anodized = overlay.clone();
        anodized.insert("F_PAINT_STYLE".into(), Param::Int(3));
        assert!(unpainted_reason(&anodized).is_some());
        let mut aged = params(7, Some("bronze.vtex"));
        aged.insert("F_PATINA_AGE".into(), Param::Int(1));
        assert!(unpainted_reason(&aged).is_some());
        // Case hardening needs its ramp.
        let mut hardened = params(7, Some("oiled.vtex"));
        hardened.insert("F_CASE_HARDENING".into(), Param::Int(1));
        assert!(unpainted_reason(&hardened).is_some());
        hardened.insert(
            "g_tCaseHardeningColorRamp".into(),
            Param::Texture("ramp.vtex".into()),
        );
        assert_eq!(unpainted_reason(&hardened), None);
        // Fades project their pattern from the weapon's position texture.
        assert!(unpainted_reason(&params(5, Some("fade.vtex"))).is_some());
        let mut fade = params(5, Some("fade.vtex"));
        fade.insert(
            "g_tPosition".into(),
            Param::Texture("glock_pos.vtex".into()),
        );
        assert_eq!(unpainted_reason(&fade), None);
        // A custom paint job without a pattern has nothing.
        assert!(unpainted_reason(&params(6, Some(""))).is_some());
        assert!(unpainted_reason(&params(6, None)).is_some());
        assert!(unpainted_reason(&params(9, Some("x.vtex"))).is_some());
    }
}
