//! CS2's environment shaders: `csgo_environment` (one layer) and `csgo_environment_blend` (two layers blended by
//! height), the materials of the newer maps — 334 + 369 of de_inferno's 922.
//!
//! Where the knowledge comes from:
//! - Valve's compiled shaders as decompiled by Source 2 Viewer and tracked in SteamTracking's GameTracking-CS2
//!   (`csgo_environment_blend.slang`): parameter names and defaults, texture channels, and the default pixel
//!   shader path (plain height blend, no border/bevel or overlay) — certain.
//! - Source 2 Viewer's (MIT) reconstruction of the `MatrixColorCorrect2` and `MatrixColorTint2` expression
//!   functions, which Valve's files only name — not checked against the game yet.
//!
//! Textures of layer N:
//!
//! | Texture | R | G | B | A |
//! |---|---|---|---|---|
//! | `g_tColorN` (sRGB) | colour | colour | colour | ambient occlusion (translucency with `F_ALPHA_TEST`) |
//! | `g_tHeightN` | height | tint mask | ambient occlusion with `F_ALPHA_TEST` | metalness |
//! | `g_tNormalN` | hemi-octahedral normal | hemi-octahedral normal | roughness | — |
//!
//! glTF has no place for the per-layer colour adjustments, so OFAST bakes them: [`bake_color`] writes the adjusted
//! colour, [`bake_surface`] a glTF metallic-roughness texture (R occlusion, G roughness, B metalness). The height
//! blend itself depends on per-vertex paint and is rebuilt in Unreal (`import_map.py`) from [`HeightBlend`].
//!
//! A bake is keyed only by what changes its pixels ([`ColorJob::new`], [`SurfaceJob::new`]): a colour bake ignores
//! the surface settings and a surface bake the colour ones, a model tint counts only where the layer takes one
//! (`g_bModelTintN`, a tint mask that is not zero), and a colour that nothing changes is not baked at all — the
//! material uses the plain colour texture. de_inferno's placements carry 100+ different tints on one plaster texture,
//! most of them on layers with the model tint switched off; before this rule each wrote its own 2048² copy (1049
//! colour bakes, 8.9 GB; now 555 files, 3.3 GB). The glTF writer then stores identical pixels once, whatever made
//! them, and names a bake after its pixel hash (see "Texture files" in `gltf.rs`). The table of what counts is in
//! `docs/formats/material.md`.

use crate::content::Content;
use crate::export::ExportError;
use crate::export::gltf::{MaterialInfo, decode_raw};
use crate::texture::Image;
use std::cmp::Ordering;
use std::sync::LazyLock;

/// `csgo_environment`: one layer with colour adjustments.
pub(crate) const ENVIRONMENT: &str = "csgo_environment.vfx";
/// `csgo_environment_blend`: two layers mixed by painted weight and height maps.
pub(crate) const ENVIRONMENT_BLEND: &str = "csgo_environment_blend.vfx";

/// `true` for the shaders this module handles.
pub(crate) fn is_environment(shader: &str) -> bool {
    shader == ENVIRONMENT || shader == ENVIRONMENT_BLEND
}

/// Rec. 709 luminance weights, as the shaders write them.
const LUMA: [f32; 3] = [0.2125, 0.7154, 0.0721];

/// Colour and surface settings of one layer, with the shader's defaults for missing parameters.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Layer {
    /// `g_fTextureColorContrastN`, `…SaturationN`, `…BrightnessN` (defaults 1).
    pub(crate) csb: [f32; 3],
    /// `g_vTextureColorTintN` (default white, no tint).
    pub(crate) texture_tint: [f32; 3],
    /// `g_nColorCorrectionModeN` = 1, "Mask only Tint": contrast, saturation and brightness apply everywhere and only
    /// the tint follows the tint mask. 0, "Mask All" (default): all of them follow the tint mask.
    pub(crate) mask_only_tint: bool,
    /// `g_fTintMaskContrastN`, `g_fTintMaskBrightnessN`, applied to the height texture's G.
    pub(crate) tint_mask: [f32; 2],
    /// `g_fTextureRoughnessContrastN`, `g_fTextureRoughnessBrightnessN`, applied to the normal texture's B.
    pub(crate) roughness: [f32; 2],
    /// `g_bMetalnessN` (default on): metalness is the height texture's A.
    pub(crate) metalness: bool,
    /// `g_vAmbientOcclusionLevelsN` after the shader's expression: occlusion = saturate(lerp(x, z, a^y)).
    pub(crate) ao_levels: [f32; 3],
    /// `g_flModelTintAmount` × `g_bModelTintN`: how much a model tint recolours the tint-masked area.
    pub(crate) model_tint: f32,
}

impl Layer {
    /// Reads layer `n` (1 or 2) of a material.
    pub(crate) fn read(info: &MaterialInfo, n: u8) -> Self {
        let float = |name: &str, default: f32| {
            info.floats
                .get(&format!("{name}{n}"))
                .map_or(default, |v| *v as f32)
        };
        let int = |name: &str, default: i64| {
            info.ints
                .get(&format!("{name}{n}"))
                .copied()
                .unwrap_or(default)
        };
        let vector = |name: &str, default: [f32; 3]| {
            info.vectors
                .get(&format!("{name}{n}"))
                .map_or(default, |v| [v[0] as f32, v[1] as f32, v[2] as f32])
        };
        // The shader declares Default2(0, 0.5), so z is 0 when the material does not set it; most authored
        // materials set (0, 0.5, 1). Expression: (−x, −log2(max(0.0001, 1 − y)), 2 − z).
        let ao = vector("g_vAmbientOcclusionLevels", [0.0, 0.5, 0.0]);
        Self {
            csb: [
                float("g_fTextureColorContrast", 1.0),
                float("g_fTextureColorSaturation", 1.0),
                float("g_fTextureColorBrightness", 1.0),
            ],
            texture_tint: vector("g_vTextureColorTint", [1.0; 3]),
            mask_only_tint: int("g_nColorCorrectionMode", 0) == 1,
            tint_mask: [
                float("g_fTintMaskContrast", 1.0),
                float("g_fTintMaskBrightness", 1.0),
            ],
            roughness: [
                float("g_fTextureRoughnessContrast", 1.0),
                float("g_fTextureRoughnessBrightness", 1.0),
            ],
            metalness: int("g_bMetalness", 1) != 0,
            ao_levels: [-ao[0], -(1.0 - ao[1]).max(0.0001).log2(), 2.0 - ao[2]],
            model_tint: info
                .floats
                .get("g_flModelTintAmount")
                .map_or(1.0, |v| *v as f32)
                * if int("g_bModelTint", 1) != 0 {
                    1.0
                } else {
                    0.0
                },
        }
    }

    /// All settings as bits, so a layer can key a texture job (f32 has no total order).
    fn key(&self) -> [u32; 16] {
        let f = |v: f32| v.to_bits();
        [
            f(self.csb[0]),
            f(self.csb[1]),
            f(self.csb[2]),
            f(self.texture_tint[0]),
            f(self.texture_tint[1]),
            f(self.texture_tint[2]),
            u32::from(self.mask_only_tint),
            f(self.tint_mask[0]),
            f(self.tint_mask[1]),
            f(self.roughness[0]),
            f(self.roughness[1]),
            u32::from(self.metalness),
            f(self.ao_levels[0]),
            f(self.ao_levels[1]),
            f(self.ao_levels[2]),
            f(self.model_tint),
        ]
    }

    /// The tint mask at a height-texture G value.
    fn tint_mask_at(&self, g: f32) -> f32 {
        contrast_brightness(g, self.tint_mask[0], self.tint_mask[1])
    }

    /// The final linear colour of a texel: colour correction and texture tint, masked as the mode says, then the
    /// model tint (linear RGB; white = none). `average` is the colour texture's average (its reflectivity).
    pub(crate) fn adjust_color(
        &self,
        rgb: [f32; 3],
        average: [f32; 3],
        mask: f32,
        tint: [f32; 3],
    ) -> [f32; 3] {
        let corrected = color_correct(rgb, self.csb, average);
        let adjusted = color_tint(corrected, self.texture_tint);
        let unmasked = if self.mask_only_tint { corrected } else { rgb };
        let color = lerp3(unmasked, adjusted, mask);
        model_tint(color, tint, self.model_tint * mask)
    }
}

impl PartialEq for Layer {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Layer {}
impl PartialOrd for Layer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Layer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

/// `saturate(((x − 0.5) · contrast + 0.5) · brightness)`, the shaders' adjustment of masks and roughness.
pub(crate) fn contrast_brightness(x: f32, contrast: f32, brightness: f32) -> f32 {
    (((x - 0.5) * contrast + 0.5) * brightness).clamp(0.0, 1.0)
}

/// `MatrixColorCorrect2((contrast, saturation, brightness), average)`: contrast around the texture's average
/// colour, then brightness, then saturation around a grey. Source 2 Viewer builds it as a matrix chain (scale by the
/// normalized luminance weights l, rotate l onto Z, scale X/Y by the saturation, undo); worked out, the grey is
/// Σ cᵢ·lᵢ² / Σ lⱼ² — weights ≈ (0.080, 0.910, 0.009) — and the colour moves away from it by the saturation.
pub(crate) fn color_correct(rgb: [f32; 3], csb: [f32; 3], average: [f32; 3]) -> [f32; 3] {
    let [contrast, saturation, brightness] = csb;
    let v: [f32; 3] =
        std::array::from_fn(|i| ((rgb[i] - average[i]) * contrast + average[i]) * brightness);
    let grey = dot(v, GREY_WEIGHTS);
    v.map(|c| grey + saturation * (c - grey))
}

/// Rec. 709 luminance weights as Source 2 Viewer's colour matrices use them (normalized there).
const REC709: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Σ lᵢ² normalized, for l = [`REC709`]: the grey that saturation pivots on.
const GREY_WEIGHTS: [f32; 3] = {
    const L: [f32; 3] = REC709;
    const SUM: f32 = L[0] * L[0] + L[1] * L[1] + L[2] * L[2];
    [L[0] * L[0] / SUM, L[1] * L[1] / SUM, L[2] * L[2] / SUM]
};

/// `MatrixColorTint2(tint, 1)`: pulls a colour towards a tint by the tint's saturation s = (max − min) / max. In
/// the space where each channel is scaled by the normalized luminance weight, the chroma part moves to the tint's by
/// s and the luminance part by s² (Source 2 Viewer's reconstruction; white is no change). Valve's expression
/// multiplies the tint matrix with the colour-correction one; OFAST applies the correction first, then the tint —
/// the order is not checked against the game yet.
pub(crate) fn color_tint(rgb: [f32; 3], tint: [f32; 3]) -> [f32; 3] {
    let s = saturation(tint);
    if s == 0.0 {
        return rgb;
    }
    let l = normalized(REC709);
    let u: [f32; 3] = std::array::from_fn(|i| rgb[i] * l[i]);
    let t: [f32; 3] = std::array::from_fn(|i| tint[i] * l[i]);
    let (u_par, t_par) = (dot(u, l), dot(t, l));
    let par = u_par + (t_par - u_par) * s * s;
    std::array::from_fn(|i| {
        let (u_perp, t_perp) = (u[i] - u_par * l[i], t[i] - t_par * l[i]);
        (u_perp + (t_perp - u_perp) * s + par * l[i]) / l[i]
    })
}

/// The shaders' model tint: towards the tint's hue at the same luminance (capped at 3 × luminance × the tint's
/// brightest channel), by `strength` × (1 − the tint's darkest channel). A white tint changes nothing.
pub(crate) fn model_tint(rgb: [f32; 3], tint: [f32; 3], strength: f32) -> [f32; 3] {
    let amount = strength * (1.0 - tint[0].min(tint[1]).min(tint[2]));
    let hue = normalized(tint.map(|c| c.max(0.001)));
    let y = dot(rgb, LUMA);
    let target =
        hue.map(|c| c * (y / dot(hue, LUMA)).min(3.0 * y * tint[0].max(tint[1]).max(tint[2])));
    lerp3(rgb, target, amount).map(|c| c.clamp(0.0, 1.0))
}

/// The height blend of `csgo_environment_blend` (`g_flBlendSoftness2`, `g_flHeightMapZeroPointN`,
/// `g_flHeightMapScaleN`, `g_flMaskWithHeight2`). The weights are painted per vertex (`TEXCOORD 4`, semantic
/// `VertexPaintBlendParams`): x = layer 2, w = extra softness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HeightBlend {
    /// Softness of the transition (default 0.01), plus the vertex's w.
    pub(crate) softness: f32,
    /// Height that counts as "zero" per layer (default 0.5).
    pub(crate) zero_point: [f32; 2],
    /// Height scale per layer (default 1).
    pub(crate) scale: [f32; 2],
    /// How much layer 2's own height masks it (default 0).
    pub(crate) mask_with_height: f32,
}

impl HeightBlend {
    /// Reads the blend settings of a material.
    pub(crate) fn read(info: &MaterialInfo) -> Self {
        let float = |name: &str, default: f32| info.floats.get(name).map_or(default, |v| *v as f32);
        Self {
            softness: float("g_flBlendSoftness2", 0.01),
            zero_point: [
                float("g_flHeightMapZeroPoint1", 0.5),
                float("g_flHeightMapZeroPoint2", 0.5),
            ],
            scale: [
                float("g_flHeightMapScale1", 1.0),
                float("g_flHeightMapScale2", 1.0),
            ],
            mask_with_height: float("g_flMaskWithHeight2", 0.0),
        }
    }

    /// Layer 2's weight for heights `h` (the height textures' R) and the vertex paint x and w — the default path of
    /// Valve's pixel shader without its view-distance softening. `import_map.py` writes the same formula in HLSL.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the reference for import_map.py's HLSL, checked by the tests"
        )
    )]
    pub(crate) fn weight(&self, h: [f32; 2], paint_x: f32, paint_w: f32) -> f32 {
        let paint = (paint_x * 1.1 - 0.05).clamp(0.0, 1.0);
        let soft = (paint_w + self.softness).clamp(0.0001, 1.0);
        let [z1, z2] = self.zero_point;
        let (s1, s2) = (self.scale[0] + soft, self.scale[1] + soft);
        let h1 = (h[0] - z1) * s1;
        let slope = if s2 - 2.0 * soft == 0.0 {
            0.0001
        } else {
            s2 - 2.0 * soft
        };
        let low = -z1 * s1 - (1.0 - z2) * s2 - soft;
        let high = (1.0 - z1) * s1 + z2 * s2;
        let h2 = (h[1] - z2) * slope + low + (high - low) * paint;
        let top = h1.max(h2);
        let (w1, w2) = ((h1 - top + soft).max(0.0), (h2 - top + soft).max(0.0));
        let mask = 0.5 + (h[1] - 0.5) * self.mask_with_height;
        (w2 / (w1 + w2 + 0.0001) * mask * 2.0).clamp(0.0, 1.0)
    }
}

/// Settings that change nothing (the neutral value of each), put in place of the settings a bake does not read so
/// that two layers differing only there make the same job.
const NEUTRAL: Layer = Layer {
    csb: [1.0; 3],
    texture_tint: [1.0; 3],
    mask_only_tint: false,
    tint_mask: [1.0, 1.0],
    roughness: [1.0, 1.0],
    metalness: true,
    ao_levels: [0.0, 1.0, 1.0],
    model_tint: 0.0,
};

/// A baked colour texture to write: a layer's colour after its adjustments and a model tint. Made by
/// [`ColorJob::new`], which keeps only what changes the result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ColorJob {
    /// `g_tColorN`.
    pub(crate) color: String,
    /// `g_tHeightN` (tint mask in G).
    pub(crate) height: Option<String>,
    /// The layer's colour settings (the surface settings are [`NEUTRAL`]).
    pub(crate) layer: Layer,
    /// Model tint, sRGB bytes (white = none).
    pub(crate) tint: [u8; 3],
    /// Keep the colour texture's alpha (translucent and alpha-tested materials).
    pub(crate) keep_alpha: bool,
}

impl ColorJob {
    /// The bake of layer colour `color` with tint mask texture `height`, or `None` when the adjustments and the
    /// model tint `tint` (sRGB bytes) leave every texel as it is: the plain colour texture is then exact (a bake would
    /// round dark values through the linear table, `linear_to_srgb`).
    ///
    /// Settings that cannot change the result are reset, so layers that differ only there share one bake:
    /// - the surface settings (roughness, metalness, occlusion);
    /// - the model tint when it is white or the layer takes none (`g_bModelTintN` 0 or `g_flModelTintAmount` 0) —
    ///   de_inferno's tinted placements mostly use layers without it;
    /// - a texture tint without saturation (white or grey), which [`color_tint`] leaves alone.
    pub(crate) fn new(
        color: String,
        height: Option<String>,
        layer: Layer,
        tint: [u8; 3],
        keep_alpha: bool,
    ) -> Option<Self> {
        let corrects = layer.csb != NEUTRAL.csb;
        let tints = saturation(layer.texture_tint) != 0.0;
        // The mode only matters with a correction: without one, "corrected" is the colour itself.
        let mask_only_tint = layer.mask_only_tint && corrects;
        // Without a height texture the mask is one value everywhere; at 0 it hides everything it governs.
        let masked_out = height.is_none() && layer.tint_mask_at(1.0) == 0.0;
        let model_tinted = layer.model_tint != 0.0 && tint != [255; 3] && !masked_out;
        // "Mask All" multiplies every adjustment by the mask; "Mask only Tint" applies the correction everywhere.
        let adjusted = if masked_out {
            mask_only_tint
        } else {
            corrects || tints
        };
        if !adjusted && !model_tinted {
            return None;
        }
        // The mask governs the model tint, the texture tint and, in "Mask All", the correction.
        let masked = model_tinted || tints || (corrects && !mask_only_tint);
        let layer = Layer {
            csb: layer.csb,
            texture_tint: if tints {
                layer.texture_tint
            } else {
                NEUTRAL.texture_tint
            },
            mask_only_tint,
            tint_mask: if masked {
                layer.tint_mask
            } else {
                NEUTRAL.tint_mask
            },
            model_tint: if model_tinted { layer.model_tint } else { 0.0 },
            ..NEUTRAL
        };
        Some(Self {
            color,
            height: if masked { height } else { None },
            layer,
            tint: if model_tinted { tint } else { [255; 3] },
            keep_alpha,
        })
    }
}

/// A baked glTF metallic-roughness texture to write for a layer. Made by [`SurfaceJob::new`], which keeps only what
/// changes the result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SurfaceJob {
    /// `g_tColorN` (occlusion in A, unless alpha-tested).
    pub(crate) color: Option<String>,
    /// `g_tNormalN` (roughness in B).
    pub(crate) normal: Option<String>,
    /// `g_tHeightN` (metalness in A, occlusion in B when alpha-tested).
    pub(crate) height: Option<String>,
    /// The layer's surface settings (the colour settings are [`NEUTRAL`]).
    pub(crate) layer: Layer,
    /// `F_ALPHA_TEST`: the colour's alpha is translucency, occlusion comes from the height texture.
    pub(crate) alpha_test: bool,
}

impl SurfaceJob {
    /// The surface bake of a layer. The colour settings are reset, and a texture the bake does not read is left
    /// out: the colour texture when alpha-tested (occlusion then comes from the height texture), the height texture
    /// when it holds neither metalness nor occlusion.
    pub(crate) fn new(
        color: Option<String>,
        normal: Option<String>,
        height: Option<String>,
        layer: Layer,
        alpha_test: bool,
    ) -> Self {
        Self {
            color: if alpha_test { None } else { color },
            normal,
            height: if layer.metalness || alpha_test {
                height
            } else {
                None
            },
            layer: Layer {
                roughness: layer.roughness,
                metalness: layer.metalness,
                ao_levels: layer.ao_levels,
                ..NEUTRAL
            },
            alpha_test,
        }
    }
}

/// A colour's saturation as [`color_tint`] reads it: (max − min) / max, 0 for black.
fn saturation(rgb: [f32; 3]) -> f32 {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    if max > 0.0 { (max - min) / max } else { 0.0 }
}

/// sRGB byte → linear, as a table.
static SRGB_TO_LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| {
    std::array::from_fn(|i| crate::export::gltf::srgb_to_linear(i as f32 / 255.0))
});

/// Linear (0–1) → sRGB byte, through a 4096-step table.
fn linear_to_srgb(c: f32) -> u8 {
    static TABLE: LazyLock<Vec<u8>> = LazyLock::new(|| {
        (0..4096)
            .map(|i| {
                let c = i as f32 / 4095.0;
                let s = if c <= 0.003_130_8 {
                    c * 12.92
                } else {
                    1.055 * c.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0).round() as u8
            })
            .collect()
    });
    TABLE[(c.clamp(0.0, 1.0) * 4095.0).round() as usize]
}

/// Byte `channel` of `image` at output pixel (x, y) of a `width` × `height` output (nearest sample).
fn sample(image: &Image, x: usize, y: usize, width: usize, height: usize, channel: usize) -> u8 {
    let sx = x * image.width / width;
    let sy = y * image.height / height;
    image.rgba[(sy * image.width + sx) * 4 + channel]
}

/// Writes a layer's adjusted colour at the colour texture's size.
pub(crate) fn bake_color(content: &Content, job: &ColorJob) -> Result<Image, ExportError> {
    let (header, color) = decode_raw(content, &job.color)?;
    let height = match &job.height {
        Some(path) => Some(decode_raw(content, path)?.1),
        None => None,
    };
    let average = [
        header.reflectivity[0],
        header.reflectivity[1],
        header.reflectivity[2],
    ];
    let tint = job.tint.map(|c| SRGB_TO_LINEAR[usize::from(c)]);
    let (width, h) = (color.width, color.height);
    let mut rgba = Vec::with_capacity(width * h * 4);
    for y in 0..h {
        for x in 0..width {
            let px = &color.rgba[(y * width + x) * 4..][..4];
            let mask = match &height {
                Some(img) => job
                    .layer
                    .tint_mask_at(f32::from(sample(img, x, y, width, h, 1)) / 255.0),
                None => job.layer.tint_mask_at(1.0),
            };
            let rgb = [px[0], px[1], px[2]].map(|c| SRGB_TO_LINEAR[usize::from(c)]);
            let out = job.layer.adjust_color(rgb, average, mask, tint);
            rgba.extend(out.map(linear_to_srgb));
            rgba.push(if job.keep_alpha { px[3] } else { 255 });
        }
    }
    Ok(Image::new(width, h, rgba))
}

/// Writes a layer's glTF metallic-roughness texture (R occlusion, G roughness, B metalness) at the size of its
/// largest source.
pub(crate) fn bake_surface(content: &Content, job: &SurfaceJob) -> Result<Image, ExportError> {
    let load = |path: &Option<String>| -> Result<Option<Image>, ExportError> {
        path.as_ref()
            .map(|p| decode_raw(content, p).map(|(_, image)| image))
            .transpose()
    };
    let (color, normal, height) = (load(&job.color)?, load(&job.normal)?, load(&job.height)?);
    let (width, h) = [&color, &normal, &height]
        .iter()
        .filter_map(|img| img.as_ref().map(|i| (i.width, i.height)))
        .max_by_key(|(w, h)| w * h)
        .unwrap_or((1, 1));
    let layer = &job.layer;
    let [ao_low, ao_power, ao_high] = layer.ao_levels;
    let mut rgba = Vec::with_capacity(width * h * 4);
    for y in 0..h {
        for x in 0..width {
            let byte = |img: &Option<Image>, channel: usize, default: u8| {
                img.as_ref()
                    .map_or(default, |i| sample(i, x, y, width, h, channel))
            };
            let ao_source = if job.alpha_test {
                byte(&height, 2, 255)
            } else {
                byte(&color, 3, 255)
            };
            let ao = f32::from(ao_source) / 255.0;
            let occlusion = (ao_low + (ao_high - ao_low) * ao.powf(ao_power)).clamp(0.0, 1.0);
            let roughness = contrast_brightness(
                f32::from(byte(&normal, 2, 255)) / 255.0,
                layer.roughness[0],
                layer.roughness[1],
            );
            let metal = if layer.metalness {
                byte(&height, 3, 0)
            } else {
                0
            };
            rgba.extend_from_slice(&[
                (occlusion * 255.0).round() as u8,
                (roughness * 255.0).round() as u8,
                metal,
                255,
            ]);
        }
    }
    Ok(Image::new(width, h, rgba))
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt().max(1e-9);
    v.map(|c| c / length)
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn neutral_settings_change_nothing() {
        let info = MaterialInfo::default();
        let layer = Layer::read(&info, 1);
        let rgb = [0.3, 0.5, 0.2];
        assert!(close(
            layer.adjust_color(rgb, [0.4, 0.4, 0.4], 1.0, [1.0; 3]),
            rgb
        ));
    }

    #[test]
    fn saturation_zero_gives_the_grey_of_the_weights() {
        let grey = color_correct([1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0; 3]);
        assert!(close(grey, [GREY_WEIGHTS[0]; 3]));
        // The weights are Σ lᵢ² normalized: ≈ (0.080, 0.910, 0.009).
        assert!((GREY_WEIGHTS[1] - 0.910).abs() < 0.001);
    }

    #[test]
    fn contrast_pivots_on_the_average_colour() {
        let average = [0.25, 0.5, 0.75];
        assert!(close(
            color_correct(average, [3.0, 1.0, 1.0], average),
            average
        ));
    }

    #[test]
    fn a_saturated_tint_replaces_the_colour() {
        // s = 1 (pure red): chroma and luminance both move all the way to the tint.
        assert!(close(
            color_tint([0.2, 0.6, 0.4], [1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0]
        ));
    }

    #[test]
    fn mask_all_mode_only_adjusts_inside_the_tint_mask() {
        let mut info = MaterialInfo::default();
        info.floats.insert("g_fTextureColorSaturation1".into(), 0.0);
        let layer = Layer::read(&info, 1);
        let rgb = [0.6, 0.2, 0.1];
        assert!(close(layer.adjust_color(rgb, [0.0; 3], 0.0, [1.0; 3]), rgb));
        info.ints.insert("g_nColorCorrectionMode1".into(), 1);
        let layer = Layer::read(&info, 1);
        let grey = layer.adjust_color(rgb, [0.0; 3], 0.0, [1.0; 3]);
        assert!((grey[0] - grey[1]).abs() < 1e-5 && (grey[1] - grey[2]).abs() < 1e-5);
    }

    #[test]
    fn default_occlusion_levels_double_the_alpha() {
        // Unset levels are (0, 0.5, 0): occlusion = saturate(2a). Authored (0, 0.5, 1): occlusion = a.
        let layer = Layer::read(&MaterialInfo::default(), 1);
        assert!(close(layer.ao_levels, [0.0, 1.0, 2.0]));
        let mut info = MaterialInfo::default();
        info.vectors
            .insert("g_vAmbientOcclusionLevels1".into(), [0.0, 0.5, 1.0, 0.0]);
        assert!(close(Layer::read(&info, 1).ao_levels, [0.0, 1.0, 1.0]));
    }

    /// Layers covering every case [`ColorJob::new`] tells apart.
    fn layer_variants() -> Vec<Layer> {
        let mut layers = Vec::new();
        for csb in [[1.0; 3], [1.3, 0.6, 0.9]] {
            for texture_tint in [[1.0; 3], [0.5; 3], [1.0, 0.7, 0.4]] {
                for mask_only_tint in [false, true] {
                    for tint_mask in [[1.0, 1.0], [0.7, 1.2], [1.0, 0.0]] {
                        for model_tint in [0.0, 1.0] {
                            layers.push(Layer {
                                csb,
                                texture_tint,
                                mask_only_tint,
                                tint_mask,
                                model_tint,
                                roughness: [1.2, 0.8],
                                ..NEUTRAL
                            });
                        }
                    }
                }
            }
        }
        layers
    }

    #[test]
    fn colour_jobs_keep_the_pixels_they_canonicalize() {
        // What bake_color computes for a texel, with the mask from the height texture's G (or 1 without one).
        let texel = |layer: &Layer, height: bool, g: f32, rgb: [f32; 3], tint: [u8; 3]| {
            let mask = layer.tint_mask_at(if height { g } else { 1.0 });
            let tint = tint.map(|c| SRGB_TO_LINEAR[usize::from(c)]);
            layer.adjust_color(rgb, [0.3, 0.4, 0.2], mask, tint)
        };
        let mut plain = 0;
        for layer in layer_variants() {
            for height in [false, true] {
                for tint in [[255; 3], [200, 120, 60]] {
                    let path = height.then(|| "height.vtex".to_owned());
                    let job = ColorJob::new("color.vtex".into(), path, layer, tint, false);
                    for rgb in [[0.1, 0.5, 0.9], [0.8, 0.2, 0.3]] {
                        for g in [0.0, 0.4, 1.0] {
                            let expected = texel(&layer, height, g, rgb, tint);
                            let actual = match &job {
                                Some(job) => {
                                    texel(&job.layer, job.height.is_some(), g, rgb, job.tint)
                                }
                                None => rgb,
                            };
                            assert!(
                                close(actual, expected),
                                "{layer:?} {height} {tint:?}: {actual:?} vs {expected:?}"
                            );
                        }
                    }
                    plain += usize::from(job.is_none());
                }
            }
        }
        assert!(plain > 0, "some variants need no bake");
    }

    #[test]
    fn a_tint_the_layer_ignores_shares_the_untinted_bake() {
        let layer = Layer {
            csb: [1.2, 1.0, 1.0],
            ..NEUTRAL
        };
        let job =
            |layer: Layer, tint| ColorJob::new("c".into(), Some("h".into()), layer, tint, false);
        // Model tint off: every tint is the untinted bake.
        assert_eq!(job(layer, [10, 200, 30]), job(layer, [255; 3]));
        // Nothing adjusted and no tint: no bake at all.
        assert_eq!(job(NEUTRAL, [255; 3]), None);
        let tinted = Layer {
            model_tint: 1.0,
            ..NEUTRAL
        };
        assert_eq!(job(tinted, [255; 3]), None);
        assert_ne!(job(tinted, [10, 200, 30]), None);
        // Surface settings do not split colour bakes, nor colour settings surface bakes.
        let rough = Layer {
            roughness: [2.0, 0.5],
            ..layer
        };
        assert_eq!(job(layer, [255; 3]), job(rough, [255; 3]));
        let surface = |layer| {
            SurfaceJob::new(
                Some("c".into()),
                Some("n".into()),
                Some("h".into()),
                layer,
                false,
            )
        };
        assert_eq!(surface(NEUTRAL), surface(layer));
        assert_ne!(surface(NEUTRAL), surface(rough));
    }

    #[test]
    fn height_blend_follows_the_paint_at_its_ends() {
        let blend = HeightBlend::read(&MaterialInfo::default());
        for h1 in [0.0, 0.3, 1.0] {
            for h2 in [0.0, 0.6, 1.0] {
                assert!(blend.weight([h1, h2], 0.0, 0.0) < 1e-3);
                assert!(blend.weight([h1, h2], 1.0, 0.0) > 0.99);
            }
        }
        // Half-way, the higher surface wins.
        assert!(blend.weight([0.0, 1.0], 0.5, 0.0) > 0.99);
        assert!(blend.weight([1.0, 0.0], 0.5, 0.0) < 0.01);
    }
}
