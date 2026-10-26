//! Developer tool: each pose-recipe resource index's clip in one demo, learned from the demo itself. CS2 numbers the
//! clips of its graph's serialization context in an order no rule found gives, and the order changes with the build
//! (`docs/formats/demo.md`, "Pose recipe"), so every demo has to be matched.
//!
//! The match: OFAST's own bake runs for every player at the tick rate with a trace of the clips it samples
//! ([`ofast_core::anim::trace_start`]), and each index gets the baked clip that comes with it most exclusively on
//! the same player's same ticks ("baked with it", with the Jaccard index: ticks together over ticks either plays).
//!
//! Also printed, to judge a match: how many ticks the index plays and how fast its time runs where it keeps playing
//! from one tick to the next (a tick over the clip's length when it plays at its own speed; locomotion clips run at
//! their blend's shared rate), or the time it stands still at; the weapon (or weapon set) in hand on three quarters
//! of its ticks; and the clips that could play with that weapon (the main graph's resources and its child graphs':
//! a weapon's own graph, `worldmodel_gun.vnmgraph+ak47.vnmgraph`, plays only while that weapon is in hand, a
//! locomotion graph only with its set), their own graph's first, ranked by how well their length fits the rate.
//!
//! ```bash
//! cargo run --release --example recipe_table -- path/to/match.dem 5
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ofast_core::anim::Clip;
use ofast_core::anim::bake::{WeaponSet, bake_player, frame_samples};
use ofast_core::anim::graph::Graph;
use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::entity::handle_to_index;
use ofast_core::demo::players::track_round;
use ofast_core::demo::recipe::{self, RecipePaths, Task};
use ofast_core::export::players::load_player_rig;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::resource::{BlockKind, Resource};

const MAIN_GRAPH: &str = "animation/graphs/worldmodel/worldmodel.vnmgraph";

fn data(content: &Content, path: &str) -> Option<Value> {
    let bytes = content.read_resource(path).ok()?;
    Some(
        Resource::parse(&bytes)
            .ok()?
            .kv3(BlockKind::DATA)
            .ok()?
            .root,
    )
}

/// Every clip the main graph can reach: path → (length in seconds, the graphs that list it).
fn clips(content: &Content) -> BTreeMap<String, (f32, BTreeSet<String>)> {
    let mut out: BTreeMap<String, (f32, BTreeSet<String>)> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![MAIN_GRAPH.to_owned()];
    while let Some(path) = stack.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(graph) = data(content, &path).and_then(|d| Graph::parse(&d).ok()) else {
            eprintln!("warning: {path} does not read");
            continue;
        };
        for r in graph.resources {
            if r.ends_with(".vnmgraph") {
                stack.push(r);
            } else if r.ends_with(".vnmclip") {
                out.entry(r).or_default().1.insert(path.clone());
            }
        }
    }
    for (path, (length, _)) in out.iter_mut() {
        *length = data(content, path)
            .and_then(|d| Clip::parse(&d).ok())
            .map_or(0.0, |c| c.duration);
    }
    out
}

/// What follows the `+` of a graph variation's name (`…_gun.vnmgraph+ak47.vnmgraph` → `ak47`), and its base.
fn variation(graph: &str) -> Option<(&str, &str)> {
    let (base, rest) = graph.split_once('+')?;
    Some((base, rest.trim_end_matches(".vnmgraph")))
}

/// A clip listed by `graphs` can play with `weapon` (`weapon_ak47`) of `set` in hand.
fn fits(graphs: &BTreeSet<String>, weapon: &str, set: WeaponSet) -> bool {
    let name = weapon.trim_start_matches("weapon_").replace('_', "");
    graphs.iter().any(|g| match variation(g) {
        None => true,
        Some((base, v)) if base.contains("locomotion") => format!("{set:?}").to_lowercase() == v,
        Some((_, v)) => {
            let v = v.replace('_', "");
            name.contains(&v) || v.contains(&name)
        }
    })
}

/// Each player's decoded recipe tasks by (tick, name) over the round.
fn recipes(data: &[u8], first: u32, last: u32) -> HashMap<(u32, String), Vec<Task>> {
    let mut parser = DemoParser::new(data).expect("demo header");
    let mut out = HashMap::new();
    while let Some(frame) = parser.next_frame().expect("frame parses") {
        let Some(tick) = frame.tick else { continue };
        if tick < first || !frame.entities_changed {
            continue;
        }
        if tick > last {
            break;
        }
        let serializers = parser.serializers().expect("serializers");
        let paths = RecipePaths::find(serializers).expect("pose recipe fields");
        let controller = serializers.by_name("CCSPlayerController");
        let pawn_field = controller.and_then(|c| serializers.path_of(c, "m_hPlayerPawn"));
        let name_field = controller.and_then(|c| serializers.path_of(c, "m_iszPlayerName"));
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
            let (Some(name), Some(r)) = (names.get(&e.index), recipe::read(e, &paths)) else {
                continue;
            };
            if let Ok(decoded) = recipe::decode(&r) {
                out.insert((tick, name.clone()), decoded.tasks);
            }
        }
    }
    out
}

/// What is seen of one resource index.
#[derive(Default)]
struct Seen {
    /// Ticks it was sampled on.
    ticks: usize,
    /// Weapons in hand (type ID and set) and on how many ticks.
    weapons: BTreeMap<(String, String), usize>,
    /// Time steps from one tick to the next where it kept playing.
    steps: Vec<f32>,
    /// Its times, every tick.
    times: Vec<f32>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let demo = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let track = track_round(&demo, round).expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let rig = load_player_rig(&content, &track).expect("rig");
    let clips = clips(&content);
    eprintln!("{} clips reachable from the main graph", clips.len());
    let first = track.samples.first().map_or(0, |s| s.tick);
    let last = track.samples.last().map_or(0, |s| s.tick);
    let recipes = recipes(&demo, first, last);
    let mut seen: BTreeMap<u16, Seen> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            let Some(tasks) = recipes.get(&(sample.tick, p.name.clone())) else {
                continue;
            };
            let item = p.rig.weapon.as_ref().map(|w| w.item);
            let animation = rig
                .upper
                .as_ref()
                .and_then(|u| u.weapons.get(&item.unwrap_or(0)));
            let weapon = animation.map_or("none".to_owned(), |w| w.weapon.clone());
            let set = animation.map_or(WeaponSet::for_item(item), |w| {
                WeaponSet::for_category(&w.category)
            });
            let previous = recipes.get(&(sample.tick.wrapping_sub(1), p.name.clone()));
            for (position, task) in tasks.iter().enumerate() {
                let Task::Sample { resource, time } = *task else {
                    continue;
                };
                let entry = seen.entry(resource).or_default();
                entry.ticks += 1;
                entry.times.push(time);
                *entry
                    .weapons
                    .entry((weapon.clone(), format!("{set:?}")))
                    .or_default() += 1;
                if let Some(Task::Sample {
                    resource: before,
                    time: then,
                }) = previous.and_then(|t| t.get(position))
                    && *before == resource
                    && time > *then
                {
                    entry.steps.push(time - then);
                }
            }
        }
    }
    // What OFAST's own bake samples on each tick (ofast_core::anim::trace_start): every player baked at the tick
    // rate, the sampled clips' addresses turned back into paths.
    let mut by_address: HashMap<usize, String> = HashMap::new();
    if let Some(graphs) = &rig.graphs {
        for (path, clip) in graphs.library.clips() {
            by_address.insert(std::ptr::from_ref(clip) as usize, path.to_owned());
        }
    }
    for set in WeaponSet::ALL {
        for (path, clip) in &rig.sets.get(set).clips {
            by_address.insert(std::ptr::from_ref(clip) as usize, path.clone());
        }
    }
    if let Some(upper) = &rig.upper {
        for w in upper.weapons.values() {
            let clips = w.idle.iter().flatten().chain(w.shoot.as_ref());
            for (clip, path) in clips.zip(&w.paths) {
                by_address.insert(std::ptr::from_ref(clip) as usize, path.clone());
            }
        }
    }
    let fps = 1.0 / track.tick_interval;
    let frames = frame_samples(&track, fps);
    let mut baked: HashMap<(u32, String), BTreeSet<String>> = HashMap::new();
    let mut players: Vec<(u32, String)> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .map(|p| (p.controller, p.name.clone()))
        .collect();
    players.sort();
    players.dedup();
    for (controller, name) in &players {
        ofast_core::anim::trace_start();
        let baked_ok = bake_player(
            &track,
            *controller,
            &rig.sets,
            rig.upper.as_ref(),
            rig.graphs.as_ref(),
            fps,
            &HashMap::new(),
        )
        .is_ok();
        let traced = ofast_core::anim::trace_end();
        if !baked_ok {
            eprintln!("warning: {name} does not bake");
            continue;
        }
        for (&(tick, _), addresses) in frames.iter().zip(&traced) {
            let paths: BTreeSet<String> = addresses
                .iter()
                .filter_map(|a| by_address.get(a).cloned())
                .collect();
            baked.insert((tick.round() as u32, name.clone()), paths);
        }
    }
    // Each index against the clips the bake sampled on the same player's same ticks: how often each comes with it.
    let mut together: HashMap<u16, HashMap<String, usize>> = HashMap::new();
    let mut clip_ticks: HashMap<String, usize> = HashMap::new();
    let mut index_ticks: HashMap<u16, usize> = HashMap::new();
    for (key, tasks) in &recipes {
        let Some(paths) = baked.get(key) else {
            continue;
        };
        let indices: BTreeSet<u16> = tasks
            .iter()
            .filter_map(|t| match t {
                Task::Sample { resource, .. } => Some(*resource),
                _ => None,
            })
            .collect();
        for path in paths {
            *clip_ticks.entry(path.clone()).or_default() += 1;
        }
        for &index in &indices {
            *index_ticks.entry(index).or_default() += 1;
            let entry = together.entry(index).or_default();
            for path in paths {
                *entry.entry(path.clone()).or_default() += 1;
            }
        }
    }
    let tick = track.tick_interval;
    // Per Jaccard threshold (0.3, 0.5, 0.7): indices matched and the sampled clips they cover.
    let mut coverage = [(0usize, 0usize); 3];
    let mut all_ticks = 0usize;
    // Baked clip → indices matching it at 0.5 or more.
    let mut claimed: BTreeMap<String, usize> = BTreeMap::new();
    for (index, s) in &seen {
        // The baked clip that comes with it most exclusively (Jaccard: together ÷ either).
        let best = together.get(index).and_then(|with| {
            with.iter()
                .map(|(path, &n)| {
                    let either = index_ticks[index] + clip_ticks[path] - n;
                    (path, n as f32 / either.max(1) as f32)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        });
        let mut steps = s.steps.clone();
        steps.sort_by(f32::total_cmp);
        let rate = steps.get(steps.len() / 2).copied();
        let implied = rate.map(|r| tick / r);
        // The weapon in hand on most of its ticks (a clip still blends out a few ticks after a switch), if one has
        // three quarters of them.
        let dominant = s
            .weapons
            .iter()
            .max_by_key(|(_, n)| **n)
            .filter(|(_, n)| **n * 4 >= s.ticks * 3)
            .map(|((weapon, set), _)| {
                let set = match set.as_str() {
                    "Rifle" => WeaponSet::Rifle,
                    "Pistol" => WeaponSet::Pistol,
                    _ => WeaponSet::Knife,
                };
                (weapon.clone(), set)
            });
        // Without one, the weapon set on three quarters of them (rifle locomotion plays with every rifle).
        let mut sets: BTreeMap<&str, usize> = BTreeMap::new();
        for ((_, set), n) in &s.weapons {
            *sets.entry(set.as_str()).or_default() += n;
        }
        let dominant_set = sets
            .iter()
            .max_by_key(|(_, n)| **n)
            .filter(|(_, n)| **n * 4 >= s.ticks * 3)
            .map(|(set, _)| set.to_lowercase());
        // Candidates: with a dominant weapon, the clips that can play with it, its own graphs' first; with a dominant
        // set, the clips of that set's locomotion graph and the shared ones; else the shared clips (listed by graphs
        // that are no weapon's variation).
        let mut candidates: Vec<(&String, f32, bool, bool)> = clips
            .iter()
            .filter(|(_, (_, graphs))| match (&dominant, &dominant_set) {
                (Some((weapon, set)), _) => fits(graphs, weapon, *set),
                (None, Some(set)) => graphs.iter().any(|g| match variation(g) {
                    None => true,
                    Some((base, v)) => base.contains("locomotion") && v == set,
                }),
                (None, None) => graphs.iter().any(|g| variation(g).is_none()),
            })
            .map(|(path, (length, graphs))| {
                let locomotion = graphs.iter().all(|g| g.contains("locomotion"));
                let own = graphs.iter().all(|g| variation(g).is_some());
                (path, *length, locomotion, own)
            })
            .collect();
        candidates.sort_by(|a, b| {
            let miss = |c: &(&String, f32, bool, bool)| {
                let length = if c.2 {
                    f32::MAX / 4.0
                } else {
                    implied.map_or(0.0, |i| (c.1 - i).abs())
                };
                (!c.3 as u8, length)
            };
            let (ma, mb) = (miss(a), miss(b));
            ma.0.cmp(&mb.0).then(ma.1.total_cmp(&mb.1))
        });
        let held = match (&dominant, &dominant_set) {
            (Some((w, _)), _) => {
                let n: usize = s
                    .weapons
                    .iter()
                    .filter(|((x, _), _)| x == w)
                    .map(|(_, n)| n)
                    .sum();
                format!(
                    "{} {}%",
                    w.trim_start_matches("weapon_"),
                    n * 100 / s.ticks.max(1)
                )
            }
            (None, Some(set)) => format!("{set}s"),
            (None, None) => "mixed weapons".to_owned(),
        };
        // A time that stands still: its usual value.
        let mut times = s.times.clone();
        times.sort_by(f32::total_cmp);
        let still = if s.steps.is_empty() && !times.is_empty() {
            format!(" at {:.3}", times[times.len() / 2])
        } else {
            String::new()
        };
        let top: Vec<String> = candidates
            .iter()
            .take(3)
            .map(|(path, length, _, _)| {
                let stem = path.rsplit('/').next().unwrap_or(path);
                format!("{stem} ({length:.2} s)")
            })
            .collect();
        println!(
            "{index:4} {:6} ticks, {} steps, length {}{still} — {held} — baked with it: {} — {} candidates: {}",
            s.ticks,
            s.steps.len(),
            implied.map_or("?".to_owned(), |i| format!("{i:.2} s")),
            best.map_or("-".to_owned(), |(path, j)| {
                let stem = path.rsplit('/').next().unwrap_or(path);
                let length = clips.get(path.as_str()).map_or(0.0, |c| c.0);
                format!("{stem} ({length:.2} s, Jaccard {j:.2})")
            }),
            candidates.len(),
            top.join(", ")
        );
        let j = best.map_or(0.0, |(_, j)| j);
        if j >= 0.5
            && let Some((path, _)) = best
        {
            *claimed.entry(path.clone()).or_default() += 1;
        }
        for (threshold, entry) in [0.3, 0.5, 0.7].into_iter().zip(&mut coverage) {
            if j >= threshold {
                entry.0 += 1;
                entry.1 += s.ticks;
            }
        }
        all_ticks += s.ticks;
    }
    // Clips that more than one index matches at 0.5 or more: at most one of those matches is right (OFAST may play one
    // clip where CS2 plays two, its standing and crouched idles say).
    let shared: Vec<String> = claimed
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(path, n)| format!("{} ×{n}", path.rsplit('/').next().unwrap_or(path)))
        .collect();
    println!(
        "clips matched by more than one index at 0.5 or more: {} ({})",
        shared.len(),
        shared.join(", ")
    );
    // How much of the round's sampled clips (sample tasks) the matches cover, by how exclusive they are.
    for (threshold, (indices, ticks)) in [0.3, 0.5, 0.7].into_iter().zip(coverage) {
        println!(
            "matched at a Jaccard index of {threshold} or more: {indices} of {} indices, {ticks} of {all_ticks} \
             sampled clips ({:.1} %)",
            seen.len(),
            ticks as f64 * 100.0 / all_ticks.max(1) as f64
        );
    }
}
