//! Developer tool: look inside a CS2 demo.
//!
//! ```bash
//! # header, frame counts, how far the parser gets
//! cargo run --release --example demo_inspect -- path/to/match.dem
//! # the fields of some serializers (names, types, encoders, bit counts, chosen decoder)
//! cargo run --release --example demo_inspect -- path/to/match.dem fields CCSPlayerPawn CBodyComponentBaseAnimGraph
//! # every game event name, its count and its first instance (or each instance of the names given)
//! cargo run --release --example demo_inspect -- path/to/match.dem events [weapon_fire ...]
//! # how many network messages of each id the packets carry
//! cargo run --release --example demo_inspect -- path/to/match.dem messages
//! # the rounds and their tick ranges
//! cargo run --release --example demo_inspect -- path/to/match.dem rounds
//! ```
//!
//! Without a path, the newest demo in CS2's `game/csgo/replays` folder is used.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use ofast_core::demo::DemoParser;
use ofast_core::demo::serializer::FieldKind;
use ofast_core::game::Cs2Install;

fn newest_replay() -> PathBuf {
    let install = Cs2Install::locate().expect("CS2 install found");
    std::fs::read_dir(install.replays_dir())
        .expect("replays folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dem"))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
        .expect("a .dem file in the replays folder")
}

/// Reads a protobuf varint at `*at`.
fn varint(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The fields of a protobuf message without its definition, as `{number: value, …}`; `None` when the bytes are not a
/// well-formed message. Fixed32 values are shown as float and integer (a float field and a fixed32 id look alike).
fn wire_fields(data: &[u8], depth: usize) -> Option<String> {
    let mut at = 0;
    let mut fields = Vec::new();
    while at < data.len() {
        let key = varint(data, &mut at)?;
        let (number, wire) = (key >> 3, key & 7);
        if number == 0 {
            return None;
        }
        let value = match wire {
            0 => format!("{}", varint(data, &mut at)?),
            1 => {
                let bytes: [u8; 8] = data.get(at..at + 8)?.try_into().ok()?;
                at += 8;
                format!("f64 {}", f64::from_le_bytes(bytes))
            }
            2 => {
                let len = usize::try_from(varint(data, &mut at)?).ok()?;
                let bytes = data.get(at..at + len)?;
                at += len;
                match (depth < 3).then(|| wire_fields(bytes, depth + 1)).flatten() {
                    Some(nested) if !bytes.is_empty() => nested,
                    _ => format!("bytes {:02x?}", bytes),
                }
            }
            5 => {
                let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
                at += 4;
                format!(
                    "f32 {} / u32 {}",
                    f32::from_le_bytes(bytes),
                    u32::from_le_bytes(bytes)
                )
            }
            _ => return None,
        };
        fields.push(format!("{number}: {value}"));
    }
    Some(format!("{{{}}}", fields.join(", ")))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(newest_replay);
    let data = std::fs::read(&path).expect("demo reads");
    let mut parser = DemoParser::new(&data).expect("demo header");
    println!("{}: {:#?}", path.display(), parser.header());

    if args.get(1).map(String::as_str) == Some("rounds") {
        for r in ofast_core::demo::players::list_rounds(&data).expect("rounds") {
            println!(
                "round {:2}: ticks {:6} ..= {:6}",
                r.round, r.start_tick, r.end_tick
            );
        }
        return;
    }

    if args.get(1).map(String::as_str) == Some("events") {
        // Every game event name with its count and the first one in full; with more arguments, only those names
        // (each instance printed, e.g. `events weapon_fire`).
        let wanted: Vec<&str> = args[2..].iter().map(String::as_str).collect();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        while parser.next_frame().expect("frame").is_some() {
            for event in parser.take_events() {
                let count = counts.entry(event.name.clone()).or_default();
                *count += 1;
                if (wanted.is_empty() && *count == 1) || wanted.contains(&event.name.as_str()) {
                    println!("{:6} {} {:?}", event.tick, event.name, event.keys);
                }
            }
        }
        println!("\nevents by name:");
        for (name, count) in counts {
            println!("  {count:6} {name}");
        }
        return;
    }

    if args.get(1).map(String::as_str) == Some("messages") {
        // How many network messages of each id the demo's packets carry (ids: netmessages.proto's SVC_Messages,
        // networkbasetypes.proto's NET_Messages, gameevents.proto, and Valve's CS2 protobufs OFAST does not bundle).
        while parser.next_frame().expect("frame").is_some() {
            parser.take_events();
        }
        for (kind, count) in parser.message_counts() {
            println!("  message {kind:5}: {count}");
        }
        return;
    }

    if args.get(1).map(String::as_str) == Some("raw") {
        // The raw protobuf fields of the first messages of one id (`raw 452 20`), for messages OFAST has no
        // definition of: field number, wire type and value; length-delimited fields are shown nested when they parse
        // as a message.
        let kind: i32 = args
            .get(2)
            .and_then(|k| k.parse().ok())
            .expect("raw <message id> [count]");
        let count: usize = args.get(3).and_then(|c| c.parse().ok()).unwrap_or(10);
        parser.keep_messages(&[kind]);
        let mut shown = 0;
        while shown < count && parser.next_frame().expect("frame").is_some() {
            parser.take_events();
            for message in parser.take_messages() {
                if shown < count {
                    println!(
                        "tick {}: {}",
                        message.tick,
                        wire_fields(&message.body, 0).unwrap_or_default()
                    );
                    shown += 1;
                }
            }
        }
        return;
    }

    if args.get(1).map(String::as_str) == Some("fields") {
        // Read until the send tables are known, then print the requested serializers.
        while parser.serializers().is_none() {
            parser
                .next_frame()
                .expect("frame")
                .expect("send tables before the end");
        }
        let s = parser.serializers().expect("checked");
        for name in &args[2..] {
            let Some(id) = s.by_name(name) else {
                println!("no serializer {name}");
                continue;
            };
            println!("\n{name} (version {}):", s.serializers[id].version);
            for (i, &f) in s.serializers[id].fields.iter().enumerate() {
                let f = &s.fields[f];
                let kind = match &f.kind {
                    FieldKind::Pointer {
                        serializer,
                        polymorphic,
                    } => format!(
                        "Pointer -> {} (+{} polymorphic)",
                        s.serializers[*serializer].name,
                        polymorphic.len().saturating_sub(1)
                    ),
                    FieldKind::VariableTable(inner) => {
                        format!("VariableTable -> {}", s.serializers[*inner].name)
                    }
                    other => format!("{other:?}"),
                };
                println!(
                    "  [{i:3}] {:40} {:50} enc={:16} bits={:?} low={:?} high={:?} flags={:?}  {kind}",
                    f.name,
                    f.var_type,
                    f.encoder,
                    f.bit_count,
                    f.low_value,
                    f.high_value,
                    f.encode_flags
                );
            }
        }
        return;
    }

    let start = Instant::now();
    let mut frames: BTreeMap<u32, usize> = BTreeMap::new();
    let mut last_tick = None;
    loop {
        match parser.next_frame() {
            Ok(Some(f)) => {
                *frames.entry(f.raw_command).or_default() += 1;
                last_tick = f.tick.or(last_tick);
            }
            Ok(None) => break,
            Err(e) => {
                println!("error at tick {:?}: {e}", parser.tick());
                break;
            }
        }
    }
    println!("frames by command: {frames:?}");
    println!(
        "last tick: {last_tick:?}, tick interval {:?}",
        parser.tick_interval()
    );
    println!(
        "entities alive at the end: {}",
        parser.entities().iter().count()
    );
    println!("parsed in {:.2?}", start.elapsed());
}
