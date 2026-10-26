//! Developer tool: each player's AnimGraph2 pose recipe as the demo sends it, decoded ([`ofast_core::demo::recipe`]):
//! what CS2's server animated, tick by tick.
//!
//! ```bash
//! cargo run --release --example pose_recipe -- path/to/match.dem 37940 37990 [player name filter]
//! ```
//!
//! Prints one line per player and tick whose recipe changed: tick, name, active slot, then each task as
//! `type(inputs):payload` (`1:#321@0.259` samples resource 321 at 0.259 of its length, `7(0,1):w0.13` blends tasks 0
//! and 1 by 0.13, `8(4,5):w1.00[mask0]` with bone mask 0, `0(2):write22` / `6:read22` cached poses, `13:empty`),
//! `?` from the first task whose payload is not known, then the data bit where decoding stopped and the next two
//! 16-bit values. `OFAST_HEX=1` adds the topology and data bytes.
//!
//! `OFAST_CHECK=1` prints only a summary instead: recipes read, errors by kind, the task types decoding stops at, how
//! many recipes have every clip and blend decoded (only a 14, 15 or 5 left after the stop), the distinct topologies
//! (how many fit the input counts, where each type sits); `OFAST_TOPOLOGIES=<file>` writes those topologies in hex.
//! With it, `OFAST_PROBE=<type>` leaves that type undecoded ([`recipe::decode_with`]) and scores fixed payload widths
//! for it: before a sample, the widths that keep the sample's clip and step its time a little (1 – 13107 of 65535)
//! from one server tick to the next in the same topology; before a 14, the widths that put 32768 (the 14's usual
//! first value) after it. Widths short of the true one by bits that never change score alike: the true width is the
//! widest of a tie. Blends' payloads vary in length (the mask list) and cannot be scored this way.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::entity::handle_to_index;
use ofast_core::demo::recipe::{self, MaskTask, RecipePaths, Task};

/// A pawn's last probed recipe at one width: its server tick, topology and the clips after the probed type.
type LastProbe = (u32, Vec<u8>, Vec<u16>);

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
    let (mut read, mut complete, mut only_aim_left, mut clips_known) =
        (0usize, 0usize, 0usize, 0usize);
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut stops: BTreeMap<u8, usize> = BTreeMap::new();
    let mut topologies: BTreeSet<Vec<u8>> = BTreeSet::new();
    let probe: Option<u8> = std::env::var("OFAST_PROBE")
        .ok()
        .and_then(|p| p.parse().ok());
    let mut probed = 0usize;
    // Width → (recipes decoding without an error, clips after the type keeping their index, clips compared,
    // recipes with 32768 where a 14 starts).
    let mut scores: BTreeMap<usize, (usize, usize, usize, usize)> = BTreeMap::new();
    // Type decoded before a stop at a 14 → (recipes, with 32768 where the 14 starts).
    let mut aim_marks: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
    let mut last_probe: HashMap<(u32, usize), LastProbe> = HashMap::new();
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
                // Alignment: decoding stopped at a 14 whose first value is 32768 (the aim's, nearly always), per type
                // decoded before it.
                if left[0] == 14 {
                    let aligned =
                        recipe::read_bits(&recipe.data, decoded.end_bit, 16) == Some(32768);
                    let kinds: BTreeSet<u8> = decoded.topology[..decoded.tasks.len()]
                        .iter()
                        .map(|t| t.kind)
                        .collect();
                    for kind in kinds {
                        let entry = aim_marks.entry(kind).or_default();
                        entry.0 += 1;
                        entry.1 += usize::from(aligned);
                    }
                }
                // Only a 14, a 15 or the 5 after the stop: every clip, time and weight is known.
                if left.iter().all(|k| matches!(k, 5 | 14 | 15)) {
                    clips_known += 1;
                }
            }
            // `OFAST_PROBE=<type>`: read that type's payloads as 0 – 32 raw bits and decode the rest of the recipe
            // each way. At the right width the decode goes through and the clips after it keep their resource index
            // from one server tick to the next (same player and topology); where decoding then stops at a 14, the
            // aim's first value is 32768 nearly always.
            if let Some(kind) = probe
                && decoded.topology.iter().any(|t| t.kind == kind)
            {
                probed += 1;
                for width in 0..=32 {
                    let score = scores.entry(width).or_default();
                    let Ok(d) = recipe::decode_with(&recipe, &[], Some((kind, width))) else {
                        continue;
                    };
                    score.0 += 1;
                    let after = d.tasks.iter().position(|t| matches!(t, Task::Raw { .. }));
                    let indices: Vec<u16> = d
                        .tasks
                        .iter()
                        .skip(after.map_or(usize::MAX, |a| a + 1))
                        .filter_map(|t| match t {
                            Task::Sample { resource, .. } => Some(*resource),
                            _ => None,
                        })
                        .collect();
                    if let Some((t, topology, before)) = last_probe.get(&(e.index, width))
                        && *t + 1 == d.tick
                        && *topology == recipe.topology
                    {
                        score.1 += before.iter().zip(&indices).filter(|(a, b)| a == b).count();
                        score.2 += before.len().min(indices.len());
                    }
                    if d.topology.get(d.tasks.len()).map(|t| t.kind) == Some(14)
                        && recipe::read_bits(&recipe.data, d.end_bit, 16) == Some(32768)
                    {
                        score.3 += 1;
                    }
                    last_probe.insert((e.index, width), (d.tick, recipe.topology.clone(), indices));
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
                            Task::CachedPoseWrite(id) => format!("write{id}"),
                            Task::CachedPoseRead(id) => format!("read{id}"),
                            Task::EmptyPose => "empty".to_owned(),
                            Task::Raw { bits, .. } => format!("raw{bits}"),
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
        let known = complete + clips_known;
        println!(
            "{read} recipes: errors {errors:?}; {complete} decoded to the end; the rest stop at task types \
             {stops:?} (type → recipes), {only_aim_left} of them with only a 14 and a 5 left; every clip and blend \
             known in {known} ({:.1} %: only a 14, 15 or 5 after the stop)",
            known as f64 * 100.0 / read.max(1) as f64
        );
        let marks: Vec<String> = aim_marks
            .iter()
            .map(|(kind, (n, aligned))| {
                format!(
                    "{kind}: {:.1} % of {n}",
                    *aligned as f64 * 100.0 / (*n).max(1) as f64
                )
            })
            .collect();
        println!(
            "stops at a 14 with 32768 there, by type decoded before it: {}",
            marks.join(", ")
        );
        if let Some(kind) = probe {
            // Ranked by the share of clips after the type that keep their index, then by clean decodes.
            let mut best: Vec<(usize, f64, usize, usize)> = scores
                .iter()
                .map(|(&w, &(ok, kept, compared, aim))| {
                    (w, kept as f64 / compared.max(1) as f64, ok, aim)
                })
                .collect();
            best.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)));
            println!(
                "type {kind}: in {probed} recipes; best payload widths (width: clips after it kept, clean decodes, 32768 at a 14):"
            );
            for (w, kept, ok, aim) in best.iter().take(6) {
                println!(
                    "  {w:2} bits: {:.1} % kept, {ok} decode, {aim} with 32768",
                    kept * 100.0
                );
            }
            // The most clean decodes after the most, wherever its width ranks.
            let mut by_decodes: Vec<(usize, usize, usize)> =
                best.iter().map(|b| (b.2, b.0, b.3)).collect();
            by_decodes.sort_unstable_by(|a, b| b.cmp(a));
            if let [(top, top_width, top_aim), (next, next_width, next_aim), ..] = by_decodes[..] {
                println!(
                    "  most clean decodes: {top} at {top_width} bits ({top_aim} with 32768); the next most {next} at \
                     {next_width} bits ({next_aim} with 32768)"
                );
            }
        }
        // `OFAST_TOPOLOGIES=<file>`: the distinct topologies' bytes in hex, one per line, to fit input counts offline.
        if let Ok(file) = std::env::var("OFAST_TOPOLOGIES") {
            let lines: Vec<String> = topologies.iter().map(|t| hex(t)).collect();
            std::fs::write(&file, lines.join("\n") + "\n").expect("topologies written");
        }
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
