//! Gloves painted with height-blended layers (M10): the glove composite's "Source2 height blending" program
//! (`F_BACKWARDS_COMPATIBILITY` 1), the one 2025's glove kits render with (`gloves/paints/volatile_02/glove_*`:
//! `docs/research/cs2-skins.md`, "Gloves"). Its static combos compile to two pixel programs: 1 and 3, and with tint
//! ids 5 and 7 (`S_ANISOTROPIC_GLOSS` changes nothing in the pixel stage). This reads the second, decompiled as the
//! default combo was ([the module above](super)); the first is the same program with every texel's id tint white
//! and of no strength, and the pattern over every id. Per texel:
//!
//! - **Layers**: `g_tLayerId` RGB, and what is left to 1, weigh four material layers (normalized). Each has a
//!   surface and a substrate (`g_tSurface*`, `g_tSubstrate*`: sRGB colour, tint mask in A; `…Properties*`: occlusion,
//!   metalness, cloth, height; `…Normal*`: a hemi-octahedral normal in G and A, two roughnesses in R and B) at its own
//!   placement (`g_vUvScale*`, `g_vUvOffset*`), a damage height and a grime colour at the rolled damage and grime
//!   placements times its own scale, and its values. Textures, values and colour-adjust matrices blend by the
//!   weights; two burnishing roughness brightnesses and the bevel's tint-mask switch are the last present layer's,
//!   as the compiled program reads them.
//! - **Tint ids** (`F_TINT_ID`): `g_tTintId` R (`ceil(R × 7)`: 8 ids) at the texel and its 8 neighbours; an id
//!   keeps the centre's where its column or row agrees with it, else a weighted count (the program counts the
//!   top-right corner twice and not the top centre). The ids' colours (`g_vId*Color`, sRGB, A their strength) blend
//!   into one tint.
//! - **Pattern** (`F_PATTERN`, sRGB, placed by `g_vPatternTexCoordXform*`, the other hand by negating u, moved by the
//!   surface normal × `g_fPatternNormalDistortion`): over the ids `g_bId*Pattern` names. Without `F_PATTERN_PAINT`
//!   it joins the tint (its colour, its alpha the strength; the substrate reads it at its own normal) and its
//!   roughness contrast; with it, it is painted over the surface: colour, metalness and roughness from
//!   `g_tPatternProperties`, raised by `F_PUFFY_PAINT` (its alpha over `g_fPatternTranslucencyThreshold` lifts the
//!   surface's height and sharpens the damage at its edges).
//! - **Colours**: five colour-adjust matrices per layer (surface, substrate, damage bevel, and burnishing on each):
//!   `MatrixColorCorrect2` around the texture's average colour (its stored reflectivity), then `MatrixColorTint2`,
//!   OFAST's versions of which the environment shaders already use (`environment::color_correct`,
//!   `environment::color_tint`). The tint is carried onto the adjusted colours by luminance and hue
//!   ([`tint_transfer`]) where the tint masks are.
//! - **Damage**: the surface stays where its height over the substrate's, less the damage height, the object's
//!   curvature and the wear (`g_vDamageMinMax`), stays over zero (`g_fDamageHeightBlendSoftness`); a bevel along
//!   that edge (`g_fDamageBevelBlendSoftness`) takes the damage colour, metalness and roughness. Burnishing where
//!   the object's high-touch mask meets the wear (`g_vSurfaceBurnishingMinMax`…), grime where height, occlusion
//!   and wear add up (`g_vSurfaceGrimeMinMax`…, `g_fGrimeTranslucency`). The wear is gamma-encoded first
//!   (`SrgbLinearToGamma`, this program's expression for `g_fWearProgress`).
//! - **Outputs**: colour (mode 0, written sRGB), roughness (mode 1's B; with `F_ANISOTROPIC_GLOSS` mode 4's two
//!   roughnesses: OFAST averages them, glTF has one), metalness (mode 2's G).
//!
//! **Not baked**: the normal (puffy paint's emboss, the bevel's), occlusion and cloth outputs; the substrate's
//! pattern mip bias. Textures are read at the mip the 1024 output needs (box-filtered), the program's own
//! derivatives choose the 2048 composite's.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::{GLOVE_SIZE, Shade};
use crate::content::Content;
use crate::export::ExportError;
use crate::export::environment::{color_correct, color_tint};
use crate::export::gltf::{decode_raw, srgb_to_linear};
use crate::export::paint::{
    Sampler, clamp3, halve, lerp, lerp3, luminance, rgb, scale3, smoothstep,
};
use crate::paint::Param;
use crate::texture::Image;

/// The 9 taps of the tint id filter, in the program's order: offsets in the id texture's texels.
const TINT_TAPS: [[f32; 2]; 9] = [
    [-1.0, -1.0],
    [0.0, -1.0],
    [1.0, -1.0],
    [-1.0, 0.0],
    [0.0, 0.0],
    [1.0, 0.0],
    [-1.0, 1.0],
    [0.0, 1.0],
    [1.0, 1.0],
];

/// What a layer texture the material leaves out reads as (the declarations' defaults): colour white, fully tinted;
/// occlusion 1, no metal or cloth, height 0.5; a flat normal of roughness 0.5.
const WHITE: [f32; 4] = [1.0; 4];
const PROPERTIES: [f32; 4] = [1.0, 0.0, 0.0, 0.5];
const FLAT: [f32; 4] = [0.5, 0.502, 0.5, 0.502];

/// The colour-adjust matrices of a layer, in [`Layer::adjust`].
const SURFACE: usize = 0;
const SUBSTRATE: usize = 1;
const DAMAGE: usize = 2;
const SURFACE_BURNISHING: usize = 3;
const SUBSTRATE_BURNISHING: usize = 4;

/// `true` for the parameters of a glove paint this module bakes.
pub(super) fn applies(params: &BTreeMap<String, Param>) -> bool {
    matches!(params.get("F_BACKWARDS_COMPATIBILITY"), Some(Param::Int(i)) if *i != 0)
}

/// Why a height-blended glove paint is not baked (its layer ids missing), or `None`.
pub(super) fn unpainted_reason(params: &BTreeMap<String, Param>) -> Option<String> {
    (!matches!(params.get("g_tLayerId"), Some(Param::Texture(path)) if !path.is_empty()))
        .then(|| "no glove layer ids".into())
}

/// A texture as the program reads it, at the mip level the output needs.
struct Texture {
    sampler: Sampler,
    /// sRGB-decode R, G, B (`SrgbRead(true)`).
    srgb: bool,
    /// The texture compiler's average colour (linear): `TextureAverageColor`.
    average: [f32; 3],
}

impl Texture {
    /// Channels at (u, v), filtered, R, G, B decoded for an sRGB texture.
    fn at(&self, u: f32, v: f32) -> [f32; 4] {
        let s = self.sampler.at(u, v);
        if self.srgb {
            [
                srgb_to_linear(s[0]),
                srgb_to_linear(s[1]),
                srgb_to_linear(s[2]),
                s[3],
            ]
        } else {
            s
        }
    }

    /// R of the texel (u, v) falls in, unfiltered: ids must not blend.
    fn nearest_r(&self, u: f32, v: f32) -> f32 {
        let image = &self.sampler.image;
        let x = ((u.rem_euclid(1.0) * image.width as f32) as usize).min(image.width - 1);
        let y = ((v.rem_euclid(1.0) * image.height as f32) as usize).min(image.height - 1);
        f32::from(image.rgba[(y * image.width + x) * 4]) / 255.0
    }
}

/// A texture decoded at full size, with its average colour (`None`: it could not be read).
type Decoded = Option<Arc<(Image, [f32; 3])>>;

/// Reads the textures a glove paint names, each decoded once and box-filtered to each mip level asked for.
struct Loader<'a> {
    content: &'a Content,
    params: &'a BTreeMap<String, Param>,
    decoded: HashMap<String, Decoded>,
    levels: HashMap<(String, u32, bool), Arc<Texture>>,
}

impl Loader<'_> {
    /// The texture parameter `name` holds, repeated `repeats` times across the output. A texture that cannot be
    /// read is left out (its default stands in).
    fn texture(&mut self, name: &str, repeats: f32, srgb: bool) -> Option<Arc<Texture>> {
        let Some(Param::Texture(path)) = self.params.get(name) else {
            return None;
        };
        let content = self.content;
        let decoded = self
            .decoded
            .entry(path.clone())
            .or_insert_with(|| {
                let (header, image) = decode_raw(content, path).ok()?;
                let [r, g, b, _] = header.reflectivity;
                Some(Arc::new((image, [r, g, b])))
            })
            .clone()?;
        let (image, average) = &*decoded;
        // Texels per output texel, as a mip level (the program's own derivatives pick the composite's at 2048).
        let texels = image.width.max(image.height) as f32 * repeats.abs() / GLOVE_SIZE as f32;
        let level = if texels > 1.0 {
            texels.log2().round() as u32
        } else {
            0
        };
        let texture = self
            .levels
            .entry((path.clone(), level, srgb))
            .or_insert_with(|| {
                let mut image = image.clone();
                for _ in 0..level {
                    if image.width.min(image.height) < 2 {
                        break;
                    }
                    image = halve(&image);
                }
                Arc::new(Texture {
                    sampler: Sampler { image },
                    srgb,
                    average: *average,
                })
            });
        Some(texture.clone())
    }
}

/// A placement as the vertex stage applies it: `u' = a·u + b·v + c`, `v' = d·u + e·v + f`.
#[derive(Debug, Clone, Copy)]
struct Xform([[f32; 3]; 2]);

impl Xform {
    /// `g_vPatternTexCoordXform0`, `1`: scaled per axis from (0, 0), turned by `degrees` about `center`, then moved
    /// by `offset`.
    fn new(scale: [f32; 2], degrees: f32, center: [f32; 2], offset: [f32; 2]) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Self([
            [
                c * scale[0],
                -s * scale[1],
                center[0] + offset[0] + s * center[1] - c * center[0],
            ],
            [
                s * scale[0],
                c * scale[1],
                center[1] + offset[1] - (s * center[0] + c * center[1]),
            ],
        ])
    }

    /// `g_vDamageTexCoordXform*` and `g_vGrimeTexCoordXform*`: turned about the middle, then moved.
    fn turned(degrees: f32, offset: [f32; 2]) -> Self {
        Self::new([1.0; 2], degrees, [0.5; 2], offset)
    }

    fn apply(&self, u: f32, v: f32) -> [f32; 2] {
        let [a, b] = self.0;
        [a[0] * u + a[1] * v + a[2], b[0] * u + b[1] * v + b[2]]
    }
}

/// One colour-adjust matrix: `MatrixMultiply(MatrixColorTint2(tint, 1), MatrixColorCorrect2((contrast, saturation,
/// brightness), average))`.
#[derive(Debug, Clone, Copy)]
struct Adjust {
    csb: [f32; 3],
    tint: [f32; 3],
    average: [f32; 3],
}

impl Adjust {
    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        color_tint(color_correct(c, self.csb, self.average), self.tint)
    }
}

/// The per-layer values the program blends by layer weight.
#[derive(Debug, Clone, Copy, Default)]
struct Values {
    /// `g_fDamageHeightBlendSoftness` (its expression: at least 0.001).
    height_softness: f32,
    /// `g_fDamageBevelBlendSoftness` (its expression: plus the height softness, at least 0.001).
    bevel_softness: f32,
    bevel_to_substrate: f32,
    bevel_metalness: f32,
    bevel_anisotropy: f32,
    bevel_roughness: f32,
    burnishing_grime: f32,
    burnishing_metalness: f32,
    grime_roughness: f32,
    grime_translucency: f32,
    /// `g_fSubstrateCompositeColorTranslucency`: how much of the tint reaches the substrate.
    substrate_translucency: f32,
    damage: [f32; 2],
    surface_grime: [f32; 2],
    substrate_grime: [f32; 2],
    surface_burnishing: [f32; 2],
    substrate_burnishing: [f32; 2],
}

impl Values {
    fn add(&mut self, o: &Self, w: f32) {
        let add2 = |a: &mut [f32; 2], b: [f32; 2]| {
            a[0] += b[0] * w;
            a[1] += b[1] * w;
        };
        self.height_softness += o.height_softness * w;
        self.bevel_softness += o.bevel_softness * w;
        self.bevel_to_substrate += o.bevel_to_substrate * w;
        self.bevel_metalness += o.bevel_metalness * w;
        self.bevel_anisotropy += o.bevel_anisotropy * w;
        self.bevel_roughness += o.bevel_roughness * w;
        self.burnishing_grime += o.burnishing_grime * w;
        self.burnishing_metalness += o.burnishing_metalness * w;
        self.grime_roughness += o.grime_roughness * w;
        self.grime_translucency += o.grime_translucency * w;
        self.substrate_translucency += o.substrate_translucency * w;
        add2(&mut self.damage, o.damage);
        add2(&mut self.surface_grime, o.surface_grime);
        add2(&mut self.substrate_grime, o.substrate_grime);
        add2(&mut self.surface_burnishing, o.surface_burnishing);
        add2(&mut self.substrate_burnishing, o.substrate_burnishing);
    }
}

/// One material layer (`…1` – `…4`).
struct Layer {
    /// `g_vUvScale*` and `g_vUvOffset*`: scale u, v, offset u, v.
    uv: [f32; 4],
    surface: Option<Arc<Texture>>,
    surface_properties: Option<Arc<Texture>>,
    surface_normal: Option<Arc<Texture>>,
    substrate: Option<Arc<Texture>>,
    substrate_properties: Option<Arc<Texture>>,
    substrate_normal: Option<Arc<Texture>>,
    grime: Option<Arc<Texture>>,
    damage: Option<Arc<Texture>>,
    damage_scale: f32,
    grime_scale: f32,
    values: Values,
    /// `g_fSurfaceBurnishingRoughnessBrightness*`, `g_fSubstrateBurnishingRoughnessBrightness*`.
    burnishing_roughness: [f32; 2],
    /// `g_bDamageBevelUseTintMask*`.
    bevel_tint_mask: bool,
    /// The colour-adjust matrices: [`SURFACE`], [`SUBSTRATE`], [`DAMAGE`], [`SURFACE_BURNISHING`],
    /// [`SUBSTRATE_BURNISHING`].
    adjust: [Adjust; 5],
}

/// The layers blended at a texel.
#[derive(Default)]
struct Blend {
    surface: [f32; 4],
    surface_properties: [f32; 4],
    surface_normal: [f32; 4],
    substrate: [f32; 4],
    substrate_properties: [f32; 4],
    substrate_normal: [f32; 4],
    grime: [f32; 3],
    damage: f32,
    values: Values,
    /// The last present layer's, times its weight (surface, substrate).
    burnishing_roughness: [f32; 2],
    bevel_tint_mask: bool,
}

impl Layer {
    /// Adds this layer at weight `w` to `b`: its textures at (u, v) and the damage and grime places.
    fn add(&self, b: &mut Blend, w: f32, [u, v]: [f32; 2], damage: [f32; 2], grime: [f32; 2]) {
        let (lu, lv) = (u * self.uv[0] + self.uv[2], v * self.uv[1] + self.uv[3]);
        let add = |sum: &mut [f32; 4], texture: &Option<Arc<Texture>>, default: [f32; 4]| {
            let s = texture.as_ref().map_or(default, |t| t.at(lu, lv));
            for c in 0..4 {
                sum[c] += s[c] * w;
            }
        };
        add(&mut b.surface, &self.surface, WHITE);
        add(
            &mut b.surface_properties,
            &self.surface_properties,
            PROPERTIES,
        );
        add(&mut b.surface_normal, &self.surface_normal, FLAT);
        add(&mut b.substrate, &self.substrate, WHITE);
        add(
            &mut b.substrate_properties,
            &self.substrate_properties,
            PROPERTIES,
        );
        add(&mut b.substrate_normal, &self.substrate_normal, FLAT);
        let grime = self.grime.as_ref().map_or([srgb_to_linear(0.5); 4], |t| {
            t.at(grime[0] * self.grime_scale, grime[1] * self.grime_scale)
        });
        for (sum, g) in b.grime.iter_mut().zip(grime) {
            *sum += g * w;
        }
        b.damage += self.damage.as_ref().map_or(0.5, |t| {
            t.at(damage[0] * self.damage_scale, damage[1] * self.damage_scale)[0]
        }) * w;
        b.values.add(&self.values, w);
        b.burnishing_roughness = self.burnishing_roughness.map(|r| r * w);
        b.bevel_tint_mask = self.bevel_tint_mask;
    }
}

/// The pattern at a texel.
struct Pattern {
    /// Colour (linear) and strength: its alpha, sharpened over the threshold for puffy paint.
    color: [f32; 4],
    /// The alpha puffy paint raises the surface by.
    raise: f32,
    /// The pattern under the substrate's normal.
    under: [f32; 4],
    /// `g_tPatternProperties`: metalness, roughness.
    properties: [f32; 2],
}

/// What the bake reads from a height-blended glove paint's parameters.
pub(super) struct Inputs {
    layers: [Layer; 4],
    layer_id: Option<Arc<Texture>>,
    /// `None` without `F_TINT_ID`.
    tint_id: Option<Arc<Texture>>,
    object: Option<Arc<Texture>>,
    pattern: Option<Arc<Texture>>,
    pattern_properties: Option<Arc<Texture>>,
    patterned: bool,
    paint_layer: bool,
    puffy: bool,
    respects_tint_mask: bool,
    /// `g_vId*Color`: linear colour, strength.
    id_colors: [[f32; 4]; 8],
    /// `g_bId*Pattern`.
    id_pattern: [f32; 8],
    flip: bool,
    pattern_at: Xform,
    /// `g_fPatternNormalDistortion` (its expression: a tenth) × the pattern's scale.
    distortion: [f32; 2],
    damage_at: Xform,
    grime_at: Xform,
    threshold: f32,
    emboss: f32,
    /// `g_vPatternRoughnessContrastBrightness`.
    roughness_contrast: [f32; 2],
    /// `g_fWearProgress` after its expression (gamma-encoded).
    wear: f32,
}

impl Inputs {
    pub(super) fn read(
        content: &Content,
        params: &BTreeMap<String, Param>,
    ) -> Result<Self, ExportError> {
        let floats = |name: &str| match params.get(name) {
            Some(Param::Floats(v)) => Some(*v),
            Some(Param::Int(i)) => Some([*i as f32, 0.0, 0.0, 0.0]),
            _ => None,
        };
        let float = |name: &str, default: f32| floats(name).map_or(default, |v| v[0]);
        let pair = |name: &str, default: [f32; 2]| floats(name).map_or(default, |v| [v[0], v[1]]);
        let triple =
            |name: &str, default: [f32; 3]| floats(name).map_or(default, |v| [v[0], v[1], v[2]]);
        let int = |name: &str, default: i64| match params.get(name) {
            Some(Param::Int(i)) => *i,
            Some(Param::Floats(v)) => v[0] as i64,
            _ => default,
        };
        let mut loader = Loader {
            content,
            params,
            decoded: HashMap::new(),
            levels: HashMap::new(),
        };
        let mut layer = |n: usize| {
            let key = |name: &str| format!("{name}{n}");
            let scale = pair(&key("g_vUvScale"), [4.0, 4.0]);
            let offset = pair(&key("g_vUvOffset"), [0.0, 0.0]);
            let repeats = scale[0].abs().max(scale[1].abs());
            let damage_scale = float(&key("g_fDamageUvScale"), 4.0);
            let grime_scale = float(&key("g_fGrimeUvScale"), 4.0);
            let surface = loader.texture(&key("g_tSurface"), repeats, true);
            let substrate = loader.texture(&key("g_tSubstrate"), repeats, true);
            let surface_average = surface.as_ref().map_or([1.0; 3], |t| t.average);
            let substrate_average = substrate.as_ref().map_or([1.0; 3], |t| t.average);
            let adjust = |tint: &str, prefix: &str, average: [f32; 3]| Adjust {
                csb: ["Contrast", "Saturation", "Brightness"]
                    .map(|what| float(&key(&format!("{prefix}{what}")), 1.0)),
                tint: triple(&key(tint), [1.0; 3]),
                average,
            };
            let height_softness = float(&key("g_fDamageHeightBlendSoftness"), 0.01);
            Layer {
                uv: [scale[0], scale[1], offset[0], offset[1]],
                surface_properties: loader.texture(&key("g_tSurfaceProperties"), repeats, false),
                surface_normal: loader.texture(&key("g_tSurfaceNormal"), repeats, false),
                substrate_properties: loader.texture(
                    &key("g_tSubstrateProperties"),
                    repeats,
                    false,
                ),
                substrate_normal: loader.texture(&key("g_tSubstrateNormal"), repeats, false),
                grime: loader.texture(&key("g_tGrime"), grime_scale, true),
                damage: loader.texture(&key("g_tDamage"), damage_scale, false),
                surface,
                substrate,
                damage_scale,
                grime_scale,
                values: Values {
                    height_softness: height_softness.max(0.001),
                    bevel_softness: (float(&key("g_fDamageBevelBlendSoftness"), 0.5)
                        + height_softness)
                        .max(0.001),
                    bevel_to_substrate: if int(&key("g_bDamageBevelBlendToSubstrate"), 0) != 0 {
                        1.0
                    } else {
                        0.0
                    },
                    bevel_metalness: float(&key("g_fDamageBevelMetalness"), 0.0),
                    bevel_anisotropy: float(&key("g_fDamageBevelAnisotropy"), 0.0),
                    bevel_roughness: float(&key("g_fDamageBevelRoughnessBrightness"), 1.0),
                    burnishing_grime: float(&key("g_fBurnishingGrime"), 0.5),
                    burnishing_metalness: float(&key("g_fBurnishingMetalness"), 0.0),
                    grime_roughness: float(&key("g_fGrimeRoughnessBrightness"), 1.0),
                    grime_translucency: float(&key("g_fGrimeTranslucency"), 0.5),
                    substrate_translucency: float(
                        &key("g_fSubstrateCompositeColorTranslucency"),
                        0.5,
                    ),
                    damage: pair(&key("g_vDamageMinMax"), [0.0; 2]),
                    surface_grime: pair(&key("g_vSurfaceGrimeMinMax"), [0.0; 2]),
                    substrate_grime: pair(&key("g_vSubstrateGrimeMinMax"), [0.0; 2]),
                    surface_burnishing: pair(&key("g_vSurfaceBurnishingMinMax"), [0.0; 2]),
                    substrate_burnishing: pair(&key("g_vSubstrateBurnishingMinMax"), [0.0; 2]),
                },
                burnishing_roughness: [
                    float(&key("g_fSurfaceBurnishingRoughnessBrightness"), 1.0),
                    float(&key("g_fSubstrateBurnishingRoughnessBrightness"), 1.0),
                ],
                bevel_tint_mask: int(&key("g_bDamageBevelUseTintMask"), 0) != 0,
                adjust: [
                    adjust("g_vSurfaceColorTint", "g_fSurfaceColor", surface_average),
                    adjust(
                        "g_vSubstrateColorTint",
                        "g_fSubstrateColor",
                        substrate_average,
                    ),
                    adjust(
                        "g_vDamageBevelColorTint",
                        "g_fDamageBevelColor",
                        surface_average,
                    ),
                    adjust(
                        "g_vBurnishingColorTint",
                        "g_fBurnishingColor",
                        surface_average,
                    ),
                    adjust(
                        "g_vBurnishingColorTint",
                        "g_fBurnishingColor",
                        substrate_average,
                    ),
                ],
            }
        };
        let layers = [layer(1), layer(2), layer(3), layer(4)];
        let pattern_scale = pair("g_vPatternTexCoordScale", [1.0, 1.0]);
        let pattern_repeats = pattern_scale[0].abs().max(pattern_scale[1].abs());
        let distortion = float("g_fPatternNormalDistortion", 0.0) * 0.1;
        Ok(Self {
            layers,
            layer_id: loader.texture("g_tLayerId", 1.0, false),
            // Ids are read at full size: a smaller mip would blend them.
            tint_id: if int("F_TINT_ID", 0) != 0 {
                loader.texture("g_tTintId", 0.0, false)
            } else {
                None
            },
            object: loader.texture("g_tObjectProperties", 1.0, false),
            pattern: loader.texture("g_tPattern", pattern_repeats, true),
            pattern_properties: loader.texture("g_tPatternProperties", pattern_repeats, false),
            patterned: int("F_PATTERN", 0) != 0,
            paint_layer: int("F_PATTERN_PAINT", 0) != 0,
            puffy: int("F_PUFFY_PAINT", 0) != 0,
            respects_tint_mask: int("g_fPatternPaintRespectsTintMask", 0) != 0,
            id_colors: std::array::from_fn(|i| {
                let [r, g, b, a] = floats(&format!("g_vId{}Color", i + 1)).unwrap_or([1.0; 4]);
                [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
            }),
            id_pattern: std::array::from_fn(|i| {
                if int(&format!("g_bId{}Pattern", i + 1), 1) != 0 {
                    1.0
                } else {
                    0.0
                }
            }),
            flip: int("g_bFlipFixup", 0) != 0,
            pattern_at: Xform::new(
                pattern_scale,
                float("g_fPatternTexCoordRotation", 0.0),
                pair("g_vPatternTexCoordCenter", [0.5, 0.5]),
                pair("g_vPatternTexCoordOffset", [0.0, 0.0]),
            ),
            distortion: pattern_scale.map(|s| s * distortion),
            damage_at: Xform::turned(
                float("g_fDamageTexCoordRotation", 0.0),
                pair("g_vDamageTexCoordOffset", [0.0, 0.0]),
            ),
            grime_at: Xform::turned(
                float("g_fGrimeTexCoordRotation", 0.0),
                pair("g_vGrimeTexCoordOffset", [0.0, 0.0]),
            ),
            threshold: float("g_fPatternTranslucencyThreshold", 0.01),
            emboss: float("g_fPatternEmboss", 0.5),
            roughness_contrast: pair("g_vPatternRoughnessContrastBrightness", [1.0, 0.0]),
            // The colour pass copies the wear amount into the wear progress.
            wear: srgb_encode(
                float("g_fWearProgress", float("g_flWearAmount", 0.0)).clamp(0.0, 1.0),
            ),
        })
    }

    /// The four layer weights at (u, v): `g_tLayerId` RGB and what is left to 1, normalized.
    fn weights(&self, u: f32, v: f32) -> [f32; 4] {
        let id = self.layer_id.as_ref().map_or([0.0; 4], |t| t.at(u, v));
        let sum = id[0] + id[1] + id[2];
        let rest = (1.0 - sum).max(0.0);
        let total = sum + rest;
        [id[0] / total, id[1] / total, id[2] / total, rest / total]
    }

    /// The eight tint ids' weights at (u, v) (see the module docs), or `None` without tint ids.
    fn tint_ids(&self, u: f32, v: f32) -> Option<[f32; 8]> {
        let texture = self.tint_id.as_ref()?;
        let image = &texture.sampler.image;
        let (du, dv) = (1.0 / image.width as f32, 1.0 / image.height as f32);
        Some(id_weights(TINT_TAPS.map(|[x, y]| {
            (texture.nearest_r(u + x * du, v + y * dv) * 7.0).ceil()
        })))
    }

    /// The pattern at (u, v), moved by the surface's and the substrate's normals.
    fn pattern(&self, u: f32, v: f32, surface: [f32; 2], substrate: [f32; 2]) -> Pattern {
        let [pu, pv] = self.pattern_at.apply(if self.flip { -u } else { u }, v);
        let at = |n: [f32; 2]| {
            (
                pu - self.distortion[0] * n[0],
                pv - self.distortion[1] * n[1],
            )
        };
        let sample = |t: &Option<Arc<Texture>>, (u, v): (f32, f32)| {
            t.as_ref().map_or([0.0; 4], |t| t.at(u, v))
        };
        let mut color = sample(&self.pattern, at(surface));
        let properties = sample(&self.pattern_properties, at(surface));
        let alpha = color[3];
        let mut raise = alpha;
        if self.puffy {
            color[3] = smoothstep(self.threshold, self.threshold + 0.04, alpha);
            raise = (alpha * (1.0 + self.threshold) - self.threshold).clamp(0.0, 1.0);
        }
        Pattern {
            color,
            raise,
            under: sample(&self.pattern, at(substrate)),
            properties: [properties[0], properties[1]],
        }
    }

    /// Matrix `kind` of each layer, blended by `weights`, applied to `c`.
    fn adjust(&self, kind: usize, weights: [f32; 4], c: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        for (layer, w) in self.layers.iter().zip(weights) {
            if w > 0.0 {
                let a = layer.adjust[kind].apply(c);
                for k in 0..3 {
                    out[k] += a[k] * w;
                }
            }
        }
        out
    }

    /// The texel at (u, v) painted (see the module docs).
    pub(super) fn shade(&self, u: f32, v: f32) -> Shade {
        let weights = self.weights(u, v);
        // The id tint, and how much of the texel the pattern's ids cover. Without tint ids: white, of no strength,
        // the pattern everywhere.
        let (id_tint, pattern_ids) = match self.tint_ids(u, v) {
            Some(ids) => {
                let mut sum = [0.0f32; 4];
                for (c, w) in self.id_colors.iter().zip(ids) {
                    for k in 0..3 {
                        sum[k] += c[k] * c[3] * w;
                    }
                    sum[3] += c[3] * w;
                }
                let tint = if sum[3] > 0.0 {
                    [sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3], sum[3]]
                } else {
                    [1.0, 1.0, 1.0, sum[3]]
                };
                let covered = ids.iter().zip(self.id_pattern).map(|(w, p)| w * p).sum();
                (tint, covered)
            }
            None => ([1.0, 1.0, 1.0, 0.0], 1.0),
        };

        // The layers.
        let damage_uv = self.damage_at.apply(u, v);
        let grime_uv = self.grime_at.apply(u, v);
        let mut b = Blend::default();
        for (layer, w) in self.layers.iter().zip(weights) {
            if w > 0.0 {
                layer.add(&mut b, w, [u, v], damage_uv, grime_uv);
            }
        }
        let values = b.values;
        let pattern = self.patterned.then(|| {
            self.pattern(
                u,
                v,
                normal_xy(b.surface_normal),
                normal_xy(b.substrate_normal),
            )
        });

        // The tints: the ids', with the pattern joined in when it is not painted as a layer of its own.
        let (surface_tint, substrate_tint, coverage) = match &pattern {
            Some(p) if !self.paint_layer => {
                let coverage = pattern_ids * p.color[3];
                let untinted = if id_tint[3] == 0.0 { 1.0 } else { 0.0 };
                let share = if id_tint[3] > 0.0 {
                    (p.color[3] / id_tint[3]).clamp(0.0, 1.0)
                } else {
                    untinted
                };
                let with = |c: [f32; 3], a: f32| [c[0], c[1], c[2], a];
                (
                    with(
                        lerp3(rgb(id_tint), rgb(p.color), share * coverage),
                        id_tint[3].max(p.color[3] * coverage),
                    ),
                    with(
                        lerp3(
                            rgb(id_tint),
                            rgb(p.under),
                            untinted.max(p.under[3]) * coverage,
                        ),
                        id_tint[3].max(p.under[3] * coverage),
                    ),
                    coverage,
                )
            }
            _ => (id_tint, id_tint, 0.0),
        };
        let mask = |a: f32| {
            if self.respects_tint_mask {
                a
            } else {
                a.max(coverage)
            }
        };
        let (surface_mask, substrate_mask) = (mask(b.surface[3]), mask(b.substrate[3]));
        let (surface_rgb, substrate_rgb) = (rgb(b.surface), rgb(b.substrate));
        let adjusted = |kind: usize, c: [f32; 3]| self.adjust(kind, weights, c);
        let mut surface_color = lerp3(
            surface_rgb,
            tint_transfer(
                adjusted(SURFACE, surface_rgb),
                surface_tint,
                b.surface_properties[1],
                surface_tint[3],
            ),
            surface_mask,
        );
        let translucent = substrate_tint[3] * values.substrate_translucency;
        let substrate_color = lerp3(
            substrate_rgb,
            tint_transfer(
                adjusted(SUBSTRATE, substrate_rgb),
                substrate_tint,
                b.substrate_properties[1],
                translucent,
            ),
            substrate_mask,
        );
        let mut damage_color = lerp3(
            surface_color,
            tint_transfer(
                adjusted(DAMAGE, surface_rgb),
                substrate_tint,
                values.bevel_metalness,
                translucent,
            ),
            if b.bevel_tint_mask { surface_mask } else { 1.0 },
        );
        let surface_burnished = adjusted(SURFACE_BURNISHING, surface_rgb);
        let substrate_burnished = adjusted(SUBSTRATE_BURNISHING, substrate_rgb);
        // Burnished from the damage colour before puffy paint takes it over, as the program does.
        let damage_burnished = adjusted(SURFACE_BURNISHING, damage_color);
        let damage_rough = {
            let [x, z] =
                [b.surface_normal[0], b.surface_normal[2]].map(|r| r * values.bevel_roughness);
            [x, lerp(x, z, values.bevel_anisotropy)]
        };

        // The surface's own values, and the pattern painted over it.
        let mut surface_rough = [b.surface_normal[0], b.surface_normal[2]];
        let mut surface_metal = b.surface_properties[1];
        let mut surface_height = b.surface_properties[3];
        let mut height_softness = values.height_softness;
        let mut bevel_softness = values.bevel_softness;
        match &pattern {
            Some(_) if !self.paint_layer => {
                let [contrast, brightness] = self.roughness_contrast;
                surface_rough = surface_rough.map(|r| {
                    lerp(
                        r,
                        ((r - 0.5 + brightness) * contrast + 0.5).clamp(0.0, 1.0),
                        coverage * surface_mask,
                    )
                });
            }
            Some(p) => {
                let painted = pattern_ids
                    * p.color[3]
                    * if self.respects_tint_mask {
                        surface_mask
                    } else {
                        1.0
                    };
                let paint = rgb(p.color);
                let over = lerp3(surface_color, paint, painted);
                if self.puffy {
                    let raised = (p.raise - self.threshold).clamp(0.0, 1.0) * painted;
                    if self.emboss >= 0.0 {
                        surface_height += raised;
                    }
                    damage_color = lerp3(over, paint, painted);
                    height_softness = lerp(height_softness, 0.0, raised);
                    bevel_softness = lerp(bevel_softness, 0.01, raised);
                }
                surface_color = over;
                surface_metal = lerp(surface_metal, p.properties[0], painted);
                surface_rough = surface_rough.map(|r| lerp(r, p.properties[1], painted));
            }
            None => {}
        }

        // Damage: where the surface wears through to the substrate, and the bevel along its edge.
        let object = self
            .object
            .as_ref()
            .map_or([1.0, 0.5, 0.0, 1.0], |t| t.at(u, v));
        let w = self.wear;
        let substrate_height = b.substrate_properties[3];
        let above = surface_height - substrate_height;
        let [low, high] = values.damage;
        let depth =
            (2.0 - (w * (high - low) + low)) - b.damage - object[1] - surface_height + above;
        let soft = height_softness.max(1e-6);
        let surface_left = smoothstep(-height_softness, height_softness, depth * above / soft);
        let exposed = surface_height
            .min(depth.clamp(0.0, 1.0))
            .max(substrate_height);
        let bevel_t = depth / bevel_softness.max(1e-6);
        let bevel = smoothstep(1.0 + bevel_softness, 1.0 - bevel_softness, bevel_t);
        let bevel_t = bevel_t.clamp(0.0, 1.0);
        let burnish = ((w - 1.0) + object[2]).clamp(0.0, 1.0) * (exposed + w);
        let substrate_burnish = smoothstep(
            values.substrate_burnishing[0],
            values.substrate_burnishing[1],
            burnish,
        );
        let surface_burnish = smoothstep(
            values.surface_burnishing[0],
            values.surface_burnishing[1],
            burnish,
        );
        let burnish_clamped = burnish.clamp(0.0, 1.0);
        let edge = ((exposed + 0.01) / soft).clamp(0.0, 1.0)
            * (1.0 - ((exposed - 0.01) / soft).clamp(0.0, 1.0));
        let occlusion = object[0]
            * lerp(
                lerp(
                    b.substrate_properties[0],
                    b.surface_properties[0],
                    surface_left,
                ) * (1.0 - edge * surface_left * surface_height * 0.75).clamp(0.0, 1.0),
                1.0,
                substrate_burnish,
            );
        let under_color = lerp3(substrate_color, substrate_burnished, substrate_burnish);
        let rough_scale = lerp(
            1.0,
            lerp(
                b.burnishing_roughness[1],
                b.burnishing_roughness[0],
                bevel_t,
            ),
            burnish_clamped,
        );
        let rough: [f32; 2] = std::array::from_fn(|i| {
            lerp(
                [b.substrate_normal[0], b.substrate_normal[2]][i],
                lerp(surface_rough[i], damage_rough[i], bevel),
                surface_left,
            ) * rough_scale
        });

        // Grime.
        let grimed = exposed * occlusion * (w * 2.0);
        let grime = lerp(
            smoothstep(values.substrate_grime[0], values.substrate_grime[1], grimed),
            smoothstep(values.surface_grime[0], values.surface_grime[1], grimed),
            lerp(surface_left, 1.0 - bevel, values.bevel_to_substrate),
        ) * (1.0 - rough[0].min(rough[1]));
        let grime = lerp(grime, grime.max(values.burnishing_grime), burnish_clamped)
            * values.grime_translucency;
        let rough = rough.map(|r| lerp(r, rough[0] * values.grime_roughness, grime));
        let metalness = lerp(
            lerp(
                b.substrate_properties[1],
                lerp(surface_metal, values.bevel_metalness, bevel),
                surface_left,
            ),
            values.burnishing_metalness,
            burnish_clamped,
        ) * (1.0 - grime);
        let color = lerp3(
            lerp3(
                under_color,
                lerp3(
                    lerp3(surface_color, surface_burnished, surface_burnish),
                    lerp3(
                        lerp3(damage_color, damage_burnished, surface_burnish),
                        under_color,
                        values.bevel_to_substrate * (1.0 - bevel_t),
                    ),
                    bevel,
                ),
                surface_left,
            ),
            b.grime,
            grime,
        );
        Shade {
            color: clamp3(color),
            roughness: ((rough[0] + rough[1]) * 0.5).clamp(0.0, 1.0),
            metalness: metalness.clamp(0.0, 1.0),
        }
    }
}

/// Each tint id's weight from the ids at the 9 taps ([`TINT_TAPS`]): the centre's where its column or its row agrees
/// with it, else a count weighted 1/32 at the corners, 3/32 at the sides, 1/2 at the centre — with the top-right
/// corner where the top side belongs, as the program has it.
fn id_weights(taps: [f32; 9]) -> [f32; 8] {
    std::array::from_fn(|id| {
        let hit = |k: usize| if taps[k] == id as f32 { 1.0 } else { 0.0 };
        if (hit(4) == hit(1) && hit(4) == hit(7)) || (hit(4) == hit(3) && hit(4) == hit(5)) {
            hit(4)
        } else {
            0.03125 * (hit(0) + hit(2) + hit(6) + hit(8))
                + 0.09375 * (hit(2) + hit(3) + hit(5) + hit(7))
                + 0.5 * hit(4)
        }
    })
}

/// X and Y of the hemi-octahedral normal a layer texture stores in G and A, as the program decodes it.
fn normal_xy(t: [f32; 4]) -> [f32; 2] {
    let x = t[3] + t[1] - 1.003_921_6;
    let y = t[3] - t[1];
    let z = 1.0 - x.abs() - y.abs();
    let length = (x * x + y * y + z * z).sqrt();
    if length > 0.0 {
        [x / length, y / length]
    } else {
        [0.0; 2]
    }
}

/// The tint carried onto a colour, by `amount`: brought towards the tint's luminance (within bounds set by the
/// colour's own luminance and `metalness`), then towards the tint's hue pushed off grey, by the tint's saturation.
fn tint_transfer(c: [f32; 3], tint: [f32; 4], metalness: f32, amount: f32) -> [f32; 3] {
    let t = rgb(tint);
    let lum = luminance(t);
    if lum <= 0.0 {
        return c;
    }
    let normalize = |v: [f32; 3]| {
        let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        (length > 0.0).then(|| v.map(|x| x / length))
    };
    let Some(hue) = normalize(t) else {
        return c;
    };
    let max = t[0].max(t[1]).max(t[2]);
    let saturation = (max - t[0].min(t[1]).min(t[2])) / max;
    let off_grey = hue.map(|x| x - 0.577);
    let Some(direction) = normalize(off_grey) else {
        return c;
    };
    let pushed = clamp3(direction.map(|x| x * 2.0 + 1.0));
    let own = (c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11).max(0.001);
    let scale = lerp(
        1.0,
        (1.0 + 4.0 * (lum - 0.5))
            .max(lerp(0.03, 0.134, metalness) / own)
            .min(lerp(0.9, 0.98, metalness) / own),
        amount,
    );
    let along = (off_grey.iter().sum::<f32>() * 0.577).abs();
    lerp3(
        scale3(c, scale),
        scale3(pushed, lum / luminance(pushed).max(1e-6)),
        (saturation * along.powf(0.2)).clamp(0.0, 1.0) * amount,
    )
}

/// `SrgbLinearToGamma`: a linear value sRGB-encoded.
fn srgb_encode(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placements_scale_from_the_corner_and_turn_about_the_centre() {
        // No turn: scaled from (0, 0), then the offset.
        let p = Xform::new([2.0, 3.0], 0.0, [0.5, 0.5], [0.1, -0.2]);
        let [u, v] = p.apply(0.5, 0.5);
        assert!((u - 1.1).abs() < 1e-6 && (v - 1.3).abs() < 1e-6, "{u} {v}");
        // A quarter turn about the middle sends (1, 0.5) to (0.5, 1).
        let [u, v] = Xform::turned(90.0, [0.0; 2]).apply(1.0, 0.5);
        assert!((u - 0.5).abs() < 1e-5 && (v - 1.0).abs() < 1e-5, "{u} {v}");
    }

    #[test]
    fn tint_ids_keep_edges_and_weigh_lone_texels() {
        // Inside a region, and along a straight edge: the centre's id, whole.
        assert_eq!(id_weights([3.0; 9])[3], 1.0);
        let edge = id_weights([1.0, 2.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0, 2.0]);
        assert_eq!((edge[1], edge[2]), (0.0, 1.0));
        // A lone texel keeps half, its neighbours' id takes the rest.
        let mut lone = [5.0; 9];
        lone[4] = 2.0;
        let weights = id_weights(lone);
        assert_eq!((weights[2], weights[5]), (0.5, 0.5));
    }

    #[test]
    fn a_flat_normal_decodes_flat() {
        let [x, y] = normal_xy(FLAT);
        assert!(x.abs() < 1e-3 && y.abs() < 1e-6, "{x} {y}");
    }

    #[test]
    fn the_tint_transfer_needs_an_amount() {
        let c = [0.3, 0.2, 0.1];
        let blue = [0.05, 0.1, 0.8, 1.0];
        // No amount, or a black tint: unchanged.
        assert_eq!(tint_transfer(c, blue, 0.0, 0.0), c);
        assert_eq!(tint_transfer(c, [0.0, 0.0, 0.0, 1.0], 0.0, 1.0), c);
        // A full one turns the colour blue.
        let t = tint_transfer(c, blue, 0.0, 1.0);
        assert!(t[2] > t[0] && t[2] > t[1], "{t:?}");
    }

    #[test]
    fn halving_averages_four_texels() {
        let mut rgba = [0, 100, 200, 255].repeat(4);
        rgba[0] = 40;
        let half = halve(&Image::new(2, 2, rgba));
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half.rgba, vec![10, 100, 200, 255]);
    }

    #[test]
    fn the_wear_is_gamma_encoded() {
        assert!((srgb_encode(0.1633) - 0.4405).abs() < 1e-3);
        assert_eq!(srgb_encode(0.0), 0.0);
        assert!((srgb_encode(1.0) - 1.0).abs() < 1e-6);
    }
}
