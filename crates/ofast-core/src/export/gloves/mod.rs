//! Gloves painted (M10): a glove model's colour and metallic-roughness, painted with a glove skin as CS2's glove
//! composite generates them into the glove's own textures (`docs/research/cs2-skins.md`, "Gloves").
//!
//! # CS2's glove shader, as OFAST's own Rust
//!
//! The glove composite renders its paint material (`csgo_customglove.vfx`) over the glove's UV layout, at 2048, once
//! per output (`F_OUTPUT_MODE`: colour, normal with roughness, metalness and cloth, occlusion, gloss). This bake
//! reads the colour, roughness and metalness of the default static combo ("Source1 texture layout", every glove kit
//! but one), decompiled as the weapons' paint shader was (decision 024: Source 2 Viewer 20.1, SPIRV-Cross; the
//! placements from its reconstructed expressions), written at 1024 (`GLOVE_SIZE`). Per texel:
//!
//! - **Layers**: `g_tLayerMask` RGB picks one of four materials (`lerp(lerp(lerp(v1, v2, R), v3, G), v4, B)`), each
//!   its detail (`g_tDetail*`, black point lifted), grunge (`g_tGrunge*`, sRGB colour) and values; its A is an index
//!   into eight tints (`g_vColorTint*`, sRGB), blurred over 9 taps (`g_fColorMaskBlur`).
//! - **Pattern** (`F_PATTERN`, placed with the rolled offset and rotation, mirrored for the left hand by
//!   `g_bFlipFixup`, nudged by the detail): over the tint the mask index `g_nPatternReplaceIndex` names (modes 0 and
//!   2) or everywhere (1, 3); modes 0 and 1 paint a palette (`g_vPatternPaletteIndices`) with a thickness that wears
//!   and lifts the durability, 2 and 3 the pattern's own colours.
//! - **Damage**: where curvature (`g_tSurface` R, to a power) × durability (its A) × the grunge's alpha, plus a
//!   curvature boost, passes the layer's `g_vDamageLevels` with the wear; the pattern wears faster where the detail's
//!   B is. Edges brighten by the screen-space derivatives of detail × curvature (here: the next texel at 2048).
//! - **Colour**: the albedo brought to the detail's light, grunge and bleaching (wear and damage), grime
//!   re-saturated, damage brightness and saturation, the grunge's max where the surface is open, hue shifts by
//!   luminance and noise, then the textile albedo levels by wear.
//! - **Surface**: roughness from the detail's, the pattern's and the grunge's, damaged and at damage edges (where the
//!   grunge's alpha changes around the texel); metalness from the detail and the pattern, damaged and at the edges.
//!
//! **Not baked**: the normal, occlusion and cloth outputs (the glove keeps its own), anisotropic gloss in this
//! layout (one kit); the mip-biased pattern sample that shades thick paint's edges is read unbiased.
//! `g_nRandomSeedAlt` (the second hand's seed) is the pattern seed until checked against CS2.
//!
//! The "Source2 height blending" program (`F_BACKWARDS_COMPATIBILITY`, 2025's kits) is [`blended`].

mod blended;

use std::collections::BTreeMap;

use rayon::prelude::*;

use super::ExportError;
use super::gltf::srgb_to_linear;
use super::paint::{
    Placement, Sampler, clamp3, lerp, lerp3, luminance, mul3, pick, rgb, scale3, smoothstep,
};
use crate::content::Content;
use crate::export::environment::linear_to_srgb;
use crate::paint::Param;
use crate::texture::Image;

/// The size the glove composite renders at: the derivatives are taken between its texels.
const COMPOSITE_SIZE: f32 = 2048.0;

/// The size a glove's painted textures are written at: half the composite's. Gloves are small on screen, and a
/// round's ten glove skins at 2048, two hands, colour and surface, in the players' and the viewmodels' files, took
/// Unreal past its video memory (23.6 GB of 9.5 on Aurora vs FURIA's round 17).
const GLOVE_SIZE: usize = 1024;

/// The 9 taps the colour mask's index is blurred over: offset (in 1/1024 × `g_fColorMaskBlur`) and weight.
const MASK_TAPS: [[f32; 3]; 9] = [
    [0.0, 0.0, 0.319_727_9],
    [-1.0, -1.0, 0.051_020_406],
    [-1.0, 0.0, 0.119_047_62],
    [-1.0, 1.0, 0.051_020_406],
    [0.0, -1.0, 0.119_047_62],
    [0.0, 1.0, 0.119_047_62],
    [1.0, -1.0, 0.051_020_406],
    [1.0, 0.0, 0.119_047_62],
    [1.0, 1.0, 0.051_020_406],
];

/// The 4 directions the damage edge is looked for in.
const EDGE_TAPS: [[f32; 2]; 4] = [[-1.0, -1.0], [-1.0, 1.0], [1.0, 1.0], [1.0, -1.0]];

/// `g_vTextileAlbedoLevels` and `g_vMetallicTextileAlbedoLevels` as their expressions make them from (−0.045, 0.6,
/// 1) and (−0.08, 0.6, 1.08): floor, exponent (`−log2(0.4)`), ceiling.
const TEXTILE_LEVELS: [f32; 3] = [0.045, 1.321_928, 1.0];
const METALLIC_TEXTILE_LEVELS: [f32; 3] = [0.08, 1.321_928, 0.92];

/// The tints a glove paint leaves out take the declarations' defaults (sRGB).
const DEFAULT_TINTS: [[f32; 3]; 8] = [
    [0.15, 0.11, 0.08],
    [0.23, 0.16, 0.26],
    [0.16, 0.16, 0.28],
    [0.2, 0.3, 0.3],
    [0.09, 0.16, 0.1],
    [0.29, 0.26, 0.11],
    [0.32, 0.22, 0.12],
    [0.21, 0.12, 0.09],
];

/// A layer's values, one per material (`…1` – `…4`).
type PerLayer = [f32; 4];

/// What the glove bake reads from a skin's parameters.
struct Inputs {
    layer_mask: Option<Sampler>,
    surface: Option<Sampler>,
    noise: Option<Sampler>,
    pattern: Option<Sampler>,
    details: [Option<Sampler>; 4],
    grunges: [Option<Sampler>; 4],
    /// `g_fDetailBlackPoint*` (the compensation is `1 −` it).
    black_point: PerLayer,
    detail_scale: PerLayer,
    detail_metalness: PerLayer,
    detail_rough_contrast: PerLayer,
    detail_rough_brightness: PerLayer,
    detail_grunge: PerLayer,
    curvature_power: PerLayer,
    curvature_wear_boost: PerLayer,
    damage_levels: [PerLayer; 2],
    damage_saturation: PerLayer,
    damage_brightness: PerLayer,
    damage_bleaching: PerLayer,
    damage_metalness: PerLayer,
    damage_edge_metalness: PerLayer,
    damage_edge_roughness: PerLayer,
    damage_edge_width: PerLayer,
    damage_rough_contrast: PerLayer,
    damage_rough_brightness: PerLayer,
    wear_bleaching: PerLayer,
    grime_saturation: PerLayer,
    grime_brightness: PerLayer,
    grunge_max: PerLayer,
    grunge_rough_brightness: PerLayer,
    /// `g_fFlipFixup`: −1 for the mirrored hand.
    flip: f32,
    patterned: bool,
    pattern_mode: i64,
    /// The mask region the pattern paints over (modes 0 and 2): `g_nPatternReplaceIndex` − 1, its declaration's
    /// expression (`this-1`). Authored 1 – 8: Crimson Kimono's 1 paints region 0 (the back of the hand; region 1, its
    /// knuckle panel, keeps its tint), as CS2 shows it side by side (Aurora vs FURIA, round 17).
    replace_index: usize,
    paint_thickness: f32,
    paint_durability: f32,
    pattern_detail_influence: f32,
    pattern_metalness: f32,
    pattern_rough_brightness: f32,
    color_mask_blur: f32,
    /// The eight tints, as authored (sRGB).
    tints: [[f32; 3]; 8],
    /// `g_vPatternPaletteIndices`: the tints (1 – 8) the pattern's R, G, B pick over the first.
    palette: [usize; 4],
    wear: f32,
    pattern_at: Placement,
    grunge_at: Placement,
}

/// What the colour and surface both need at a texel, and the edges' derivatives read again at its neighbours.
struct Local {
    /// The layer weights (mask RGB) and the surface texture (curvature, …, durability).
    layers: [f32; 3],
    surface: [f32; 4],
    curvature: f32,
    /// The detail: R and G lifted by layer 1's black point again, as the shader does; B, A as they are.
    detail: [f32; 4],
    /// The tint weights by mask index.
    tint_weights: [f32; 8],
    pattern: Patterned,
}

/// The pattern's part at a texel.
struct Patterned {
    metalness: f32,
    durability: f32,
    /// The detail's G under the paint (`lerp(G, 0.5, paint)`).
    light: f32,
    /// How much of the texel the pattern covers.
    covered: f32,
    thickness: f32,
    /// The pattern's colour (sRGB) and roughness.
    color: [f32; 3],
    roughness: f32,
}

/// A texel painted: colour (linear), roughness, metalness.
struct Shade {
    color: [f32; 3],
    roughness: f32,
    metalness: f32,
}

/// The layer weights of `lerp(lerp(lerp(v1, v2, R), v3, G), v4, B)`.
fn layer_weights([r, g, b]: [f32; 3]) -> [f32; 4] {
    [
        (1.0 - r) * (1.0 - g) * (1.0 - b),
        r * (1.0 - g) * (1.0 - b),
        g * (1.0 - b),
        b,
    ]
}

/// The shader's saturation adjust of `c`: by `amount` above 0 towards a colour pushed off grey along `hue`'s
/// direction at luminance `lum`; below 0 towards `c`'s grey.
fn resaturate(c: [f32; 3], hue: [f32; 3], lum: f32, amount: f32) -> [f32; 3] {
    let normalize = |v: [f32; 3]| {
        let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        (length > 1e-12).then(|| v.map(|x| x / length))
    };
    if amount <= 0.0 {
        return lerp3(c, [luminance(c); 3], -amount);
    }
    let Some(direction) = normalize(hue).map(|h| h.map(|x| x - 0.577)) else {
        return c;
    };
    let Some(pushed) = normalize(direction) else {
        return c;
    };
    let pushed = clamp3(pushed.map(|x| x * 2.0 + 1.0));
    let weight = pushed[0] * 0.2125 + pushed[1] * 0.7154 + pushed[2] * 0.0721;
    let along = direction.iter().map(|x| x * 0.577).sum::<f32>().abs();
    lerp3(
        c,
        scale3(pushed, lum / weight.max(1e-6)),
        (amount * along.powf(0.2)).clamp(0.0, 1.0),
    )
}

impl Inputs {
    fn read(content: &Content, params: &BTreeMap<String, Param>) -> Result<Self, ExportError> {
        let floats = |name: &str| match params.get(name) {
            Some(Param::Floats(v)) => Some(*v),
            Some(Param::Int(i)) => Some([*i as f32, 0.0, 0.0, 0.0]),
            _ => None,
        };
        let float = |name: &str, default: f32| floats(name).map_or(default, |v| v[0]);
        let int = |name: &str, default: i64| match params.get(name) {
            Some(Param::Int(i)) => *i,
            Some(Param::Floats(v)) => v[0] as i64,
            _ => default,
        };
        let layer = |name: &str, default: f32| -> PerLayer {
            std::array::from_fn(|i| float(&format!("{name}{}", i + 1), default))
        };
        // An input that cannot be read is left out (the declarations' defaults stand in).
        let texture = |name: &str| match params.get(name) {
            Some(Param::Texture(path)) => Sampler::load(content, path).ok(),
            _ => None,
        };
        let levels: [[f32; 4]; 4] = std::array::from_fn(|i| {
            floats(&format!("g_vDamageLevels{}", i + 1)).unwrap_or([0.0; 4])
        });
        let offset = |name: &str| {
            let v = floats(name).unwrap_or([0.0; 4]);
            [v[0], v[1]]
        };
        let flipped = int("g_bFlipFixup", 0) != 0;
        let rotation = float("g_fPatternTexCoordRotation", 0.0);
        let palette = floats("g_vPatternPaletteIndices").unwrap_or([5.0, 6.0, 7.0, 8.0]);
        Ok(Self {
            layer_mask: texture("g_tLayerMask"),
            surface: texture("g_tSurface"),
            noise: texture("g_tNoise"),
            pattern: texture("g_tPattern"),
            details: std::array::from_fn(|i| texture(&format!("g_tDetail{}", i + 1))),
            grunges: std::array::from_fn(|i| texture(&format!("g_tGrunge{}", i + 1))),
            black_point: layer("g_fDetailBlackPoint", 0.047),
            detail_scale: layer("g_fDetailScale", 4.0),
            detail_metalness: layer("g_fDetailMetalness", 0.0),
            detail_rough_contrast: layer("g_fDetailRoughnessContrast", 0.0),
            detail_rough_brightness: layer("g_fDetailRoughnessBrightness", 1.0),
            detail_grunge: layer("g_fDetailGrunge", 0.0),
            curvature_power: layer("g_fCurvaturePower", 1.0),
            curvature_wear_boost: layer("g_fCurvatureWearBoost", 0.0),
            damage_levels: [
                std::array::from_fn(|i| levels[i][0]),
                std::array::from_fn(|i| levels[i][1]),
            ],
            damage_saturation: layer("g_fDamageSaturation", 0.0),
            damage_brightness: layer("g_fDamageBrightness", 0.0),
            damage_bleaching: layer("g_fDamageBleaching", 0.0),
            damage_metalness: layer("g_fDamageMetalness", 0.0),
            damage_edge_metalness: layer("g_fDamageEdgeMetalness", 0.0),
            damage_edge_roughness: layer("g_fDamageEdgeRoughness", 0.8),
            damage_edge_width: layer("g_fDamageNormalEdgeWidth", 1.0),
            damage_rough_contrast: layer("g_fDamageRoughnessContrast", 0.0),
            damage_rough_brightness: layer("g_fDamageRoughnessBrightness", 1.0),
            wear_bleaching: layer("g_fWearBleaching", 0.25),
            grime_saturation: layer("g_fGrimeSaturation", 0.0),
            grime_brightness: layer("g_fGrimeBrightness", 0.0),
            grunge_max: layer("g_fGrungeMax", 1.0),
            grunge_rough_brightness: layer("g_fGrungeRoughnessBrightness", 0.8),
            flip: if flipped { -1.0 } else { 1.0 },
            patterned: int("F_PATTERN", 0) != 0,
            pattern_mode: int("g_nPatternMode", 0),
            replace_index: (int("g_nPatternReplaceIndex", 1) - 1).clamp(0, 7) as usize,
            paint_thickness: float("g_fPaintThickness", 1.0),
            paint_durability: float("g_fPaintDurability", 0.5),
            pattern_detail_influence: float("g_fPatternDetailInfluence", 0.0),
            pattern_metalness: float("g_fPatternMetalness", 0.0),
            pattern_rough_brightness: float("g_fPatternRoughnessBrightness", 1.0),
            color_mask_blur: float("g_fColorMaskBlur", 2.0),
            tints: std::array::from_fn(|i| {
                let [r, g, b] = DEFAULT_TINTS[i];
                let v = floats(&format!("g_vColorTint{}", i + 1)).unwrap_or([r, g, b, 1.0]);
                [v[0], v[1], v[2]]
            }),
            palette: palette.map(|i| (i.round() as i64).clamp(1, 8) as usize - 1),
            // The colour pass copies the wear amount into the wear progress.
            wear: float("g_fWearProgress", float("g_flWearAmount", 0.0)).clamp(0.0, 1.0),
            pattern_at: Placement::exact(
                float("g_fPatternTexCoordScale", 2.5),
                if flipped { -rotation } else { rotation },
                offset("g_fPatternTexCoordOffset"),
            ),
            grunge_at: Placement::exact(
                float("g_fGrungeTexCoordScale", 2.5),
                float("g_fGrungeTexCoordRotation", 0.0),
                offset("g_fGrungeTexCoordOffset"),
            ),
        })
    }

    /// The grunge blended at (u, v): RGB sRGB-decoded, A as stored.
    fn grunge(&self, weights: [f32; 4], u: f32, v: f32) -> [f32; 4] {
        let mut out = [0.0; 4];
        for (texture, weight) in self.grunges.iter().zip(weights) {
            if weight <= 0.0 {
                continue;
            }
            let s = texture.as_ref().map_or([1.0; 4], |t| {
                let s = t.at(u, v);
                [
                    srgb_to_linear(s[0]),
                    srgb_to_linear(s[1]),
                    srgb_to_linear(s[2]),
                    s[3],
                ]
            });
            for c in 0..4 {
                out[c] += s[c] * weight;
            }
        }
        out
    }

    /// The texel's layers, detail, tint weights and pattern (see [`Local`]).
    fn local(&self, u: f32, v: f32) -> Local {
        let mask = self.layer_mask.as_ref().map_or([0.0; 4], |m| m.at(u, v));
        let layers = [mask[0], mask[1], mask[2]];
        let weights = layer_weights(layers);
        let p4 = |values: PerLayer| pick(values, layers);
        let surface = self.surface.as_ref().map_or([1.0; 4], |s| s.at(u, v));
        let curvature = surface[0].powf(p4(self.curvature_power));
        let scale = p4(self.detail_scale);
        // Each layer's R and G lifted by its black point, blended, then lifted by layer 1's once more.
        let mut detail = [0.0f32; 4];
        for (i, (texture, weight)) in self.details.iter().zip(weights).enumerate() {
            if weight <= 0.0 {
                continue;
            }
            let mut s = texture
                .as_ref()
                .map_or([0.5, 0.5, 1.0, 1.0], |t| t.at(u * scale, v * scale));
            let bp = self.black_point[i];
            s[0] = bp + s[0] * (1.0 - bp);
            s[1] = bp + s[1] * (1.0 - bp);
            for c in 0..4 {
                detail[c] += s[c] * weight;
            }
        }
        let bp = self.black_point[0];
        detail[0] = bp + detail[0] * (1.0 - bp);
        detail[1] = bp + detail[1] * (1.0 - bp);
        let mut tint_weights = [0.0f32; 8];
        if let Some(mask) = &self.layer_mask {
            for [du, dv, weight] in MASK_TAPS {
                let k = self.color_mask_blur / 1024.0;
                let index = (mask.at(u + du * k, v + dv * k)[3] * 8.0).floor();
                if (0.0..8.0).contains(&index) {
                    tint_weights[index as usize] += weight;
                }
            }
        } else {
            tint_weights[0] = 1.0;
        }
        let pattern = self.patterned(u, v, layers, surface, detail, &tint_weights);
        Local {
            layers,
            surface,
            curvature,
            detail,
            tint_weights,
            pattern,
        }
    }

    /// The pattern's part at (u, v): see [`Patterned`] and the module docs.
    fn patterned(
        &self,
        u: f32,
        v: f32,
        layers: [f32; 3],
        surface: [f32; 4],
        detail: [f32; 4],
        tint_weights: &[f32; 8],
    ) -> Patterned {
        let p4 = |values: PerLayer| pick(values, layers);
        let inverse = 1.0 - detail[3];
        let detail_rough = ((p4(self.detail_rough_contrast) * (inverse * inverse * 0.85 - 0.35)
            + 0.5)
            * p4(self.detail_rough_brightness))
        .clamp(0.0, 1.0);
        let unpatterned = Patterned {
            metalness: p4(self.detail_metalness),
            durability: surface[3],
            light: detail[1],
            covered: 1.0,
            thickness: 1.0,
            color: [0.0; 3],
            roughness: detail_rough,
        };
        if !self.patterned {
            return unpatterned;
        }
        let covered = if matches!(self.pattern_mode, 1 | 3) {
            1.0
        } else {
            tint_weights[self.replace_index]
        };
        let nudge = (detail[1] * 2.0 - 1.0) / 1024.0 * self.pattern_detail_influence;
        let (pu, pv) = self.pattern_at.apply(u * self.flip, v);
        let p = self
            .pattern
            .as_ref()
            .map_or([0.0; 4], |s| s.at(pu + nudge, pv + nudge));
        let metalness = lerp(p4(self.detail_metalness), self.pattern_metalness, covered);
        if self.pattern_mode >= 2 {
            return Patterned {
                metalness,
                covered,
                color: rgb(p),
                ..unpatterned
            };
        }
        let sum = p[0] + p[1] + p[2];
        let paint = sum.clamp(0.0, 1.0)
            * covered
            * (self.paint_thickness - detail[2] * self.wear.powi(4)).clamp(0.0, 1.0);
        let painted = paint * self.paint_thickness.min(1.0);
        let tint = |k: usize| self.tints[self.palette[k]];
        Patterned {
            metalness,
            durability: surface[3] - painted * self.paint_durability,
            light: lerp(detail[1], 0.5, paint),
            covered,
            thickness: sum * self.paint_thickness,
            color: lerp3(
                lerp3(lerp3(tint(0), tint(1), p[0]), tint(2), p[1]),
                tint(3),
                p[2],
            ),
            roughness: detail_rough * lerp(1.0, self.pattern_rough_brightness, painted),
        }
    }

    /// The texel at (u, v) painted (see the module docs).
    fn shade(&self, u: f32, v: f32) -> Shade {
        let here = self.local(u, v);
        let step = 1.0 / COMPOSITE_SIZE;
        let (right, below) = (self.local(u + step, v), self.local(u, v + step));
        let layers = here.layers;
        let weights = layer_weights(layers);
        let p4 = |values: PerLayer| pick(values, layers);
        let w = self.wear;
        let surface = here.surface;
        let curvature = here.curvature;
        let [dx, _, dz, _] = here.detail;
        let pattern = &here.pattern;
        let (gu, gv) = self.grunge_at.apply(u, v);
        let grunge = self.grunge(weights, gu, gv);
        let grunge_rgb = rgb(grunge);
        let grunge_lum = luminance(grunge_rgb);
        let noise = self.noise.as_ref().map_or([0.5; 4], |s| {
            let n = s.at(u * 2.0, v * 2.0);
            [
                srgb_to_linear(n[0]),
                srgb_to_linear(n[1]),
                srgb_to_linear(n[2]),
                n[3],
            ]
        });

        // The base: the tints by mask index.
        let mut base = [0.0f32; 3];
        for (tint, weight) in self.tints.iter().zip(here.tint_weights) {
            for c in 0..3 {
                base[c] += tint[c] * weight;
            }
        }
        let base = base.map(srgb_to_linear);

        // Damage.
        let damage = curvature * pattern.durability;
        let boost = p4(self.curvature_wear_boost) * curvature;
        let (low, high) = (p4(self.damage_levels[0]), p4(self.damage_levels[1]));
        let damaged_at =
            |alpha: f32| smoothstep(low, high, (damage * alpha + boost).clamp(0.0, 1.0) * w);
        let damaged = damaged_at(grunge[3]);
        let intact = 1.0 - damaged;
        let reach = (damage * grunge[3] + boost).clamp(0.0, 1.0);
        let worn = (damaged + smoothstep(low, high, reach * w * 4.0).clamp(0.0, 1.0) * intact * dz)
            .clamp(0.0, 1.0);
        let albedo = if self.patterned {
            let painted = pattern.color.map(srgb_to_linear);
            lerp3(base, lerp3(painted, base, worn), pattern.covered)
        } else {
            base
        };
        let albedo_lum = luminance(albedo);

        // Edges from the derivatives of detail × curvature and of the surface's sum.
        let edge_of = |f: &dyn Fn(&Local) -> f32| {
            let (ddx, ddy) = (f(&right) - f(&here), f(&below) - f(&here));
            (1.0 - ddx.abs()) * (1.0 - ddy.abs()) * (ddx.abs() + ddy.abs()).clamp(0.0, 1.0)
        };
        let light_edge = edge_of(&|l: &Local| l.pattern.light * l.curvature);
        let detail_edge = edge_of(&|l: &Local| l.detail[0] * l.curvature);
        let sum = |l: &Local| l.surface[0] + l.surface[1] + l.surface[2];
        let (sdx, sdy) = (sum(&right) - sum(&here), sum(&below) - sum(&here));
        let edge = lerp(light_edge, detail_edge, damaged)
            * (1.0 - sdx.abs().clamp(0.0, 1.0))
            * (1.0 - sdy.abs().clamp(0.0, 1.0));
        let light = (lerp(pattern.light, dx, worn) + edge).min(1.0);

        // The colour lit by the detail.
        let boosted = albedo.map(|c| c.max(c * (1.0 + curvature * 0.5)));
        let hue = {
            let m = boosted.map(|c| c.max(0.001));
            let length = m.iter().map(|x| x * x).sum::<f32>().sqrt();
            m.map(|x| x / length)
        };
        let largest = |c: [f32; 3]| c[0].max(c[1]).max(c[2]);
        let lit = clamp3(scale3(
            hue,
            (light / luminance(hue)).min(3.0 * light * largest(boosted)),
        ));
        let bleach = lerp(
            w * surface[2]
                * p4(self.wear_bleaching)
                * lerp(dz, 0.0, (pattern.thickness - 1.0).clamp(0.0, 1.0)),
            p4(self.damage_bleaching),
            damaged,
        );
        let grime = resaturate(
            grunge_rgb,
            scale3(grunge_rgb, grunge_lum),
            grunge_lum,
            p4(self.grime_saturation),
        );
        let grimy = scale3(mul3(lit, grunge_rgb), 2.0);
        let detail_grunge = p4(self.detail_grunge);
        let c1 = lerp3(
            lerp3(
                lit,
                grimy,
                detail_grunge * (1.0 + w * detail_grunge) * intact,
            ),
            scale3(grime, (1.0 + p4(self.grime_brightness)) * light),
            bleach,
        );
        let c2 = resaturate(
            c1,
            scale3(albedo, albedo_lum),
            luminance(c1),
            p4(self.damage_saturation),
        );
        let c3 = lerp3(c1, scale3(c2, 1.0 + p4(self.damage_brightness)), damaged);
        let open = smoothstep(0.45, 0.75, (1.0 - surface[0] * surface[1] * surface[1]) * w)
            * (1.0 - albedo_lum * 0.5)
            * p4(self.grunge_max);
        let c4 = lerp3(c3, mul3(grunge_rgb, c3), open);

        // Hue shifts by luminance and noise.
        let half = luminance(c4) * 0.5;
        let [r, g, b] = c4;
        let (a_hue, b_hue) = if r > g && r > b {
            ([0.6 + b, 1.0 + r, g], [0.2 + b, 0.6 + g, 1.0 + r])
        } else if g > r && g > b {
            ([0.6 + b, 1.0 + r, g], [0.2 + g, 0.6 + b, 1.0 + r])
        } else {
            ([0.6, 1.0, 0.0], [0.2, 0.6, 1.0])
        };
        let distance = c4
            .iter()
            .map(|c| (c - half) * (c - half))
            .sum::<f32>()
            .sqrt();
        let amount = (1.0 - albedo_lum).powf(3.5) * 0.3 + distance * 0.15 + 0.05;
        let toward = |h: [f32; 3]| resaturate(c4, scale3(h, luminance(h)), half, amount);
        let gloss = surface[1].powi(8);
        let color = clamp3(lerp3(
            lerp3(
                toward(b_hue),
                toward(a_hue),
                (gloss * noise[0]).clamp(0.0, 1.0),
            ),
            c4,
            (noise[2] * (0.5 + gloss * 0.5)).clamp(0.0, 1.0),
        ));

        // Damage edges: where the grunge's alpha around the texel would damage differently.
        let thick = pattern.thickness;
        let width = p4(self.damage_edge_width) * (1.0 + thick) / 1024.0;
        let edges = EDGE_TAPS
            .iter()
            .map(|[du, dv]| {
                let alpha = self.grunge(weights, gu + du * width, gv + dv * width)[3];
                (damaged - damaged_at(alpha)).abs()
            })
            .sum::<f32>()
            * 0.25;
        let metalness = lerp(
            lerp(pattern.metalness, p4(self.damage_metalness), damaged),
            p4(self.damage_edge_metalness),
            edges,
        );
        let worn_rough = 1.0 - dx;
        let roughness = lerp(
            lerp(
                lerp(
                    pattern.roughness.min(1.0),
                    1.0 - grunge_lum + p4(self.grunge_rough_brightness),
                    bleach,
                ),
                ((p4(self.damage_rough_contrast) * (worn_rough * worn_rough * 0.85 - 0.35) + 0.5)
                    * p4(self.damage_rough_brightness))
                .clamp(0.0, 1.0),
                damaged,
            ),
            p4(self.damage_edge_roughness),
            edges,
        );

        // The textile levels by wear.
        let levels = std::array::from_fn::<f32, 3, _>(|i| {
            lerp(TEXTILE_LEVELS[i], METALLIC_TEXTILE_LEVELS[i], metalness)
        });
        let hue = {
            let m = color.map(|c| c.max(0.0003));
            let length = m.iter().map(|x| x * x).sum::<f32>().sqrt();
            m.map(|x| x / length)
        };
        let level = lerp(
            levels[0].min(luminance(lerp3(lit, grimy, detail_grunge))),
            levels[2],
            largest(color).powf(levels[1]).clamp(0.0, 1.0),
        );
        let top = largest(hue);
        Shade {
            color: lerp3(color, hue.map(|h| h * level / top), w),
            roughness: roughness.clamp(0.0, 1.0),
            metalness: metalness.clamp(0.0, 1.0),
        }
    }
}

/// Why a glove material is not painted with these parameters (a combo not baked yet, its layer mask missing), or
/// `None`.
pub(crate) fn unpainted_reason(params: &BTreeMap<String, Param>) -> Option<String> {
    if blended::applies(params) {
        return blended::unpainted_reason(params);
    }
    if matches!(params.get("F_ANISOTROPIC_GLOSS"), Some(Param::Int(i)) if *i != 0) {
        return Some("anisotropic glove gloss (Source1 layout) not baked yet".into());
    }
    (!matches!(params.get("g_tLayerMask"), Some(Param::Texture(path)) if !path.is_empty()))
        .then(|| "no glove layer mask".into())
}

/// A glove material's painted colour (sRGB, opaque): see the [module docs](self).
pub(crate) fn bake_color(
    content: &Content,
    params: &BTreeMap<String, Param>,
) -> Result<Image, ExportError> {
    bake(content, params, |shade| {
        let [r, g, b] = shade.color.map(linear_to_srgb);
        [r, g, b, 255]
    })
}

/// A glove material's painted metallic-roughness (glTF channels: G roughness, B metalness).
pub(crate) fn bake_surface(
    content: &Content,
    params: &BTreeMap<String, Param>,
) -> Result<Image, ExportError> {
    bake(content, params, |shade| {
        let byte = |c: f32| (c * 255.0).round() as u8;
        [255, byte(shade.roughness), byte(shade.metalness), 255]
    })
}

/// The glove painted at [`GLOVE_SIZE`], by the program its parameters name.
fn bake(
    content: &Content,
    params: &BTreeMap<String, Param>,
    pixel: impl Fn(&Shade) -> [u8; 4] + Sync,
) -> Result<Image, ExportError> {
    if blended::applies(params) {
        let inputs = blended::Inputs::read(content, params)?;
        return Ok(render(|u, v| inputs.shade(u, v), pixel));
    }
    let inputs = Inputs::read(content, params)?;
    Ok(render(|u, v| inputs.shade(u, v), pixel))
}

/// Every texel of the output shaded at its centre, rows in parallel.
fn render(
    shade: impl Fn(f32, f32) -> Shade + Sync,
    pixel: impl Fn(&Shade) -> [u8; 4] + Sync,
) -> Image {
    let size = GLOVE_SIZE;
    let mut rgba = vec![0; size * size * 4];
    rgba.par_chunks_mut(size * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, texel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (u, v) = (
                    (x as f32 + 0.5) / size as f32,
                    (y as f32 + 0.5) / size as f32,
                );
                texel.copy_from_slice(&pixel(&shade(u, v)));
            }
        });
    Image::new(size, size, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_weights_are_the_sequential_picks() {
        let values = [1.0, 2.0, 3.0, 4.0];
        for layers in [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.3, 0.6, 0.2],
            [0.5, 1.0, 1.0],
        ] {
            let weights = layer_weights(layers);
            let blended: f32 = weights.iter().zip(values).map(|(w, v)| w * v).sum();
            assert!((blended - pick(values, layers)).abs() < 1e-5, "{layers:?}");
        }
    }

    #[test]
    fn resaturating_keeps_or_greys_the_colour() {
        let c = [0.4, 0.2, 0.1];
        // No amount: unchanged; a negative one greys it towards its luminance.
        assert_eq!(resaturate(c, c, luminance(c), 0.0), c);
        let grey = resaturate(c, c, luminance(c), -1.0);
        assert!(
            grey.iter().all(|x| (x - luminance(c)).abs() < 1e-6),
            "{grey:?}"
        );
        // A positive one pushes it off grey, warmer here, at the luminance asked.
        let pushed = resaturate(c, scale3(c, luminance(c)), luminance(c), 1.0);
        assert!(pushed[0] > pushed[2], "{pushed:?}");
    }
}
