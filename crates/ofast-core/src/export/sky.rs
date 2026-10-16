//! Exports a map's 2D sky (the `env_sky` cube map) as an equirectangular Radiance HDR image, ready for Unreal.
//!
//! **The CS2 sky.** `env_sky` names a material using `sky.vfx`, whose `g_tSkyTexture` is an HDR cube map (BC6H,
//! six faces, see [`crate::texture::sample_cube`] for the face layout). The engine draws it behind everything as
//! `cube(R⁻¹ · direction) × tint × brightnessscale × 2^(g_flBrightnessExposureBias + g_flRenderOnlyExposureBias)`,
//! where `R` is the `env_sky` entity's rotation and `direction` the Source world view direction. When a map has a
//! 3D skybox, the `env_sky` *inside the skybox world* is the one drawn (Source 2 Viewer's `Renderer.BackgroundFor`),
//! seen through the sky camera, so it also turns with the skybox's `skybox_reference` angles.
//!
//! **The output.** One `.hdr` (Radiance RGBE, run-length encoded) in the latitude-longitude layout Unreal turns
//! into a `TextureCube` on import, so the same texture feeds the visible sky and the Sky Light. For a pixel at
//! normalized (`u`, `v`), with `D` the **Unreal** world direction it shows:
//!
//! ```text
//! u = 0.5 + atan2(D.x, −D.y) / 2π        v = acos(D.z) / π        (v = 0: zenith)
//! ```
//!
//! i.e. the image centre looks along Unreal −Y (Source +Y) and `u` grows towards +X. The `env_sky` rotation, tint,
//! brightness scale and brightness bias are baked into the pixels; the render-only bias (which brightens the
//! visible sky but not the lighting) is left to the manifest.

use std::fs;
use std::io::Write;
use std::path::Path;

use rayon::prelude::*;

use super::ExportError;
use super::gltf::read_material;
use crate::content::Content;
use crate::map::{Entity, IDENTITY, Transform, compose, entity_transform};
use crate::resource::Resource;
use crate::texture::{ImageF32, Texture, sample_cube};

/// What [`export_sky`] wrote, for the manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyExport {
    /// The sky material (`materials/skybox/sky_de_mirage.vmat`).
    pub material: String,
    /// Image size in pixels (width = 2 × height).
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Linear multiplier already baked into the pixels (tint × brightness scale × 2^brightness bias).
    pub baked_scale: [f32; 3],
    /// Extra exposure bias (stops) for the visible sky only (`g_flRenderOnlyExposureBias`).
    pub render_only_bias: f32,
    /// Average radiance over the whole sphere (linear RGB), after the baked scale.
    pub mean: [f32; 3],
    /// Average radiance over the upper hemisphere — the light the sky sends down onto flat ground.
    pub upper_mean: [f32; 3],
}

/// Writes the sky of `env_sky` to `path` (`.hdr`). `frame` places the world the entity belongs to in the main map
/// (the 3D skybox transform for a skybox's `env_sky`, [`IDENTITY`] otherwise); only its rotation matters.
/// `Ok(None)` when the entity has no sky material or the sky texture is not a cube map.
pub fn export_sky(
    content: &Content,
    env_sky: &Entity,
    frame: &Transform,
    path: &Path,
) -> Result<Option<SkyExport>, ExportError> {
    let Some(material) = env_sky
        .string("skyname")
        .or_else(|| env_sky.string("skybox_material_day"))
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let info = read_material(content, material)?;
    let Some(texture_path) = info.textures.get("g_tSkyTexture") else {
        return Ok(None);
    };
    let data = content.read_resource(texture_path)?;
    let texture = Texture::parse(&Resource::parse(&data)?)?;
    let faces = texture.decode_slices_f32(&data)?;
    if faces.len() != 6 {
        return Ok(None);
    }

    let float = |name: &str| info.floats.get(name).copied().unwrap_or(0.0) as f32;
    let tint = env_sky
        .vector3("tint_color")
        .map(|c| c.map(|v| v / 255.0))
        .unwrap_or([1.0; 3]);
    let brightness = env_sky
        .number("brightnessscale")
        .filter(|b| *b > 0.0)
        .unwrap_or(1.0);
    let bias = 2f32.powf(float("g_flBrightnessExposureBias"));
    let baked_scale = tint.map(|t| t * brightness * bias);

    // Sky → main map: the world's rotation (scale removed), then the entity's own.
    let rotation = compose(
        &rotation_of(frame),
        &entity_transform([0.0; 3], env_sky.angles(), [1.0; 3]),
    );
    let height = (faces[0].height * 2).clamp(16, 2048);
    let image = equirect(&faces, &rotation, baked_scale, height);
    write_hdr(path, &image)?;
    Ok(Some(SkyExport {
        material: material.to_owned(),
        width: image.width,
        height: image.height,
        baked_scale,
        render_only_bias: float("g_flRenderOnlyExposureBias"),
        mean: sphere_mean(&image, image.height),
        upper_mean: sphere_mean(&image, image.height / 2),
    }))
}

/// Writes a uniform sky of linear colour `rgb` (a small equirect `.hdr`), for maps lit by a flat sky colour.
pub fn write_flat_hdr(path: &Path, rgb: [f32; 3]) -> Result<(), ExportError> {
    let (width, height) = (64, 32);
    let rgba = std::iter::repeat_n([rgb[0], rgb[1], rgb[2], 1.0], width * height)
        .flatten()
        .collect();
    write_hdr(
        path,
        &ImageF32 {
            width,
            height,
            rgba,
        },
    )
}

/// The rotation part of a transform: its 3×3 with each column normalized (removes a uniform or axis scale), no
/// translation.
fn rotation_of(t: &Transform) -> Transform {
    let mut r = IDENTITY;
    for c in 0..3 {
        let length = (0..3).map(|i| t[i][c] * t[i][c]).sum::<f32>().sqrt();
        if length > 0.0 {
            for (i, row) in r.iter_mut().enumerate() {
                row[c] = t[i][c] / length;
            }
        }
    }
    r
}

/// The Unreal world direction shown by the equirect pixel at normalized (`u`, `v`) — see the module docs.
pub fn equirect_direction(u: f32, v: f32) -> [f32; 3] {
    let theta = v * std::f32::consts::PI;
    let phi = (2.0 * u - 1.0) * std::f32::consts::PI;
    let (st, ct) = theta.sin_cos();
    let (sp, cp) = phi.sin_cos();
    [st * sp, -st * cp, ct]
}

/// Resamples the cube `faces` into a `2·height × height` equirect image, turned by `rotation` (sky → world) and
/// scaled.
fn equirect(faces: &[ImageF32], rotation: &Transform, scale: [f32; 3], height: usize) -> ImageF32 {
    let width = height * 2;
    let r = rotation;
    let mut rgba = vec![0f32; width * height * 4];
    rgba.par_chunks_mut(width * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let v = (y as f32 + 0.5) / height as f32;
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let u = (x as f32 + 0.5) / width as f32;
                let d = equirect_direction(u, v);
                // Unreal → Source direction, then into the sky's own frame (inverse rotation = transpose).
                let s = [d[0], -d[1], d[2]];
                let cube: [f32; 3] =
                    std::array::from_fn(|i| r[0][i] * s[0] + r[1][i] * s[1] + r[2][i] * s[2]);
                let c = sample_cube(faces, cube);
                *px = [
                    c[0].max(0.0) * scale[0],
                    c[1].max(0.0) * scale[1],
                    c[2].max(0.0) * scale[2],
                    1.0,
                ];
            }
        });
    ImageF32 {
        width,
        height,
        rgba,
    }
}

/// Average of an equirect image over its first `rows` rows (all of them: the sphere; half: the upper hemisphere),
/// rows weighted by their solid angle.
fn sphere_mean(image: &ImageF32, rows: usize) -> [f32; 3] {
    let mut sum = [0f64; 3];
    let mut weight = 0f64;
    for y in 0..rows.min(image.height) {
        let w = ((y as f64 + 0.5) / image.height as f64 * std::f64::consts::PI).sin();
        let row = &image.rgba[y * image.width * 4..(y + 1) * image.width * 4];
        for px in row.as_chunks::<4>().0 {
            for (s, &v) in sum.iter_mut().zip(px) {
                *s += f64::from(v) * w;
            }
            weight += w;
        }
    }
    sum.map(|s| (s / weight) as f32)
}

/// Writes a Radiance `.hdr` file (RGBE, new-style run-length encoded scanlines).
pub fn write_hdr(path: &Path, image: &ImageF32) -> Result<(), ExportError> {
    let (w, h) = (image.width, image.height);
    let mut out = Vec::with_capacity(w * h * 4 / 2 + 128);
    write!(out, "#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {h} +X {w}\n")?;
    let mut channels = vec![[0u8; 4]; w];
    for y in 0..h {
        for (x, texel) in channels.iter_mut().enumerate() {
            let i = (y * w + x) * 4;
            *texel = rgbe([image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]);
        }
        if !(8..=0x7fff).contains(&w) {
            // Run-length encoding only exists for these widths: flat pixels otherwise.
            out.extend(channels.iter().flatten());
            continue;
        }
        out.extend([2, 2, (w >> 8) as u8, (w & 0xff) as u8]);
        for c in 0..4 {
            let bytes: Vec<u8> = channels.iter().map(|t| t[c]).collect();
            rle_channel(&bytes, &mut out);
        }
    }
    fs::write(path, out)?;
    Ok(())
}

/// Shared-exponent encoding: three 8-bit mantissas and one exponent byte (value = (m + 0.5) / 256 · 2^(e − 128)).
fn rgbe(c: [f32; 3]) -> [u8; 4] {
    let max = c[0].max(c[1]).max(c[2]);
    if max.is_nan() || max < 1e-32 {
        return [0; 4];
    }
    // max = m · 2^e with m in [0.5, 1).
    let e = (max.log2().floor() as i32 + 1).clamp(-127, 127);
    let scale = 256.0 / 2f32.powi(e);
    let q = |v: f32| (v.max(0.0) * scale).min(255.0) as u8;
    [q(c[0]), q(c[1]), q(c[2]), (e + 128) as u8]
}

/// Run-length encodes one channel of a scanline: `128 + n, byte` for a run of n (≤ 127) equal bytes, `n, bytes…`
/// for n (≤ 128) literal bytes.
fn rle_channel(data: &[u8], out: &mut Vec<u8>) {
    let run_at = |i: usize, max: usize| {
        let mut n = 1;
        while i + n < data.len() && n < max && data[i + n] == data[i] {
            n += 1;
        }
        n
    };
    let mut i = 0;
    while i < data.len() {
        let run = run_at(i, 127);
        if run >= 4 {
            out.extend([128 + run as u8, data[i]]);
            i += run;
            continue;
        }
        let start = i;
        while i < data.len() && i - start < 128 && run_at(i, 4) < 4 {
            i += 1;
        }
        out.push((i - start) as u8);
        out.extend(&data[start..i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes what [`rle_channel`] wrote.
    fn unrle(mut data: &[u8], len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < len {
            let n = data[0] as usize;
            if n > 128 {
                out.extend(std::iter::repeat_n(data[1], n - 128));
                data = &data[2..];
            } else {
                out.extend(&data[1..=n]);
                data = &data[n + 1..];
            }
        }
        out
    }

    #[test]
    fn run_length_encoding_round_trips() {
        let mut data: Vec<u8> = (0..300).map(|i| (i % 7) as u8).collect();
        data.extend([9; 200]);
        data.extend([1, 2, 2, 2, 3]);
        let mut encoded = Vec::new();
        rle_channel(&data, &mut encoded);
        assert_eq!(unrle(&encoded, data.len()), data);
        assert!(encoded.len() < data.len());
    }

    #[test]
    fn rgbe_keeps_three_significant_digits() {
        for value in [0.01f32, 0.18, 1.0, 1.33, 37.5, 5000.0] {
            let [r, _, _, e] = rgbe([value, 0.0, 0.0]);
            let decoded = (f32::from(r) + 0.5) / 256.0 * 2f32.powi(i32::from(e) - 128);
            assert!(
                (decoded - value).abs() / value < 0.01,
                "{value} → {decoded}"
            );
        }
        assert_eq!(rgbe([0.0; 3]), [0; 4]);
    }

    #[test]
    fn rotation_of_drops_scale_and_translation() {
        let placed = entity_transform([100.0, 5.0, 7.0], [0.0, 90.0, 0.0], [16.0; 3]);
        let turned = entity_transform([0.0; 3], [0.0, 90.0, 0.0], [1.0; 3]);
        let r = rotation_of(&placed);
        assert!(
            r.iter()
                .flatten()
                .zip(turned.iter().flatten())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
    }

    #[test]
    fn equirect_layout_matches_unreal_long_lat() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(
            close(equirect_direction(0.5, 0.0), [0.0, 0.0, 1.0]),
            "top row is the zenith"
        );
        assert!(
            close(equirect_direction(0.5, 0.5), [0.0, -1.0, 0.0]),
            "centre looks along Unreal −Y"
        );
        assert!(
            close(equirect_direction(0.75, 0.5), [1.0, 0.0, 0.0]),
            "three quarters along is +X"
        );
    }
}
