//! Build script: compiles Valve's demo protobuf definitions (`proto/*.proto`) into Rust types.
//!
//! [protox] parses the `.proto` files in pure Rust (no `protoc` binary to install, which keeps CI and new
//! contributors' machines simple) and [prost_build] turns the result into Rust structs. Valve's files declare no
//! `package`, so everything lands in one file, `OUT_DIR/valve.rs`, which `src/demo/proto.rs` includes.
//! See `proto/README.md` for where the files come from and how to update them.

use std::error::Error;
use std::path::PathBuf;

use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};

/// The files we need. Their imports (`network_connection.proto`, …) are found automatically in `proto/`.
const PROTOS: &[&str] = &[
    "demo.proto",
    "netmessages.proto",
    "networkbasetypes.proto",
    "gameevents.proto",
];

/// Reads Valve's `.proto` files from `proto/`, leaving out the Valve-internal options a standard compiler rejects.
///
/// Valve's files use a few options that are defined in Valve-internal `.proto` files which are not public, so every
/// compiler (protoc too) reports "option not defined":
///
/// - file options like `option additional_includes = "basetypes.h";`
/// - bare field options like `[default = -1, boxed_type = "CEntityIndex", synthetic_default = "-1"]`
///
/// They only steer Valve's C++ code generator and have no effect on the wire format, so we drop them while reading
/// (keeping line numbers intact for error messages) instead of editing the copied files.
struct ValveProtoResolver {
    dir: PathBuf,
}

impl FileResolver for ValveProtoResolver {
    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        let path = self.dir.join(name);
        let source =
            std::fs::read_to_string(&path).map_err(|_| protox::Error::file_not_found(name))?;
        let cleaned = source
            .lines()
            .map(clean_line)
            .collect::<Vec<_>>()
            .join("\n");
        File::from_source(name, &cleaned)
    }
}

/// Field options that standard protobuf defines without parentheses. Anything else written bare is Valve-internal.
const STANDARD_FIELD_OPTIONS: &[&str] = &[
    "default",
    "packed",
    "deprecated",
    "json_name",
    "lazy",
    "jstype",
    "ctype",
];

/// Removes Valve-internal options from one line of a `.proto` file (see [`ValveProtoResolver`]).
fn clean_line(line: &str) -> String {
    if line.trim_start().starts_with("option additional_includes") {
        return String::new();
    }
    // Field options sit in the last `[...]` before the closing `;`, e.g. `optional int32 x = 1 [default = -1];`.
    let (Some(open), Some(close)) = (line.rfind('['), line.rfind(']')) else {
        return line.to_owned();
    };
    if close < open {
        return line.to_owned();
    }
    let kept: Vec<&str> = split_options(&line[open + 1..close])
        .into_iter()
        .map(str::trim)
        .filter(|option| {
            let key = option.split('=').next().unwrap_or("").trim();
            key.starts_with('(') || STANDARD_FIELD_OPTIONS.contains(&key)
        })
        .collect();
    let before = line[..open].trim_end();
    let after = &line[close + 1..];
    if kept.is_empty() {
        format!("{before}{after}")
    } else {
        format!("{before} [{}]{after}", kept.join(", "))
    }
}

/// Splits `a = 1, b = "x, y"` on the commas that are not inside a quoted string.
fn split_options(options: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    for (i, c) in options.char_indices() {
        match c {
            '"' => in_string = !in_string,
            ',' if !in_string => {
                parts.push(&options[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&options[start..]);
    parts
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=proto");

    let mut resolver = ChainFileResolver::new();
    resolver.add(ValveProtoResolver {
        dir: PathBuf::from("proto"),
    });
    // `google/protobuf/descriptor.proto` (imported by Valve's files for custom options) is built into protox.
    resolver.add(GoogleFileResolver::new());

    let mut compiler = protox::Compiler::with_file_resolver(resolver);
    // prost needs every message that a generated type mentions, including those from imported files.
    compiler.include_imports(true);
    compiler.open_files(PROTOS)?;

    prost_build::Config::new()
        // Valve's files have no `package` line; name the output file instead of prost's default `_.rs`.
        .default_package_filename("valve")
        .compile_fds(compiler.file_descriptor_set())?;
    Ok(())
}
