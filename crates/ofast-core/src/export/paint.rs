//! Skins baked onto weapon textures (M10): a weapon material's colour and metal-roughness, painted with a skin's
//! parameters ([`crate::paint`]), as CS2's composite materials generate them into the weapon's own textures
//! (`docs/research/cs2-skins.md`).
//!
//! # OFAST's own version (first pass)
//!
//! CS2 renders the paint material (`csgo_customweapon.vfx`) over the weapon's UV layout; every input is a texture in
//! that layout (the weapon's composite inputs) or one placed on it by a transform. This bake does the same per
//! texel, with OFAST's own reading of each input (decision 024), to be checked against CS2's weapons:
//!
//! - **Pattern** (`g_tPattern`) at `uv × scale`, rotated by `g_flPatternTexCoordRotation` degrees about the
//!   texture's centre, moved by `g_vPatternTexCoordOffset`, repeating. The scale is
//!   `g_flPatternTexCoordScale`, times the weapon's `g_flUvScale1` unless `g_bIgnoreWeaponSizeScale` (custom paint
//!   jobs paint the UV layout itself).
//! - **Where paint goes**: everywhere but the composite inputs' no-paint mask (`g_tAmbientOcclusion` A: sights,
//!   glass), which keeps the weapon's own surface. A texture stored without alpha (all 255 there) has no mask.
//! - **Wear**: the wear texture (`g_tWear` R, placed by the seed's roll like the pattern) is the paint's
//!   resistance; where the wear amount passes it the paint is gone, sooner on edges (the cavity map,
//!   `g_tAmbientOcclusion` R) — the weapon's own colour and metal show there. The grunge texture (`g_tGrunge`,
//!   placed by the roll) darkens what is left as the wear grows.
//! - **Styles** (`PAINTED_STYLES`, `F_PAINT_STYLE`): custom texture (6) and gunsmith (8) paint the pattern's
//!   colour. The others (solid, hydrographic, spray paint, the anodized ones, patina) build their colour from
//!   masks and colour maps (`g_vColor0` – `3`; Fade's pattern is a red–green–blue gradient its colours replace):
//!   not read yet, so their weapons keep their own finish (`unpainted_reason`).
//! - **Surface**: paint roughness `g_flPaintRoughness` (or `g_tPaintRoughness` R), metalness 0
//!   (`g_tPaintMetalness` R for gunsmith); where the paint is gone, the weapon's own `g_tMetalness` (R roughness,
//!   G metalness, as `gltf` reads a `csgo_weapon` material).
//! - **Size**: the weapon's colour texture's or the pattern's, the larger, 2048 at most; the alpha is the weapon
//!   colour's (alpha-tested parts keep their cut-outs).
//!
//! Each colour and surface job decodes its inputs on its own (the weapon's composite inputs and the shared wear and
//! grunge textures again for each skin): a round of about 30 skins exports in about a minute and a quarter.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use crate::content::Content;
use crate::export::ExportError;
use crate::export::environment::{SRGB_TO_LINEAR, linear_to_srgb};
use crate::export::gltf::decode_raw;
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

/// A texture placement: `uv × scale`, rotated by `rotation` degrees about (0.5, 0.5), moved by `offset`.
#[derive(Debug, Clone, Copy)]
struct Placement {
    scale: f32,
    sin: f32,
    cos: f32,
    offset: [f32; 2],
}

impl Placement {
    fn new(scale: f32, rotation: f32, offset: [f32; 2]) -> Self {
        let (sin, cos) = rotation.to_radians().sin_cos();
        Self {
            scale,
            sin,
            cos,
            offset,
        }
    }

    fn apply(&self, u: f32, v: f32) -> (f32, f32) {
        let (x, y) = (u * self.scale - 0.5, v * self.scale - 0.5);
        (
            x * self.cos - y * self.sin + 0.5 + self.offset[0],
            x * self.sin + y * self.cos + 0.5 + self.offset[1],
        )
    }
}

/// What the bake reads from a job's parameters.
struct Inputs {
    pattern: Option<Sampler>,
    base_color: Option<Sampler>,
    base_surface: Option<Sampler>,
    occlusion: Option<Sampler>,
    /// The occlusion's alpha holds a no-paint mask (some texel below 250).
    masked: bool,
    wear_texture: Option<Sampler>,
    grunge: Option<Sampler>,
    paint_roughness: Option<Sampler>,
    paint_metalness: Option<Sampler>,
    pattern_at: Placement,
    wear_at: Placement,
    grunge_at: Placement,
    wear: f32,
    brightness: f32,
    roughness: f32,
}

impl Inputs {
    fn read(content: &Content, params: &BTreeMap<String, Param>) -> Result<Self, ExportError> {
        let float = |name: &str, default: f32| match params.get(name) {
            Some(Param::Floats(v)) => v[0],
            Some(Param::Int(i)) => *i as f32,
            _ => default,
        };
        let pair = |name: &str| match params.get(name) {
            Some(Param::Floats(v)) => [v[0], v[1]],
            _ => [0.0; 2],
        };
        let int = |name: &str| match params.get(name) {
            Some(Param::Int(i)) => *i,
            _ => 0,
        };
        // An input that cannot be read is left out (no mask, no wear, no grunge…); only the pattern is needed
        // ([`unpainted_reason`] checked that one is named).
        let texture = |name: &str| -> Result<Option<Sampler>, ExportError> {
            match params.get(name) {
                Some(Param::Texture(path)) if name == "g_tPattern" => {
                    Sampler::load(content, path).map(Some)
                }
                Some(Param::Texture(path)) => Ok(Sampler::load(content, path).ok()),
                _ => Ok(None),
            }
        };
        let style = int("F_PAINT_STYLE");
        let size_scale = if int("g_bIgnoreWeaponSizeScale") != 0 {
            1.0
        } else {
            float("g_flUvScale1", 1.0)
        };
        let placement = |prefix: &str, scale: f32| {
            Placement::new(
                scale,
                float(&format!("g_fl{prefix}TexCoordRotation"), 0.0),
                pair(&format!("g_v{prefix}TexCoordOffset")),
            )
        };
        // A texture without alpha decodes to 255 there: no no-paint mask, not paint nowhere.
        let occlusion = texture("g_tAmbientOcclusion")?;
        let masked = occlusion
            .as_ref()
            .is_some_and(|s| s.image.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 250));
        Ok(Self {
            pattern: texture("g_tPattern")?,
            base_color: texture("g_tColor")?,
            base_surface: texture("g_tMetalness")?,
            occlusion,
            masked,
            wear_texture: texture("g_tWear")?,
            grunge: texture("g_tGrunge")?,
            paint_roughness: if int("F_ROUGHNESS_TEXTURE") != 0 {
                texture("g_tPaintRoughness")?
            } else {
                None
            },
            paint_metalness: if style == GUNSMITH || int("F_METALNESS_TEXTURE") != 0 {
                texture("g_tPaintMetalness")?
            } else {
                None
            },
            pattern_at: placement(
                "Pattern",
                float("g_flPatternTexCoordScale", 1.0) * size_scale,
            ),
            wear_at: placement("Wear", float("g_flWearTexCoordScale", 1.0)),
            grunge_at: placement("Grunge", float("g_flGrungeTexCoordScale", 1.0)),
            wear: float("g_flWearAmount", 0.0).clamp(0.0, 1.0),
            brightness: float("g_flColorBrightness", 1.0),
            roughness: float("g_flPaintRoughness", 0.5),
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

    /// How much paint is left at (u, v), 0 – 1.
    fn paint_left(&self, u: f32, v: f32) -> f32 {
        let occlusion = self.occlusion.as_ref().map_or([0.0; 4], |s| s.at(u, v));
        let no_paint = if self.masked { occlusion[3] } else { 0.0 };
        let worn = match &self.wear_texture {
            Some(wear) => {
                let (wu, wv) = self.wear_at.apply(u, v);
                let resistance = wear.at(wu, wv)[0];
                // Edges (bright in the cavity map) give way at half the wear. The paint goes as the wear passes
                // the resistance: none at all at wear 0.
                let threshold = self.wear * (1.0 + occlusion[0]);
                1.0 - smoothstep(resistance, resistance + WEAR_SOFTNESS, threshold)
            }
            None => 1.0,
        };
        worn * (1.0 - no_paint)
    }
}

/// How far past the paint's resistance the wear goes before it is all gone (a soft edge to worn patches).
const WEAR_SOFTNESS: f32 = 0.06;

/// The paint styles this bake paints (`F_PAINT_STYLE`): custom texture, gunsmith.
pub(crate) const PAINTED_STYLES: [i64; 2] = [6, GUNSMITH];
/// Gunsmith: the paint has a metalness texture.
const GUNSMITH: i64 = 8;

/// Why a weapon material is not painted with these parameters (a style not baked yet, no pattern), or `None`.
pub(crate) fn unpainted_reason(params: &BTreeMap<String, Param>) -> Option<String> {
    let style = match params.get("F_PAINT_STYLE") {
        Some(Param::Int(style)) => *style,
        _ => 0,
    };
    if !PAINTED_STYLES.contains(&style) {
        return Some(format!("paint style {style} not baked yet"));
    }
    match params.get("g_tPattern") {
        Some(Param::Texture(path)) if !path.is_empty() => None,
        _ => Some("no pattern texture".into()),
    }
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
            let base = stored.map(|c| SRGB_TO_LINEAR[(c * 255.0).round() as usize]);
            let (pu, pv) = inputs.pattern_at.apply(u, v);
            let pattern = inputs.pattern.as_ref().map_or([1.0; 4], |s| s.at(pu, pv));
            let mut paint: [f32; 3] = std::array::from_fn(|c| {
                SRGB_TO_LINEAR[(pattern[c] * 255.0).round() as usize] * inputs.brightness
            });
            if let Some(grunge) = &inputs.grunge {
                let (gu, gv) = inputs.grunge_at.apply(u, v);
                let g = grunge.at(gu, gv);
                for (c, value) in paint.iter_mut().enumerate() {
                    let dirt = SRGB_TO_LINEAR[(g[c] * 255.0).round() as usize];
                    *value *= 1.0 + (dirt - 1.0) * inputs.wear;
                }
            }
            let left = inputs.paint_left(u, v);
            for c in 0..3 {
                rgba.push(linear_to_srgb(base[c] + (paint[c] - base[c]) * left));
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
            let (pu, pv) = inputs.pattern_at.apply(u, v);
            let roughness = inputs
                .paint_roughness
                .as_ref()
                .map_or(inputs.roughness, |s| s.at(pu, pv)[0]);
            let metalness = inputs
                .paint_metalness
                .as_ref()
                .map_or(0.0, |s| s.at(pu, pv)[0]);
            let left = inputs.paint_left(u, v);
            let mix =
                |base: f32, paint: f32| ((base + (paint - base) * left) * 255.0).round() as u8;
            rgba.extend_from_slice(&[255, mix(base[0], roughness), mix(base[1], metalness), 255]);
        }
    }
    Ok(Image::new(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placements_scale_rotate_about_the_centre_and_move() {
        let (u, v) = Placement::new(1.0, 0.0, [0.0; 2]).apply(0.25, 0.75);
        assert!((u - 0.25).abs() < 1e-6 && (v - 0.75).abs() < 1e-6);
        // A quarter turn about the centre: (0.75, 0.5) → (0.5, 0.75).
        let (u, v) = Placement::new(1.0, 90.0, [0.0; 2]).apply(0.75, 0.5);
        assert!((u - 0.5).abs() < 1e-6 && (v - 0.75).abs() < 1e-6, "{u} {v}");
        let (u, v) = Placement::new(2.0, 0.0, [0.1, 0.2]).apply(0.5, 0.5);
        assert!((u - 1.1).abs() < 1e-6 && (v - 1.2).abs() < 1e-6);
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

    #[test]
    fn wear_takes_the_paint_where_resistance_is_low_and_on_edges_first() {
        let inputs = |wear: f32, resistance: u8, edge: u8| Inputs {
            pattern: None,
            base_color: None,
            base_surface: None,
            occlusion: Some(Sampler {
                image: Image::new(1, 1, vec![edge, 255, 0, 0]),
            }),
            masked: true,
            wear_texture: Some(Sampler {
                image: Image::new(1, 1, vec![resistance, 0, 0, 255]),
            }),
            grunge: None,
            paint_roughness: None,
            paint_metalness: None,
            pattern_at: Placement::new(1.0, 0.0, [0.0; 2]),
            wear_at: Placement::new(1.0, 0.0, [0.0; 2]),
            grunge_at: Placement::new(1.0, 0.0, [0.0; 2]),
            wear,
            brightness: 1.0,
            roughness: 0.4,
        };
        assert_eq!(inputs(0.0, 128, 0).paint_left(0.5, 0.5), 1.0);
        assert_eq!(inputs(0.8, 128, 0).paint_left(0.5, 0.5), 0.0);
        // The same wear keeps paint inside and takes it off an edge.
        assert_eq!(inputs(0.3, 128, 0).paint_left(0.5, 0.5), 1.0);
        assert_eq!(inputs(0.3, 128, 255).paint_left(0.5, 0.5), 0.0);
        // Factory new keeps all its paint, even where the wear texture is black.
        assert_eq!(inputs(0.0, 0, 255).paint_left(0.5, 0.5), 1.0);
    }

    #[test]
    fn only_baked_styles_with_a_pattern_are_painted() {
        let params = |style: i64, pattern: Option<&str>| {
            let mut params = BTreeMap::from([("F_PAINT_STYLE".to_owned(), Param::Int(style))]);
            if let Some(path) = pattern {
                params.insert("g_tPattern".into(), Param::Texture(path.into()));
            }
            params
        };
        assert_eq!(unpainted_reason(&params(6, Some("fireserpent.vtex"))), None);
        assert_eq!(unpainted_reason(&params(8, Some("gungnir.vtex"))), None);
        // Fade's gradient is a mask for its colour maps, as hydrographic patterns are.
        assert!(unpainted_reason(&params(5, Some("fade.vtex"))).is_some());
        // Hydrographic (Crimson Web) too; a custom paint job without a pattern has nothing.
        assert!(unpainted_reason(&params(1, Some("webs.vtex"))).is_some());
        assert!(unpainted_reason(&params(6, Some(""))).is_some());
        assert!(unpainted_reason(&params(6, None)).is_some());
    }
}
