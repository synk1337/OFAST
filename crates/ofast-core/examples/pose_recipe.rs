//! Developer tool: each player's AnimGraph2 pose recipe as the demo sends it, decoded ([`ofast_core::demo::recipe`]):
//! what CS2's server animated, tick by tick.
//!
//! ```bash
//! cargo run --release --example pose_recipe -- path/to/match.dem 37940 37990 [player name filter]
//! ```
//!
//! Prints one line per player and tick whose recipe changed: tick, name, active slot, then each task as
//! `type(inputs):payload` (`1:#321@0.259` samples resource 321 at 0.259 of its length, `7(0,1):w0.13` blends tasks 0
//! and 1 by 0.13, `8(4,5):w1.00[mask0]` with bone mask 0), `?` from the first task whose payload is not known.
//! `OFAST_HEX=1` adds the topology and data bytes. `OFAST_CHECK=1` prints only a summary: recipes read, topologies
//! that parse, recipes decoded to the end, and which task type stops the others.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::entity::handle_to_index;
use ofast_core::demo::recipe::{self, MaskTask, RecipePaths, Task};

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn mask(list: &[MaskTask]) -> String {
    let entries: Vec<String> = list
        .iter()
        .map(|m| match m {
            MaskTask::Mask(i) => format!("mask{i}"),
            MaskTask::Generate(w) => format!("gen{w:.2}"),
            MaskTask::Blend(a, b, w) => format!("blend{a},{b},{w:.2}"),
            MaskTask::Scale(a, w) => format!("scale{a},{w:.2}"),
            MaskTask::Combine(a, b) => format!("comb{a},{b}"),
        })
        .collect();
    format!("[{}]", entries.join(" "))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let (first, last): (u32, u32) = (
        args[1].parse().expect("first tick"),
        args[2].parse().expect("last tick"),
    );
    let name_filter = args.get(3).cloned().unwrap_or_default();
    let show_hex = std::env::var_os("OFAST_HEX").is_some();
    let check = std::env::var_os("OFAST_CHECK").is_some();
    let mut parser = DemoParser::new(&data).expect("demo header");
    let mut previous: HashMap<u32, recipe::PoseRecipe> = HashMap::new();
    // Summary: recipes read, errors by kind, decoded to the end, first undecoded type → count, and of those stopping
    // at a type 14 how many have only 14 and 5 left; the distinct topologies.
    let (mut read, mut complete, mut only_aim_left) = (0usize, 0usize, 0usize);
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut stops: BTreeMap<u8, usize> = BTreeMap::new();
    let mut topologies: BTreeSet<Vec<u8>> = BTreeSet::new();
    while let Some(frame) = parser.next_frame().expect("frame parses") {
        let Some(tick) = frame.tick else { continue };
        if tick < first || !frame.entities_changed {
            continue;
        }
        if tick > last {
            break;
        }
        let serializers = parser.serializers().expect("serializers");
        let Some(paths) = RecipePaths::find(serializers) else {
            eprintln!("this demo's pawns have no pose recipe fields");
            return;
        };
        let controller = serializers.by_name("CCSPlayerController");
        let pawn_field = controller.and_then(|c| serializers.path_of(c, "m_hPlayerPawn"));
        let name_field = controller.and_then(|c| serializers.path_of(c, "m_iszPlayerName"));
        // Pawn entity index → player name.
        let mut names: HashMap<u32, String> = HashMap::new();
        for e in parser.entities().iter() {
            if parser.class_name(e.class_id) != Some("CCSPlayerController") {
                continue;
            }
            let pawn = pawn_field
                .as_ref()
                .and_then(|p| e.get(p))
                .and_then(FieldValue::as_u64)
                .and_then(handle_to_index);
            let name = name_field
                .as_ref()
                .and_then(|p| e.get(p))
                .and_then(FieldValue::as_str);
            if let (Some(pawn), Some(name)) = (pawn, name) {
                names.insert(pawn, name.to_owned());
            }
        }
        for e in parser.entities().iter() {
            if parser.class_name(e.class_id) != Some("CCSPlayerPawn") {
                continue;
            }
            let name = names.get(&e.index).map_or("?", String::as_str);
            if !name.contains(name_filter.as_str()) {
                continue;
            }
            let Some(recipe) = recipe::read(e, &paths) else {
                continue;
            };
            if previous.get(&e.index) == Some(&recipe) {
                continue;
            }
            read += 1;
            topologies.insert(recipe.topology.clone());
            let decoded = match recipe::decode(&recipe) {
                Ok(d) => d,
                Err(error) => {
                    *errors.entry(error.to_string()).or_default() += 1;
                    if !check {
                        println!("{tick} {name} slot {}: {error}", recipe.slot);
                    }
                    previous.insert(e.index, recipe);
                    continue;
                }
            };
            let left: Vec<u8> = decoded.topology[decoded.tasks.len()..]
                .iter()
                .map(|t| t.kind)
                .collect();
            if decoded.complete() {
                complete += 1;
            } else {
                *stops.entry(left[0]).or_default() += 1;
                if left == [14, 5] {
                    only_aim_left += 1;
                }
            }
            if !check {
                let mut parts: Vec<String> = decoded
                    .topology
                    .iter()
                    .zip(&decoded.tasks)
                    .map(|(node, task)| {
                        let inputs = if node.inputs.is_empty() {
                            String::new()
                        } else {
                            let list: Vec<String> = node.inputs.iter().map(u8::to_string).collect();
                            format!("({})", list.join(","))
                        };
                        let payload = match task {
                            Task::Sample { resource, time } => format!("#{resource}@{time:.3}"),
                            Task::Blend {
                                weight, mask: m, ..
                            } if m.is_empty() => {
                                format!("w{weight:.2}")
                            }
                            Task::Blend {
                                weight, mask: m, ..
                            } => {
                                format!("w{weight:.2}{}", mask(m))
                            }
                            Task::Type0(v) => format!("u{v}"),
                        };
                        format!("{}{inputs}:{payload}", node.kind)
                    })
                    .collect();
                for node in &decoded.topology[decoded.tasks.len()..] {
                    let list: Vec<String> = node.inputs.iter().map(u8::to_string).collect();
                    parts.push(format!("{}({})?", node.kind, list.join(",")));
                }
                // Where decoding stopped, and the next two 16-bit values there.
                let next: Vec<String> = [0, 16]
                    .iter()
                    .filter_map(|&k| recipe::read_bits(&recipe.data, decoded.end_bit + k, 16))
                    .map(|v| v.to_string())
                    .collect();
                println!(
                    "{tick} {name} slot {}: {} | stop at bit {}, next 16-bit values {}",
                    recipe.slot,
                    parts.join(" "),
                    decoded.end_bit,
                    next.join(" ")
                );
                if show_hex {
                    println!("    topology {}", hex(&recipe.topology));
                    println!("    data {}", hex(&recipe.data));
                }
            }
            previous.insert(e.index, recipe);
        }
    }
    if check {
        println!(
            "{read} recipes: errors {errors:?}; {complete} decoded to the end; the rest stop at task types \
             {stops:?} (type → recipes), {only_aim_left} of them with only a 14 and a 5 left"
        );
        // The distinct topologies: how many fit the input counts, and where each type sits.
        let mut fit = 0;
        // Type → (topologies with it, first in, last in, next type → count).
        let mut types: BTreeMap<u8, (usize, usize, usize, BTreeMap<u8, usize>)> = BTreeMap::new();
        for bytes in &topologies {
            let Ok(t) = recipe::check_topology(bytes) else {
                continue;
            };
            if t.all_used && t.spare_bits < 8 {
                fit += 1;
            }
            let kinds: Vec<u8> = t.tasks.iter().map(|t| t.kind).collect();
            for (i, &kind) in kinds.iter().enumerate() {
                let entry = types.entry(kind).or_default();
                if !kinds[..i].contains(&kind) {
                    entry.0 += 1;
                }
                entry.1 += usize::from(i == 0);
                entry.2 += usize::from(i + 1 == kinds.len());
                if let Some(&next) = kinds.get(i + 1) {
                    *entry.3.entry(next).or_default() += 1;
                }
            }
        }
        println!(
            "{} different topologies, {fit} of them fitting the input counts (every task but the last used, the bits \
             ending in the last byte)",
            topologies.len()
        );
        for (kind, (with, first, last, next)) in types {
            println!(
                "  type {kind:2}: in {with} topologies, first in {first}, last in {last}, followed by {next:?}"
            );
        }
    }
}
