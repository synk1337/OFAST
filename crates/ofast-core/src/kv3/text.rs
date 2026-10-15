//! Writer for the KeyValues3 **text** encoding.
//!
//! Used for debug output (`ofast kv3 dump`), for documentation and to diff against other tools' decompiled text.
//! A text *parser* comes later (see `docs/roadmap.md`).
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

use super::{Guid, Kv3File, Value};

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
    if s.contains('\n') && !s.contains("\"\"\"") {
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
