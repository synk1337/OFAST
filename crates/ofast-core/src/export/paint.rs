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
//! **Not baked**: overlays, case hardening, patina age, spray paint's halftone and all-masks variants — those
//! weapons keep their own finish (`unpainted_reason`). Pearlescence and the normal-map pass are not read.
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
use crate::export::loadout::Finish;
use crate::paint::{PaintKit, Param, Recipe, kit_recipe, paint_kits};
use crate::resource::Resource;
use crate::texture::{Image, ImageF32, Texture};

/// A skin to paint on a model's materials: what a weapon look carries into the glTF writer.
#[derive(Debug, Clone)]
pub struct Paint {
    /// A short name for the painted materials and textures, unique to the kit, seed and wear:
    /// `cu_fireserpent_ak47_bravo_245_1434` ([`Paint::label_of`]).
    pub label: String,
    /// The paint kit's recipe.
    pub recipe: Arc<Recipe>,
    /// Pattern seed.
    pub seed: u32,
    /// Wear, 0 – 1.
    pub wear: f32,
}

impl Paint {
    /// The label of kit `kit` at `seed` and `wear` (wear to 4 digits, as CS2's inspect panel shows more).
    pub fn label_of(kit: &str, seed: u32, wear: f32) -> String {
        format!("{kit}_{seed}_{:04}", (wear * 10_000.0).round() as u32)
    }
}

/// The paint kits of `items_game.txt`, and each kit's recipe read once (on first use), for a round's export.
#[derive(Debug, Default)]
pub struct Paints {
    kits: BTreeMap<u32, PaintKit>,
    recipes: Mutex<HashMap<u32, Option<Arc<Recipe>>>>,
}

impl Paints {
    /// The kits of `items_game.txt`'s text (none without it: weapons keep their own finish).
    pub fn new(items_game: Option<&str>) -> Self {
        Self {
            kits: items_game.map(paint_kits).unwrap_or_default(),
            recipes: Mutex::default(),
        }
    }

    /// The paint of `finish`; `None`, with a warning the first time for the kit, when its kit or recipe cannot be
    /// read (without `items_game.txt`, quietly: the export warned about it already).
    pub fn paint(
        &self,
        content: &Content,
        finish: Finish,
        warnings: &mut Vec<String>,
    ) -> Option<Paint> {
        if self.kits.is_empty() {
            return None;
        }
        let mut recipes = self.recipes.lock().expect("lock");
        let Some(kit) = self.kits.get(&finish.kit) else {
            recipes.entry(finish.kit).or_insert_with(|| {
                warnings.push(format!(
                    "paint kit {}: not in items_game.txt, left unpainted",
                    finish.kit
                ));
                None
            });
            return None;
        };
        let recipe = recipes
            .entry(finish.kit)
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
        Some(Paint {
            label: Paint::label_of(&kit.name, finish.seed, finish.wear()),
            recipe,
            seed: finish.seed,
            wear: finish.wear(),
        })
    }
}

/// One weapon material painted with one skin: the parameters to bake from ([`crate::paint::weapon_params`]). Two
/// jobs are the same when their keys are (material, kit, seed, wear).
#[derive(Debug, Clone)]
pub(crate) struct PaintJob {
    pub(crate) key: String,
    pub(crate) params: Arc<BTreeMap<String, Param>>,
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
enum Address {
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
struct Sampler {
    image: Image,
}

impl Sampler {
    fn load(content: &Content, path: &str) -> Result<Self, ExportError> {
        Ok(Self {
            image: decode_raw(content, path)?.1,
        })
    }

    /// Channels 0 – 3 at (`u`, `v`), each 0 – 1 as stored, wrapping around.
    fn at(&self, u: f32, v: f32) -> [f32; 4] {
        self.at_with(u, v, [Address::Wrap; 2])
    }

    /// Channels 0 – 3 at (`u`, `v`), with an address mode per axis.
    fn at_with(&self, u: f32, v: f32, address: [Address; 2]) -> [f32; 4] {
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
struct Placement {
    rows: [[f32; 3]; 2],
}

/// `x` rounded to hundredths, as the shader's expressions do: `floor((x + 0.005) × 100) / 100`.
fn hundredths(x: f32) -> f32 {
    ((x + 0.005) * 100.0).floor() / 100.0
}

impl Placement {
    fn new(scale: f32, rotation: f32, offset: [f32; 2]) -> Self {
        let s = hundredths(scale);
        let angle = hundredths(rotation) * std::f32::consts::PI / 180.0;
        let [ou, ov] = offset.map(hundredths);
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

    fn apply(&self, u: f32, v: f32) -> (f32, f32) {
        let [x, y] = self.rows.map(|r| r[0] * u + r[1] * v + r[2]);
        (x, y)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|c| lerp(a[c], b[c], t))
}

fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| a[c] * b[c])
}

fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    a.map(|x| x * k)
}

fn clamp3(a: [f32; 3]) -> [f32; 3] {
    a.map(|x| x.clamp(0.0, 1.0))
}

fn rgb(a: [f32; 4]) -> [f32; 3] {
    [a[0], a[1], a[2]]
}

/// The shaders' luminance weights.
fn luminance(c: [f32; 3]) -> f32 {
    c[0] * 0.2125 + c[1] * 0.7154 + c[2] * 0.0721
}

/// GLSL's `smoothstep`, also with `low` above `high` (a falling edge, as the shader uses it); equal edges step.
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
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
fn pick(values: [f32; 4], t: [f32; 3]) -> f32 {
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
        Ok(Self {
            style,
            separate: int("F_SEPARATE_CHANNEL_INPUTS", 0) != 0,
            pattern: if style == SOLID || style == ANODIZED {
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
            pattern_at: placement("Pattern", size_scale),
            projection_at: placement(
                "Pattern",
                if ignore_size {
                    1.0
                } else {
                    float("g_flWeaponLength1", 36.0) / 36.0
                },
            ),
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
    fn pattern_projected(&self, u: f32, v: f32) -> [f32; 4] {
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
        let at = |a: f32, b: f32| {
            let (pu, pv) = self.projection_at.apply(a, b);
            self.pattern_sample(pu, pv)
        };
        let (yz, xz, yx) = (at(y, z), at(x, z), at(y, x));
        let to_xz = self.spray_blend[0] * n[1].abs().powi(7);
        let to_yx = self.spray_blend[1] * n[2].abs().powi(7);
        std::array::from_fn(|c| lerp(lerp(yz[c], xz[c], to_xz), yx[c], to_yx))
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

    /// Solid (0): colours by paint mask.
    fn solid(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let d = pick(self.durability, rgb(t.masks));
        let k = self.softness * d;
        let gone = self.gone((t.no_paint + self.worn(t)) * self.growth() * d, k);
        let grime = self.grime(t);
        let metal = self.paint_metalness(u, v, pick(self.metalness, rgb(t.masks)));
        let base = std::array::from_fn(|c| pick(channel(&self.colors, c), rgb(t.masks)));
        let painted = mul3(base, rgb(grime));
        let roughness = self.paint_roughness(u, v, |r| pick(r, rgb(t.masks)));
        let (roughness, metalness) = self.painted_surface(t, grime, roughness, metal, gone);
        Shade {
            color: lerp3(
                albedo_levels(base, painted, metal, self.wear),
                t.weapon,
                gone,
            ),
            roughness,
            metalness,
        }
    }

    /// Hydrographic and spray paint (1, 2): the pattern's channels pick the colours, and the wear peels them off
    /// layer by layer (R, then G, then B) before the paint goes.
    fn layered(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let w = self.wear;
        let spray = self.style == SPRAY;
        let p = if spray {
            self.pattern_projected(u, v)
        } else {
            self.pattern_at(u, v)
        };
        let cut = cutout(p[3]);
        // Hydrographic also by the paint masks G and B, and it paints only where they or the pattern's alpha are.
        let (d, worn) = if spray {
            let d = pick(self.durability, rgb(p));
            (d, (t.no_paint + self.worn(t)) * self.growth() * d)
        } else {
            let d = pick_masked(self.durability, rgb(p), t.masks);
            let reach = (t.masks[1] + t.masks[2])
                .clamp(0.0, 1.0)
                .max(smoothstep(0.0, 0.5, p[3]));
            (
                d,
                ((t.no_paint + self.worn(t)) * self.growth() + cut) * reach * d,
            )
        };
        let k = self.softness * d;
        let (first, second, third) = (0.56 - 0.06 * w, 0.54 - 0.12 * w, 0.52 - 0.18 * w);
        let layers = [
            p[0] * smoothstep(0.58 + k, first - k, worn),
            p[1] * smoothstep(first + k, second - k, worn),
            p[2] * smoothstep(second + k, third - k, worn),
        ];
        let gone = self.gone(worn, k);
        let grime = self.grime(t);
        let by_layer = |values: [f32; 4]| {
            if spray {
                pick(values, layers)
            } else {
                pick_masked(values, layers, t.masks)
            }
        };
        let metal = self.paint_metalness(u, v, by_layer(self.metalness));
        let base: [f32; 3] = std::array::from_fn(|c| by_layer(channel(&self.colors, c)));
        let edge = self.edge(gone, worn, k) * (1.0 - cut);
        let color = clamp3(scale3(base, lerp(self.brightness, 1.0, edge)));
        let painted = mul3(color, rgb(grime));
        let roughness = self.paint_roughness(u, v, by_layer);
        let (roughness, metalness) = self.painted_surface(t, grime, roughness, metal, gone);
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
                    self.pattern_projected(u, v)
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

    /// Custom paint job (6): the pattern's own colours, its alpha the paint's durability (and a cut-out band).
    fn custom(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let p = self.pattern_at(u, v);
        let d = smoothstep(0.0, 0.5, p[3]).max(0.0);
        let k = self.softness * d;
        let gone = self.gone(
            ((t.no_paint + self.worn(t)) * self.growth() + cutout(p[3])) * d,
            k,
        );
        let grime = self.grime(t);
        let metal = self.paint_metalness(u, v, self.metalness_flat);
        let color = clamp3(scale3(rgb(p), self.brightness));
        let painted = mul3(color, rgb(grime));
        let roughness = self.paint_roughness(u, v, |_| self.roughness);
        let (roughness, metalness) = self.painted_surface(t, grime, roughness, metal, gone);
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
    fn metal(&self, t: &Texel, u: f32, v: f32) -> Shade {
        let w = self.wear;
        let m = t.masks[0];
        let p = self.pattern_at(u, v);
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
            lerp(1.0, (patina * grime[3] * lum_g).sqrt(), w),
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
            let roughness = rough_with(self.paint_roughness(u, v, |_| self.roughness));
            let painted = mul3(metal_color, rgb(grime));
            let levels = albedo_levels(mul3(pattern, c1), painted, 1.0, w);
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
        let d = smoothstep(0.0, 0.5, a).max(0.0);
        let k = self.softness * d;
        let worn = ((t.no_paint.min(1.0 - m) + self.worn(t)) * self.growth() + cutout(a)) * d;
        let metal_part = m > 0.99;
        let gone = if metal_part { worn } else { self.gone(worn, k) };
        let paint_metal = self.paint_metalness(u, v, self.metalness_flat);
        let painted = mul3(lerp3(pattern, metal_color, m), rgb(grime));
        let levels = albedo_levels(
            lerp3(pattern, mul3(pattern, c1), m),
            painted,
            lerp(paint_metal, 1.0, m),
            w,
        );
        let r = 1.0 - (a * 2.0).min(1.0);
        let base_rough = self.paint_roughness(u, v, |_| self.roughness);
        let roughness = rough_with(lerp(
            base_rough,
            lerp(r * r * 0.85 + 0.15, base_rough, f32::from(a >= 0.5)),
            m,
        ));
        Shade {
            color: lerp3(levels, t.weapon, gone * (1.0 - m)),
            roughness: lerp(
                t.surface[0],
                lerp((roughness + dull * w * 0.5).min(1.0), roughness, m),
                if metal_part {
                    1.0
                } else {
                    (1.0 - gone).max(0.0)
                },
            ),
            metalness: lerp(lerp(paint_metal, t.surface[1], gone), metalness, m),
        }
    }
}

/// Every paint style (`F_PAINT_STYLE` 0 – 8) is baked.
pub(crate) const PAINTED_STYLES: std::ops::RangeInclusive<i64> = SOLID..=GUNSMITH;

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
        ("F_OVERLAY_TEXTURE", "overlay"),
        ("F_CASE_HARDENING", "case hardening"),
        ("F_PATINA_AGE", "patina age"),
        ("F_SPRAYPAINT_HALFTONE", "halftone"),
        ("F_USE_ALL_MASKS", "all-masks spray paint"),
    ] {
        if int(feature) != 0 {
            return Some(format!("{what} not baked yet"));
        }
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

/// The painted colour texture of `job` (sRGB; alpha the weapon colour's): see the [module docs](self).
pub(crate) fn bake_color(content: &Content, job: &PaintJob) -> Result<Image, ExportError> {
    let inputs = Inputs::read(content, &job.params)?;
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
    let inputs = Inputs::read(content, &job.params)?;
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
        }
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
        // Solid needs its colours but no pattern (Whiteout); with an overlay (Royal Guard) it waits.
        let mut solid = params(0, None);
        solid.insert("g_vColor0".into(), Param::Floats([0.86, 0.85, 0.83, 1.0]));
        assert_eq!(unpainted_reason(&solid), None);
        assert!(unpainted_reason(&BTreeMap::new()).is_some());
        let mut overlay = solid.clone();
        overlay.insert("F_OVERLAY_TEXTURE".into(), Param::Int(1));
        assert!(unpainted_reason(&overlay).is_some());
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
