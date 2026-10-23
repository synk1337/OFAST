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

/// One animated sequence of a sprite sheet (the `SHEET` extra data): CS2's particle textures pack several in one
/// image (`materials/particle/fire_explosion_1`: a fireball's sequences side by side), and a particle system plays
/// one of them by its id.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetSequence {
    /// The id the sequence was authored with (particle systems pick sequences by it).
    pub id: u32,
    /// Its name, as authored (often empty or a `CDme…` class name).
    pub name: String,
    /// Holds its last frame instead of starting over.
    pub clamp: bool,
    /// Its frames' alpha was cropped by the compiler.
    pub alpha_crop: bool,
    /// Its colour is unused (an alpha-only sequence).
    pub no_color: bool,
    /// Its alpha is unused.
    pub no_alpha: bool,
    /// Length of one play-through, seconds (0 or less: 1 s).
    pub total_time: f32,
    /// Named float parameters authored with the sequence.
    pub params: Vec<(String, f32)>,
    /// The frames in order.
    pub frames: Vec<SheetFrame>,
}

/// One frame of a [`SheetSequence`].
#[derive(Debug, Clone, PartialEq)]
pub struct SheetFrame {
    /// How long it shows, seconds (its share of the sequence's time).
    pub display_time: f32,
    /// Its images (one per layer; usually one).
    pub images: Vec<SheetImage>,
}

/// Where a frame's image sits in the texture, in UV (0 – 1 of the texture's size; the corners sit inside the texels
/// they cover, a quarter texel in on the sheets seen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetImage {
    /// The image as stored, cropped to its content: `[min, max]` corners, each `[u, v]`.
    pub cropped: [[f32; 2]; 2],
    /// The image's full frame before cropping: `[min, max]`.
    pub uncropped: [[f32; 2]; 2],
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
    /// Average colour (linear RGBA) the compiler stored. Shaders read it as `TextureAverageColor`, e.g. the pivot of
    /// csgo_environment's contrast adjustment.
    pub reflectivity: [f32; 4],
    /// Number of mips stored.
    pub mip_count: u8,
    /// Texture flags.
    pub flags: Flags,
    /// Channel conversions recorded by the compiler.
    pub conversions: Conversions,
    /// Sprite-sheet sequences (`SHEET` extra data), empty for a plain texture or a sheet that does not read.
    pub sheet: Vec<SheetSequence>,
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
const EXTRA_SHEET: u32 = 2;
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
        let mut reflectivity = [0.0; 4];
        for c in &mut reflectivity {
            *c = r.f32("texture reflectivity")?;
        }
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
            reflectivity,
            mip_count: mip_count.max(1),
            flags,
            conversions: read_conversions(resource),
            sheet: Vec::new(),
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
                // A sheet that does not read leaves the texture itself readable.
                EXTRA_SHEET => texture.sheet = read_sheet(&payload).unwrap_or_default(),
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

    /// Decodes every slice of mip 0 to linear float RGBA — the six faces of a cube map (in the stored order
    /// +X, −X, +Y, −Y, +Z, −Z), or the single image of a 2D texture. Supports the HDR formats skies use (BC6H,
    /// RGBA16F, RGBA32F) and falls back to the 8-bit decoders (values / 255) for the rest.
    pub fn decode_slices_f32(&self, file: &[u8]) -> Result<Vec<ImageF32>, TextureError> {
        let bytes = self.top_mip_bytes(file)?;
        let (w, h, slices) = self.mip_dims(0);
        let slice_size = self.format.image_size(w, h, 1);
        let mut out = Vec::with_capacity(slices);
        for slice in 0..slices {
            let src = bytes
                .get(slice * slice_size..(slice + 1) * slice_size)
                .ok_or(UnexpectedEof {
                    what: "texture slice",
                    offset: slice * slice_size,
                    needed: slice_size,
                    available: bytes.len().saturating_sub(slice * slice_size),
                })?;
            out.push(decode_slice_f32(self.format, src, w, h)?);
        }
        Ok(out)
    }
}

/// A linear floating-point RGBA image (HDR).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageF32 {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width × height × 4` floats, row-major, top row first.
    pub rgba: Vec<f32>,
}

impl ImageF32 {
    /// Bilinear sample at normalized coordinates, clamped at the borders (`v` = 0 at the top).
    pub fn sample_bilinear(&self, u: f32, v: f32) -> [f32; 4] {
        let fx = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (a, b) = (self.pixel(x0, y0), self.pixel(x1, y0));
        let (c, d) = (self.pixel(x0, y1), self.pixel(x1, y1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            top + (bottom - top) * ty
        })
    }

    fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Samples a cube map in direction `dir` (any length), with the Direct3D face convention Source 2 uses: faces in
/// the order +X, −X, +Y, −Y, +Z, −Z, and for each face the image axes below (`u` right, `v` down, both −1..1).
///
/// | face | direction of texel (u, v) |
/// |---|---|
/// | +X | (1, −v, −u) |
/// | −X | (−1, −v, u) |
/// | +Y | (u, 1, v) |
/// | −Y | (u, −1, −v) |
/// | +Z | (u, −v, 1) |
/// | −Z | (−u, −v, −1) |
///
/// CS2 samples sky cubes with the Source (Z-up) world direction as is, so +Z is the zenith. Checked on
/// `sky_de_mirage` (zenith gradient on +Z, ground colour on −Z, horizon on the edges this table predicts) and in
/// Source 2 Viewer's `texture_decode` shader.
pub fn sample_cube(faces: &[ImageF32], dir: [f32; 3]) -> [f32; 4] {
    let [x, y, z] = dir;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    // (face, u, v, major axis length): invert the table for the face the direction points at.
    let (face, u, v, major) = if ax >= ay && ax >= az {
        if x > 0.0 {
            (0, -z, -y, ax)
        } else {
            (1, z, -y, ax)
        }
    } else if ay >= az {
        if y > 0.0 {
            (2, x, z, ay)
        } else {
            (3, x, -z, ay)
        }
    } else if z > 0.0 {
        (4, x, -y, az)
    } else {
        (5, -x, -y, az)
    };
    match faces.get(face) {
        Some(image) if major > 0.0 => {
            image.sample_bilinear((u / major + 1.0) * 0.5, (v / major + 1.0) * 0.5)
        }
        _ => [0.0; 4],
    }
}

/// Decodes one slice to float RGBA.
fn decode_slice_f32(
    format: Format,
    src: &[u8],
    w: usize,
    h: usize,
) -> Result<ImageF32, TextureError> {
    let mut rgba = vec![0f32; w * h * 4];
    match format {
        Format::Bc6h => {
            let blocks_x = w.div_ceil(4).max(1);
            let mut block_out = [0f32; 4 * 4 * 3];
            for (index, block) in src.as_chunks::<16>().0.iter().enumerate() {
                let (bx, by) = (index % blocks_x, index / blocks_x);
                // Unsigned BC6H (UF16), 3 floats per pixel, row pitch in floats.
                bcdec_rs::bc6h_float(block, &mut block_out, 12, false);
                for y in 0..4 {
                    for x in 0..4 {
                        let (px, py) = (bx * 4 + x, by * 4 + y);
                        if px < w && py < h {
                            let o = (py * w + px) * 4;
                            let i = (y * 4 + x) * 3;
                            rgba[o..o + 3].copy_from_slice(&block_out[i..i + 3]);
                            rgba[o + 3] = 1.0;
                        }
                    }
                }
            }
        }
        Format::Rgba16161616F => {
            let texels = rgba.as_chunks_mut::<4>().0.iter_mut();
            for (o, px) in texels.zip(src.as_chunks::<8>().0) {
                for (c, half) in o.iter_mut().zip(px.as_chunks::<2>().0) {
                    *c = crate::model::half_to_f32(u16::from_le_bytes(*half));
                }
            }
        }
        Format::Rgba32323232F => {
            let texels = rgba.as_chunks_mut::<4>().0.iter_mut();
            for (o, px) in texels.zip(src.as_chunks::<16>().0) {
                for (c, float) in o.iter_mut().zip(px.as_chunks::<4>().0) {
                    *c = f32::from_le_bytes(*float);
                }
            }
        }
        other => {
            let mut rgba8 = vec![0u8; w * h * 4];
            decode_slice(other, src, w, h, &mut rgba8)?;
            for (o, &v) in rgba.iter_mut().zip(&rgba8) {
                *o = f32::from(v) / 255.0;
            }
        }
    }
    Ok(ImageF32 {
        width: w,
        height: h,
        rgba,
    })
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

/// Reads a string through a self-relative `i32` offset (NUL-terminated UTF-8), leaving `r` after the offset.
fn read_offset_string(r: &mut ByteReader<'_>) -> Result<String, UnexpectedEof> {
    let field = r.position();
    let offset = r.i32("sheet string offset")?;
    let mut at = r.clone();
    at.seek((field as i64 + i64::from(offset)) as usize, "sheet string")?;
    Ok(String::from_utf8_lossy(at.cstr("sheet string")?).into_owned())
}

/// Reads a `SHEET` extra-data blob starting at `blob` (`docs/formats/vtex.md`, "Sprite sheets"): `i32` offset of the
/// sequence table (from the blob's start), `u32` count; per sequence 32 bytes: `u32` id, four flag bytes (clamp,
/// alpha crop, no colour, no alpha), `i32` frames offset and `u32` count, `f32` total time, name (string offset),
/// `i32` float-parameter offset and `u32` count; per frame `f32` display time, `i32` image offset, `u32` count; per
/// image 8 `f32`: cropped min, max, uncropped min, max (UV). Offsets other than the first are relative to their own
/// field. Layout from ValveResourceFormat (MIT).
fn read_sheet(blob: &ByteReader<'_>) -> Result<Vec<SheetSequence>, UnexpectedEof> {
    let start = blob.position();
    let mut r = blob.clone();
    let table = r.i32("sheet sequence offset")?;
    let count = r.u32("sheet sequence count")?;
    r.seek(
        (start as i64 + i64::from(table)) as usize,
        "sheet sequences",
    )?;
    // Self-relative offset: where the table it points to starts.
    let target = |r: &mut ByteReader<'_>, what| -> Result<usize, UnexpectedEof> {
        let field = r.position();
        Ok((field as i64 + i64::from(r.i32(what)?)) as usize)
    };
    let mut out = Vec::new();
    for _ in 0..count.min(4096) {
        let id = r.u32("sheet sequence id")?;
        let [clamp, alpha_crop, no_color, no_alpha] = r.array::<4>("sheet sequence flags")?;
        let frames_at = target(&mut r, "sheet frames offset")?;
        let frame_count = r.u32("sheet frame count")?;
        let total_time = r.f32("sheet total time")?;
        let name = read_offset_string(&mut r)?;
        let params_at = target(&mut r, "sheet parameters offset")?;
        let param_count = r.u32("sheet parameter count")?;
        let mut params = Vec::new();
        let mut p = r.clone();
        p.seek(params_at, "sheet parameters")?;
        for _ in 0..param_count.min(4096) {
            let name = read_offset_string(&mut p)?;
            params.push((name, p.f32("sheet parameter")?));
        }
        let mut frames = Vec::new();
        let mut f = r.clone();
        f.seek(frames_at, "sheet frames")?;
        for _ in 0..frame_count.min(65536) {
            let display_time = f.f32("sheet frame time")?;
            let images_at = target(&mut f, "sheet images offset")?;
            let image_count = f.u32("sheet image count")?;
            let mut i = f.clone();
            i.seek(images_at, "sheet images")?;
            let mut images = Vec::new();
            for _ in 0..image_count.min(64) {
                let mut v = [0.0f32; 8];
                for x in &mut v {
                    *x = i.f32("sheet image coordinate")?;
                }
                images.push(SheetImage {
                    cropped: [[v[0], v[1]], [v[2], v[3]]],
                    uncropped: [[v[4], v[5]], [v[6], v[7]]],
                });
            }
            frames.push(SheetFrame {
                display_time,
                images,
            });
        }
        out.push(SheetSequence {
            id,
            name,
            clamp: clamp != 0,
            alpha_crop: alpha_crop != 0,
            no_color: no_color != 0,
            no_alpha: no_alpha != 0,
            total_time,
            params,
            frames,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheets_read_sequences_frames_and_images() {
        // A blob with one sequence of two frames, one image each, laid out as the compiler does: header, sequence
        // table, frame table, images, strings.
        let mut b: Vec<u8> = Vec::new();
        b.extend(8i32.to_le_bytes()); // sequences at 8 from the blob's start
        b.extend(1u32.to_le_bytes());
        // Sequence (at 8): id 3, clamp, frames, time 0.5, name, no parameters.
        b.extend(3u32.to_le_bytes());
        b.extend([1u8, 0, 0, 0]);
        let frames_field = b.len();
        b.extend(0i32.to_le_bytes()); // patched below
        b.extend(2u32.to_le_bytes());
        b.extend(0.5f32.to_le_bytes());
        let name_field = b.len();
        b.extend(0i32.to_le_bytes()); // patched below
        b.extend(0i32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        let frames_at = b.len();
        let mut image_fields = Vec::new();
        for time in [0.25f32, 0.25] {
            b.extend(time.to_le_bytes());
            image_fields.push(b.len());
            b.extend(0i32.to_le_bytes()); // patched below
            b.extend(1u32.to_le_bytes());
        }
        let mut images_at = Vec::new();
        for k in 0..2 {
            images_at.push(b.len());
            let u = 0.5 * k as f32;
            for v in [u, 0.0, u + 0.5, 1.0, u, 0.0, u + 0.5, 1.0] {
                b.extend(v.to_le_bytes());
            }
        }
        let name_at = b.len();
        b.extend(b"fire\0");
        let patch = |b: &mut Vec<u8>, field: usize, target: usize| {
            b[field..field + 4].copy_from_slice(&((target as i32 - field as i32).to_le_bytes()));
        };
        patch(&mut b, frames_field, frames_at);
        patch(&mut b, name_field, name_at);
        for (field, at) in image_fields.into_iter().zip(images_at) {
            patch(&mut b, field, at);
        }
        // The blob somewhere inside a larger block.
        let mut block = vec![0xEEu8; 5];
        block.extend(&b);
        let mut r = ByteReader::new(&block);
        r.seek(5, "blob").unwrap();
        let sheet = read_sheet(&r).unwrap();
        assert_eq!(sheet.len(), 1);
        let s = &sheet[0];
        assert_eq!(
            (s.id, s.name.as_str(), s.clamp, s.alpha_crop),
            (3, "fire", true, false)
        );
        assert_eq!(s.total_time, 0.5);
        assert_eq!(s.frames.len(), 2);
        assert_eq!(s.frames[1].display_time, 0.25);
        assert_eq!(s.frames[1].images[0].cropped, [[0.5, 0.0], [1.0, 1.0]]);
        assert_eq!(s.frames[0].images[0].uncropped, [[0.0, 0.0], [0.5, 1.0]]);
    }

    #[test]
    fn cube_lookup_picks_the_face_and_side_the_d3d_table_says() {
        // Six 2×1 faces: value = face × 10 + column (0 = left, 1 = right).
        let faces: Vec<ImageF32> = (0..6)
            .map(|f| ImageF32 {
                width: 2,
                height: 1,
                rgba: [0.0, 1.0]
                    .iter()
                    .flat_map(|c| [f as f32 * 10.0 + c, 0.0, 0.0, 1.0])
                    .collect(),
            })
            .collect();
        let at = |dir| sample_cube(&faces, dir)[0];
        // Zenith and nadir.
        assert_eq!(at([0.0, 0.0, 1.0]) as i32 / 10, 4);
        assert_eq!(at([0.0, 0.0, -1.0]) as i32 / 10, 5);
        // +X face: up (+Z) is on its left (u = −z), so sky-ward directions read the left column.
        assert_eq!(at([1.0, 0.0, 0.9]), 0.0);
        assert_eq!(at([1.0, 0.0, -0.9]), 1.0);
        // −X face: up is on its right.
        assert_eq!(at([-1.0, 0.0, 0.9]), 11.0);
        // +Y face: +X is on its right.
        assert_eq!(at([0.9, 1.0, 0.0]), 21.0);
    }

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
