//! Compiled textures (`.vtex_c`): header, mip layout and decoding of the top mip to plain pixels.
//!
//! A texture resource has a small `DATA` block (size, format, mip count, extra-data records). The pixel data comes
//! **after** the last block, mips stored **smallest first**, each optionally LZ4-compressed on its own. To get the
//! full-resolution image OFAST skips the small mips, decompresses mip 0 and decodes its pixel format:
//!
//! | CS2 format | Used for | Decoded as |
//! |---|---|---|
//! | BC7 | colour, packed masks | RGBA8 |
//! | DXT1 (BC1) / DXT5 (BC3) | older colour textures | RGBA8 (DXT1 forced opaque) |
//! | ATI2N (BC5) | normal maps, two-channel masks | RG in RGBA8 (B = 0, A = 255) |
//! | ATI1N (BC4) | single-channel masks (AO…) | grey RGBA8 |
//! | RGBA8888 / BGRA8888 / I8 / IA88 / R8 | uncompressed | RGBA8 |
//!
//! Some textures also need a **channel conversion** (YCoCg colour, hemi-octahedral normals). Which one applies is
//! recorded by the texture compiler in the resource's `RED2` block; see [`Conversions`].
//!
//! Layout details: `docs/formats/vtex.md`. Knowledge from ValveResourceFormat (MIT); block decompression of the
//! standard BCn formats uses the `bcdec_rs` crate (MIT), see `docs/decisions/002-standard-codecs.md`.

use crate::bytes::{ByteReader, UnexpectedEof};
use crate::resource::{BlockKind, Resource};

/// Everything that can go wrong while reading a texture.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    /// The data ended in the middle of a structure.
    #[error(transparent)]
    UnexpectedEof(#[from] UnexpectedEof),
    /// The resource has no `DATA` block.
    #[error("texture has no DATA block")]
    MissingData,
    /// The texture header version is not 1.
    #[error("unsupported texture version {0}")]
    UnsupportedVersion(u16),
    /// A pixel format OFAST cannot decode yet.
    #[error("texture format {0:?} is not supported yet")]
    UnsupportedFormat(Format),
    /// Mip data is compressed with a method OFAST cannot decode yet.
    #[error("texture mip compression method {0} is not supported")]
    UnsupportedCompression(u32),
    /// LZ4 decompression of a mip failed.
    #[error("LZ4 decompression of mip {mip} failed: {message}")]
    Lz4 {
        /// Mip level.
        mip: u8,
        /// Decoder message.
        message: String,
    },
}

/// The pixel format byte of a texture (`VTexFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)] // names are Valve's own format names
pub enum Format {
    Dxt1,
    Dxt5,
    I8,
    Rgba8888,
    R16,
    Rg1616,
    Rgba16161616,
    R16F,
    Rg1616F,
    Rgba16161616F,
    R32F,
    Rg3232F,
    Rgb323232F,
    Rgba32323232F,
    JpegRgba8888,
    PngRgba8888,
    JpegDxt5,
    PngDxt5,
    Bc6h,
    Bc7,
    Ati2n,
    Ia88,
    Etc2,
    Etc2Eac,
    R11Eac,
    Rg11Eac,
    Ati1n,
    Bgra8888,
    WebpRgba8888,
    WebpDxt5,
    R32Uint,
    A8,
    R8,
    /// A format byte this reader does not know.
    Unknown(u8),
}

impl Format {
    /// Maps the on-disk format byte.
    pub fn from_byte(value: u8) -> Self {
        use Format::*;
        match value {
            1 => Dxt1,
            2 => Dxt5,
            3 => I8,
            4 => Rgba8888,
            5 => R16,
            6 => Rg1616,
            7 => Rgba16161616,
            8 => R16F,
            9 => Rg1616F,
            10 => Rgba16161616F,
            11 => R32F,
            12 => Rg3232F,
            13 => Rgb323232F,
            14 => Rgba32323232F,
            15 => JpegRgba8888,
            16 => PngRgba8888,
            17 => JpegDxt5,
            18 => PngDxt5,
            19 => Bc6h,
            20 => Bc7,
            21 => Ati2n,
            22 => Ia88,
            23 => Etc2,
            24 => Etc2Eac,
            25 => R11Eac,
            26 => Rg11Eac,
            27 => Ati1n,
            28 => Bgra8888,
            29 => WebpRgba8888,
            30 => WebpDxt5,
            31 => R32Uint,
            32 => A8,
            33 => R8,
            other => Unknown(other),
        }
    }

    /// For block-compressed formats: bytes per 4×4 block.
    fn block_bytes(self) -> Option<usize> {
        use Format::*;
        match self {
            Dxt1 | Etc2 | R11Eac | Ati1n => Some(8),
            Dxt5 | Bc6h | Bc7 | Etc2Eac | Rg11Eac | Ati2n => Some(16),
            _ => None,
        }
    }

    /// For uncompressed formats: bytes per pixel.
    fn pixel_bytes(self) -> usize {
        use Format::*;
        match self {
            Rgba8888 | Bgra8888 | R32F | Rg1616 | Rg1616F | R32Uint => 4,
            R16 | R16F | Ia88 => 2,
            Rgba16161616 | Rgba16161616F | Rg3232F => 8,
            Rgb323232F => 12,
            Rgba32323232F => 16,
            _ => 1,
        }
    }

    /// Size in bytes of one image of `width × height × depth` in this format.
    pub fn image_size(self, width: usize, height: usize, depth: usize) -> usize {
        match self.block_bytes() {
            Some(block) => width.div_ceil(4).max(1) * height.div_ceil(4).max(1) * depth * block,
            None => width * height * depth * self.pixel_bytes(),
        }
    }
}

/// Texture flags (`VTexFlags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags(pub u16);

impl Flags {
    /// The texture is a cube map (6 faces per slice).
    pub const CUBE_TEXTURE: u16 = 16;
    /// The texture is a 3D volume (depth halves with each mip).
    pub const VOLUME_TEXTURE: u16 = 32;
    /// The texture is an array of slices.
    pub const TEXTURE_ARRAY: u16 = 64;

    /// `true` when `flag` is set.
    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }
}

/// Channel conversions the texture compiler applied, read from the `RED2` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Conversions {
    /// Colour stored as YCoCg in DXT5 (decode back to RGB).
    pub ycocg: bool,
    /// Normals stored hemi-octahedrally in RG, with roughness in B.
    pub hemi_oct_normal: bool,
    /// Normals were normalized by the compiler.
    pub normalize_normals: bool,
}

/// Header of a compiled texture.
#[derive(Debug, Clone)]
pub struct Texture {
    /// Width of mip 0 as stored (may be padded; see [`Texture::display_size`]).
    pub width: u16,
    /// Height of mip 0 as stored.
    pub height: u16,
    /// Depth (volume) or slice count (array). 1 for plain 2D textures.
    pub depth: u16,
    /// Pixel format.
    pub format: Format,
    /// Number of mips stored.
    pub mip_count: u8,
    /// Texture flags.
    pub flags: Flags,
    /// Channel conversions recorded by the compiler.
    pub conversions: Conversions,
    /// Visible size, when the stored image was padded (e.g. to a power of two).
    display: Option<(u16, u16)>,
    /// Mip compression: 0 none, 1 LZ4, 2 ZSTD.
    compression: u32,
    /// Compressed size of each mip (index 0 = largest), when compressed.
    compressed_sizes: Vec<usize>,
    /// Offset in the file where pixel data starts (end of the DATA block).
    data_offset: usize,
}

/// Extra-data record types.
const EXTRA_METADATA: u32 = 3;
const EXTRA_COMPRESSED_MIP_SIZE: u32 = 4;

impl Texture {
    /// Reads the texture header from a parsed resource.
    pub fn parse(resource: &Resource<'_>) -> Result<Self, TextureError> {
        let block = resource
            .block(BlockKind::DATA)
            .ok_or(TextureError::MissingData)?;
        let data = resource.block_data(block);
        let mut r = ByteReader::new(data);
        let version = r.u16("texture version")?;
        if version != 1 {
            return Err(TextureError::UnsupportedVersion(version));
        }
        let flags = Flags(r.u16("texture flags")?);
        r.skip(16, "texture reflectivity")?;
        let width = r.u16("texture width")?;
        let height = r.u16("texture height")?;
        let depth = r.u16("texture depth")?;
        let format = Format::from_byte(r.u8("texture format")?);
        let mip_count = r.u8("texture mip count")?;
        r.skip(4, "texture picmip0")?;
        // Offsets here are relative to the position of the offset field itself.
        let extra_field = r.position();
        let extra_offset = r.u32("texture extra data offset")? as usize;
        let extra_count = r.u32("texture extra data count")?;

        let mut texture = Self {
            width,
            height,
            depth: depth.max(1),
            format,
            mip_count: mip_count.max(1),
            flags,
            conversions: read_conversions(resource),
            display: None,
            compression: 0,
            compressed_sizes: Vec::new(),
            data_offset: block.offset + block.size,
        };

        r.seek(extra_field + extra_offset, "texture extra data")?;
        for _ in 0..extra_count {
            let kind = r.u32("texture extra data type")?;
            let offset_field = r.position();
            let offset = r.u32("texture extra data offset")? as usize;
            let _size = r.u32("texture extra data size")?;
            let resume = r.position();
            let mut payload = r.clone();
            payload.seek(offset_field + offset, "texture extra data payload")?;
            match kind {
                EXTRA_METADATA => {
                    payload.skip(2, "texture metadata")?;
                    let w = payload.u16("texture display width")?;
                    let h = payload.u16("texture display height")?;
                    // Same acceptance rule as the reference reader: non-zero and inside the stored size.
                    if w > 0 && h > 0 && w <= width && h <= height && (w != 1 || width == 4) {
                        texture.display = Some((w, h));
                    }
                }
                EXTRA_COMPRESSED_MIP_SIZE => {
                    texture.compression = payload.u32("texture mip compression")?;
                    let sizes_field = payload.position();
                    let sizes_offset = payload.u32("texture mip sizes offset")? as usize;
                    let count = payload.u32("texture mip size count")?;
                    payload.seek(sizes_field + sizes_offset, "texture mip sizes")?;
                    for _ in 0..count {
                        texture
                            .compressed_sizes
                            .push(payload.i32("texture mip size")?.max(0) as usize);
                    }
                }
                _ => {}
            }
            r.seek(resume, "texture extra data")?;
        }
        Ok(texture)
    }

    /// The visible image size (crops padding added by the compiler).
    pub fn display_size(&self) -> (u16, u16) {
        self.display.unwrap_or((self.width, self.height))
    }

    /// Stored dimensions of mip `level`: `(width, height, depth)`.
    fn mip_dims(&self, level: u8) -> (usize, usize, usize) {
        let w = (self.width as usize >> level).max(1);
        let h = (self.height as usize >> level).max(1);
        let mut d = self.depth as usize;
        if self.flags.has(Flags::VOLUME_TEXTURE) {
            d = (d >> level).max(1);
        }
        if self.flags.has(Flags::CUBE_TEXTURE) {
            d *= 6;
        }
        (w, h, d)
    }

    /// Uncompressed size of mip `level` in bytes.
    fn mip_size(&self, level: u8) -> usize {
        let (w, h, d) = self.mip_dims(level);
        self.format.image_size(w, h, d)
    }

    /// Bytes mip `level` occupies in the file (compressed size when smaller).
    fn stored_size(&self, level: u8) -> usize {
        let raw = self.mip_size(level);
        match self.compressed_sizes.get(level as usize) {
            Some(&compressed) if self.compression != 0 && compressed < raw => compressed,
            _ => raw,
        }
    }

    /// Raw (still block-compressed) bytes of mip 0, decompressed from LZ4 if needed.
    pub fn top_mip_bytes(&self, file: &[u8]) -> Result<Vec<u8>, TextureError> {
        // Mips are stored smallest first: skip every mip after level 0.
        let start = self.data_offset
            + (1..self.mip_count)
                .map(|level| self.stored_size(level))
                .sum::<usize>();
        let stored = self.stored_size(0);
        let raw_size = self.mip_size(0);
        let bytes = file.get(start..start + stored).ok_or(UnexpectedEof {
            what: "texture mip 0",
            offset: start,
            needed: stored,
            available: file.len().saturating_sub(start),
        })?;
        if stored == raw_size {
            return Ok(bytes.to_vec());
        }
        match self.compression {
            1 => lz4_flex::block::decompress(bytes, raw_size).map_err(|e| TextureError::Lz4 {
                mip: 0,
                message: e.to_string(),
            }),
            other => Err(TextureError::UnsupportedCompression(other)),
        }
    }

    /// Decodes mip 0 (first slice / face) to 8-bit RGBA, cropped to the display size.
    pub fn decode_rgba8(&self, file: &[u8]) -> Result<Image, TextureError> {
        let bytes = self.top_mip_bytes(file)?;
        let (w, h, _) = self.mip_dims(0);
        let mut rgba = vec![0u8; w * h * 4];
        decode_slice(self.format, &bytes, w, h, &mut rgba)?;
        if self.conversions.ycocg && self.format == Format::Dxt5 {
            rgba.as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(ycocg_to_rgb);
        }
        let (dw, dh) = self.display_size();
        Ok(Image::new(w, h, rgba).cropped(dw as usize, dh as usize))
    }
}

/// An 8-bit RGBA image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width × height × 4` bytes, row-major, top row first.
    pub rgba: Vec<u8>,
}

impl Image {
    /// Wraps existing pixels.
    pub fn new(width: usize, height: usize, rgba: Vec<u8>) -> Self {
        debug_assert_eq!(rgba.len(), width * height * 4);
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Keeps the top-left `width × height` pixels.
    pub fn cropped(self, width: usize, height: usize) -> Self {
        if width >= self.width && height >= self.height {
            return self;
        }
        let mut rgba = Vec::with_capacity(width * height * 4);
        for row in self.rgba.chunks_exact(self.width * 4).take(height) {
            rgba.extend_from_slice(&row[..width * 4]);
        }
        Self {
            width,
            height,
            rgba,
        }
    }
}

/// Decodes one `w × h` image of `format` from `src` into `dst` (RGBA8).
fn decode_slice(
    format: Format,
    src: &[u8],
    w: usize,
    h: usize,
    dst: &mut [u8],
) -> Result<(), TextureError> {
    use Format::*;
    let need = format.image_size(w, h, 1);
    if src.len() < need {
        return Err(UnexpectedEof {
            what: "texture pixels",
            offset: 0,
            needed: need,
            available: src.len(),
        }
        .into());
    }
    match format {
        Dxt1 | Dxt5 | Bc7 | Ati1n | Ati2n => decode_blocks(format, src, w, h, dst),
        Rgba8888 => dst.copy_from_slice(&src[..w * h * 4]),
        Bgra8888 => {
            for (out, px) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0.iter())
            {
                out.copy_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        I8 => {
            for (out, &v) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                out.copy_from_slice(&[v, v, v, 255]);
            }
        }
        R8 => {
            for (out, &v) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                out.copy_from_slice(&[v, 0, 0, 255]);
            }
        }
        Ia88 => {
            for (out, px) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<2>().0.iter())
            {
                out.copy_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        other => return Err(TextureError::UnsupportedFormat(other)),
    }
    Ok(())
}

/// Decodes 4×4 blocks one at a time and copies the visible part of each into `dst`.
fn decode_blocks(format: Format, src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    let block_bytes = format.block_bytes().expect("block format");
    let blocks_x = w.div_ceil(4).max(1);
    let mut rgba_block = [0u8; 4 * 4 * 4];
    let mut channel_block = [0u8; 4 * 4 * 2];
    for (index, block) in src
        .chunks_exact(block_bytes)
        .take(blocks_x * h.div_ceil(4).max(1))
        .enumerate()
    {
        let (bx, by) = (index % blocks_x, index / blocks_x);
        match format {
            Format::Dxt1 => {
                bcdec_rs::bc1(block, &mut rgba_block, 16);
                // DXT1 textures in Source 2 carry no meaningful alpha.
                rgba_block
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .for_each(|px| px[3] = 255);
            }
            Format::Dxt5 => bcdec_rs::bc3(block, &mut rgba_block, 16),
            Format::Bc7 => bcdec_rs::bc7(block, &mut rgba_block, 16),
            Format::Ati1n => {
                bcdec_rs::bc4(block, &mut channel_block[..16], 4, false);
                for (px, &v) in rgba_block
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(&channel_block[..16])
                {
                    px.copy_from_slice(&[v, v, v, 255]);
                }
            }
            Format::Ati2n => {
                bcdec_rs::bc5(block, &mut channel_block, 8, false);
                for (px, rg) in rgba_block
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(channel_block.as_chunks::<2>().0.iter())
                {
                    px.copy_from_slice(&[rg[0], rg[1], 0, 255]);
                }
            }
            _ => unreachable!("only block formats reach decode_blocks"),
        }
        for y in 0..4 {
            let py = by * 4 + y;
            if py >= h {
                break;
            }
            for x in 0..4 {
                let px = bx * 4 + x;
                if px >= w {
                    break;
                }
                let out = (py * w + px) * 4;
                let inp = (y * 4 + x) * 4;
                dst[out..out + 4].copy_from_slice(&rgba_block[inp..inp + 4]);
            }
        }
    }
}

/// YCoCg (stored in DXT5 as Co, Cg, scale, Y) back to RGB.
fn ycocg_to_rgb(px: &mut [u8; 4]) {
    let scale = f32::from(px[2]) / 255.0 * (255.0 / 8.0) + 1.0;
    let co = (f32::from(px[0]) / 255.0 - 128.0 / 255.0) / scale;
    let cg = (f32::from(px[1]) / 255.0 - 128.0 / 255.0) / scale;
    let y = f32::from(px[3]) / 255.0;
    let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    px[0] = to_byte(y + co - cg);
    px[1] = to_byte(y + cg);
    px[2] = to_byte(y - co - cg);
    px[3] = 255;
}

/// Reconstructs a tangent-space normal map stored as two channels (BC5: X in R, Y in G) into full RGB.
pub fn reconstruct_normal_z(image: &mut Image) {
    for px in image.rgba.as_chunks_mut::<4>().0.iter_mut() {
        let x = f32::from(px[0]) / 127.5 - 1.0;
        let y = f32::from(px[1]) / 127.5 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        px[2] = ((z * 0.5 + 0.5) * 255.0).round() as u8;
    }
}

/// Decodes hemi-octahedral normals (RG) to a regular normal map; the roughness in B moves to alpha.
pub fn decode_hemi_oct_normal(image: &mut Image) {
    for px in image.rgba.as_chunks_mut::<4>().0.iter_mut() {
        let r = f32::from(px[0]) / 255.0;
        let g = f32::from(px[1]) / 255.0;
        let nx = r + g - 1.003_922;
        let ny = r - g;
        let nz = 1.0 - nx.abs() - ny.abs();
        let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
        let to_byte = |v: f32| ((v / len * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
        let roughness = px[2];
        px[0] = to_byte(nx);
        px[1] = to_byte(ny);
        px[2] = to_byte(nz);
        px[3] = roughness;
    }
}

/// Reads the compiler's conversion markers from `RED2 → m_SpecialDependencies`.
fn read_conversions(resource: &Resource<'_>) -> Conversions {
    let mut conversions = Conversions::default();
    let Ok(red2) = resource.kv3(BlockKind::RED2) else {
        return conversions;
    };
    let Some(deps) = red2
        .root
        .get("m_SpecialDependencies")
        .and_then(|v| v.as_array())
    else {
        return conversions;
    };
    for dep in deps {
        if dep.get("m_CompilerIdentifier").and_then(|v| v.as_str()) != Some("CompileTexture") {
            continue;
        }
        match dep.get("m_String").and_then(|v| v.as_str()) {
            Some("Texture Compiler Version Image YCoCg Conversion") => conversions.ycocg = true,
            Some("Texture Compiler Version Image NormalizeNormals") => {
                conversions.normalize_normals = true
            }
            Some("Texture Compiler Version Mip HemiOctIsoRoughness_RG_B")
            | Some("Texture Compiler Version Mip HemiOctAnisoRoughness") => {
                conversions.hemi_oct_normal = true
            }
            _ => {}
        }
    }
    conversions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_sizes_round_up_to_whole_blocks() {
        assert_eq!(Format::Bc7.image_size(4096, 4096, 1), 4096 * 4096);
        assert_eq!(Format::Dxt1.image_size(4096, 4096, 1), 4096 * 4096 / 2);
        assert_eq!(
            Format::Bc7.image_size(1, 1, 1),
            16,
            "a 1×1 mip still needs one block"
        );
        assert_eq!(
            Format::Dxt1.image_size(6, 6, 1),
            4 * 8,
            "6×6 rounds up to 2×2 blocks"
        );
        assert_eq!(Format::Rgba8888.image_size(3, 2, 1), 24);
    }

    #[test]
    fn uncompressed_formats_decode_to_rgba() {
        let mut dst = vec![0; 2 * 4];
        decode_slice(Format::Bgra8888, &[1, 2, 3, 4, 5, 6, 7, 8], 2, 1, &mut dst).unwrap();
        assert_eq!(dst, [3, 2, 1, 4, 7, 6, 5, 8]);
        decode_slice(Format::I8, &[9, 10], 2, 1, &mut dst).unwrap();
        assert_eq!(dst, [9, 9, 9, 255, 10, 10, 10, 255]);
    }

    #[test]
    fn solid_bc1_block_decodes_to_one_colour() {
        // BC1 block: colour0 = colour1 = pure red (RGB565 0xF800), all indices 0.
        let block = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let mut dst = vec![0; 4 * 4 * 4];
        decode_slice(Format::Dxt1, &block, 4, 4, &mut dst).unwrap();
        assert!(
            dst.as_chunks::<4>()
                .0
                .iter()
                .all(|px| *px == [255, 0, 0, 255]),
            "{dst:?}"
        );
    }

    #[test]
    fn flat_normal_reconstructs_to_straight_up() {
        let mut image = Image::new(1, 1, vec![128, 128, 0, 255]);
        reconstruct_normal_z(&mut image);
        assert_eq!(image.rgba[2], 255);
    }

    #[test]
    fn crop_keeps_the_top_left_corner() {
        let image = Image::new(2, 2, (0..16).collect()).cropped(1, 1);
        assert_eq!(image.rgba, [0, 1, 2, 3]);
    }
}
