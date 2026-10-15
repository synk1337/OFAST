//! Decoding one networked field value from the entity bit stream.
//!
//! Every field in a serializer (see `serializer.rs`) gets a [`Decoder`] chosen from its C++ type name
//! (`float32`, `Vector`, `QAngle`, `uint32`, `CUtlString`, …), its *encoder* name (`coord`, `normal`,
//! `qangle_precise`, `fixed64`, …) and its bit count / range. The choice rules are in [`Decoder::for_field`] and
//! documented in `docs/formats/demo.md`.
//!
//! Knowledge source: demoinfocs-golang `sendtables/field_decoder.go` and `quantizedfloat.go`, demoparser2
//! `second_pass/decoder.rs` (both MIT). Where the two disagree, demoinfocs-golang's newer handling is followed and
//! the difference is noted in the docs.

use super::bits::{BitReader, BitsResult};

/// A decoded field value.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// A boolean.
    Bool(bool),
    /// A signed integer (zigzag varint on the wire).
    Signed(i64),
    /// An unsigned integer: counts, enums, entity handles, colors, string tokens…
    Unsigned(u64),
    /// A single float.
    Float(f32),
    /// Three floats: a position, direction or angle (pitch, yaw, roll).
    Vector3([f32; 3]),
    /// Any other number of floats (2D/4D vectors, quaternions, transforms).
    Floats(Box<[f32]>),
    /// Text.
    String(Box<str>),
    /// Raw bytes.
    Bytes(Box<[u8]>),
    /// A pointer field's state: `None` = not present; `Some(i)` = present with serializer `i` (0 = the field's own
    /// serializer, 1.. = one of its polymorphic alternatives).
    Pointer(Option<u32>),
}

impl FieldValue {
    /// The value as a float, if it is one.
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Self::Float(v) => Some(v),
            _ => None,
        }
    }

    /// The value as three floats, if it is a 3-vector.
    pub fn as_vector3(&self) -> Option<[f32; 3]> {
        match *self {
            Self::Vector3(v) => Some(v),
            _ => None,
        }
    }

    /// The value as an unsigned integer (signed values that are not negative also convert).
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::Unsigned(v) => Some(v),
            Self::Signed(v) => u64::try_from(v).ok(),
            _ => None,
        }
    }

    /// The value as a signed integer (unsigned values that fit also convert).
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Signed(v) => Some(v),
            Self::Unsigned(v) => i64::try_from(v).ok(),
            _ => None,
        }
    }

    /// The value as a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(v) => Some(v),
            _ => None,
        }
    }

    /// The value as text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(v) => Some(v),
            _ => None,
        }
    }
}

/// How one float is stored.
#[derive(Debug, Clone, PartialEq)]
pub enum FloatDecoder {
    /// The raw 32 bits of an IEEE float.
    NoScale,
    /// A "bit coord" (see [`BitReader::coord`]).
    Coord,
    /// Simulation time: a varint number of ticks, times 1/64 s.
    SimulationTime,
    /// "Rune time": 4 bits reinterpreted as float bits (a Dota leftover, kept for completeness).
    RuneTime,
    /// A fixed-point value in a known range (see [`QuantizedFloat`]).
    Quantized(QuantizedFloat),
}

impl FloatDecoder {
    /// Reads one float.
    pub fn read(&self, r: &mut BitReader<'_>) -> BitsResult<f32> {
        match self {
            Self::NoScale => r.f32("noscale float"),
            Self::Coord => r.coord("coord float"),
            Self::SimulationTime => Ok(r.varint_u32("simulation time")? as f32 * (1.0 / 64.0)),
            Self::RuneTime => Ok(f32::from_bits(r.bits(4, "rune time")?)),
            Self::Quantized(q) => q.read(r),
        }
    }
}

/// How a field value is stored in the bit stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoder {
    /// One bit.
    Bool,
    /// Zigzag varint.
    Signed,
    /// Varint.
    Unsigned,
    /// 8 raw bytes, little-endian (`uint64` fields with the `fixed64` encoder, e.g. Steam IDs).
    Fixed64,
    /// NUL-terminated string.
    String,
    /// Varint length + that many bytes.
    BinaryBlock,
    /// Ammo count: varint minus one (so "no clip" decodes as -1).
    Ammo,
    /// One float.
    Float(FloatDecoder),
    /// `n` floats, each with the same float decoder (`Vector`, `Vector2D`, `Vector4D`, `Quaternion`, `CTransform`).
    Floats {
        /// Number of components.
        n: u8,
        /// Decoder of each component.
        element: FloatDecoder,
    },
    /// A unit vector stored as two 11-bit components and a sign (see [`BitReader::normal_vector`]).
    NormalVector,
    /// `QAngle` with the `qangle_precise` encoder: 3 presence bits, then 20-bit angles in -180..180.
    QAnglePrecise,
    /// `QAngle` with a bit count of 32 or more: three raw floats.
    QAngleNoScale,
    /// `QAngle` with a bit count below 32: three `n`-bit angles in 0..360.
    QAngleBits(u32),
    /// `QAngle` without bit count: 3 presence bits, then a bit coord for each present component.
    QAngleCoord,
    /// The "is present" flag of a pointer field: one bit.
    PointerFlag,
    /// The flag of a polymorphic pointer field: one bit, then (if present) a ubitvar choosing the serializer.
    PolymorphicPointer,
}

/// The parts of a field description that decide its decoder.
#[derive(Debug, Clone, Default)]
pub struct FieldEncoding<'a> {
    /// C++ type without template arguments, pointer or array suffix, e.g. `float32`, `Vector`, `CHandle`.
    pub base_type: &'a str,
    /// Field name, e.g. `m_iClip1` (a few fields are special-cased by name).
    pub var_name: &'a str,
    /// Encoder name (`""` if none), e.g. `coord`, `normal`, `qangle_precise`, `fixed64`.
    pub encoder: &'a str,
    /// Bit count, if given.
    pub bit_count: Option<i32>,
    /// Lowest value of a quantized range, if given.
    pub low_value: Option<f32>,
    /// Highest value of a quantized range, if given.
    pub high_value: Option<f32>,
    /// Quantization flags, if given.
    pub encode_flags: Option<i32>,
}

impl FieldEncoding<'_> {
    /// True when the bit count asks for a full 32-bit float (missing, ≤ 0 or ≥ 32).
    fn is_noscale(&self) -> bool {
        self.bit_count.is_none_or(|b| b <= 0 || b >= 32)
    }

    /// The decoder for a single float of this field.
    fn float(&self) -> FloatDecoder {
        // Valve's engine overrides these two fields to "simtime" regardless of what the serializer says.
        let encoder = match self.var_name {
            "m_flSimulationTime" | "m_flAnimTime" => "simtime",
            _ => self.encoder,
        };
        match encoder {
            "coord" => FloatDecoder::Coord,
            "simtime" => FloatDecoder::SimulationTime,
            "runetime" => FloatDecoder::RuneTime,
            _ => self.quantized(),
        }
    }

    /// A quantized float, or a raw float if the bit count says so.
    fn quantized(&self) -> FloatDecoder {
        if self.is_noscale() {
            FloatDecoder::NoScale
        } else {
            FloatDecoder::Quantized(QuantizedFloat::new(
                self.bit_count.unwrap_or(0) as u32,
                self.encode_flags.unwrap_or(0) as u32,
                self.low_value.unwrap_or(0.0),
                self.high_value.unwrap_or(1.0),
            ))
        }
    }
}

impl Decoder {
    /// Chooses the decoder for a field, in this order:
    /// 1. types whose decoder depends on encoder / bit count (`float32`, vectors, `QAngle`, `uint64`, …);
    /// 2. fields special-cased by name (`m_iClip1`);
    /// 3. types with a fixed decoder (`bool`, signed integers, strings, …);
    /// 4. everything else is an unsigned varint (unsigned integers, enums, handles, colors, tokens…).
    pub fn for_field(f: &FieldEncoding<'_>) -> Self {
        match f.base_type {
            "float32" => return Self::Float(f.float()),
            "CNetworkedQuantizedFloat" => return Self::Float(f.quantized()),
            "uint64" | "CStrongHandle" => {
                return if f.encoder == "fixed64" {
                    Self::Fixed64
                } else {
                    Self::Unsigned
                };
            }
            "Vector" | "VectorWS" if f.encoder == "normal" => return Self::NormalVector,
            "Vector" | "VectorWS" => return Self::floats(3, f),
            "Vector2D" => return Self::floats(2, f),
            "Vector4D" | "Quaternion" => return Self::floats(4, f),
            "CTransform" => return Self::floats(6, f),
            "QAngle" => {
                return match f.bit_count {
                    _ if f.encoder == "qangle_precise" => Self::QAnglePrecise,
                    Some(b) if b >= 32 => Self::QAngleNoScale,
                    Some(b) if b > 0 => Self::QAngleBits(b as u32),
                    _ => Self::QAngleCoord,
                };
            }
            _ => {}
        }
        if f.var_name == "m_iClip1" {
            return Self::Ammo;
        }
        Self::for_base_type(f.base_type)
    }

    /// Decoder for types whose encoding does not depend on encoder or bit count.
    fn for_base_type(base_type: &str) -> Self {
        match base_type {
            "bool" => Self::Bool,
            "int8"
            | "int16"
            | "int32"
            | "int64"
            | "HSequence"
            | "CEntityIndex"
            | "NPC_STATE"
            | "StanceType_t"
            | "RagdollBlendDirection"
            | "BeamType_t"
            | "EntityDisolveType_t"
            | "PrecipitationType_t"
            | "AmmoIndex_t"
            | "TakeDamageFlags_t" => Self::Signed,
            "char" | "CUtlString" | "CUtlSymbolLarge" | "CGlobalSymbol" => Self::String,
            "CUtlBinaryBlock" => Self::BinaryBlock,
            "GameTime_t" => Self::Float(FloatDecoder::NoScale),
            "CBodyComponent" | "CPhysicsComponent" | "CLightComponent" | "CRenderComponent" => {
                Self::Bool
            }
            _ => Self::Unsigned,
        }
    }

    /// Decoder for the elements of a variable-length array (`CUtlVector< T >`): like [`Decoder::for_field`] for
    /// `T`, but without the name-based special cases.
    pub fn for_element(f: &FieldEncoding<'_>) -> Self {
        let element = FieldEncoding {
            var_name: "",
            ..f.clone()
        };
        Self::for_field(&element)
    }

    fn floats(n: u8, f: &FieldEncoding<'_>) -> Self {
        Self::Floats {
            n,
            element: f.float(),
        }
    }

    /// Reads one value.
    pub fn read(&self, r: &mut BitReader<'_>) -> BitsResult<FieldValue> {
        Ok(match self {
            Self::Bool => FieldValue::Bool(r.bool("bool field")?),
            Self::Signed => FieldValue::Signed(r.varint_i64("signed field")?),
            Self::Unsigned => FieldValue::Unsigned(r.varint_u64("unsigned field")?),
            Self::Fixed64 => FieldValue::Unsigned(r.bits_u64(64, "fixed64 field")?),
            Self::String => FieldValue::String(r.string("string field")?.into()),
            Self::BinaryBlock => {
                let len = r.varint_u32("binary block length")? as usize;
                FieldValue::Bytes(r.bytes(len, "binary block")?.into())
            }
            Self::Ammo => FieldValue::Signed(i64::from(r.varint_u32("ammo field")?) - 1),
            Self::Float(d) => FieldValue::Float(d.read(r)?),
            Self::Floats { n: 3, element } => {
                FieldValue::Vector3([element.read(r)?, element.read(r)?, element.read(r)?])
            }
            Self::Floats { n, element } => {
                let values = (0..*n)
                    .map(|_| element.read(r))
                    .collect::<BitsResult<Vec<_>>>()?;
                FieldValue::Floats(values.into())
            }
            Self::NormalVector => FieldValue::Vector3(r.normal_vector("normal vector field")?),
            Self::QAnglePrecise => {
                let present = [
                    r.bool("qangle x")?,
                    r.bool("qangle y")?,
                    r.bool("qangle z")?,
                ];
                let mut v = [0.0; 3];
                for (value, present) in v.iter_mut().zip(present) {
                    if present {
                        *value = r.angle(20, "precise angle")? - 180.0;
                    }
                }
                FieldValue::Vector3(v)
            }
            Self::QAngleNoScale => {
                FieldValue::Vector3([r.f32("qangle")?, r.f32("qangle")?, r.f32("qangle")?])
            }
            Self::QAngleBits(n) => FieldValue::Vector3([
                r.angle(*n, "qangle")?,
                r.angle(*n, "qangle")?,
                r.angle(*n, "qangle")?,
            ]),
            Self::QAngleCoord => {
                let present = [
                    r.bool("qangle x")?,
                    r.bool("qangle y")?,
                    r.bool("qangle z")?,
                ];
                let mut v = [0.0; 3];
                for (value, present) in v.iter_mut().zip(present) {
                    if present {
                        *value = r.coord("qangle coord")?;
                    }
                }
                FieldValue::Vector3(v)
            }
            Self::PointerFlag => FieldValue::Pointer(r.bool("pointer flag")?.then_some(0)),
            Self::PolymorphicPointer => {
                if r.bool("pointer flag")? {
                    FieldValue::Pointer(Some(r.ubitvar("polymorphic type")?))
                } else {
                    FieldValue::Pointer(None)
                }
            }
        })
    }
}

/// Quantization flag: values at or below `low` are sent as a single "is low" bit.
const ROUND_DOWN: u32 = 1 << 0;
/// Quantization flag: values at or above `high` are sent as a single "is high" bit.
const ROUND_UP: u32 = 1 << 1;
/// Quantization flag: exact zero is sent as a single "is zero" bit.
const ENCODE_ZERO: u32 = 1 << 2;
/// Quantization flag: integers are represented exactly (the bit count may grow to make that possible).
const ENCODE_INTEGERS: u32 = 1 << 3;

/// A float stored as an `n`-bit fraction of a range `low..high` — Valve's `CNetworkedQuantizedFloat`.
///
/// The decoder must mirror the encoder's setup exactly, because the setup decides which optional flag bits
/// precede the value: the range is narrowed for round-up/round-down, widened for exact integers, and flags that turn
/// out to be redundant are dropped. One extra or missing flag bit would shift every later value in the packet.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedFloat {
    low: f32,
    high: f32,
    high_low_mul: f32,
    dec_mul: f32,
    bit_count: u32,
    flags: u32,
}

impl QuantizedFloat {
    /// Sets up the decoder exactly like Valve's encoder does. Real fields have 1..=31 bits (32 and more means
    /// "plain float" and never gets here); other values are clamped so a corrupt demo cannot cause a panic.
    pub fn new(bit_count: u32, flags: u32, low: f32, high: f32) -> Self {
        let mut q = Self {
            low,
            high,
            high_low_mul: 0.0,
            dec_mul: 0.0,
            bit_count: bit_count.clamp(1, 32),
            flags,
        };
        q.validate_flags();

        let mut steps = 1u64 << q.bit_count;
        if q.flags & ROUND_DOWN != 0 {
            q.high -= (q.high - q.low) / steps as f32;
        } else if q.flags & ROUND_UP != 0 {
            q.low += (q.high - q.low) / steps as f32;
        }

        if q.flags & ENCODE_INTEGERS != 0 {
            let delta = (q.high - q.low).max(1.0);
            // A power of two ≥ the range; capped at 2^31 so the bit count stays ≤ 32 even for absurd ranges.
            let range = 1u64 << (f64::from(delta).log2().ceil().min(31.0) as u32);
            let mut bits = q.bit_count;
            while (1u64 << bits) <= range {
                bits += 1;
            }
            if bits > q.bit_count {
                q.bit_count = bits;
                steps = 1 << bits;
            }
            let offset = range as f32 / steps as f32;
            q.high = q.low + range as f32 - offset;
        }

        q.assign_multipliers(steps);

        // Drop flags whose special case the plain encoding already hits exactly; the encoder does the same, so it
        // never writes their flag bit.
        if q.flags & ROUND_DOWN != 0 && q.quantize(q.low) == q.low {
            q.flags &= !ROUND_DOWN;
        }
        if q.flags & ROUND_UP != 0 && q.quantize(q.high) == q.high {
            q.flags &= !ROUND_UP;
        }
        if q.flags & ENCODE_ZERO != 0 && q.quantize(0.0) == 0.0 {
            q.flags &= !ENCODE_ZERO;
        }
        q
    }

    /// Removes contradictory or useless flags (Valve's `ValidateFlags`).
    fn validate_flags(&mut self) {
        if self.flags == 0 {
            return;
        }
        if (self.low == 0.0 && self.flags & ROUND_DOWN != 0)
            || (self.high == 0.0 && self.flags & ROUND_UP != 0)
        {
            self.flags &= !ENCODE_ZERO;
        }
        if self.low == 0.0 && self.flags & ENCODE_ZERO != 0 {
            self.flags |= ROUND_DOWN;
            self.flags &= !ENCODE_ZERO;
        }
        if self.high == 0.0 && self.flags & ENCODE_ZERO != 0 {
            self.flags |= ROUND_UP;
            self.flags &= !ENCODE_ZERO;
        }
        if self.low > 0.0 || self.high < 0.0 {
            self.flags &= !ENCODE_ZERO;
        }
        if self.flags & ENCODE_INTEGERS != 0 {
            self.flags &= !(ROUND_UP | ROUND_DOWN | ENCODE_ZERO);
        }
    }

    /// Computes the encode multiplier (float → integer) and decode multiplier (integer → float).
    fn assign_multipliers(&mut self, steps: u64) {
        let range = self.high - self.low;
        let max_int: u32 = if self.bit_count == 32 {
            0xFFFF_FFFE
        } else {
            ((1u64 << self.bit_count) - 1) as u32
        };
        let overflows =
            |mul: f32| mul * range > max_int as f32 || f64::from(mul * range) > f64::from(max_int);
        let mut mul = if range.abs() <= 0.0 {
            max_int as f32
        } else {
            max_int as f32 / range
        };
        if overflows(mul) {
            // Back off slightly so the top of the range never encodes past the largest integer.
            for factor in [0.9999, 0.99, 0.9, 0.8, 0.7] {
                mul = max_int as f32 / range * factor;
                if !overflows(mul) {
                    break;
                }
            }
        }
        self.high_low_mul = mul;
        self.dec_mul = 1.0 / (steps - 1) as f32;
    }

    /// What the encoder would send for `value` (used only to detect redundant flags). Rounds to the nearest step,
    /// like the engine's encoder; truncating instead can keep a flag the encoder dropped (demoinfocs-golang found a
    /// real case: 10 bits, range 0..102.3, round-up).
    fn quantize(&self, value: f32) -> f32 {
        if value < self.low {
            return self.low;
        }
        if value > self.high {
            return self.high;
        }
        let i = ((value - self.low) * self.high_low_mul + 0.5) as u32;
        self.low + (self.high - self.low) * (i as f32 * self.dec_mul)
    }

    /// Reads one value: optional flag bits first, then `bit_count` bits.
    pub fn read(&self, r: &mut BitReader<'_>) -> BitsResult<f32> {
        if self.flags & ROUND_DOWN != 0 && r.bool("quantized round-down flag")? {
            return Ok(self.low);
        }
        if self.flags & ROUND_UP != 0 && r.bool("quantized round-up flag")? {
            return Ok(self.high);
        }
        if self.flags & ENCODE_ZERO != 0 && r.bool("quantized zero flag")? {
            return Ok(0.0);
        }
        let raw = r.bits(self.bit_count, "quantized float")?;
        Ok(self.low + (self.high - self.low) * (raw as f32 * self.dec_mul))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::bits::BitWriter;

    fn encoding(
        base_type: &'static str,
        encoder: &'static str,
        bit_count: Option<i32>,
    ) -> FieldEncoding<'static> {
        FieldEncoding {
            base_type,
            encoder,
            bit_count,
            ..Default::default()
        }
    }

    #[test]
    fn decoder_choice_follows_type_encoder_and_bits() {
        use Decoder as D;
        assert_eq!(D::for_field(&encoding("bool", "", None)), D::Bool);
        assert_eq!(D::for_field(&encoding("int32", "", None)), D::Signed);
        assert_eq!(D::for_field(&encoding("uint16", "", None)), D::Unsigned);
        assert_eq!(D::for_field(&encoding("CHandle", "", None)), D::Unsigned);
        assert_eq!(
            D::for_field(&encoding("uint64", "fixed64", None)),
            D::Fixed64
        );
        assert_eq!(D::for_field(&encoding("CUtlString", "", None)), D::String);
        assert_eq!(
            D::for_field(&encoding("float32", "", None)),
            D::Float(FloatDecoder::NoScale)
        );
        assert_eq!(
            D::for_field(&encoding("float32", "", Some(32))),
            D::Float(FloatDecoder::NoScale)
        );
        assert_eq!(
            D::for_field(&encoding("float32", "coord", None)),
            D::Float(FloatDecoder::Coord)
        );
        assert!(matches!(
            D::for_field(&encoding("float32", "", Some(10))),
            D::Float(FloatDecoder::Quantized(_))
        ));
        assert_eq!(
            D::for_field(&encoding("Vector", "normal", None)),
            D::NormalVector
        );
        assert_eq!(
            D::for_field(&encoding("Vector", "coord", None)),
            D::Floats {
                n: 3,
                element: FloatDecoder::Coord
            }
        );
        assert_eq!(
            D::for_field(&encoding("QAngle", "qangle_precise", Some(32))),
            D::QAnglePrecise
        );
        assert_eq!(
            D::for_field(&encoding("QAngle", "", Some(32))),
            D::QAngleNoScale
        );
        assert_eq!(
            D::for_field(&encoding("QAngle", "", Some(10))),
            D::QAngleBits(10)
        );
        assert_eq!(D::for_field(&encoding("QAngle", "", None)), D::QAngleCoord);
        let sim = FieldEncoding {
            var_name: "m_flSimulationTime",
            ..encoding("float32", "", None)
        };
        assert_eq!(D::for_field(&sim), D::Float(FloatDecoder::SimulationTime));
    }

    #[test]
    fn quantized_float_plain_range() {
        // 8 bits over 0..255 without flags: raw integer n decodes to n.
        let q = QuantizedFloat::new(8, 0, 0.0, 255.0);
        let mut w = BitWriter::default();
        w.bits(0, 8).bits(128, 8).bits(255, 8);
        let data = w.finish();
        let mut r = BitReader::new(&data);
        assert_eq!(q.read(&mut r).unwrap(), 0.0);
        assert_eq!(q.read(&mut r).unwrap(), 128.0);
        assert_eq!(q.read(&mut r).unwrap(), 255.0);
    }

    #[test]
    fn quantized_float_round_down_flag_bit() {
        // Round-down over 0..1024 in 10 bits: the top is narrowed by one step to 1023, low (0) is exactly reachable
        // by the plain encoding so the round-down flag is dropped again → no flag bit is read.
        let q = QuantizedFloat::new(10, ROUND_DOWN, 0.0, 1024.0);
        assert_eq!(q.flags, 0);
        assert_eq!(q.high, 1023.0);

        // Encode-zero over -1..1: zero is not on the grid of 2 bits (-1, -1/3, 1/3, 1) so the flag stays: "1" = zero.
        let q = QuantizedFloat::new(2, ENCODE_ZERO, -1.0, 1.0);
        assert_eq!(q.flags, ENCODE_ZERO);
        let mut w = BitWriter::default();
        w.bool(true).bool(false).bits(3, 2);
        let data = w.finish();
        let mut r = BitReader::new(&data);
        assert_eq!(q.read(&mut r).unwrap(), 0.0);
        assert_eq!(q.read(&mut r).unwrap(), 1.0);
    }

    #[test]
    fn quantized_float_round_up_regression_from_demoinfocs() {
        // demoinfocs-golang's regression case: 10 bits, 0..102.3, round-up. With nearest rounding the round-up flag
        // is redundant and must be dropped (truncation would keep it and desync the stream).
        let q = QuantizedFloat::new(10, ROUND_UP, 0.0, 102.3);
        assert_eq!(q.flags & ROUND_UP, 0);
    }

    #[test]
    fn precise_qangle_reads_only_present_components() {
        let mut w = BitWriter::default();
        // x present, y absent, z present; 20-bit angles: half a turn → 0°, three quarters → 90°.
        w.bool(true)
            .bool(false)
            .bool(true)
            .bits(1 << 19, 20)
            .bits(3 << 18, 20);
        let data = w.finish();
        let v = Decoder::QAnglePrecise
            .read(&mut BitReader::new(&data))
            .unwrap();
        assert_eq!(v, FieldValue::Vector3([0.0, 0.0, 90.0]));
    }
}
