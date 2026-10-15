//! Bit-level reader for the parts of a CS2 demo that are not byte-aligned.
//!
//! Network packets, string-table entries and entity updates are **bit streams**: a value can start in the middle
//! of a byte and take any number of bits. Valve's bit buffers are *least-significant-bit first*: the first bit of
//! the stream is bit 0 (value 1) of byte 0, the ninth bit is bit 0 of byte 1, and a multi-bit value read from the
//! stream has its first bit as its lowest bit. That is the same as reading the whole buffer as one huge
//! little-endian integer and taking bits from the bottom.
//!
//! On top of raw bit reads, Source 2 uses a handful of variable-length and fixed-point encodings, each explained on
//! its method below and in `docs/formats/demo.md`. Knowledge source: demoparser2 `first_pass/read_bits.rs` and
//! demoinfocs-golang `pkg/demoinfocs/bitread` (both MIT), re-derived and re-written here.

/// Error returned when a bit read runs past the end of the buffer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unexpected end of bit stream while reading {what} at bit {bit} (needed {needed} bits, {available} left)"
)]
pub struct BitsEof {
    /// Human-readable name of the value being read.
    pub what: &'static str,
    /// Bit position where the read started.
    pub bit: usize,
    /// Number of bits the read needed.
    pub needed: usize,
    /// Number of bits that were left.
    pub available: usize,
}

/// Result type of every [`BitReader`] method.
pub type BitsResult<T> = Result<T, BitsEof>;

/// Number of integer bits in a "bit coord" (world coordinates up to ±16384 units).
const COORD_INTEGER_BITS: u32 = 14;
/// Number of fraction bits in a "bit coord" (1/32 unit resolution).
const COORD_FRACTIONAL_BITS: u32 = 5;
/// Value of one fraction step of a "bit coord".
const COORD_RESOLUTION: f32 = 1.0 / (1 << COORD_FRACTIONAL_BITS) as f32;
/// Bits in a fixed-point component of a unit normal vector.
const NORMAL_FRACTIONAL_BITS: u32 = 11;
/// Value of one step of a normal component: the 11-bit range maps to 0.0 ..= 1.0.
const NORMAL_RESOLUTION: f32 = 1.0 / ((1 << NORMAL_FRACTIONAL_BITS) - 1) as f32;

/// Cursor over a byte slice that reads values bit by bit, least-significant bit first.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits from the start of `data`.
    bit: usize,
}

impl<'a> BitReader<'a> {
    /// Starts reading `data` at bit 0.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    /// Current position, in bits from the start.
    pub fn position(&self) -> usize {
        self.bit
    }

    /// Number of bits left.
    pub fn remaining_bits(&self) -> usize {
        // Saturating: on 32-bit targets a buffer over 512 MB has more bits than `usize` can count.
        self.data.len().saturating_mul(8) - self.bit
    }

    /// Fails with [`BitsEof`] unless at least `n` bits are left.
    fn ensure(&self, n: usize, what: &'static str) -> BitsResult<()> {
        let available = self.remaining_bits();
        if n > available {
            return Err(BitsEof {
                what,
                bit: self.bit,
                needed: n,
                available,
            });
        }
        Ok(())
    }

    /// Reads `n` bits (0..=32) as an unsigned number. The first bit read becomes bit 0 of the result.
    pub fn bits(&mut self, n: u32, what: &'static str) -> BitsResult<u32> {
        debug_assert!(n <= 32, "bits() reads at most 32 bits");
        if n == 0 {
            return Ok(0);
        }
        self.ensure(n as usize, what)?;
        let byte = self.bit / 8;
        let shift = (self.bit % 8) as u32;
        // Load up to 8 bytes starting at the current byte as a little-endian u64. `shift + n` is at most 39, so the
        // wanted bits always fit; bytes past the end of the buffer read as zero.
        let mut window = [0u8; 8];
        let available = (self.data.len() - byte).min(8);
        window[..available].copy_from_slice(&self.data[byte..byte + available]);
        let value = (u64::from_le_bytes(window) >> shift) & ((1u64 << n) - 1);
        self.bit += n as usize;
        Ok(value as u32)
    }

    /// Reads up to 64 bits as an unsigned number.
    pub fn bits_u64(&mut self, n: u32, what: &'static str) -> BitsResult<u64> {
        if n <= 32 {
            return self.bits(n, what).map(u64::from);
        }
        self.ensure(n as usize, what)?;
        let low = u64::from(self.bits(32, what)?);
        let high = u64::from(self.bits(n - 32, what)?);
        Ok(low | (high << 32))
    }

    /// Reads one bit as a boolean.
    pub fn bool(&mut self, what: &'static str) -> BitsResult<bool> {
        Ok(self.bits(1, what)? == 1)
    }

    /// Reads 8 bits.
    pub fn u8(&mut self, what: &'static str) -> BitsResult<u8> {
        Ok(self.bits(8, what)? as u8)
    }

    /// Reads `len` whole bytes (not necessarily byte-aligned in the stream).
    pub fn bytes(&mut self, len: usize, what: &'static str) -> BitsResult<Vec<u8>> {
        // `len` usually comes from the file; an absurd one must fail cleanly, not overflow.
        let bits = len.checked_mul(8).ok_or(BitsEof {
            what,
            bit: self.bit,
            needed: usize::MAX,
            available: self.remaining_bits(),
        })?;
        self.ensure(bits, what)?;
        if self.bit.is_multiple_of(8) {
            let start = self.bit / 8;
            self.bit += bits;
            return Ok(self.data[start..start + len].to_vec());
        }
        (0..len).map(|_| self.u8(what)).collect()
    }

    /// Skips `n` bits.
    pub fn skip(&mut self, n: usize, what: &'static str) -> BitsResult<()> {
        self.ensure(n, what)?;
        self.bit += n;
        Ok(())
    }

    /// Reads a 32-bit IEEE float stored as its raw 32 bits.
    pub fn f32(&mut self, what: &'static str) -> BitsResult<f32> {
        Ok(f32::from_bits(self.bits(32, what)?))
    }

    /// Reads an unsigned varint (LEB128: 7 data bits per byte, high bit = "more follows"), at most 5 bytes.
    pub fn varint_u32(&mut self, what: &'static str) -> BitsResult<u32> {
        let mut value = 0u32;
        for i in 0..5 {
            let byte = self.u8(what)?;
            value |= u32::from(byte & 0x7F).wrapping_shl(7 * i);
            if byte & 0x80 == 0 {
                break;
            }
        }
        Ok(value)
    }

    /// Reads an unsigned varint of up to 64 bits (at most 10 bytes).
    pub fn varint_u64(&mut self, what: &'static str) -> BitsResult<u64> {
        let mut value = 0u64;
        for i in 0..10 {
            let byte = self.u8(what)?;
            value |= u64::from(byte & 0x7F) << (7 * i);
            if byte & 0x80 == 0 {
                break;
            }
        }
        Ok(value)
    }

    /// Reads a signed 32-bit varint stored with "zigzag" encoding (0 → 0, 1 → -1, 2 → 1, 3 → -2, …), which keeps
    /// small negative numbers short.
    pub fn varint_i32(&mut self, what: &'static str) -> BitsResult<i32> {
        let raw = self.varint_u32(what)?;
        Ok((raw >> 1) as i32 ^ -((raw & 1) as i32))
    }

    /// Reads a signed 64-bit zigzag varint.
    pub fn varint_i64(&mut self, what: &'static str) -> BitsResult<i64> {
        let raw = self.varint_u64(what)?;
        Ok((raw >> 1) as i64 ^ -((raw & 1) as i64))
    }

    /// Reads Valve's "ubitvar": 6 bits, whose top two bits say how many more bits follow.
    ///
    /// | top bits of the first 6 | result |
    /// |---|---|
    /// | `00` | the low 4 bits as-is (0..=15) |
    /// | `01` | low 4 bits + next 4 bits shifted left by 4 |
    /// | `10` | low 4 bits + next 8 bits shifted left by 4 |
    /// | `11` | low 4 bits + next 28 bits shifted left by 4 |
    ///
    /// Used for message type ids in packets and for entity index deltas.
    pub fn ubitvar(&mut self, what: &'static str) -> BitsResult<u32> {
        let first = self.bits(6, what)?;
        let extra = match first & 0x30 {
            0x10 => 4,
            0x20 => 8,
            0x30 => 28,
            _ => return Ok(first),
        };
        Ok((first & 0x0F) | (self.bits(extra, what)? << 4))
    }

    /// Reads the field-path variant of "ubitvar": a unary prefix picks 2, 4, 10, 17 or 31 bits.
    ///
    /// `1` → 2 bits, `01` → 4 bits, `001` → 10 bits, `0001` → 17 bits, `0000` → 31 bits.
    pub fn ubitvar_field_path(&mut self, what: &'static str) -> BitsResult<u32> {
        for bits in [2, 4, 10, 17] {
            if self.bool(what)? {
                return self.bits(bits, what);
            }
        }
        self.bits(31, what)
    }

    /// Reads a NUL-terminated string (one byte at a time). Invalid UTF-8 is replaced, not rejected.
    pub fn string(&mut self, what: &'static str) -> BitsResult<String> {
        let mut bytes = Vec::new();
        loop {
            let byte = self.u8(what)?;
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Reads a "bit coord": a world coordinate with optional integer and fraction parts.
    ///
    /// Layout: `has_int` (1 bit), `has_frac` (1 bit); if either is set: `sign` (1 bit), then the integer part
    /// (14 bits, stored minus one) if `has_int`, then the fraction (5 bits, in 1/32 steps) if `has_frac`.
    /// Both flags clear means exactly 0.0.
    pub fn coord(&mut self, what: &'static str) -> BitsResult<f32> {
        let has_int = self.bool(what)?;
        let has_frac = self.bool(what)?;
        if !has_int && !has_frac {
            return Ok(0.0);
        }
        let negative = self.bool(what)?;
        let int = if has_int {
            self.bits(COORD_INTEGER_BITS, what)? + 1
        } else {
            0
        };
        let frac = if has_frac {
            self.bits(COORD_FRACTIONAL_BITS, what)?
        } else {
            0
        };
        let value = int as f32 + frac as f32 * COORD_RESOLUTION;
        Ok(if negative { -value } else { value })
    }

    /// Reads an angle stored as an `n`-bit fraction of a full turn: `raw * 360 / 2^n` degrees.
    pub fn angle(&mut self, n: u32, what: &'static str) -> BitsResult<f32> {
        let raw = self.bits(n, what)?;
        Ok(raw as f32 * 360.0 / (1u64 << n) as f32)
    }

    /// Reads one component of a unit normal: a sign bit and an 11-bit magnitude in 0.0 ..= 1.0.
    pub fn normal(&mut self, what: &'static str) -> BitsResult<f32> {
        let negative = self.bool(what)?;
        let magnitude = self.bits(NORMAL_FRACTIONAL_BITS, what)? as f32 * NORMAL_RESOLUTION;
        Ok(if negative { -magnitude } else { magnitude })
    }

    /// Reads a unit normal vector: optional x and y (each a [`normal`](Self::normal) component, absent = 0), then
    /// the sign of z. The length of z follows from `x² + y² + z² = 1`.
    pub fn normal_vector(&mut self, what: &'static str) -> BitsResult<[f32; 3]> {
        let has_x = self.bool(what)?;
        let has_y = self.bool(what)?;
        let x = if has_x { self.normal(what)? } else { 0.0 };
        let y = if has_y { self.normal(what)? } else { 0.0 };
        let negative_z = self.bool(what)?;
        let xy = x * x + y * y;
        let z = if xy < 1.0 { (1.0 - xy).sqrt() } else { 0.0 };
        Ok([x, y, if negative_z { -z } else { z }])
    }
}

/// Writes bits least-significant first — the exact inverse of [`BitReader`]. Used by tests to build synthetic
/// packets and entity updates from the documented format.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

#[cfg(test)]
impl BitWriter {
    /// Appends the low `n` bits of `value`.
    pub(crate) fn bits(&mut self, value: u64, n: u32) -> &mut Self {
        for i in 0..n {
            if self.bit.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.bytes.last_mut().expect("pushed above") |= 1 << (self.bit % 8);
            }
            self.bit += 1;
        }
        self
    }

    /// Appends one bit.
    pub(crate) fn bool(&mut self, value: bool) -> &mut Self {
        self.bits(u64::from(value), 1)
    }

    /// Appends whole bytes.
    pub(crate) fn bytes(&mut self, data: &[u8]) -> &mut Self {
        for &b in data {
            self.bits(u64::from(b), 8);
        }
        self
    }

    /// Appends an unsigned varint.
    pub(crate) fn varint(&mut self, mut value: u64) -> &mut Self {
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                self.bits(u64::from(byte), 8);
                return self;
            }
            self.bits(u64::from(byte | 0x80), 8);
        }
    }

    /// Appends a zigzag-encoded signed varint.
    pub(crate) fn varint_signed(&mut self, value: i64) -> &mut Self {
        self.varint(((value << 1) ^ (value >> 63)) as u64)
    }

    /// Appends a "ubitvar" in its shortest form.
    pub(crate) fn ubitvar(&mut self, value: u32) -> &mut Self {
        let low = u64::from(value & 0x0F);
        let high = u64::from(value >> 4);
        match high {
            0 => self.bits(low, 6),
            h if h < (1 << 4) => self.bits(low | 0x10, 6).bits(h, 4),
            h if h < (1 << 8) => self.bits(low | 0x20, 6).bits(h, 8),
            h => self.bits(low | 0x30, 6).bits(h, 28),
        }
    }

    /// Appends the field-path "ubitvar" in its shortest form.
    pub(crate) fn ubitvar_field_path(&mut self, value: u32) -> &mut Self {
        for bits in [2u32, 4, 10, 17] {
            if u64::from(value) < (1u64 << bits) {
                return self.bool(true).bits(u64::from(value), bits);
            }
            self.bool(false);
        }
        self.bits(u64::from(value), 31)
    }

    /// Returns the bytes written so far (the last byte is zero-padded).
    pub(crate) fn finish(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_read_lsb_first_across_bytes() {
        // 0b1011_0110, 0b0000_0001 → first 3 bits = 0b110, next 6 bits = 0b1_10110.
        let mut r = BitReader::new(&[0b1011_0110, 0b0000_0001]);
        assert_eq!(r.bits(3, "a").unwrap(), 0b110);
        assert_eq!(r.bits(6, "b").unwrap(), 0b1_10110);
        assert_eq!(r.remaining_bits(), 7);
        assert!(r.bits(8, "c").is_err());
    }

    #[test]
    fn writer_and_reader_round_trip() {
        let mut w = BitWriter::default();
        w.bool(true)
            .bits(0x1234_5678_9ABC, 48)
            .varint(300)
            .varint_signed(-5)
            .ubitvar(3)
            .ubitvar(200)
            .ubitvar(5000)
            .ubitvar(1 << 30)
            .ubitvar_field_path(3)
            .ubitvar_field_path(1000)
            .ubitvar_field_path(1 << 20)
            .bytes(b"hi\0");
        let data = w.finish();
        let mut r = BitReader::new(&data);
        assert!(r.bool("flag").unwrap());
        assert_eq!(r.bits_u64(48, "wide").unwrap(), 0x1234_5678_9ABC);
        assert_eq!(r.varint_u32("varint").unwrap(), 300);
        assert_eq!(r.varint_i32("zigzag").unwrap(), -5);
        assert_eq!(r.ubitvar("u1").unwrap(), 3);
        assert_eq!(r.ubitvar("u2").unwrap(), 200);
        assert_eq!(r.ubitvar("u3").unwrap(), 5000);
        assert_eq!(r.ubitvar("u4").unwrap(), 1 << 30);
        assert_eq!(r.ubitvar_field_path("f1").unwrap(), 3);
        assert_eq!(r.ubitvar_field_path("f2").unwrap(), 1000);
        assert_eq!(r.ubitvar_field_path("f3").unwrap(), 1 << 20);
        assert_eq!(r.string("s").unwrap(), "hi");
    }

    #[test]
    fn coord_decodes_sign_integer_and_fraction() {
        // -(100 + 8/32): has_int, has_frac, sign, int-1 = 99 (14 bits), frac = 8 (5 bits).
        let mut w = BitWriter::default();
        w.bool(true).bool(true).bool(true).bits(99, 14).bits(8, 5);
        w.bool(false).bool(false); // exactly zero
        let data = w.finish();
        let mut r = BitReader::new(&data);
        assert_eq!(r.coord("c").unwrap(), -100.25);
        assert_eq!(r.coord("zero").unwrap(), 0.0);
    }

    #[test]
    fn angle_and_normal() {
        let mut w = BitWriter::default();
        w.bits(1 << 7, 8); // half a turn in 8 bits
        // normal: has_x, no y, x = negative 2047/2047, then negative z → (-1, 0, -0)
        w.bool(true)
            .bool(false)
            .bool(true)
            .bits(2047, 11)
            .bool(true);
        let data = w.finish();
        let mut r = BitReader::new(&data);
        assert_eq!(r.angle(8, "a").unwrap(), 180.0);
        let n = r.normal_vector("n").unwrap();
        assert_eq!(n[0], -1.0);
        assert_eq!(n[1], 0.0);
        assert_eq!(n[2], 0.0);
    }
}
