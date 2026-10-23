//! The KeyValues3 **text** encoding: a writer ([`to_string`]) and a reader ([`parse`]).
//!
//! The writer serves debug output (`ofast kv3 dump`), documentation and diffs against other tools' decompiled text.
//! The reader serves the text that compiled resources carry inside them: a model's `m_modelInfo.m_keyValueText`
//! holds its procedural-bone constraints, physics markup and ambient-occlusion capsules
//! (`format:vmdlkeys4`).
//!
//! The syntax: an optional `<!-- kv3 encoding:… format:<name>:version{<guid>} -->` header, then one value. Objects
//! are `{ key = value … }` (keys bare identifiers or quoted, members separated by whitespace, a comma tolerated),
//! arrays `[ value, value, ]` (trailing comma allowed), strings `"…"` with `\"` `\\` `\n` `\t` `\r` escapes or
//! `"""` blocks spanning lines, binary blobs `#[ 0A FF … ]`, numbers (an integer unless it has a `.` or an
//! exponent), `true` / `false` / `null`, and a flag prefix such as `resource:"…"`. `//` and `/* */` comments are
//! skipped.
//!
//! Output looks like:
//!
//! ```text
//! <!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->
//! {
//!     m_shaderName = "csgo_environment.vfx"
//!     m_textureParams =
//!     [
//!         {
//!             m_name = "g_tColor"
//!             m_pValue = resource:"materials/dev/grid_color.vtex"
//!         },
//!     ]
//! }
//! ```

use std::fmt::Write;

use super::binary::{Kv3Error, MAX_DEPTH};
use super::{Flag, Guid, Kv3File, Object, Value};

/// Indentation used per nesting level.
const INDENT: &str = "\t";

/// Renders a KV3 document as text, including the `<!-- kv3 … -->` header.
pub fn to_string(file: &Kv3File) -> String {
    let format_name = if file.format == Guid::FORMAT_GENERIC {
        "generic"
    } else {
        "unknown"
    };
    let mut out = format!(
        "<!-- kv3 encoding:text:version{{{}}} format:{}:version{{{}}} -->\n",
        Guid::ENCODING_TEXT,
        format_name,
        file.format
    );
    write_value(&mut out, &file.root, 0);
    out.push('\n');
    out
}

/// Renders a single value (no header). Handy for logging one field.
pub fn value_to_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

/// `true` for values that print on their own lines (objects and arrays).
fn is_block(value: &Value) -> bool {
    matches!(value.unflagged(), Value::Object(_) | Value::Array(_))
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::UInt(u) => {
            let _ = write!(out, "{u}");
        }
        Value::Float(f) => write_float(out, *f),
        Value::String(s) => write_string(out, s),
        Value::Blob(bytes) => {
            out.push_str("#[");
            for byte in bytes {
                let _ = write!(out, " {byte:02X}");
            }
            out.push_str(" ]");
        }
        Value::Flagged(flag, inner) => {
            out.push_str(flag.name());
            out.push(':');
            write_value(out, inner, depth);
        }
        Value::Array(items) => {
            out.push('[');
            out.push('\n');
            for item in items {
                indent(out, depth + 1);
                write_value(out, item, depth + 1);
                out.push_str(",\n");
            }
            indent(out, depth);
            out.push(']');
        }
        Value::Object(object) => {
            out.push('{');
            out.push('\n');
            for (key, member) in object.iter() {
                indent(out, depth + 1);
                write_key(out, key);
                if is_block(member) {
                    // Blocks open on the next line, aligned with the key, like Valve's tools write them.
                    out.push_str(" =\n");
                    indent(out, depth + 1);
                } else {
                    out.push_str(" = ");
                }
                write_value(out, member, depth + 1);
                out.push('\n');
            }
            indent(out, depth);
            out.push('}');
        }
    }
}

/// Keys that are plain identifiers stay unquoted; anything else is quoted.
fn write_key(out: &mut String, key: &str) {
    let mut chars = key.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if identifier {
        out.push_str(key);
    } else {
        // Keys are always single-line quoted strings (newlines escaped), never `"""` blocks.
        write_quoted(out, key);
    }
}

fn write_string(out: &mut String, s: &str) {
    // A `\r` stays escaped on one line: the reader takes a `\r\n` before the closing `"""` as the line break.
    if s.contains('\n') && !s.contains("\"\"\"") && !s.contains('\r') {
        // Multi-line strings use triple quotes, with the content on its own lines.
        out.push_str("\"\"\"\n");
        out.push_str(s);
        out.push_str("\n\"\"\"");
        return;
    }
    write_quoted(out, s);
}

/// Writes `s` as a single-line `"…"` string with escapes.
fn write_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_float(out: &mut String, f: f64) {
    if f.is_nan() {
        out.push_str("nan");
    } else if f.is_infinite() {
        out.push_str(if f > 0.0 { "inf" } else { "-inf" });
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        // Keep a decimal point so the value reads back as a float, not an integer.
        let _ = write!(out, "{f:.1}");
    } else if f.fract() == 0.0 {
        // Huge whole numbers: scientific notation keeps them floats (`1e20`, not `100000000000000000000`).
        let _ = write!(out, "{f:e}");
    } else {
        let _ = write!(out, "{f}");
    }
}

/// Reads a KV3 text document (see the module docs for the syntax). Without a header the format is
/// [`Guid::FORMAT_GENERIC`].
pub fn parse(text: &str) -> Result<Kv3File, Kv3Error> {
    let mut parser = Parser {
        text: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let format = parser.header()?;
    let root = parser.value()?;
    parser.skip_blank()?;
    if parser.pos < parser.text.len() {
        return Err(parser.error("text after the root value"));
    }
    Ok(Kv3File { format, root })
}

/// Reading position in a KV3 text.
struct Parser<'a> {
    text: &'a [u8],
    pos: usize,
    /// Objects and arrays open around the position (bounded by [`MAX_DEPTH`]).
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> Kv3Error {
        let line = 1 + self.text[..self.pos.min(self.text.len())]
            .iter()
            .filter(|&&c| c == b'\n')
            .count();
        Kv3Error::Text {
            line,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.text[self.pos..].starts_with(s.as_bytes())
    }

    /// Skips whitespace and comments.
    fn skip_blank(&mut self) -> Result<(), Kv3Error> {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.pos += 1;
            }
            if self.starts_with("//") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.pos += 1;
                }
            } else if self.starts_with("/*") {
                let end = find(&self.text[self.pos + 2..], b"*/")
                    .ok_or_else(|| self.error("unterminated /* comment"))?;
                self.pos += 2 + end + 2;
            } else {
                return Ok(());
            }
        }
    }

    /// The `<!-- kv3 … -->` header's format GUID, or the generic format when there is no header.
    fn header(&mut self) -> Result<Guid, Kv3Error> {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
        if !self.starts_with("<!--") {
            return Ok(Guid::FORMAT_GENERIC);
        }
        let end = find(&self.text[self.pos..], b"-->")
            .ok_or_else(|| self.error("unterminated header"))?;
        let header = String::from_utf8_lossy(&self.text[self.pos..self.pos + end]).into_owned();
        self.pos += end + 3;
        // `format:<name>:version{<guid>}`; an unreadable GUID falls back to the generic format.
        let format = header
            .split_whitespace()
            .find(|part| part.starts_with("format:"))
            .and_then(|part| part.split_once("version{"))
            .and_then(|(_, rest)| rest.split_once('}'))
            .and_then(|(guid, _)| Guid::parse(guid));
        Ok(format.unwrap_or(Guid::FORMAT_GENERIC))
    }

    fn value(&mut self) -> Result<Value, Kv3Error> {
        self.skip_blank()?;
        match self.peek() {
            None => Err(self.error("a value was expected, the text ended")),
            Some(b'{') => self.nested(Self::object),
            Some(b'[') => self.nested(Self::array),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'#') => self.blob(),
            Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let word = self.word();
                // `resource:"…"`: a flag on the value that follows. Counted as a level of nesting, so that a
                // chain of flags (`a:a:a:…`) cannot recurse without bound.
                if self.peek() == Some(b':') {
                    self.pos += 1;
                    let flag = flag_named(&word);
                    self.depth += 1;
                    if self.depth > MAX_DEPTH {
                        return Err(Kv3Error::TooDeep);
                    }
                    let inner = self.value();
                    self.depth -= 1;
                    return Ok(Value::Flagged(flag, Box::new(inner?)));
                }
                match word.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" => Ok(Value::Null),
                    "nan" => Ok(Value::Float(f64::NAN)),
                    "inf" => Ok(Value::Float(f64::INFINITY)),
                    other => Err(self.error(format!("unknown word `{other}`"))),
                }
            }
            Some(c) => Err(self.error(format!("unexpected `{}`", c as char))),
        }
    }

    /// Runs `read` one nesting level deeper.
    fn nested(
        &mut self,
        read: fn(&mut Self) -> Result<Value, Kv3Error>,
    ) -> Result<Value, Kv3Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Kv3Error::TooDeep);
        }
        let value = read(self);
        self.depth -= 1;
        value
    }

    fn object(&mut self) -> Result<Value, Kv3Error> {
        self.pos += 1; // `{`
        let mut object = Object::new();
        loop {
            self.skip_blank()?;
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(object));
                }
                Some(b',') => self.pos += 1,
                None => return Err(self.error("unterminated object")),
                Some(_) => {
                    let key = self.key()?;
                    self.skip_blank()?;
                    if self.peek() != Some(b'=') {
                        return Err(self.error(format!("`=` expected after `{key}`")));
                    }
                    self.pos += 1;
                    let value = self.value()?;
                    object.push(key, value);
                }
            }
        }
    }

    fn array(&mut self) -> Result<Value, Kv3Error> {
        self.pos += 1; // `[`
        let mut items = Vec::new();
        loop {
            self.skip_blank()?;
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err(self.error("unterminated array")),
                Some(_) => {
                    items.push(self.value()?);
                    self.skip_blank()?;
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {}
                        _ => return Err(self.error("`,` or `]` expected in an array")),
                    }
                }
            }
        }
    }

    /// An object key: a bare identifier (dots allowed) or a quoted string.
    fn key(&mut self) -> Result<String, Kv3Error> {
        match self.peek() {
            Some(b'"') => self.string(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => Ok(self.word()),
            _ => Err(self.error("a key was expected")),
        }
    }

    /// Letters, digits, `_` and `.` from the position.
    fn word(&mut self) -> String {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
        {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.text[start..self.pos]).into_owned()
    }

    fn string(&mut self) -> Result<String, Kv3Error> {
        if self.starts_with("\"\"\"") {
            // `"""` block: the text between the newline after the opening and the one before the closing.
            self.pos += 3;
            let end = find(&self.text[self.pos..], b"\"\"\"")
                .ok_or_else(|| self.error("unterminated \"\"\" string"))?;
            let mut body = &self.text[self.pos..self.pos + end];
            self.pos += end + 3;
            for opening in [&b"\r\n"[..], b"\n"] {
                if let Some(rest) = body.strip_prefix(opening) {
                    body = rest;
                    break;
                }
            }
            for closing in [&b"\r\n"[..], b"\n"] {
                if let Some(rest) = body.strip_suffix(closing) {
                    body = rest;
                    break;
                }
            }
            return Ok(String::from_utf8_lossy(body).into_owned());
        }
        self.pos += 1; // `"`
        let mut bytes = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(String::from_utf8_lossy(&bytes).into_owned());
                }
                Some(b'\\') => {
                    let escaped = self.text.get(self.pos + 1).copied();
                    bytes.push(match escaped {
                        Some(b'n') => b'\n',
                        Some(b't') => b'\t',
                        Some(b'r') => b'\r',
                        Some(c @ (b'"' | b'\\')) => c,
                        // An unknown escape is kept as written.
                        _ => {
                            self.pos += 1;
                            bytes.push(b'\\');
                            continue;
                        }
                    });
                    self.pos += 2;
                }
                Some(c) => {
                    bytes.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// `#[ 0A FF … ]`.
    fn blob(&mut self) -> Result<Value, Kv3Error> {
        if !self.starts_with("#[") {
            return Err(self.error("`#[` expected"));
        }
        self.pos += 2;
        let mut bytes = Vec::new();
        loop {
            self.skip_blank()?;
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Blob(bytes));
                }
                None => return Err(self.error("unterminated blob")),
                Some(_) => {
                    let digits = self.text.get(self.pos..self.pos + 2).unwrap_or_default();
                    // Hex digits only (`from_str_radix` alone would take a sign: `+F`).
                    let byte = std::str::from_utf8(digits)
                        .ok()
                        .filter(|d| d.len() == 2 && d.bytes().all(|c| c.is_ascii_hexdigit()))
                        .and_then(|d| u8::from_str_radix(d, 16).ok())
                        .ok_or_else(|| self.error("two hex digits expected in a blob"))?;
                    bytes.push(byte);
                    self.pos += 2;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, Kv3Error> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'+')
        {
            self.pos += 1;
        }
        let token = std::str::from_utf8(&self.text[start..self.pos]).unwrap_or_default();
        let bad = || self.error(format!("`{token}` is not a number"));
        match token {
            "-inf" => return Ok(Value::Float(f64::NEG_INFINITY)),
            "+inf" => return Ok(Value::Float(f64::INFINITY)),
            "-nan" | "+nan" => return Ok(Value::Float(f64::NAN)),
            _ => {}
        }
        if token.contains(['.', 'e', 'E']) {
            return token.parse().map(Value::Float).map_err(|_| bad());
        }
        match token.parse::<i64>() {
            Ok(i) => Ok(Value::Int(i)),
            // Past i64: an unsigned 64-bit value.
            Err(_) => token
                .trim_start_matches('+')
                .parse::<u64>()
                .map(Value::UInt)
                .map_err(|_| bad()),
        }
    }
}

/// The flag a text prefix names ([`Flag::name`] in reverse); an unknown name keeps the value, flagged unknown.
fn flag_named(name: &str) -> Flag {
    match name {
        "resource" => Flag::Resource,
        "resource_name" => Flag::ResourceName,
        "panorama" => Flag::Panorama,
        "soundevent" => Flag::SoundEvent,
        "subclass" => Flag::SubClass,
        "entity_name" => Flag::EntityName,
        "localize" => Flag::Localize,
        _ => Flag::Unknown(0),
    }
}

/// Offset of the first `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
