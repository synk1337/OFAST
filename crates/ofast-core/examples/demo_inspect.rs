//! Developer tool: look inside a CS2 demo.
//!
//! ```bash
//! # header, frame counts, how far the parser gets
//! cargo run --release --example demo_inspect -- path/to/match.dem
//! # the fields of some serializers (names, types, encoders, bit counts, chosen decoder)
//! cargo run --release --example demo_inspect -- path/to/match.dem fields CCSPlayerPawn CBodyComponentBaseAnimGraph
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
