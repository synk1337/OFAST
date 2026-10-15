//! Decoders for [meshoptimizer](https://github.com/zeux/meshoptimizer)'s vertex and index buffer compression, which
//! CS2 uses for every model mesh (`MVTX` and `MIDX` blocks with `m_bMeshoptCompressed = true`).
//!
//! # Vertex codec (header `0xA0 | version`, versions 0 and 1)
//!
//! Vertices are processed in **blocks** of up to 256 vertices. Inside a block, the data is transposed: byte 0 of
//! every vertex, then byte 1 of every vertex, … Each of these byte streams is stored as **groups of 16 bytes**,
//! each group packed with 0, 1 (v1 only), 2, 4 or 8 bits per byte; a value equal to the all-ones pattern is an
//! "escape" whose real value follows the packed bits. Before packing, each 4-byte **channel** was delta-encoded
//! against the previous vertex: per byte (zigzag), per 16-bit half (zigzag), or per 32-bit word (XOR with a
//! rotation) — version 1 picks the mode per channel and lets each byte stream be stored literally or as zeros.
//! The first vertex used as the delta base is stored in the **tail** at the end of the buffer.
//!
//! # Index codec (header `0xE0 | version`, versions 0 and 1)
//!
//! Triangles are rebuilt one at a time from a code byte: reuse an edge from a 16-entry **edge FIFO**, reuse a vertex
//! from a 16-entry **vertex FIFO**, take the next never-seen vertex, or read an explicit (zigzag, varint) delta.
//!
//! This is an independent implementation written from the meshoptimizer source and the glTF
//! `EXT_meshopt_compression` specification (both MIT). It is verified against meshoptimizer's own test vectors
//! (below) and against every mesh of the CS2 models tested in `tests/cs2_model.rs`.

/// Everything that can go wrong while decoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeshoptError {
    /// The header byte is not a meshopt header of a supported version.
    #[error("not a meshopt {codec} buffer, or unsupported version (header 0x{header:02X})")]
    BadHeader {
        /// "vertex" or "index".
        codec: &'static str,
        /// The header byte found.
        header: u8,
    },
    /// The vertex size is not a multiple of 4 or exceeds 256 bytes.
    #[error("meshopt vertex size {0} is invalid (must be a multiple of 4, at most 256)")]
    BadVertexSize(usize),
    /// The index count is not a multiple of 3.
    #[error("meshopt index count {0} is not a multiple of 3")]
    BadIndexCount(usize),
    /// The data ended too early.
    #[error("meshopt {0} data is truncated")]
    Truncated(&'static str),
    /// The data did not end exactly where it should (corrupt, or a misunderstanding of the format).
    #[error("meshopt {0} data has an unexpected size")]
    SizeMismatch(&'static str),
    /// A channel uses the reserved delta mode 3.
    #[error("meshopt vertex channel uses the reserved delta mode")]
    BadChannelMode,
}

const VERTEX_HEADER: u8 = 0xA0;
const INDEX_HEADER: u8 = 0xE0;
const BLOCK_SIZE_BYTES: usize = 8192;
const BLOCK_MAX_VERTICES: usize = 256;
const GROUP_SIZE: usize = 16;
const BITS_V0: [u8; 4] = [0, 2, 4, 8];
const BITS_V1: [u8; 5] = [0, 1, 2, 4, 8];

/// Bounds-checked forward reader over the compressed bytes.
struct Input<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Input<'a> {
    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn byte(&mut self) -> Result<u8, MeshoptError> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or(MeshoptError::Truncated(self.what))?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], MeshoptError> {
        let bytes = self
            .data
            .get(self.pos..self.pos + len)
            .ok_or(MeshoptError::Truncated(self.what))?;
        self.pos += len;
        Ok(bytes)
    }
}

/// Number of vertices per block for a given vertex size (part of the format).
fn block_vertices(vertex_size: usize) -> usize {
    ((BLOCK_SIZE_BYTES / vertex_size) & !(GROUP_SIZE - 1)).min(BLOCK_MAX_VERTICES)
}

/// Decodes a meshopt-compressed vertex buffer of `count` vertices of `vertex_size` bytes each.
pub fn decode_vertex_buffer(
    count: usize,
    vertex_size: usize,
    data: &[u8],
) -> Result<Vec<u8>, MeshoptError> {
    if vertex_size == 0 || !vertex_size.is_multiple_of(4) || vertex_size > 256 {
        return Err(MeshoptError::BadVertexSize(vertex_size));
    }
    let mut input = Input {
        data,
        pos: 0,
        what: "vertex",
    };
    let header = input.byte()?;
    let version = header & 0x0F;
    if header & 0xF0 != VERTEX_HEADER || version > 1 {
        return Err(MeshoptError::BadHeader {
            codec: "vertex",
            header,
        });
    }

    // Tail: the initial "previous vertex" (+ v1 channel modes), padded to a minimum size.
    let tail_size = vertex_size + if version == 0 { 0 } else { vertex_size / 4 };
    let tail_padded = tail_size.max(if version == 0 { 32 } else { 24 });
    if input.remaining() < tail_padded {
        return Err(MeshoptError::Truncated("vertex"));
    }
    let tail = &data[data.len() - tail_size..];
    let mut last_vertex = tail[..vertex_size].to_vec();
    let channels = if version == 0 {
        &[][..]
    } else {
        &tail[vertex_size..]
    };
    // Blocks must stop before the tail padding.
    input.data = &data[..data.len() - tail_padded];

    let mut out = vec![0u8; count * vertex_size];
    let per_block = block_vertices(vertex_size);
    let mut start = 0;
    while start < count {
        let n = per_block.min(count - start);
        decode_vertex_block(
            &mut input,
            &mut out[start * vertex_size..(start + n) * vertex_size],
            n,
            vertex_size,
            &mut last_vertex,
            channels,
            version,
        )?;
        start += n;
    }
    if input.remaining() != 0 {
        return Err(MeshoptError::SizeMismatch("vertex"));
    }
    Ok(out)
}

/// Decodes one block of `n` vertices into `out` (`n × vertex_size` bytes).
fn decode_vertex_block(
    input: &mut Input<'_>,
    out: &mut [u8],
    n: usize,
    vertex_size: usize,
    last_vertex: &mut [u8],
    channels: &[u8],
    version: u8,
) -> Result<(), MeshoptError> {
    let n16 = n.next_multiple_of(GROUP_SIZE);
    let control = if version == 0 {
        &[][..]
    } else {
        input.take(vertex_size / 4)?
    };
    // Four byte streams of one channel, each `n16` long (only the first `n` bytes matter).
    let mut streams = vec![0u8; 4 * n16];

    for k in (0..vertex_size).step_by(4) {
        let ctrl_byte = control.get(k / 4).copied().unwrap_or(0);
        for j in 0..4 {
            let stream = &mut streams[j * n16..(j + 1) * n16];
            let ctrl = (ctrl_byte >> (2 * j)) & 3;
            match ctrl {
                3 => stream[..n].copy_from_slice(input.take(n)?),
                2 => stream.fill(0),
                _ => {
                    let bits: &[u8] = if version == 0 {
                        &BITS_V0
                    } else {
                        &BITS_V1[ctrl as usize..]
                    };
                    decode_bytes(input, stream, bits)?;
                }
            }
        }

        let channel = channels.get(k / 4).copied().unwrap_or(0);
        let prev = &last_vertex[k..k + 4];
        let streams = &streams[..];
        let byte = |j: usize, i: usize| streams[j * n16 + i];
        match channel & 3 {
            0 => {
                // Per byte: zigzag delta.
                for j in 0..4 {
                    let mut p = prev[j];
                    for i in 0..n {
                        let v = unzigzag8(byte(j, i)).wrapping_add(p);
                        out[i * vertex_size + k + j] = v;
                        p = v;
                    }
                }
            }
            1 => {
                // Per 16-bit half: zigzag delta.
                for h in [0, 2] {
                    let mut p = u16::from_le_bytes([prev[h], prev[h + 1]]);
                    for i in 0..n {
                        let raw = u16::from_le_bytes([byte(h, i), byte(h + 1, i)]);
                        let v = unzigzag16(raw).wrapping_add(p);
                        out[i * vertex_size + k + h..][..2].copy_from_slice(&v.to_le_bytes());
                        p = v;
                    }
                }
            }
            2 => {
                // Per 32-bit word: XOR with the previous word after rotating right by `channel >> 4`.
                let rotate = u32::from(channel >> 4);
                let mut p = u32::from_le_bytes([prev[0], prev[1], prev[2], prev[3]]);
                for i in 0..n {
                    let raw = u32::from_le_bytes([byte(0, i), byte(1, i), byte(2, i), byte(3, i)]);
                    let v = raw.rotate_left((32 - rotate) & 31) ^ p;
                    out[i * vertex_size + k..][..4].copy_from_slice(&v.to_le_bytes());
                    p = v;
                }
            }
            _ => return Err(MeshoptError::BadChannelMode),
        }
    }
    last_vertex.copy_from_slice(&out[(n - 1) * vertex_size..n * vertex_size]);
    Ok(())
}

/// Decodes one byte stream: a header of 2-bit group modes, then the groups.
fn decode_bytes(input: &mut Input<'_>, stream: &mut [u8], bits: &[u8]) -> Result<(), MeshoptError> {
    let groups = stream.len() / GROUP_SIZE;
    let header = input.take(groups.div_ceil(4))?;
    // The reference decoder requires 24 bytes left (tail included) before each group so it can skip bounds checks.
    // Our `Input` ends before the padded tail (always ≥ 24 bytes) and every read is bounds-checked, so valid data
    // decodes identically and invalid data reports `Truncated`.
    for (g, group) in stream
        .as_chunks_mut::<GROUP_SIZE>()
        .0
        .iter_mut()
        .enumerate()
    {
        let mode = (header[g / 4] >> ((g % 4) * 2)) & 3;
        decode_group(input, group, bits[mode as usize])?;
    }
    Ok(())
}

/// Decodes one group of 16 bytes packed with `bits` bits each (plus escapes).
fn decode_group(input: &mut Input<'_>, group: &mut [u8], bits: u8) -> Result<(), MeshoptError> {
    match bits {
        0 => group.fill(0),
        8 => group.copy_from_slice(input.take(GROUP_SIZE)?),
        1 | 2 | 4 => {
            let packed_len = GROUP_SIZE * bits as usize / 8;
            let packed = input.take(packed_len)?;
            let sentinel = (1u8 << bits) - 1;
            for (i, out) in group.iter_mut().enumerate() {
                let value = match bits {
                    // 1-bit groups are LSB-first, the others MSB-first.
                    1 => (packed[i / 8] >> (i % 8)) & 1,
                    2 => (packed[i / 4] >> (6 - 2 * (i % 4))) & 3,
                    _ => (packed[i / 2] >> (4 - 4 * (i % 2))) & 15,
                };
                *out = if value == sentinel {
                    input.byte()?
                } else {
                    value
                };
            }
        }
        _ => unreachable!("bit widths come from the fixed tables"),
    }
    Ok(())
}

fn unzigzag8(v: u8) -> u8 {
    (v >> 1) ^ (v & 1).wrapping_neg()
}

fn unzigzag16(v: u16) -> u16 {
    (v >> 1) ^ (v & 1).wrapping_neg()
}

/// Decodes a meshopt-compressed triangle index buffer of `count` indices (a multiple of 3).
pub fn decode_index_buffer(count: usize, data: &[u8]) -> Result<Vec<u32>, MeshoptError> {
    if !count.is_multiple_of(3) {
        return Err(MeshoptError::BadIndexCount(count));
    }
    let triangles = count / 3;
    if data.len() < 1 + triangles + 16 {
        return Err(MeshoptError::Truncated("index"));
    }
    let header = data[0];
    let version = header & 0x0F;
    if header & 0xF0 != INDEX_HEADER || version > 1 {
        return Err(MeshoptError::BadHeader {
            codec: "index",
            header,
        });
    }

    let codes = &data[1..1 + triangles];
    let safe_end = data.len() - 16;
    let codeaux = &data[safe_end..];
    // Free-index data lives between the codes and the 16-byte codeaux table.
    let mut input = Input {
        data: &data[..safe_end],
        pos: 1 + triangles,
        what: "index",
    };

    let mut edges = Fifo::new([u32::MAX; 2]);
    let mut vertices = Fifo::new(u32::MAX);
    let (mut next, mut last) = (0u32, 0u32);
    let fec_max = if version >= 1 { 13 } else { 15 };
    let mut out = Vec::with_capacity(count);

    for &code in codes {
        if code < 0xF0 {
            // Reuse edge `fe` from the edge FIFO; the third vertex comes from code nibble `fec`.
            let fe = (code >> 4) as usize;
            let [a, b] = edges.back(1 + fe);
            let fec = (code & 15) as usize;
            let c = if fec < fec_max {
                // 0 = the next new vertex, 1..=12 (or 14 in v0) = a recent vertex from the FIFO.
                let c = if fec == 0 {
                    next
                } else {
                    vertices.back(1 + fec)
                };
                next += u32::from(fec == 0);
                vertices.push(c, fec == 0);
                c
            } else {
                // v1: 13 → previous free index − 1, 14 → + 1; 15 → explicit zigzag varint delta.
                let c = if fec != 15 {
                    last.wrapping_add((2 * fec as u32).wrapping_sub(27))
                } else {
                    decode_index(&mut input, last)?
                };
                last = c;
                vertices.push(c, true);
                c
            };
            out.extend_from_slice(&[a, b, c]);
            edges.push([c, b], true);
            edges.push([a, c], true);
        } else {
            // A triangle with no reused edge. 0xF0..=0xFD: nibbles from the codeaux table; 0xFE/0xFF: an explicit
            // aux byte (0xFF also means the first vertex is an explicit index).
            let slow = code >= 0xFE;
            let (fea, aux) = if slow {
                (if code == 0xFE { 0 } else { 15 }, input.byte()?)
            } else {
                (0, codeaux[(code & 15) as usize])
            };
            let feb = (aux >> 4) as usize;
            let fec = (aux & 15) as usize;
            if slow && aux == 0 {
                next = 0; // explicit reset of the "next new vertex" counter
            }
            let take = |fe: usize, next: &mut u32| {
                if fe == 0 {
                    *next += 1;
                    *next - 1
                } else {
                    vertices.back(fe)
                }
            };
            let mut a = if fea == 0 { take(0, &mut next) } else { 0 };
            let mut b = take(feb, &mut next);
            let mut c = take(fec, &mut next);
            if slow {
                for (value, fe) in [(&mut a, fea), (&mut b, feb), (&mut c, fec)] {
                    if fe == 15 {
                        *value = decode_index(&mut input, last)?;
                        last = *value;
                    }
                }
            }
            out.extend_from_slice(&[a, b, c]);
            vertices.push(a, true);
            vertices.push(b, feb == 0 || (slow && feb == 15));
            vertices.push(c, fec == 0 || (slow && fec == 15));
            edges.push([b, a], true);
            edges.push([c, b], true);
            edges.push([a, c], true);
        }
    }
    if input.pos != safe_end {
        return Err(MeshoptError::SizeMismatch("index"));
    }
    Ok(out)
}

/// A 16-entry ring buffer of recently used edges or vertices.
struct Fifo<T: Copy> {
    items: [T; 16],
    pos: usize,
}

impl<T: Copy> Fifo<T> {
    fn new(fill: T) -> Self {
        Self {
            items: [fill; 16],
            pos: 0,
        }
    }

    /// The entry `distance` places behind the write position.
    fn back(&self, distance: usize) -> T {
        self.items[self.pos.wrapping_sub(distance) & 15]
    }

    /// Writes `value` at the write position, advancing it only when `advance` is true (the format sometimes
    /// overwrites the slot without advancing).
    fn push(&mut self, value: T, advance: bool) {
        self.items[self.pos] = value;
        self.pos = (self.pos + usize::from(advance)) & 15;
    }
}

/// Reads a zigzag varint delta and applies it to `last`.
fn decode_index(input: &mut Input<'_>, last: u32) -> Result<u32, MeshoptError> {
    let v = decode_vbyte(input)?;
    Ok(last.wrapping_add((v >> 1) ^ (v & 1).wrapping_neg()))
}

/// Little-endian base-128 varint (at most 5 bytes).
fn decode_vbyte(input: &mut Input<'_>) -> Result<u32, MeshoptError> {
    let lead = input.byte()?;
    if lead < 128 {
        return Ok(u32::from(lead));
    }
    let mut result = u32::from(lead & 127);
    let mut shift = 7;
    for _ in 0..4 {
        let group = input.byte()?;
        result |= u32::from(group & 127) << shift;
        shift += 7;
        if group < 128 {
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vectors from meshoptimizer's demo/tests.cpp (MIT, © Arseny Kapoulkine).

    /// The 4 vertices both vertex vectors encode: `{u16 px,py,pz; u8 nu,nv; u16 tx,ty}`.
    fn expected_vertices() -> Vec<u8> {
        let mut out = Vec::new();
        for (px, py, tx, ty) in [
            (0u16, 0u16, 0u16, 0u16),
            (300, 0, 500, 0),
            (0, 300, 0, 500),
            (300, 300, 500, 500),
        ] {
            out.extend_from_slice(&px.to_le_bytes());
            out.extend_from_slice(&py.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&tx.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
        }
        out
    }

    #[rustfmt::skip]
    const VERTEX_V0: [u8; 85] = [
        0xa0, 0x01, 0x3f, 0x00, 0x00, 0x00, 0x58, 0x57, 0x58, 0x01, 0x26, 0x00, 0x00, 0x00, 0x01, 0x0c,
        0x00, 0x00, 0x00, 0x58, 0x01, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x3f, 0x00,
        0x00, 0x00, 0x17, 0x18, 0x17, 0x01, 0x26, 0x00, 0x00, 0x00, 0x01, 0x0c, 0x00, 0x00, 0x00, 0x17,
        0x01, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    #[rustfmt::skip]
    const VERTEX_V1: [u8; 44] = [
        0xa1, 0xee, 0xaa, 0xee, 0x00, 0x4b, 0x4b, 0x4b, 0x00, 0x00, 0x4b, 0x00, 0x00, 0x7d, 0x7d, 0x7d,
        0x00, 0x00, 0x7d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x62, 0x00, 0x62,
    ];

    #[test]
    fn vertex_v0_reference_vector() {
        assert_eq!(
            decode_vertex_buffer(4, 12, &VERTEX_V0).unwrap(),
            expected_vertices()
        );
    }

    #[test]
    fn vertex_v1_reference_vector() {
        assert_eq!(
            decode_vertex_buffer(4, 12, &VERTEX_V1).unwrap(),
            expected_vertices()
        );
    }

    #[test]
    fn vertex_errors_are_reported() {
        assert!(matches!(
            decode_vertex_buffer(4, 10, &VERTEX_V1),
            Err(MeshoptError::BadVertexSize(10))
        ));
        let mut bad = VERTEX_V1;
        bad[0] = 0xa5;
        assert!(matches!(
            decode_vertex_buffer(4, 12, &bad),
            Err(MeshoptError::BadHeader { .. })
        ));
        assert!(decode_vertex_buffer(4, 12, &VERTEX_V1[..30]).is_err());
        // Asking for more vertices than were encoded runs out of data.
        assert!(decode_vertex_buffer(40, 12, &VERTEX_V1).is_err());
    }

    #[rustfmt::skip]
    const INDEX_V0: [u8; 27] = [
        0xe0, 0xf0, 0x10, 0xfe, 0xff, 0xf0, 0x0c, 0xff, 0x02, 0x02, 0x02, 0x00, 0x76, 0x87, 0x56, 0x67,
        0x78, 0xa9, 0x86, 0x65, 0x89, 0x68, 0x98, 0x01, 0x69, 0x00, 0x00,
    ];

    #[rustfmt::skip]
    const INDEX_V1: [u8; 24] = [
        0xe1, 0xf0, 0x10, 0xfe, 0x1f, 0x3d, 0x00, 0x0a, 0x00, 0x76, 0x87, 0x56, 0x67, 0x78, 0xa9, 0x86,
        0x65, 0x89, 0x68, 0x98, 0x01, 0x69, 0x00, 0x00,
    ];

    #[test]
    fn index_v0_reference_vector() {
        assert_eq!(
            decode_index_buffer(12, &INDEX_V0).unwrap(),
            [0, 1, 2, 2, 1, 3, 4, 6, 5, 7, 8, 9]
        );
    }

    #[test]
    fn index_v1_reference_vector() {
        assert_eq!(
            decode_index_buffer(15, &INDEX_V1).unwrap(),
            [0, 1, 2, 2, 1, 3, 0, 1, 2, 2, 1, 5, 2, 1, 4]
        );
    }

    #[test]
    fn index_errors_are_reported() {
        assert!(matches!(
            decode_index_buffer(10, &INDEX_V1),
            Err(MeshoptError::BadIndexCount(10))
        ));
        assert!(decode_index_buffer(15, &INDEX_V1[..20]).is_err());
        let mut bad = INDEX_V1;
        bad[0] = 0xe7;
        assert!(matches!(
            decode_index_buffer(15, &bad),
            Err(MeshoptError::BadHeader { .. })
        ));
    }
}
