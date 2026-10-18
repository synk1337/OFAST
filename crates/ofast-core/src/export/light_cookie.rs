//! Light cookies of CS2's barn lights (`light_barn`'s `lightcookie`), reduced to what a plain spot light can carry.
//!
//! **CS2's barn light** (Source 2 Viewer's `SceneLight.cs` and `Shaders/common/lighting.barn.slang`, CS2's run-time
//! shading of stationary and dynamic barn lights): `size_params` = (x, y, z) is "Aperture Size (x, y, angle)" in
//! Hammer (`lights2.fgd`, default 16 16 0.0625). The light is a perspective frustum whose near plane, 1/z behind the
//! entity's origin, is x by y half-sized: half-tangents **x·z** and **y·z** (the default gives 45°; de_vertigo's hall
//! lamps 4 × 1.85 = 7.41, 82°). Inside the frustum the intensity is uniform — `brightness` (EV) as linear brightness
//! at 100 units past the luminaire, falling off with the square of the distance from the frustum's eye — and
//! outside it there is none. A cookie replaces the frustum's soft edges: the texture is stretched over the frustum
//! (`uv = ndc × −0.5 + 0.5`) and multiplies the light. Hammer's stored lumens agree with this frustum: lumens ≈ 81 ×
//! linear brightness × the frustum's solid angle for every barn light on de_vertigo.
//!
//! **The cookies are sRGB textures.** `materials/effects/lightcookies/flashlight.vtex`, the one every barn light of
//! de_vertigo uses, stores a linear average (`reflectivity`) of 0.098: the mean of its sRGB-decoded pixels (0.099),
//! not of its raw ones (0.22). Decoded, it lets through 0.36 – 0.43 within half the frustum's radius, 0.16 at 0.55,
//! 0.06 at 0.65 and nothing past 0.9: a round spot well inside the frustum's square.
//!
//! [`CookieProfile`] is that radial profile; [`fit_spot`] the Unreal spot light (a point with a cone and Unreal's
//! own falloff from its inner to its outer angle) that comes closest to it.

use super::ExportError;
use super::gltf::srgb_to_linear;
use crate::content::Content;
use crate::resource::Resource;
use crate::texture::{Image, Texture};

/// Rings of the radial profile, over the frustum's radius 0 – 1 (its inscribed circle).
pub(crate) const PROFILE_RINGS: usize = 20;

/// Unreal's widest spot cone (`USpotLightComponent` clamps the outer angle to 80°).
const MAX_OUTER_CONE_DEG: f32 = 80.0;

/// Rec. 709 luminance weights.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// How much of a barn light a cookie lets through, by distance from the frustum's axis.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CookieProfile {
    /// Mean linear luminance of the cookie over each ring: ring `i` covers the normalized radius
    /// `i / PROFILE_RINGS` – `(i + 1) / PROFILE_RINGS`, where 1 touches the middle of the frustum's sides.
    pub rings: [f32; PROFILE_RINGS],
    /// Mean linear luminance over the whole texture (the frustum's square).
    pub mean: f32,
    /// Whether the pixels were read as sRGB (see [`profile_of`]).
    pub srgb: bool,
}

/// Reads a cookie texture (`materials/effects/lightcookies/….vtex`) and returns its radial profile.
pub(crate) fn cookie_profile(content: &Content, path: &str) -> Result<CookieProfile, ExportError> {
    let data = content.read_resource(path)?;
    let resource = Resource::parse(&data)?;
    let texture = Texture::parse(&resource)?;
    let image = texture.decode_rgba8(&data)?;
    let average = texture.reflectivity;
    let reflectivity = LUMA[0] * average[0] + LUMA[1] * average[1] + LUMA[2] * average[2];
    Ok(profile_of(&image, reflectivity))
}

/// Radial profile of a decoded cookie. The texture compiler stores a linear average colour (`reflectivity`): the
/// pixels are read as sRGB when that average is closer to the mean of their sRGB-decoded values than to the mean of
/// the raw values (it is, by far, for de_vertigo's flashlight cookie).
pub(crate) fn profile_of(image: &Image, reflectivity: f32) -> CookieProfile {
    let luminance = |srgb: bool| -> Vec<f32> {
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                (0..3)
                    .map(|c| {
                        let v = f32::from(p[c]) / 255.0;
                        LUMA[c] * if srgb { srgb_to_linear(v) } else { v }
                    })
                    .sum()
            })
            .collect()
    };
    let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len().max(1) as f32;
    let (decoded, raw) = (luminance(true), luminance(false));
    let srgb = (mean(&decoded) - reflectivity).abs() <= (mean(&raw) - reflectivity).abs();
    let values = if srgb { decoded } else { raw };

    let (mut sums, mut counts) = ([0.0f32; PROFILE_RINGS], [0usize; PROFILE_RINGS]);
    for y in 0..image.height {
        for x in 0..image.width {
            // The shader's mapping, inverted: ndc = 1 − 2 uv.
            let nx = 1.0 - 2.0 * (x as f32 + 0.5) / image.width as f32;
            let ny = 1.0 - 2.0 * (y as f32 + 0.5) / image.height as f32;
            let ring = ((nx * nx + ny * ny).sqrt() * PROFILE_RINGS as f32) as usize;
            if ring < PROFILE_RINGS {
                sums[ring] += values[y * image.width + x];
                counts[ring] += 1;
            }
        }
    }
    let mut rings = [0.0; PROFILE_RINGS];
    for i in 0..PROFILE_RINGS {
        rings[i] = if counts[i] > 0 {
            sums[i] / counts[i] as f32
        } else {
            0.0
        };
    }
    CookieProfile {
        rings,
        mean: mean(&values),
        srgb,
    }
}

/// An Unreal spot light standing in for a barn light and its cookie.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpotFit {
    /// Multiplies the barn light's intensity (the cookie's transmission inside the inner cone).
    pub scale: f32,
    /// Unreal inner and outer cone half-angles, degrees.
    pub inner_deg: f32,
    pub outer_deg: f32,
}

/// The Unreal spot light closest to a cookie `profile` on a frustum of half-tangent `tan` (the geometric mean of
/// x·z and y·z: the cookies are round). Unreal lights a spot fully inside its inner cone and fades it as
/// ((cos θ − cos outer) / (cos inner − cos outer))² out to the outer cone (`SpotAttenuation` in the engine's
/// shaders). The fit minimizes the squared difference in intensity, weighted by each ring's solid angle, over the
/// two cone angles (searched in steps of 1 % of the frustum's radius) and the scale (solved directly).
pub(crate) fn fit_spot(profile: &CookieProfile, tan: f32) -> SpotFit {
    let n = PROFILE_RINGS as f32;
    let angle = |radius: f32| (radius * tan).atan();
    let rings: Vec<(f32, f32, f32)> = (0..PROFILE_RINGS)
        .map(|i| {
            let (lo, hi) = (angle(i as f32 / n), angle((i + 1) as f32 / n));
            // Solid angle of the ring (up to 2π) and the cosine at its middle.
            let weight = lo.cos() - hi.cos();
            let middle = angle((i as f32 + 0.5) / n).cos();
            (middle, weight, profile.rings[i])
        })
        .collect();
    let max_outer = (MAX_OUTER_CONE_DEG.to_radians().tan() / tan).min(1.2);
    let mut best = (
        f32::INFINITY,
        SpotFit {
            scale: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
        },
    );
    for a in 0..=100 {
        let inner_r = a as f32 / 100.0;
        for b in (a + 2)..=120 {
            let outer_r = b as f32 / 100.0;
            if outer_r > max_outer {
                break;
            }
            let (cos_inner, cos_outer) = (angle(inner_r).cos(), angle(outer_r).cos());
            let falloff = |cos: f32| {
                ((cos - cos_outer) / (cos_inner - cos_outer))
                    .clamp(0.0, 1.0)
                    .powi(2)
            };
            let (mut fg, mut ff) = (0.0, 0.0);
            for &(cos, w, g) in &rings {
                let f = falloff(cos);
                fg += w * f * g;
                ff += w * f * f;
            }
            if ff <= 0.0 {
                continue;
            }
            let scale = fg / ff;
            let error: f32 = rings
                .iter()
                .map(|&(cos, w, g)| w * (scale * falloff(cos) - g).powi(2))
                .sum();
            if error < best.0 {
                best = (
                    error,
                    SpotFit {
                        scale,
                        inner_deg: angle(inner_r).to_degrees(),
                        outer_deg: angle(outer_r).to_degrees(),
                    },
                );
            }
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round cookie, `inside` up to radius 0.5 (of the frustum's half-size), black beyond, as sRGB levels.
    fn disc(inside: u8) -> Image {
        let size = 64;
        let mut rgba = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                let nx = 1.0 - 2.0 * (x as f32 + 0.5) / size as f32;
                let ny = 1.0 - 2.0 * (y as f32 + 0.5) / size as f32;
                let v = if (nx * nx + ny * ny).sqrt() < 0.5 {
                    inside
                } else {
                    0
                };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Image::new(size, size, rgba)
    }

    #[test]
    fn profile_reads_srgb_when_the_stored_average_says_so() {
        // sRGB level 160 is 0.351 linear; the disc covers π/16 of the square.
        let image = disc(160);
        let linear_mean = 0.351 * std::f32::consts::PI / 16.0;
        let profile = profile_of(&image, linear_mean);
        assert!(profile.srgb);
        assert!(
            (profile.rings[0] - 0.351).abs() < 0.01,
            "{:?}",
            profile.rings
        );
        assert!(profile.rings[15] < 1e-6);
        // The same pixels with a raw average: read as they are.
        let raw = profile_of(&image, 160.0 / 255.0 * std::f32::consts::PI / 16.0);
        assert!(!raw.srgb);
        assert!((raw.rings[0] - 160.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn spot_fit_keeps_the_disc_strength_and_edge() {
        let profile = profile_of(&disc(160), 0.351 * std::f32::consts::PI / 16.0);
        let fit = fit_spot(&profile, 1.0);
        // The disc's edge (radius 0.5 on a 45° frustum) is at atan(0.5) = 26.6°.
        assert!(
            fit.inner_deg < 26.6 && fit.outer_deg > 22.0 && fit.outer_deg < 32.0,
            "{fit:?}"
        );
        assert!((fit.scale - 0.351).abs() < 0.06, "{fit:?}");
    }

    #[test]
    fn spot_fit_respects_unreals_widest_cone() {
        let mut profile = profile_of(&disc(255), std::f32::consts::PI / 16.0);
        profile.rings = [1.0; PROFILE_RINGS];
        let fit = fit_spot(&profile, 20.0);
        assert!(fit.outer_deg <= MAX_OUTER_CONE_DEG + 1e-3, "{fit:?}");
    }
}
