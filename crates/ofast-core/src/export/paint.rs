//! Skins baked onto weapon textures (M10): a weapon material's colour and metal-roughness, painted with a skin's
//! parameters ([`crate::paint`]), as CS2's composite materials generate them into the weapon's own textures
//! (`docs/research/cs2-skins.md`).
//!
//! # CS2's paint shader, as OFAST's own Rust
//!
//! CS2 renders the paint material (`csgo_customweapon.vfx`) over the weapon's UV layout twice, once for the colour
//! and once for roughness and metalness (`F_ROUGHNESS_MODE`); every input is a texture in that layout (the weapon's
//! composite inputs) or one placed on it by a transform. This bake does the same per texel.
//!
//! Where the knowledge comes from — Valve's compiled shader as decompiled by Source 2 Viewer and tracked in
//! SteamDatabase's GameTracking-CS2 (`game/csgo/shaders_vulkan_dir/shaders/vfx/csgo_customweapon.slang`), which
//! holds the vertex stage and the pixel stage's default variant: the solid style (`F_PAINT_STYLE` 0) with no
//! overlay, case hardening or paint textures (decision 024):
//!
//! - **Placements** (vertex stage, every style): pattern, wear and grunge each at `s·R(θ)·uv + t` (`Placement`),
//!   the scale `g_fl*TexCoordScale` times the weapon's size (`g_flUvScale1`, 1 with `g_bIgnoreWeaponSizeScale`),
//!   scale, rotation and offset rounded to hundredths, and the translation the shader derives from them.
//! - **Paint gone** (where the weapon's own colour and surface show), with the composite inputs' cavity
//!   (`g_tAmbientOcclusion` R, sRGB), no-paint mask (its A) and paint masks (`g_tMasks` RGB), the wear texture
//!   (`g_tWear` R) and the wear amount `w`: `smoothstep(0.58 − k, 0.68 + k, (no_paint + wear × cavity) × (6w + 1)
//!   × d)`, `d` the paint's durability (`1 − g_vPaintDurability`, one per mask) and `k = g_fWearSoftness × d`. A
//!   no-paint texture stored without alpha (all 255) counts as no mask.
//! - **Grunge** (`g_tGrunge`, sRGB colour): `lerp(1, grunge, (1 − cavity)⁴ / 4 + 3w / 4)` multiplies the paint's
//!   colour; its alpha roughens it: `+ (1 − a) × w² / 2`.
//! - **Solid colour**: the colours `g_vColor0` – `3` (sRGB) chosen by the paint masks, times the grunge, pulled
//!   towards the paint's albedo levels by the wear (`albedo_levels`).
//! - **Surface**: roughness `g_flPaintRoughness` (`g_vPaintRoughness` per mask with `F_ROUGHNESS_PER_COLOR`),
//!   metalness `g_vPaintMetalness` per mask; where the paint is gone, the weapon's own `g_tMetalness` (R roughness,
//!   G metalness, as `gltf` reads a `csgo_weapon` material).
//!
//! Not in the tracked variant, so OFAST's reading, to check against the variants' own code:
//!
//! - **Custom paint jobs and gunsmith** (6, 8): the pattern's colour (sRGB, × `g_flColorBrightness`) in place of
//!   the solid colours, under the same wear, grunge and surface; no albedo levels.
//! - **Paint textures**: `g_tPaintRoughness` R (`F_ROUGHNESS_TEXTURE`; at the pattern's placement unless
//!   `g_bRoughnessUsesPatternUVs` is off) and `g_tPaintMetalness` R (`F_METALNESS_TEXTURE`, gunsmith; at the
//!   pattern's placement with `g_bMetalnessUsesPatternUVs`) in place of the per-mask values — the declarations say
//!   which placement.
//! - **Not baked**: the other styles (hydrographic, spray paint, the three anodized, patina), overlays, case
//!   hardening, patina age: their weapons keep their own finish (`unpainted_reason`).
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
use crate::texture::Image;

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

/// Bilinear, repeating samples of a decoded texture at UV coordinates.
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
        let (w, h) = (self.image.width, self.image.height);
        let x = (u.rem_euclid(1.0) * w as f32 - 0.5).rem_euclid(w as f32);
        let y = (v.rem_euclid(1.0) * h as f32 - 0.5).rem_euclid(h as f32);
        let (x0, y0) = (x.floor() as usize % w, y.floor() as usize % h);
        let (x1, y1) = ((x0 + 1) % w, (y0 + 1) % h);
        let (fx, fy) = (x.fract(), y.fract());
        let px = |x: usize, y: usize| &self.image.rgba[(y * w + x) * 4..][..4];
        std::array::from_fn(|c| {
            let top = f32::from(px(x0, y0)[c]) * (1.0 - fx) + f32::from(px(x1, y0)[c]) * fx;
            let bottom = f32::from(px(x0, y1)[c]) * (1.0 - fx) + f32::from(px(x1, y1)[c]) * fx;
            (top * (1.0 - fy) + bottom * fy) / 255.0
        })
    }
}

/// A texture placement as the shader's vertex stage builds it (`g_v*TexCoordXform0`, `1`): scale `s`, rotation `θ`
/// (degrees) and offset rounded to hundredths, then `u' = s·(cos θ·u − sin θ·v) + tu`,
/// `v' = s·(sin θ·u + cos θ·v) + tv`. The translation is the shader's own expression of the three: the offset
/// itself at 0° and 90°, the offset plus a rotation-dependent shift in between.
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

/// `values` chosen by paint masks `m` (R, G, B), as the shader mixes per-colour values:
/// `lerp(lerp(lerp(v0, v1, R), v2, G), v3, B)`.
fn by_mask(values: [f32; 4], m: [f32; 4]) -> f32 {
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(
        lerp(lerp(values[0], values[1], m[0]), values[2], m[1]),
        values[3],
        m[2],
    )
}

/// What the paint is at one texel of the weapon's layout, before its style's colour.
struct Texel {
    /// 1 where the paint is worn off (or never was), 0 where it is whole.
    gone: f32,
    /// The grunge multiplying the paint (RGB linear), and its alpha.
    grunge: [f32; 4],
    /// The paint masks (`g_tMasks` RGB).
    masks: [f32; 4],
}

/// What the bake reads from a job's parameters.
struct Inputs {
    style: i64,
    pattern: Option<Sampler>,
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
    wear_at: Placement,
    grunge_at: Placement,
    wear: f32,
    brightness: f32,
    /// The solid colours, linear.
    colors: [[f32; 3]; 4],
    /// Per mask: durability (`1 − g_vPaintDurability`), roughness, metalness.
    durability: [f32; 4],
    roughness: [f32; 4],
    metalness: [f32; 4],
    softness: f32,
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
        // An input that cannot be read is left out (no mask, no wear, no grunge…); only a pattern the style paints
        // is needed ([`unpainted_reason`] checked that one is named).
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
        let size_scale = if int("g_bIgnoreWeaponSizeScale", 0) != 0 {
            1.0
        } else {
            float("g_flUvScale1", 1.0)
        };
        let placement = |prefix: &str| {
            let offset = floats(&format!("g_v{prefix}TexCoordOffset"), [0.0; 4]);
            Placement::new(
                float(&format!("g_fl{prefix}TexCoordScale"), 1.0) * size_scale,
                float(&format!("g_fl{prefix}TexCoordRotation"), 0.0),
                [offset[0], offset[1]],
            )
        };
        // A texture without alpha decodes to 255 there: no no-paint mask, not paint nowhere.
        let occlusion = texture("g_tAmbientOcclusion")?;
        let masked = occlusion
            .as_ref()
            .is_some_and(|s| s.image.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 250));
        // The declarations' defaults: colours 0.5, 0.59, 0.38, 0.42 grey; roughness 0.6; metalness and durability 0.
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
        let roughness = if int("F_ROUGHNESS_PER_COLOR", 0) != 0 {
            floats("g_vPaintRoughness", [0.6; 4])
        } else {
            [float("g_flPaintRoughness", 0.6); 4]
        };
        Ok(Self {
            style,
            pattern: if style == SOLID {
                None
            } else {
                texture("g_tPattern")?
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
            paint_metalness: if style == GUNSMITH || int("F_METALNESS_TEXTURE", 0) != 0 {
                texture("g_tPaintMetalness")?
            } else {
                None
            },
            roughness_on_pattern: int("g_bRoughnessUsesPatternUVs", 1) != 0,
            metalness_on_pattern: int("g_bMetalnessUsesPatternUVs", 0) != 0,
            pattern_at: placement("Pattern"),
            wear_at: placement("Wear"),
            grunge_at: placement("Grunge"),
            wear: float("g_flWearAmount", 0.0).clamp(0.0, 1.0),
            brightness: float("g_flColorBrightness", 1.0),
            colors,
            durability: floats("g_vPaintDurability", [0.0; 4]).map(|d| 1.0 - d),
            roughness,
            metalness: floats("g_vPaintMetalness", [0.0; 4]),
            softness: float("g_fWearSoftness", 0.0),
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

    /// The paint at (u, v): how much is gone, the grunge and the masks (see the [module docs](self)).
    fn texel(&self, u: f32, v: f32) -> Texel {
        // Cavity R is read as sRGB, the no-paint mask A as stored; without the texture, the declarations'
        // defaults (cavity 0.5, paint everywhere).
        let (cavity, no_paint) = match &self.occlusion {
            Some(s) => {
                let o = s.at(u, v);
                let no_paint = if self.masked { o[3] } else { 0.0 };
                (SRGB_TO_LINEAR[(o[0] * 255.0).round() as usize], no_paint)
            }
            None => (srgb_to_linear(0.5), 0.0),
        };
        let masks = self.masks.as_ref().map_or([1.0; 4], |s| s.at(u, v));
        let wear = self.wear_texture.as_ref().map_or(0.0, |s| {
            let (wu, wv) = self.wear_at.apply(u, v);
            s.at(wu, wv)[0]
        });
        let durability = by_mask(self.durability, masks);
        let soft = self.softness * durability;
        let gone = smoothstep(
            0.58 - soft,
            0.68 + soft,
            (no_paint + wear * cavity) * (self.wear * 6.0 + 1.0) * durability,
        );
        let amount = (1.0 - cavity).powi(4) * 0.25 + 0.75 * self.wear;
        let grunge = self.grunge.as_ref().map_or([1.0; 4], |s| {
            let (gu, gv) = self.grunge_at.apply(u, v);
            let g = s.at(gu, gv);
            [
                SRGB_TO_LINEAR[(g[0] * 255.0).round() as usize],
                SRGB_TO_LINEAR[(g[1] * 255.0).round() as usize],
                SRGB_TO_LINEAR[(g[2] * 255.0).round() as usize],
                g[3],
            ]
        });
        Texel {
            gone,
            grunge: grunge.map(|g| 1.0 + (g - 1.0) * amount),
            masks,
        }
    }

    /// The paint's metalness at (u, v): its texture where it has one, else per mask.
    fn metalness_at(&self, u: f32, v: f32, masks: [f32; 4]) -> f32 {
        match &self.paint_metalness {
            Some(s) if self.metalness_on_pattern => {
                let (pu, pv) = self.pattern_at.apply(u, v);
                s.at(pu, pv)[0]
            }
            Some(s) => s.at(u, v)[0],
            None => by_mask(self.metalness, masks),
        }
    }
}

/// The paint styles this bake paints (`F_PAINT_STYLE`): solid, custom texture, gunsmith.
pub(crate) const PAINTED_STYLES: [i64; 3] = [SOLID, 6, GUNSMITH];
/// Solid: colours by mask, the decompiled default variant.
const SOLID: i64 = 0;
/// Gunsmith: the paint has a metalness texture.
const GUNSMITH: i64 = 8;

/// Why a weapon material is not painted with these parameters (a style or feature not baked yet, no pattern), or
/// `None`.
pub(crate) fn unpainted_reason(params: &BTreeMap<String, Param>) -> Option<String> {
    let int = |name: &str| match params.get(name) {
        Some(Param::Int(i)) => *i,
        _ => 0,
    };
    let style = int("F_PAINT_STYLE");
    if !PAINTED_STYLES.contains(&style) {
        return Some(format!("paint style {style} not baked yet"));
    }
    for (feature, what) in [
        ("F_OVERLAY_TEXTURE", "overlay"),
        ("F_CASE_HARDENING", "case hardening"),
        ("F_PATINA_AGE", "patina age"),
    ] {
        if int(feature) != 0 {
            return Some(format!("{what} not baked yet"));
        }
    }
    // A solid paint names its colours (a recipe without a paint material would otherwise paint the defaults' greys).
    if style == SOLID {
        let named = (0..4).any(|i| params.contains_key(&format!("g_vColor{i}")));
        return (!named).then(|| "no paint colours".into());
    }
    match params.get("g_tPattern") {
        Some(Param::Texture(path)) if !path.is_empty() => None,
        _ => Some("no pattern texture".into()),
    }
}

/// Solid paint's colour levels, mixed in by the wear: `lerp(painted, hue × lerp(min(floor, luminance), 1,
/// saturate(max(painted)^exponent)), wear)`, the hue `normalize(painted)` over its largest channel, `base` the colour
/// before grunge. The levels are `g_vPaintAlbedoLevels` (−0.045, 0.6, 1) and `g_vMetallicPaintAlbedoLevels`
/// (−0.08, 0.6, 1), mixed by the paint's metalness, as their expressions make them a floor (0.045 – 0.08), an
/// exponent (`−log2(1 − 0.6)`) and a ceiling (1).
fn albedo_levels(base: [f32; 3], painted: [f32; 3], metalness: f32, wear: f32) -> [f32; 3] {
    let floor = 0.045 + (0.08 - 0.045) * metalness;
    let exponent = -(1.0f32 - 0.6).log2();
    let largest = |c: [f32; 3]| c[0].max(c[1]).max(c[2]);
    let clamped = painted.map(|c| c.max(0.0003));
    let length = clamped.iter().map(|c| c * c).sum::<f32>().sqrt();
    let hue = clamped.map(|c| c / length);
    let luminance = base[0] * 0.2125 + base[1] * 0.7154 + base[2] * 0.0721;
    let low = floor.min(luminance);
    let level = low + (1.0 - low) * largest(painted).powf(exponent).clamp(0.0, 1.0);
    let top = largest(hue);
    std::array::from_fn(|i| painted[i] + (hue[i] * level / top - painted[i]) * wear)
}

fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
            let stored = inputs
                .base_color
                .as_ref()
                .map_or([0.5, 0.5, 0.5, 1.0], |s| s.at(u, v));
            let weapon = stored.map(|c| SRGB_TO_LINEAR[(c * 255.0).round() as usize]);
            let texel = inputs.texel(u, v);
            let paint: [f32; 3] = if inputs.style == SOLID {
                let base: [f32; 3] = std::array::from_fn(|c| {
                    by_mask(inputs.colors.map(|color| color[c]), texel.masks)
                });
                let painted = std::array::from_fn(|c| base[c] * texel.grunge[c]);
                let metalness = inputs.metalness_at(u, v, texel.masks);
                albedo_levels(base, painted, metalness, inputs.wear)
            } else {
                let (pu, pv) = inputs.pattern_at.apply(u, v);
                let pattern = inputs.pattern.as_ref().map_or([1.0; 4], |s| s.at(pu, pv));
                std::array::from_fn(|c| {
                    SRGB_TO_LINEAR[(pattern[c] * 255.0).round() as usize]
                        * inputs.brightness
                        * texel.grunge[c]
                })
            };
            for c in 0..3 {
                rgba.push(linear_to_srgb(
                    paint[c] + (weapon[c] - paint[c]) * texel.gone,
                ));
            }
            rgba.push((stored[3] * 255.0).round() as u8);
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
            let base = inputs
                .base_surface
                .as_ref()
                .map_or([1.0, 0.0, 0.0, 0.0], |s| s.at(u, v));
            let texel = inputs.texel(u, v);
            let roughness = match &inputs.paint_roughness {
                Some(s) if inputs.roughness_on_pattern => {
                    let (pu, pv) = inputs.pattern_at.apply(u, v);
                    s.at(pu, pv)[0]
                }
                Some(s) => s.at(u, v)[0],
                None => by_mask(inputs.roughness, texel.masks),
            };
            let roughness =
                (roughness + (1.0 - texel.grunge[3]) * inputs.wear * inputs.wear * 0.5).min(1.0);
            let metalness = inputs.metalness_at(u, v, texel.masks);
            let mix = |paint: f32, base: f32| {
                ((paint + (base - paint) * texel.gone) * 255.0).round() as u8
            };
            rgba.extend_from_slice(&[255, mix(roughness, base[0]), mix(metalness, base[1]), 255]);
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
    fn samples_wrap_and_blend() {
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
    }

    fn pixel(rgba: [u8; 4]) -> Option<Sampler> {
        Some(Sampler {
            image: Image::new(1, 1, rgba.to_vec()),
        })
    }

    /// A one-texel weapon: wear amount `wear`, wear texture `wear_texel`, cavity `cavity` (sRGB byte) and no-paint
    /// mask `no_paint`.
    fn inputs(wear: f32, wear_texel: u8, cavity: u8, no_paint: u8) -> Inputs {
        let at = Placement::new(1.0, 0.0, [0.0; 2]);
        Inputs {
            style: SOLID,
            pattern: None,
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
            wear_at: at,
            grunge_at: at,
            wear,
            brightness: 1.0,
            colors: [[0.5; 3]; 4],
            durability: [1.0; 4],
            roughness: [0.6; 4],
            metalness: [0.0; 4],
            softness: 0.0,
        }
    }

    #[test]
    fn paint_goes_where_wear_texture_and_cavity_are_bright() {
        let gone = |inputs: Inputs| inputs.texel(0.5, 0.5).gone;
        // (wear texture × cavity) × (6 × wear + 1) past 0.58 – 0.68: a white wear texel on the brightest edge loses
        // its paint even factory new, a mid-grey one only once worn.
        assert_eq!(gone(inputs(0.0, 255, 255, 0)), 1.0);
        assert_eq!(gone(inputs(0.0, 128, 255, 0)), 0.0);
        assert_eq!(gone(inputs(0.1, 128, 255, 0)), 1.0);
        // The same wear keeps the paint in a recess (cavity 128, about 0.22 linear).
        assert_eq!(gone(inputs(0.1, 128, 128, 0)), 0.0);
        // The no-paint mask has none, worn or not.
        assert_eq!(gone(inputs(0.0, 0, 0, 255)), 1.0);
        // A fully durable paint (g_vPaintDurability 1) never wears.
        let mut durable = inputs(1.0, 255, 255, 0);
        durable.durability = [0.0; 4];
        assert_eq!(gone(durable), 0.0);
    }

    #[test]
    fn grunge_grows_with_wear_and_in_recesses() {
        let grunge = |wear: f32, cavity: u8| {
            let mut inputs = inputs(wear, 0, cavity, 0);
            inputs.grunge = pixel([0, 0, 0, 0]);
            inputs.texel(0.5, 0.5).grunge
        };
        // On an edge (cavity 1) none when new, 3/4 of it fully worn; in a recess (cavity 0) 1/4 already.
        assert_eq!(grunge(0.0, 255), [1.0; 4]);
        assert!((grunge(1.0, 255)[0] - 0.25).abs() < 1e-6);
        assert!((grunge(0.0, 0)[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn solid_colours_take_their_levels_with_wear() {
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
        // The masks choose the colour: R picks colour 1 over 0, then G colour 2, then B colour 3.
        assert_eq!(by_mask([0.0, 1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0]), 0.0);
        assert_eq!(by_mask([0.0, 1.0, 2.0, 3.0], [1.0, 0.0, 0.0, 1.0]), 1.0);
        assert_eq!(by_mask([0.0, 1.0, 2.0, 3.0], [1.0, 1.0, 1.0, 1.0]), 3.0);
    }

    #[test]
    fn only_baked_styles_and_features_are_painted() {
        let params = |style: i64, pattern: Option<&str>| {
            let mut params = BTreeMap::from([("F_PAINT_STYLE".to_owned(), Param::Int(style))]);
            if let Some(path) = pattern {
                params.insert("g_tPattern".into(), Param::Texture(path.into()));
            }
            params
        };
        assert_eq!(unpainted_reason(&params(6, Some("fireserpent.vtex"))), None);
        assert_eq!(unpainted_reason(&params(8, Some("gungnir.vtex"))), None);
        // Solid needs its colours but no pattern (Whiteout); with an overlay (Royal Guard) it waits.
        let mut solid = params(0, None);
        solid.insert("g_vColor0".into(), Param::Floats([0.86, 0.85, 0.83, 1.0]));
        assert_eq!(unpainted_reason(&solid), None);
        assert!(unpainted_reason(&BTreeMap::new()).is_some());
        let mut overlay = solid.clone();
        overlay.insert("F_OVERLAY_TEXTURE".into(), Param::Int(1));
        assert!(unpainted_reason(&overlay).is_some());
        // Fade's gradient is a mask for its colours, as hydrographic patterns are: not baked yet.
        assert!(unpainted_reason(&params(5, Some("fade.vtex"))).is_some());
        assert!(unpainted_reason(&params(1, Some("webs.vtex"))).is_some());
        // A custom paint job without a pattern has nothing.
        assert!(unpainted_reason(&params(6, Some(""))).is_some());
        assert!(unpainted_reason(&params(6, None)).is_some());
    }
}
