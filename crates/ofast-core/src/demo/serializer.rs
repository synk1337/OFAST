//! Serializers ("send tables"): the description of every networked entity class and its fields.
//!
//! The `DEM_SendTables` frame holds one `CSVCMsg_FlattenedSerializer` message. It lists **serializers** (one per
//! networked class or nested struct, e.g. `CCSPlayerPawn`, `CBodyComponentBaseAnimGraph`) and **fields**. Each field
//! has a C++ type (`var_type`, e.g. `CNetworkedQuantizedFloat`, `CHandle< CBaseEntity >`, `float32[3]`), a name, and
//! encoding hints (encoder name, bit count, range, flags). All strings are indices into a shared symbol list, and
//! fields are shared: a serializer lists the indices of its fields in one global field array.
//!
//! From the type we derive the field's *kind* ([`FieldKind`]): a simple value, a fixed array, a pointer to a nested
//! serializer, a variable-length array, or a variable-length array of nested structs. Together with the decoder
//! chosen in `decoder.rs`, that tells the entity reader how to walk a [`FieldPath`] and decode its value.
//!
//! Knowledge source: demoinfocs-golang `sendtables/parser.go`, `field.go`, `field_type.go`, `serializer.go`,
//! demoparser2 `first_pass/sendtables.rs` (both MIT). See `docs/formats/demo.md`.

use std::collections::HashMap;

use super::decoder::{Decoder, FieldEncoding};
use super::fieldpath::{FieldPath, MAX_DEPTH};
use super::proto::{CsvcMsgFlattenedSerializer, ProtoFlattenedSerializerFieldT};

/// Index of a serializer in [`Serializers::serializers`].
pub type SerializerId = usize;
/// Index of a field in [`Serializers::fields`].
pub type FieldId = usize;

/// Errors while building serializers.
#[derive(Debug, thiserror::Error)]
pub enum SerializerError {
    /// The send tables payload is not a valid protobuf message.
    #[error("send tables: {0}")]
    Protobuf(#[from] prost::DecodeError),
    /// A length, symbol, field or serializer index points outside its data.
    #[error("send tables: {0}")]
    BadReference(String),
}

/// A C++ type name split into its parts, e.g. `CNetworkUtlVectorBase< CHandle< CBaseEntity > >` → base
/// `CNetworkUtlVectorBase`, generic `CHandle< CBaseEntity >`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldType {
    /// Type without template arguments, pointer star or array suffix.
    pub base: String,
    /// The template argument, if any (parsed recursively).
    pub generic: Option<Box<FieldType>>,
    /// The type ends in `*`.
    pub pointer: bool,
    /// Fixed array length from `[N]` (0 = not an array).
    pub count: usize,
}

/// Array lengths given as constant names instead of numbers in Valve's type strings.
const NAMED_ARRAY_LENGTHS: &[(&str, usize)] =
    &[("MAX_ITEM_STOCKS", 8), ("MAX_ABILITY_DRAFT_ABILITIES", 48)];

/// Largest fixed array accepted (CS2's largest is 128); bounds memory use on corrupt files.
pub const MAX_ARRAY_LEN: usize = 1 << 14;

/// Deepest template nesting parsed (real types have at most 3 levels; this bounds recursion on corrupt input).
const MAX_TYPE_DEPTH: usize = 8;

impl FieldType {
    /// Parses a type string like `CHandle< CBaseEntity >[2]`. Never fails: unexpected text ends up in `base`.
    pub fn parse(text: &str) -> Self {
        Self::parse_nested(text, 0)
    }

    fn parse_nested(text: &str, depth: usize) -> Self {
        let base_end = text.find(['<', '[', '*']).unwrap_or(text.len());
        let base = text[..base_end].trim().to_owned();
        let mut rest = &text[base_end..];

        let mut generic = None;
        if rest.starts_with('<')
            && depth < MAX_TYPE_DEPTH
            && let Some(close) = rest.rfind('>')
        {
            generic = Some(Box::new(Self::parse_nested(
                rest[1..close].trim(),
                depth + 1,
            )));
            rest = &rest[close + 1..];
        }
        let pointer = rest.trim_start().starts_with('*');
        let count = match (rest.find('['), rest.rfind(']')) {
            (Some(open), Some(close)) if open < close => {
                let inner = rest[open + 1..close].trim();
                NAMED_ARRAY_LENGTHS
                    .iter()
                    .find(|(name, _)| *name == inner)
                    .map(|&(_, n)| n)
                    .or_else(|| inner.parse().ok().filter(|&n| n > 0))
                    // An unknown constant: fixed arrays send no length on the wire, so the exact value only bounds
                    // which element indices are accepted.
                    .unwrap_or(1024)
                    .min(MAX_ARRAY_LEN)
            }
            _ => 0,
        };
        Self {
            base,
            generic,
            pointer,
            count,
        }
    }
}

/// Types that are pointers to a nested serializer even though their type string has no `*`.
const POINTER_TYPES: &[&str] = &[
    "CBodyComponent",
    "CBodyComponentBaseAnimating",
    "CBodyComponentBaseAnimatingOverlay",
    "CBodyComponentBaseModelEntity",
    "CBodyComponentDCGBaseAnimating",
    "CBodyComponentSkeletonInstance",
    "CBodyComponentPoint",
    "CLightComponent",
    "CRenderComponent",
    "CPhysicsComponent",
    "CPlayerLocalData",
];

/// How a field is laid out in field paths and on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    /// One value. Path ends at the field.
    Simple(Decoder),
    /// `T[N]` (except `char[N]`, which is a string): path has one more index, the element; no length is sent.
    FixedArray {
        /// Decoder of each element.
        decoder: Decoder,
        /// N: element indices must be below it.
        len: usize,
    },
    /// A pointer to a nested struct: the path ending at the field carries a "present" flag; longer paths address
    /// fields of the nested serializer. Polymorphic pointers can switch to one of several serializers.
    Pointer {
        /// The field's own serializer.
        serializer: SerializerId,
        /// Alternatives for polymorphic pointers: index 0 = `serializer`, 1.. = the alternatives. Empty for plain
        /// pointers.
        polymorphic: Vec<SerializerId>,
    },
    /// `CUtlVector< T >` / `CNetworkUtlVectorBase< T >`: the path ending at the field carries the new length;
    /// one more index addresses an element.
    VariableArray(Decoder),
    /// `CUtlVectorEmbeddedNetworkVar< T >` and similar vectors of structs: the path ending at the field carries the
    /// new length; then an element index, then a field index inside the element's serializer.
    VariableTable(SerializerId),
}

/// One networked field.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Name used for lookups: `var_name`, or `send_node.var_name` when the field sits under a send node (Valve
    /// flattens some nested structs into their parent, and two of them can contain the same member name).
    pub name: String,
    /// C++ member name, e.g. `m_vecX`.
    pub var_name: String,
    /// C++ type as written by Valve, e.g. `CNetworkedQuantizedFloat`.
    pub var_type: String,
    /// The flattened struct the field came from (empty for top-level fields).
    pub send_node: String,
    /// Encoder name, e.g. `coord`, `normal`, `qangle_precise` (empty if none).
    pub encoder: String,
    /// Bit count hint.
    pub bit_count: Option<i32>,
    /// Low end of the quantized range.
    pub low_value: Option<f32>,
    /// High end of the quantized range.
    pub high_value: Option<f32>,
    /// Quantization flags.
    pub encode_flags: Option<i32>,
    /// Layout and decoder.
    pub kind: FieldKind,
}

/// One serializer: a named, versioned list of fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Serializer {
    /// Class or struct name, e.g. `CCSPlayerPawn`.
    pub name: String,
    /// Version (several versions of one name can exist; lookups by name use the one declared last, which in
    /// practice is the newest).
    pub version: i32,
    /// The fields, in field-path index order.
    pub fields: Vec<FieldId>,
}

/// What a field path points at, as returned by [`Serializers::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved<'a> {
    /// The field the path ends in (for array elements: the array field).
    pub field: FieldId,
    /// How to read the value.
    pub decoder: &'a Decoder,
    /// The value is the new length of a variable-length array.
    pub is_length: bool,
}

/// Reasons a field path cannot be resolved. All mean the bit stream and the serializers disagree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// An index points past the end of a serializer's field list.
    #[error("field path {path:?}: index {index} out of range for serializer {serializer}")]
    OutOfRange {
        /// The full path.
        path: Vec<i32>,
        /// The bad index.
        index: i32,
        /// Name of the serializer being walked.
        serializer: String,
    },
    /// A path goes into a pointer whose polymorphic type is unknown or invalid.
    #[error("field path {path:?}: polymorphic pointer has no valid type")]
    NoPolymorphicType {
        /// The full path.
        path: Vec<i32>,
    },
    /// A path continues below a simple value (or stops above one).
    #[error("field path {path:?}: does not match the shape of the field it reaches")]
    BadShape {
        /// The full path.
        path: Vec<i32>,
    },
    /// An array element index is not below the array's (fixed or current) length.
    #[error("field path {path:?}: element {index} of an array with {len} elements")]
    ElementOutOfRange {
        /// The full path.
        path: Vec<i32>,
        /// The element index.
        index: i32,
        /// The array length.
        len: u64,
    },
    /// The entity refers to a serializer that does not exist.
    #[error("serializer id {0} does not exist")]
    UnknownSerializer(SerializerId),
}

/// Decoder of the "present" flag of plain pointers (a static so [`Resolved`] can borrow it).
static POINTER_FLAG: Decoder = Decoder::PointerFlag;
/// Decoder of the flag + type of polymorphic pointers.
static POLYMORPHIC_POINTER: Decoder = Decoder::PolymorphicPointer;
/// Decoder of variable-length array lengths.
static LENGTH: Decoder = Decoder::Unsigned;

/// All serializers and fields of a demo.
#[derive(Debug, Clone)]
pub struct Serializers {
    /// Every serializer, in the order they were declared.
    pub serializers: Vec<Serializer>,
    /// Every field (shared between serializers).
    pub fields: Vec<Field>,
    by_name: HashMap<String, SerializerId>,
}

impl Serializers {
    /// Builds the serializers from the `data` of a `CDemoSendTables` frame (a varint length followed by a
    /// `CSVCMsg_FlattenedSerializer` message).
    pub fn from_send_tables(data: &[u8]) -> Result<Self, SerializerError> {
        let mut r = crate::bytes::ByteReader::new(data);
        let bad = |e: crate::bytes::UnexpectedEof| SerializerError::BadReference(e.to_string());
        let len = r.varint_u32("send tables length").map_err(bad)?;
        let body = r.take(len as usize, "send tables").map_err(bad)?;
        let msg: CsvcMsgFlattenedSerializer = prost::Message::decode(body)?;
        Self::from_message(&msg)
    }

    /// Builds the serializers from a decoded `CSVCMsg_FlattenedSerializer`.
    pub fn from_message(msg: &CsvcMsgFlattenedSerializer) -> Result<Self, SerializerError> {
        let mut out = Self {
            serializers: Vec::with_capacity(msg.serializers.len()),
            fields: Vec::with_capacity(msg.fields.len()),
            by_name: HashMap::new(),
        };
        // Global field index in the message → our field id (each field is built once, on first use).
        let mut built: HashMap<i32, FieldId> = HashMap::new();

        for s in &msg.serializers {
            let mut serializer = Serializer {
                name: symbol(msg, s.serializer_name_sym)?,
                version: s.serializer_version(),
                fields: Vec::with_capacity(s.fields_index.len()),
            };
            for &index in &s.fields_index {
                let id = match built.get(&index) {
                    Some(&id) => id,
                    None => {
                        let f = msg.fields.get(index as usize).ok_or_else(|| {
                            SerializerError::BadReference(format!("field {index}"))
                        })?;
                        let field = out.build_field(msg, f)?;
                        out.fields.push(field);
                        built.insert(index, out.fields.len() - 1);
                        out.fields.len() - 1
                    }
                };
                serializer.fields.push(id);
            }
            // Fields only refer to serializers declared before them, so registering after the fields is enough.
            out.by_name
                .insert(serializer.name.clone(), out.serializers.len());
            out.serializers.push(serializer);
        }
        Ok(out)
    }

    fn build_field(
        &self,
        msg: &CsvcMsgFlattenedSerializer,
        f: &ProtoFlattenedSerializerFieldT,
    ) -> Result<Field, SerializerError> {
        let var_name = symbol(msg, f.var_name_sym)?;
        let var_type = symbol(msg, f.var_type_sym)?;
        let mut send_node = symbol(msg, f.send_node_sym)?;
        if send_node == "(root)" {
            send_node.clear();
        }
        let encoder = symbol(msg, f.var_encoder_sym)?;
        let serializer_name = symbol(msg, f.field_serializer_name_sym)?;
        let ty = FieldType::parse(&var_type);

        let encoding = FieldEncoding {
            base_type: &ty.base,
            var_name: &var_name,
            encoder: &encoder,
            bit_count: f.bit_count,
            low_value: f.low_value,
            high_value: f.high_value,
            encode_flags: f.encode_flags,
        };

        let lookup = |name: &str| -> Result<SerializerId, SerializerError> {
            self.by_name.get(name).copied().ok_or_else(|| {
                SerializerError::BadReference(format!("serializer {name:?} used before declared"))
            })
        };

        let kind = if !serializer_name.is_empty() || !f.polymorphic_types.is_empty() {
            let serializer = lookup(&serializer_name)?;
            if ty.pointer
                || POINTER_TYPES.contains(&ty.base.as_str())
                || !f.polymorphic_types.is_empty()
            {
                let mut polymorphic = Vec::new();
                if !f.polymorphic_types.is_empty() {
                    polymorphic.push(serializer);
                    for p in &f.polymorphic_types {
                        polymorphic.push(lookup(&symbol(
                            msg,
                            p.polymorphic_field_serializer_name_sym,
                        )?)?);
                    }
                }
                FieldKind::Pointer {
                    serializer,
                    polymorphic,
                }
            } else {
                FieldKind::VariableTable(serializer)
            }
        } else if ty.count > 0 && ty.base != "char" {
            FieldKind::FixedArray {
                decoder: Decoder::for_field(&encoding),
                len: ty.count,
            }
        } else if ty.base == "CUtlVector" || ty.base == "CNetworkUtlVectorBase" {
            let element_type = ty.generic.as_deref().map_or("", |g| g.base.as_str());
            FieldKind::VariableArray(Decoder::for_element(&FieldEncoding {
                base_type: element_type,
                ..encoding.clone()
            }))
        } else {
            FieldKind::Simple(Decoder::for_field(&encoding))
        };

        let name = if send_node.is_empty() {
            var_name.clone()
        } else {
            format!("{send_node}.{var_name}")
        };
        Ok(Field {
            name,
            var_name,
            var_type,
            send_node,
            encoder,
            bit_count: f.bit_count,
            low_value: f.low_value,
            high_value: f.high_value,
            encode_flags: f.encode_flags,
            kind,
        })
    }

    /// The serializer with this name (the one declared last if there are several versions).
    pub fn by_name(&self, name: &str) -> Option<SerializerId> {
        self.by_name.get(name).copied()
    }

    /// Works out which field and decoder a field path of an entity with serializer `root` refers to.
    ///
    /// `stored` returns the number the entity currently stores at a path prefix, where the walk needs it:
    /// - for a polymorphic pointer, which serializer it uses (`FieldValue::Pointer(Some(i))` → `i`);
    /// - for a variable-length array, its current length (`FieldValue::Unsigned(n)` → `n`). Element indices must
    ///   be below it — Valve always sends a new length before elements past the old end.
    pub fn resolve(
        &self,
        root: SerializerId,
        path: &FieldPath,
        stored: impl Fn(&FieldPath) -> Option<u64>,
    ) -> Result<Resolved<'_>, ResolveError> {
        let indices = path.indices();
        let mut serializer = root;
        let mut pos = 0;
        // Checks the element index after the array field at `pos` against the array length.
        let element = |pos: usize, len: u64| -> Result<(), ResolveError> {
            let index = indices[pos + 1];
            if u64::try_from(index).is_ok_and(|i| i < len) {
                Ok(())
            } else {
                Err(ResolveError::ElementOutOfRange {
                    path: indices.to_vec(),
                    index,
                    len,
                })
            }
        };
        let current_len = |pos: usize| stored(&path.prefix(pos + 1)).unwrap_or(0);
        loop {
            let s = self
                .serializers
                .get(serializer)
                .ok_or(ResolveError::UnknownSerializer(serializer))?;
            let index = indices[pos];
            let field_id = usize::try_from(index)
                .ok()
                .and_then(|i| s.fields.get(i))
                .copied()
                .ok_or_else(|| ResolveError::OutOfRange {
                    path: indices.to_vec(),
                    index,
                    serializer: s.name.clone(),
                })?;
            // Number of indices after this field's own index.
            let below = indices.len() - pos - 1;
            let leaf = |decoder, is_length| {
                Ok(Resolved {
                    field: field_id,
                    decoder,
                    is_length,
                })
            };
            match &self.fields[field_id].kind {
                FieldKind::Simple(d) if below == 0 => return leaf(d, false),
                FieldKind::FixedArray { decoder, len } if below == 1 => {
                    element(pos, *len as u64)?;
                    return leaf(decoder, false);
                }
                FieldKind::VariableArray(_) if below == 0 => return leaf(&LENGTH, true),
                FieldKind::VariableArray(d) if below == 1 => {
                    element(pos, current_len(pos))?;
                    return leaf(d, false);
                }
                FieldKind::VariableTable(_) if below == 0 => return leaf(&LENGTH, true),
                FieldKind::VariableTable(inner) if below >= 2 => {
                    // indices[pos + 1] is the element number; the element's fields start at pos + 2.
                    element(pos, current_len(pos))?;
                    serializer = *inner;
                    pos += 2;
                }
                FieldKind::Pointer { polymorphic, .. } if below == 0 => {
                    return leaf(
                        if polymorphic.is_empty() {
                            &POINTER_FLAG
                        } else {
                            &POLYMORPHIC_POINTER
                        },
                        false,
                    );
                }
                FieldKind::Pointer {
                    serializer: own,
                    polymorphic,
                } => {
                    serializer = if polymorphic.is_empty() {
                        *own
                    } else {
                        stored(&path.prefix(pos + 1))
                            .and_then(|i| polymorphic.get(usize::try_from(i).ok()?))
                            .copied()
                            .ok_or_else(|| ResolveError::NoPolymorphicType {
                                path: indices.to_vec(),
                            })?
                    };
                    pos += 1;
                }
                _ => {
                    return Err(ResolveError::BadShape {
                        path: indices.to_vec(),
                    });
                }
            }
        }
    }

    /// Finds the field path of a field by its dotted name below serializer `root`, e.g.
    /// `CBodyComponent.m_cellX` on `CCSPlayerPawn`. Walks plain pointers only (not polymorphic ones or arrays).
    ///
    /// Each name part matches a field's full [`Field::name`] (`send_node.var_name`) or, if exactly one field of the
    /// serializer has it, the bare `var_name`. That way lookups survive Valve moving a member into a different
    /// flattened struct between builds (CS2 currently sends `m_cellX` as `m_skeletonInstance.m_vecOrigin.m_cellX`).
    pub fn path_of(&self, root: SerializerId, dotted: &str) -> Option<FieldPath> {
        let mut indices = Vec::new();
        let mut serializer = root;
        let mut rest = dotted;
        'outer: while !rest.is_empty() {
            let s = self.serializers.get(serializer)?;
            // Field names can contain dots themselves (`send_node.var_name`), so try the longest match first.
            let mut ends: Vec<usize> = rest.match_indices('.').map(|(i, _)| i).collect();
            ends.push(rest.len());
            for &end in ends.iter().rev() {
                let name = &rest[..end];
                let Some(i) = self.field_index(s, name) else {
                    continue;
                };
                indices.push(i as i32);
                rest = rest.get(end + 1..).unwrap_or("");
                if rest.is_empty() {
                    break 'outer;
                }
                match &self.fields[s.fields[i]].kind {
                    FieldKind::Pointer {
                        serializer: inner,
                        polymorphic,
                    } if polymorphic.is_empty() => {
                        serializer = *inner;
                        continue 'outer;
                    }
                    _ => return None,
                }
            }
            return None;
        }
        (!indices.is_empty() && indices.len() <= MAX_DEPTH).then(|| FieldPath::new(&indices))
    }

    /// Position of a field in `s` by full name, or by bare member name if that is unambiguous.
    fn field_index(&self, s: &Serializer, name: &str) -> Option<usize> {
        if let Some(i) = s.fields.iter().position(|&f| self.fields[f].name == name) {
            return Some(i);
        }
        let mut matches = s
            .fields
            .iter()
            .enumerate()
            .filter(|&(_, &f)| self.fields[f].var_name == name);
        match (matches.next(), matches.next()) {
            (Some((i, _)), None) => Some(i),
            _ => None,
        }
    }
}

/// Looks up a symbol; an absent symbol is the empty string.
fn symbol(msg: &CsvcMsgFlattenedSerializer, sym: Option<i32>) -> Result<String, SerializerError> {
    match sym {
        None => Ok(String::new()),
        Some(i) => msg
            .symbols
            .get(i as usize)
            .cloned()
            .ok_or_else(|| SerializerError::BadReference(format!("symbol {i}"))),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Builds `CSVCMsg_FlattenedSerializer` messages for tests.

    use super::super::proto::{
        CsvcMsgFlattenedSerializer, ProtoFlattenedSerializerFieldT, ProtoFlattenedSerializerT,
    };

    /// Field description for [`message`].
    #[derive(Default, Clone)]
    pub(crate) struct F {
        pub(crate) name: &'static str,
        pub(crate) ty: &'static str,
        pub(crate) encoder: Option<&'static str>,
        pub(crate) bits: Option<i32>,
        pub(crate) low: Option<f32>,
        pub(crate) high: Option<f32>,
        pub(crate) flags: Option<i32>,
        pub(crate) serializer: Option<&'static str>,
        pub(crate) send_node: Option<&'static str>,
    }

    /// Builds a message from serializers given as (name, fields), in declaration order.
    pub(crate) fn message(serializers: &[(&'static str, Vec<F>)]) -> CsvcMsgFlattenedSerializer {
        fn sym(msg: &mut CsvcMsgFlattenedSerializer, s: &str) -> i32 {
            if let Some(i) = msg.symbols.iter().position(|x| x == s) {
                return i as i32;
            }
            msg.symbols.push(s.to_owned());
            msg.symbols.len() as i32 - 1
        }
        let mut msg = CsvcMsgFlattenedSerializer::default();
        for (name, fields) in serializers {
            let mut ser = ProtoFlattenedSerializerT {
                serializer_name_sym: Some(sym(&mut msg, name)),
                serializer_version: Some(0),
                fields_index: Vec::new(),
            };
            for f in fields {
                let field = ProtoFlattenedSerializerFieldT {
                    var_type_sym: Some(sym(&mut msg, f.ty)),
                    var_name_sym: Some(sym(&mut msg, f.name)),
                    bit_count: f.bits,
                    low_value: f.low,
                    high_value: f.high,
                    encode_flags: f.flags,
                    field_serializer_name_sym: f.serializer.map(|s| sym(&mut msg, s)),
                    field_serializer_version: f.serializer.map(|_| 0),
                    var_encoder_sym: f.encoder.map(|s| sym(&mut msg, s)),
                    send_node_sym: f.send_node.map(|s| sym(&mut msg, s)),
                    ..Default::default()
                };
                msg.fields.push(field);
                ser.fields_index.push(msg.fields.len() as i32 - 1);
            }
            msg.serializers.push(ser);
        }
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{F, message};
    use super::*;

    #[test]
    fn parses_type_strings() {
        let t = FieldType::parse("CNetworkUtlVectorBase< CHandle< CBaseEntity > >");
        assert_eq!(t.base, "CNetworkUtlVectorBase");
        let g = t.generic.as_deref().unwrap();
        assert_eq!(g.base, "CHandle");
        assert_eq!(g.generic.as_deref().unwrap().base, "CBaseEntity");
        assert_eq!(FieldType::parse("float32[3]").count, 3);
        assert_eq!(FieldType::parse("char[128]").count, 128);
        assert_eq!(
            FieldType::parse("CEconItemAttribute[MAX_ITEM_STOCKS]").count,
            8
        );
        let p = FieldType::parse("CPlayer_WeaponServices*");
        assert!(p.pointer);
        assert_eq!(p.base, "CPlayer_WeaponServices");
        assert_eq!(
            FieldType::parse("uint32"),
            FieldType {
                base: "uint32".into(),
                generic: None,
                pointer: false,
                count: 0
            }
        );
    }

    fn sample() -> Serializers {
        let quantized = F {
            name: "m_vecX",
            ty: "CNetworkedQuantizedFloat",
            bits: Some(15),
            low: Some(0.0),
            high: Some(512.0),
            ..Default::default()
        };
        let simple = |name, ty| F {
            name,
            ty,
            ..Default::default()
        };
        let msg = message(&[
            (
                "CBodyComponentBaseAnimGraph",
                vec![
                    F {
                        send_node: Some("m_skeletonInstance.m_vecOrigin"),
                        ..simple("m_cellX", "uint16")
                    },
                    quantized,
                ],
            ),
            ("CItem", vec![simple("m_iCount", "int32")]),
            (
                "CCSPlayerPawn",
                vec![
                    simple("m_iHealth", "int32"),
                    F {
                        serializer: Some("CBodyComponentBaseAnimGraph"),
                        ..simple("CBodyComponent", "CBodyComponent")
                    },
                    F {
                        encoder: Some("qangle_precise"),
                        bits: Some(32),
                        ..simple("m_angEyeAngles", "QAngle")
                    },
                    simple(
                        "m_hMyWeapons",
                        "CNetworkUtlVectorBase< CHandle< CBasePlayerWeapon > >",
                    ),
                    F {
                        serializer: Some("CItem"),
                        ..simple("m_items", "CUtlVectorEmbeddedNetworkVar< CItem >")
                    },
                    simple("m_flags", "uint8[4]"),
                ],
            ),
        ]);
        Serializers::from_message(&msg).unwrap()
    }

    #[test]
    fn builds_kinds_and_resolves_paths() {
        let s = sample();
        let pawn = s.by_name("CCSPlayerPawn").unwrap();
        let none = |_: &FieldPath| None;
        // Both variable-length arrays ([3] and [4]) currently hold 10 elements.
        let lengths = |p: &FieldPath| matches!(p.indices(), [3] | [4]).then_some(10);
        let resolve = |p: &[i32]| s.resolve(pawn, &FieldPath::new(p), lengths).unwrap();

        assert_eq!(resolve(&[0]).decoder, &Decoder::Signed);
        assert_eq!(resolve(&[1]).decoder, &Decoder::PointerFlag); // CBodyComponent present flag
        assert_eq!(resolve(&[1, 0]).decoder, &Decoder::Unsigned); // m_cellX
        assert!(matches!(resolve(&[1, 1]).decoder, Decoder::Float(_))); // m_vecX
        assert_eq!(resolve(&[2]).decoder, &Decoder::QAnglePrecise);
        assert!(resolve(&[3]).is_length);
        assert_eq!(resolve(&[3, 5]).decoder, &Decoder::Unsigned); // a weapon handle
        assert!(resolve(&[4]).is_length);
        assert_eq!(resolve(&[4, 2, 0]).decoder, &Decoder::Signed); // m_items[2].m_iCount
        assert_eq!(resolve(&[5, 3]).decoder, &Decoder::Unsigned);

        assert!(s.resolve(pawn, &FieldPath::new(&[9]), none).is_err());
        assert!(s.resolve(pawn, &FieldPath::new(&[0, 1]), none).is_err());
        // Element indices must be inside the array: fixed length 4, current length 10, unknown length = empty.
        let out_of_range = |p: &[i32], stored: &dyn Fn(&FieldPath) -> Option<u64>| {
            matches!(
                s.resolve(pawn, &FieldPath::new(p), stored),
                Err(ResolveError::ElementOutOfRange { .. })
            )
        };
        assert!(out_of_range(&[5, 4], &lengths));
        assert!(out_of_range(&[3, 10], &lengths));
        assert!(out_of_range(&[4, 10, 0], &lengths));
        assert!(out_of_range(&[3, 0], &none));

        assert_eq!(
            s.path_of(pawn, "CBodyComponent.m_vecX"),
            Some(FieldPath::new(&[1, 1]))
        );
        // Full name with send node, and the bare member name as an unambiguous fallback.
        let cell = Some(FieldPath::new(&[1, 0]));
        assert_eq!(
            s.path_of(
                pawn,
                "CBodyComponent.m_skeletonInstance.m_vecOrigin.m_cellX"
            ),
            cell
        );
        assert_eq!(s.path_of(pawn, "CBodyComponent.m_cellX"), cell);
        assert_eq!(
            s.path_of(pawn, "m_angEyeAngles"),
            Some(FieldPath::new(&[2]))
        );
        assert_eq!(s.path_of(pawn, "m_nope"), None);
    }
}
