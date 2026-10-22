//! KeyValues3 (KV3): Source 2's structured data format — the "JSON" of CS2.
//!
//! Materials, models, maps, particles and most other compiled resources store their data as KV3. It exists in two
//! encodings that describe the same tree of values:
//!
//! - **binary**, inside compiled resource blocks (`DATA`, `RED2`, …) — read by [`binary::decode`];
//! - **text**, human-readable, used by source files and by OFAST's debug output — written by [`text::to_string`].
//!
//! The data model is [`Value`]: null, bool, integers, floats, strings, binary blobs, arrays and objects (ordered
//! key → value maps). Any value can carry a [`Flag`] such as `resource:` that tells tools how to interpret it
//! (e.g. a string that is a path to another resource).
//!
//! Byte-level details are documented in `docs/formats/kv3.md`. Knowledge comes from ValveResourceFormat and
//! ValveKeyValue (MIT); the code is an independent implementation.

pub mod binary;
pub mod text;

#[cfg(test)]
mod tests;

use std::fmt;

/// A 16-byte KV3 GUID identifying an encoding or a format (stored on disk in .NET `Guid` byte order).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// The "generic" KV3 format, used by most resources.
    pub const FORMAT_GENERIC: Guid = Guid::from_str_const("7412167c-06e9-4698-aff2-e63eb59037e7");
    /// The text encoding.
    pub const ENCODING_TEXT: Guid = Guid::from_str_const("e21c7f3c-8a33-41c5-9977-a76d3a32aa0d");

    /// Parses the canonical `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` form at compile time.
    ///
    /// Panics (at compile time when used in a `const`) on malformed input.
    pub const fn from_str_const(s: &str) -> Guid {
        let s = s.as_bytes();
        assert!(s.len() == 36, "GUID must be 36 characters");
        // Read the 32 hex digits in textual order, skipping dashes.
        let mut text = [0u8; 16];
        let (mut i, mut n) = (0, 0);
        while i < 36 {
            if s[i] == b'-' {
                i += 1;
                continue;
            }
            text[n] = (hex(s[i]) << 4) | hex(s[i + 1]);
            n += 1;
            i += 2;
        }
        // .NET byte order: the first three groups are little-endian.
        let mut bytes = text;
        bytes[0] = text[3];
        bytes[1] = text[2];
        bytes[2] = text[1];
        bytes[3] = text[0];
        bytes[4] = text[5];
        bytes[5] = text[4];
        bytes[6] = text[7];
        bytes[7] = text[6];
        Guid(bytes)
    }
}

/// Value of one hexadecimal digit (compile-time helper for [`Guid::from_str_const`]).
const fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("invalid hex digit in GUID"),
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-",
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9]
        )?;
        b[10..].iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

/// A hint attached to a value telling tools how to interpret it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flag {
    /// The value is a path to another resource (`resource:"materials/foo.vmat"`).
    Resource,
    /// The value is a resource name.
    ResourceName,
    /// The value refers to a Panorama UI file.
    Panorama,
    /// The value is a sound event name.
    SoundEvent,
    /// The value names a subclass.
    SubClass,
    /// The value is an entity name.
    EntityName,
    /// The value is a localization token.
    Localize,
    /// A flag value this reader does not know.
    Unknown(u8),
}

impl Flag {
    /// Name used in the text encoding (`resource`, `soundevent`, …).
    pub fn name(self) -> &'static str {
        match self {
            Flag::Resource => "resource",
            Flag::ResourceName => "resource_name",
            Flag::Panorama => "panorama",
            Flag::SoundEvent => "soundevent",
            Flag::SubClass => "subclass",
            Flag::EntityName => "entity_name",
            Flag::Localize => "localize",
            Flag::Unknown(_) => "unknown_flag",
        }
    }
}

/// An ordered list of key → value pairs (KV3 objects keep their member order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object {
    entries: Vec<(String, Value)>,
}

impl Object {
    /// An empty object.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a member (duplicate keys are kept, as in the file).
    pub fn push(&mut self, key: impl Into<String>, value: Value) {
        self.entries.push((key.into(), value));
    }

    /// First value stored under `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Members in file order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the object has no members.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K: Into<String>> FromIterator<(K, Value)> for Object {
    fn from_iter<T: IntoIterator<Item = (K, Value)>>(iter: T) -> Self {
        Self {
            entries: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

/// One KV3 value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// A signed integer (any width in the file: 8 to 64 bits).
    Int(i64),
    /// An unsigned integer (any width in the file: 8 to 64 bits).
    UInt(u64),
    /// A floating-point number (32 or 64 bits in the file).
    Float(f64),
    /// A UTF-8 string.
    String(String),
    /// Raw bytes.
    Blob(Vec<u8>),
    /// An ordered list of values.
    Array(Vec<Value>),
    /// Key → value members.
    Object(Object),
    /// A value carrying a [`Flag`].
    Flagged(Flag, Box<Value>),
}

impl Value {
    /// The value with any [`Flag`] removed.
    pub fn unflagged(&self) -> &Value {
        match self {
            Value::Flagged(_, inner) => inner.unflagged(),
            other => other,
        }
    }

    /// The flag on this value, if any.
    pub fn flag(&self) -> Option<Flag> {
        match self {
            Value::Flagged(flag, _) => Some(*flag),
            _ => None,
        }
    }

    /// The string, if this is one (flags are looked through).
    pub fn as_str(&self) -> Option<&str> {
        match self.unflagged() {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The object, if this is one.
    pub fn as_object(&self) -> Option<&Object> {
        match self.unflagged() {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The array elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self.unflagged() {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The number as `f64`, if this is any kind of number.
    pub fn as_f64(&self) -> Option<f64> {
        match *self.unflagged() {
            Value::Int(i) => Some(i as f64),
            Value::UInt(u) => Some(u as f64),
            Value::Float(f) => Some(f),
            _ => None,
        }
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match *self.unflagged() {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// The number as `i64`, if this is an integer that fits.
    pub fn as_i64(&self) -> Option<i64> {
        match *self.unflagged() {
            Value::Int(i) => Some(i),
            Value::UInt(u) => i64::try_from(u).ok(),
            _ => None,
        }
    }

    /// Looks up `key` when this is an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.get(key)
    }
}

/// A complete KV3 document: its format GUID and root value.
#[derive(Debug, Clone, PartialEq)]
pub struct Kv3File {
    /// Format identifier (most resources use [`Guid::FORMAT_GENERIC`]).
    pub format: Guid,
    /// The root value, almost always an [`Value::Object`].
    pub root: Value,
}
