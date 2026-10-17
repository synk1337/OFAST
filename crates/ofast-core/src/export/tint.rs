//! Model tints of CS2's prop shaders — `csgo_complex`, `csgo_vertexlitgeneric`, `csgo_foliage` and
//! `csgo_simple_3layer_parallax`: a placement's tint colour (`rendercolor`, a scene object's tint) multiplies the
//! colour, and with a tint mask (`F_TINT_MASK`) only where the mask is set.
//!
//! Where the knowledge comes from — Valve's compiled shaders as decompiled by Source 2 Viewer and tracked in
//! SteamDatabase's GameTracking-CS2 (`game/csgo/shaders_vulkan_dir/shaders/vfx/<shader>.slang`):
//! - the tint every one of them builds per instance, in the vertex shader (certain):
//!   `tint = lerp(1, rendercolor, g_flModelTintAmount) × g_vColorTint`, both colours sRGB → linear (default 1 and
//!   white; `csgo_simple_3layer_parallax` has no `g_flModelTintAmount`, as if 1);
//! - without a tint mask the pixel shaders multiply the colour by it (`albedo × tint`, certain);
//!   `csgo_vertexlitgeneric`'s `F_NOTINT` ("Prevent Tinting") turns it off;
//! - `g_tTintMask` is read in R, linear, white by default, with the second UV set when
//!   `g_bUseSecondaryUvForTintMask` is on (declarations, certain); no contrast or brightness of its own;
//! - with a tint mask, `albedo × lerp(1, tint, mask)`. Valve's files decompile only the shaders' default variant,
//!   which has no tint mask, so this is Source 2 Viewer's reconstruction (MIT, `Renderer/Shaders/complex.frag.slang`),
//!   and the form of `csgo_vertexlitgeneric`'s default `lerp(albedo, albedo × tint, saturate(a + g_flInverseBlendTint))`
//!   with `g_flInverseBlendTint` = 0 under `F_TINT_MASK`. Checked against CS2 on de_inferno's T-spawn tree
//!   (`docs/decisions/007-tint-masks-of-prop-shaders.md`).
//!
//! glTF can only multiply a whole surface (`baseColorFactor`), which is exact without a tint mask. With one, the
//! tint travels in `extras.ofast.tint_mask` and Unreal's material applies it (`import_map.py`'s
//! `M_OFAST_TintMask_v1`, [`apply`] in a material graph); `ExportOptions::bake_model_tints` bakes it into the colour
//! instead ([`TintJob`], [`bake_color`]), for other glTF readers.
//!
//! Not this module: `csgo_environment(_blend)` recolour their tint mask towards the tint's hue instead of multiplying
//! (`environment::model_tint`).

use crate::content::Content;
use crate::export::ExportError;
use crate::export::environment::{SRGB_TO_LINEAR, linear_to_srgb, sample};
use crate::export::gltf::{MaterialInfo, decode_raw, srgb_to_linear};
use crate::texture::Image;

/// The shaders whose model tint this module reproduces (each checked in its decompiled code). Other shaders keep a
/// plain multiply by the placement tint ([`ModelTint::plain`]).
const SHADERS: [&str; 4] = [
    "csgo_complex.vfx",
    "csgo_vertexlitgeneric.vfx",
    "csgo_foliage.vfx",
    "csgo_simple_3layer_parallax.vfx",
];

/// A material's model tint under one placement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelTint {
    /// What the colour is multiplied by where the mask is 1 (linear RGB; white = no tint).
    pub(crate) color: [f32; 3],
    /// `g_tTintMask` (`F_TINT_MASK`): where the tint applies, in R. `None`: everywhere.
    pub(crate) mask: Option<String>,
    /// The UV set the mask is sampled with: 1 with `g_bUseSecondaryUvForTintMask`, else 0 (the colour's).
    pub(crate) mask_uv: u8,
}

impl ModelTint {
    /// The tint of a material placed with `placement` (sRGB bytes, white = none): CS2's formula for the
    /// [`SHADERS`], a plain multiply by the placement tint for the others.
    pub(crate) fn read(info: &MaterialInfo, placement: [u8; 3]) -> Self {
        if !SHADERS.contains(&info.shader.as_str()) {
            return Self::plain(placement);
        }
        let amount = info
            .floats
            .get("g_flModelTintAmount")
            .map_or(1.0, |v| *v as f32);
        let material = info
            .vectors
            .get("g_vColorTint")
            .map_or([1.0; 3], |v| [v[0] as f32, v[1] as f32, v[2] as f32]);
        let color = if info.flag("F_NOTINT") {
            [1.0; 3]
        } else {
            std::array::from_fn(|i| {
                let placed = SRGB_TO_LINEAR[usize::from(placement[i])];
                (1.0 + (placed - 1.0) * amount) * srgb_to_linear(material[i])
            })
        };
        Self {
            color,
            mask: info
                .textures
                .get("g_tTintMask")
                .filter(|_| info.flag("F_TINT_MASK"))
                .cloned(),
            mask_uv: u8::from(info.flag("g_bUseSecondaryUvForTintMask")),
        }
    }

    /// The placement tint as a plain multiply (sRGB → linear), no mask: what OFAST does for shaders it has not
    /// checked.
    pub(crate) fn plain(placement: [u8; 3]) -> Self {
        Self {
            color: placement.map(|c| SRGB_TO_LINEAR[usize::from(c)]),
            mask: None,
            mask_uv: 0,
        }
    }

    /// `true` when the tint changes nothing (within float error: sRGB 1.0 → linear is not always exactly 1).
    pub(crate) fn is_white(&self) -> bool {
        self.color.iter().all(|c| (c - 1.0).abs() < 1e-5)
    }
}

/// A texel's tinted colour (linear): `rgb × lerp(1, tint, mask)`, the mask being `g_tTintMask`.R (1 everywhere
/// without one). `import_map.py`'s `M_OFAST_TintMask_v1` computes the same in its graph; keep the two identical.
pub(crate) fn apply(rgb: [f32; 3], tint: [f32; 3], mask: f32) -> [f32; 3] {
    std::array::from_fn(|i| rgb[i] * (1.0 + (tint[i] - 1.0) * mask))
}

/// A colour texture with a masked model tint baked in (`ExportOptions::bake_model_tints`): one per (colour, mask,
/// tint). Only for masks on the colour's UV set: a mask on the second set has another layout.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TintJob {
    /// `g_tColor`.
    pub(crate) color: String,
    /// `g_tTintMask` (R).
    pub(crate) mask: String,
    /// [`ModelTint::color`] as bits (f32 has no total order).
    tint: [u32; 3],
    /// Keep the colour texture's alpha (translucent and alpha-tested materials).
    pub(crate) keep_alpha: bool,
}

impl TintJob {
    /// The bake of `tint` (linear RGB) through `mask` onto `color`.
    pub(crate) fn new(color: String, mask: String, tint: [f32; 3], keep_alpha: bool) -> Self {
        Self {
            color,
            mask,
            tint: tint.map(f32::to_bits),
            keep_alpha,
        }
    }

    fn tint(&self) -> [f32; 3] {
        self.tint.map(f32::from_bits)
    }
}

/// Writes a colour texture with its masked model tint ([`apply`]) at the colour texture's size.
pub(crate) fn bake_color(content: &Content, job: &TintJob) -> Result<Image, ExportError> {
    let (_, color) = decode_raw(content, &job.color)?;
    let (_, mask) = decode_raw(content, &job.mask)?;
    let tint = job.tint();
    let (width, height) = (color.width, color.height);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let px = &color.rgba[(y * width + x) * 4..][..4];
            let m = f32::from(sample(&mask, x, y, width, height, 0)) / 255.0;
            let rgb = [px[0], px[1], px[2]].map(|c| SRGB_TO_LINEAR[usize::from(c)]);
            rgba.extend(apply(rgb, tint, m).map(linear_to_srgb));
            rgba.push(if job.keep_alpha { px[3] } else { 255 });
        }
    }
    Ok(Image::new(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(shader: &str) -> MaterialInfo {
        MaterialInfo {
            shader: shader.into(),
            textures: [
                ("g_tColor", "plant_color.vtex"),
                ("g_tTintMask", "plant_tintmask.vtex"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .into(),
            ints: [("F_TINT_MASK".to_owned(), 1)].into(),
            ..Default::default()
        }
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn a_red_tint_reddens_only_the_masked_texels() {
        let tint = ModelTint::read(&material("csgo_foliage.vfx"), [255, 0, 0]);
        assert_eq!(tint.mask.as_deref(), Some("plant_tintmask.vtex"));
        assert!(close(tint.color, [1.0, 0.0, 0.0]));
        // de_inferno's potted tree: white flowers under the mask turn red, green leaves outside it stay green.
        let leaf = [0.1, 0.4, 0.05];
        assert!(close(apply([1.0; 3], tint.color, 1.0), [1.0, 0.0, 0.0]));
        assert!(close(apply(leaf, tint.color, 0.0), leaf));
        assert!(close(apply(leaf, tint.color, 0.5), [0.1, 0.2, 0.025]));
    }

    #[test]
    fn the_tint_follows_the_amount_the_material_tint_and_no_tint() {
        let mut info = material("csgo_complex.vfx");
        info.floats.insert("g_flModelTintAmount".into(), 0.0);
        assert!(ModelTint::read(&info, [255, 0, 0]).is_white());
        info.floats.insert("g_flModelTintAmount".into(), 0.5);
        assert!(close(ModelTint::read(&info, [0, 0, 0]).color, [0.5; 3]));
        // g_vColorTint multiplies the tint, placed or not (sRGB → linear, as the shader's expression).
        info.floats.clear();
        info.vectors
            .insert("g_vColorTint".into(), [0.5, 1.0, 1.0, 0.0]);
        let alone = ModelTint::read(&info, [255; 3]);
        assert!(close(alone.color, [srgb_to_linear(0.5), 1.0, 1.0]));
        let mut info = material("csgo_vertexlitgeneric.vfx");
        info.ints.insert("F_NOTINT".into(), 1);
        assert!(ModelTint::read(&info, [10, 20, 30]).is_white());
    }

    #[test]
    fn masks_count_only_with_their_feature_and_unchecked_shaders_multiply() {
        let mut info = material("csgo_complex.vfx");
        info.ints.clear();
        assert_eq!(ModelTint::read(&info, [255, 0, 0]).mask, None);
        info.ints.insert("F_TINT_MASK".into(), 1);
        info.ints.insert("g_bUseSecondaryUvForTintMask".into(), 1);
        assert_eq!(ModelTint::read(&info, [255, 0, 0]).mask_uv, 1);
        // A shader not checked: the placement tint on the whole surface, its amount and mask ignored.
        let mut other = material("csgo_static_overlay.vfx");
        other.floats.insert("g_flModelTintAmount".into(), 0.0);
        let tint = ModelTint::read(&other, [255, 0, 0]);
        assert_eq!(tint, ModelTint::plain([255, 0, 0]));
        assert_eq!(tint.mask, None);
        assert!(ModelTint::plain([255; 3]).is_white());
    }

    #[test]
    fn tint_jobs_tell_tints_apart() {
        let job = |tint| TintJob::new("c".into(), "m".into(), tint, false);
        assert_eq!(job([1.0, 0.0, 0.0]), job([1.0, 0.0, 0.0]));
        assert_ne!(job([1.0, 0.0, 0.0]), job([1.0, 0.0, 0.001]));
        assert_eq!(job([0.25, 0.5, 1.0]).tint(), [0.25, 0.5, 1.0]);
    }
}
