//! Decoding vertex attributes (positions, packed normals/tangents, UVs) into plain float arrays.
//!
//! CS2 packs normals and tangents tightly. Two packings exist:
//!
//! - **`R32_UINT` (current CS2, "version 2")**: one 32-bit word = octahedral normal (10 + 10 bits), tangent as an
//!   11-bit angle around the normal, and 1 bit of bitangent sign.
//! - **`R8G8B8A8_UNORM` (older, "version 1")**: two bytes for the normal and two for the tangent, each pair a
//!   folded octahedral encoding.
//!
//! The math follows ValveResourceFormat's `DecompressNormal` / `DecompressNormalTangents2` (MIT).

use super::{Attribute, DrawCall, VertexBuffer, dxgi};

/// Plain geometry for one draw call, in **Source coordinates** (inches, X forward, Y left, Z up).
///
/// Only the vertices the draw call actually uses are kept; `indices` are renumbered accordingly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geometry {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals (empty when the layout has none).
    pub normals: Vec<[f32; 3]>,
    /// Tangents with bitangent sign in `w` (empty when unavailable).
    pub tangents: Vec<[f32; 4]>,
    /// Texture coordinate sets, `uvs[n]` = `TEXCOORD n`.
    pub uvs: Vec<Vec<[f32; 2]>>,
    /// Vertex-painted layer blend weights (0–1, RGBA), from `TEXCOORD 4` stored as `R8G8B8A8_UNORM` on layered
    /// world materials. In de_mirage the four channels hold the same value: 0 = layer 1, 1 = layer 2.
    pub blend: Vec<[f32; 4]>,
    /// Vertex colour (0–1, RGBA) from `COLOR 0` stored as `R8G8B8A8_UNORM`. On `csgo_environment(_blend)` meshes it
    /// is the painted tint (semantic `VertexPaintTintColor`): colour × lerp(1, rgb, a).
    pub colors: Vec<[f32; 4]>,
    /// Skinning: four bone indices per vertex from `BLENDINDICES 0` (`R8G8B8A8_UINT`, or `R16G16B16A16_UINT`),
    /// indices into the mesh's remapping slice ([`super::Model::bone_remap`]), not skeleton bones yet. Slots whose
    /// weight is 0 can hold leftover indices.
    pub joints: Vec<[u16; 4]>,
    /// Skinning weights from `BLENDWEIGHT 0` (`R8G8B8A8_UNORM`; the bytes sum to exactly 255 on every CS2 agent
    /// vertex). Empty when the mesh stores none (one bone per vertex, `m_nBoneWeightCount` 1).
    pub weights: Vec<[u8; 4]>,
    /// Triangle list indices into the arrays above.
    pub indices: Vec<u32>,
}

/// The vertex buffers ("streams") a draw call reads together. World geometry often keeps positions in one stream
/// and UVs/normals in another; an attribute is looked up across all of them.
struct Streams<'a> {
    buffers: &'a [&'a VertexBuffer],
    /// Number of vertices available in every stream.
    count: usize,
}

impl<'a> Streams<'a> {
    fn new(buffers: &'a [&'a VertexBuffer]) -> Self {
        Self {
            buffers,
            count: buffers.iter().map(|b| b.count).min().unwrap_or(0),
        }
    }

    /// The stream holding `semantic`/`index`, and the attribute.
    fn find(&self, semantic: &str, index: u32) -> Option<(&'a VertexBuffer, &'a Attribute)> {
        self.buffers
            .iter()
            .find_map(|vb| vb.attribute(semantic, index).map(|a| (*vb, a)))
    }
}

/// Bytes of vertex `i` in `vb`.
fn vertex_bytes(vb: &VertexBuffer, i: usize) -> &[u8] {
    &vb.data[i * vb.stride..(i + 1) * vb.stride]
}

impl Geometry {
    /// Decodes the vertices referenced by `draw` from its vertex streams, with indices from `ib`.
    pub(super) fn decode(buffers: &[&VertexBuffer], ib: &[u32], draw: &DrawCall) -> Self {
        let streams = Streams::new(buffers);
        let start = draw.start_index as usize;
        let end = (start + draw.index_count as usize).min(ib.len());
        // Drop whole triangles that point outside the vertex buffers (dropping single indices would shift every
        // later triangle). A geometry without positions has nothing to draw.
        let source_indices: Vec<u32> = if streams.find("POSITION", 0).is_none() {
            Vec::new()
        } else {
            ib[start.min(end)..end]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|tri| tri.map(|i| i + draw.base_vertex))
                .filter(|tri| tri.iter().all(|&i| (i as usize) < streams.count))
                .flatten()
                .collect()
        };

        // Keep only the referenced vertex range, renumbered from 0.
        let min = source_indices.iter().copied().min().unwrap_or(0);
        let max = source_indices.iter().copied().max().unwrap_or(0);
        let vertices = if source_indices.is_empty() {
            0..0
        } else {
            min as usize..max as usize + 1
        };
        let mut geometry = Geometry {
            indices: source_indices.iter().map(|&i| i - min).collect(),
            ..Geometry::default()
        };

        if let Some((vb, attr)) = streams.find("POSITION", 0) {
            geometry.positions = vertices
                .clone()
                .map(|i| read_vec3(vertex_bytes(vb, i), attr))
                .collect();
        }

        if let Some((vb, attr)) = streams.find("NORMAL", 0) {
            let vertex = |i: usize| vertex_bytes(vb, i);
            // R32_UINT and R8G8B8A8_UNORM normals only exist in packed form; trust the format over the flag.
            let packed = draw.compressed_normal_tangent
                || matches!(attr.format, dxgi::R32_UINT | dxgi::R8G8B8A8_UNORM);
            for i in vertices.clone() {
                let bytes = &vertex(i)[attr.offset..];
                match (packed, attr.format) {
                    (true, dxgi::R32_UINT) => {
                        let (n, t) = unpack_normal_tangent_v2(u32::from_le_bytes([
                            bytes[0], bytes[1], bytes[2], bytes[3],
                        ]));
                        geometry.normals.push(n);
                        geometry.tangents.push(t);
                    }
                    (true, dxgi::R8G8B8A8_UNORM) => {
                        let n = unpack_normal_v1(bytes[0], bytes[1]);
                        let t = unpack_normal_v1(bytes[2], bytes[3]);
                        geometry.normals.push(n);
                        geometry.tangents.push([
                            t[0],
                            t[1],
                            t[2],
                            if bytes[3] < 128 { -1.0 } else { 1.0 },
                        ]);
                    }
                    _ => geometry.normals.push(normalize(read_vec3(vertex(i), attr))),
                }
            }
        }
        if geometry.tangents.is_empty()
            && let Some((vb, attr)) = streams.find("TANGENT", 0)
            && attr.format == dxgi::R32G32B32A32_FLOAT
        {
            geometry.tangents = vertices
                .clone()
                .map(|i| {
                    let b = &vertex_bytes(vb, i)[attr.offset..];
                    let f = |k: usize| {
                        f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
                    };
                    [f(0), f(1), f(2), if f(3) < 0.0 { -1.0 } else { 1.0 }]
                })
                .collect();
        }

        if let Some((vb, attr)) = streams.find("TEXCOORD", 4)
            && attr.format == dxgi::R8G8B8A8_UNORM
        {
            geometry.blend = vertices
                .clone()
                .map(|i| {
                    let b = &vertex_bytes(vb, i)[attr.offset..];
                    [b[0], b[1], b[2], b[3]].map(|c| f32::from(c) / 255.0)
                })
                .collect();
        }

        if let Some((vb, attr)) = streams.find("COLOR", 0)
            && attr.format == dxgi::R8G8B8A8_UNORM
        {
            geometry.colors = vertices
                .clone()
                .map(|i| {
                    let b = &vertex_bytes(vb, i)[attr.offset..];
                    [b[0], b[1], b[2], b[3]].map(|c| f32::from(c) / 255.0)
                })
                .collect();
        }

        if let Some((vb, attr)) = streams.find("BLENDINDICES", 0) {
            let read = |i: usize| -> Option<[u16; 4]> {
                let b = &vertex_bytes(vb, i)[attr.offset..];
                match attr.format {
                    dxgi::R8G8B8A8_UINT => Some([b[0], b[1], b[2], b[3]].map(u16::from)),
                    dxgi::R16G16B16A16_UINT => Some(std::array::from_fn(|k| {
                        u16::from_le_bytes([b[2 * k], b[2 * k + 1]])
                    })),
                    _ => None,
                }
            };
            geometry.joints = vertices.clone().map_while(read).collect();
            if geometry.joints.len() != vertices.len() {
                geometry.joints.clear();
            }
        }
        if let Some((vb, attr)) = streams.find("BLENDWEIGHT", 0)
            && attr.format == dxgi::R8G8B8A8_UNORM
        {
            geometry.weights = vertices
                .clone()
                .map(|i| {
                    let b = &vertex_bytes(vb, i)[attr.offset..];
                    [b[0], b[1], b[2], b[3]]
                })
                .collect();
        }

        // Consecutive texture coordinate sets from 0 (set 3 on world geometry is the lightmap UV, skipped when
        // sets 1–2 are absent).
        for set in 0..8 {
            let Some((vb, attr)) = streams.find("TEXCOORD", set) else {
                break;
            };
            let Some(uvs) = vertices
                .clone()
                .map(|i| read_uv(vertex_bytes(vb, i), attr))
                .collect::<Option<Vec<_>>>()
            else {
                break;
            };
            geometry.uvs.push(uvs);
        }
        geometry
    }
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_vec3(vertex: &[u8], attr: &Attribute) -> [f32; 3] {
    let o = attr.offset;
    match attr.format {
        dxgi::R32G32B32_FLOAT | dxgi::R32G32B32A32_FLOAT => [
            read_f32(vertex, o),
            read_f32(vertex, o + 4),
            read_f32(vertex, o + 8),
        ],
        dxgi::R16G16B16A16_FLOAT => {
            [0, 2, 4].map(|k| half_to_f32(u16::from_le_bytes([vertex[o + k], vertex[o + k + 1]])))
        }
        _ => [0.0; 3],
    }
}

/// Reads a 2-component texture coordinate; `None` for formats that are not 2-component UVs.
fn read_uv(vertex: &[u8], attr: &Attribute) -> Option<[f32; 2]> {
    let o = attr.offset;
    let u16_at = |k: usize| u16::from_le_bytes([vertex[o + k], vertex[o + k + 1]]);
    Some(match attr.format {
        dxgi::R32G32_FLOAT => [read_f32(vertex, o), read_f32(vertex, o + 4)],
        dxgi::R16G16_FLOAT => [half_to_f32(u16_at(0)), half_to_f32(u16_at(2))],
        dxgi::R16G16_UNORM => [
            f32::from(u16_at(0)) / 65535.0,
            f32::from(u16_at(2)) / 65535.0,
        ],
        dxgi::R16G16_SNORM => [
            (f32::from(u16_at(0) as i16) / 32767.0).max(-1.0),
            (f32::from(u16_at(2) as i16) / 32767.0).max(-1.0),
        ],
        _ => return None,
    })
}

/// IEEE 754 half-precision to single-precision.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (h >> 10) & 0x1F;
    let mantissa = f32::from(h & 0x3FF);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24), // subnormal
        31 => {
            if mantissa == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(i32::from(e) - 15),
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// CS2 `R32_UINT` packed tangent frame: bit 0 = bitangent sign, bits 1–11 = tangent angle, bits 12–21 and
/// 22–31 = octahedral normal X and Y.
pub fn unpack_normal_tangent_v2(packed: u32) -> ([f32; 3], [f32; 4]) {
    let sign_bit = packed & 1;
    let angle_bits = (packed >> 1) & 0x7FF;
    let x = (packed >> 12) & 0x3FF;
    let y = (packed >> 22) & 0x3FF;

    // Octahedral decode.
    let mut nx = x as f32 / 1023.0 * 2.0 - 1.0;
    let mut ny = y as f32 / 1023.0 * 2.0 - 1.0;
    let nz = 1.0 - nx.abs() - ny.abs();
    let fold = (-nz).clamp(0.0, 1.0);
    nx += if nx >= 0.0 { -fold } else { fold };
    ny += if ny >= 0.0 { -fold } else { fold };
    let n = normalize([nx, ny, nz]);

    // A reference tangent perpendicular to the normal (branch-free orthonormal basis), rotated by the stored angle.
    let s = if n[2] >= 0.0 { 1.0 } else { -1.0 };
    let r = 1.0 / (s + n[2]);
    let t0 = [
        -s * n[0] * n[0] * r + 1.0,
        -s * (n[0] * n[1] * r),
        -s * n[0],
    ];
    let angle = angle_bits as f32 / 2047.0 * std::f32::consts::TAU;
    let b0 = cross(n, t0);
    let (sin, cos) = angle.sin_cos();
    let t = normalize([
        t0[0] * cos + b0[0] * sin,
        t0[1] * cos + b0[1] * sin,
        t0[2] * cos + b0[2] * sin,
    ]);
    (
        n,
        [t[0], t[1], t[2], if sign_bit == 0 { -1.0 } else { 1.0 }],
    )
}

/// Older two-byte folded octahedral normal (`R8G8B8A8_UNORM` packing, one pair of bytes).
pub fn unpack_normal_v1(bx: u8, by: u8) -> [f32; 3] {
    let mut x = f32::from(bx) - 128.0;
    let mut y = f32::from(by) - 128.0;
    let z_negative = x < 0.0;
    let t_negative = y < 0.0;
    let z_sign = if z_negative { -1.0 } else { 1.0 };
    let t_sign = if t_negative { -1.0 } else { 1.0 };
    x = x * z_sign - f32::from(u8::from(z_negative));
    y = y * t_sign - f32::from(u8::from(t_negative));
    x -= 64.0;
    y -= 64.0;
    let x_negative = x < 0.0;
    let y_negative = y < 0.0;
    let x_sign = if x_negative { -1.0 } else { 1.0 };
    let y_sign = if y_negative { -1.0 } else { 1.0 };
    x = (x * x_sign - f32::from(u8::from(x_negative))) / 63.0;
    y = (y * y_sign - f32::from(u8::from(y_negative))) / 63.0;
    let z = 1.0 - x - y;
    let n = normalize([x, y, z]);
    [n[0] * x_sign, n[1] * y_sign, n[2] * z_sign]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0x0000), 0.0);
    }

    #[test]
    fn octahedral_centre_is_straight_up_with_a_perpendicular_tangent() {
        // X = Y = 511.5 is not representable; 512 is the closest to the centre of the octahedron.
        let packed = (512 << 22) | (512 << 12) | 1;
        let (n, t) = unpack_normal_tangent_v2(packed);
        assert!(close(n[2], 1.0), "{n:?}");
        let dot = n[0] * t[0] + n[1] * t[1] + n[2] * t[2];
        assert!(dot.abs() < 1e-3, "tangent ⟂ normal, dot = {dot}");
        assert_eq!(t[3], 1.0);
    }

    #[test]
    fn packed_frames_are_always_unit_length() {
        for packed in [0u32, 0xFFFF_FFFF, 0x1234_5678, 0x8000_0001, 0x0FF0_0FF0] {
            let (n, t) = unpack_normal_tangent_v2(packed);
            let ln = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let lt = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
            assert!(
                close(ln, 1.0) && close(lt, 1.0),
                "{packed:08X}: {n:?} {t:?}"
            );
        }
    }

    #[test]
    fn v1_normals_are_unit_length() {
        for (a, b) in [(0, 0), (255, 255), (128, 128), (200, 30)] {
            let n = unpack_normal_v1(a, b);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!(close(len, 1.0), "{a},{b}: {n:?}");
        }
    }
}
