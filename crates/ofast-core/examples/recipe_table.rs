//! Developer tool: one demo round's pose-recipe clip table ([`ofast_core::anim::recipe::learn_clip_table`]: each
//! resource index matched to the clip OFAST's own bake samples with it most exclusively, one clip per index), with
//! what helps judge each match.
//!
//! ```bash
//! cargo run --release --example recipe_table -- path/to/match.dem 5
//! ```
//!
//! Per index: how many sample tasks name it; how fast its time runs where it keeps playing from one tick to the next
//! (a tick over the clip's length when it plays at its own speed; locomotion clips run at their blend's shared
//! rate), or the time it stands still at; the weapon (or weapon set) in hand on three quarters of its ticks; its
//! match and the match's Jaccard index and length. An index without a match lists the clips that could play with
//! that weapon (the main graph's resources and its child graphs': a weapon's own graph,
//! `worldmodel_gun.vnmgraph+ak47.vnmgraph`, plays only while that weapon is in hand, a locomotion graph only with its
//! set), their own graph's first, ranked by how well their length fits. Last, the matches the blends' structure
//! dropped (and what the index has now) and the share of sample tasks the table names.

use std::collections::{BTreeMap, BTreeSet};

use ofast_core::anim::Clip;
use ofast_core::anim::bake::WeaponSet;
use ofast_core::anim::graph::Graph;
use ofast_core::anim::recipe::learn_clip_table;
use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::demo::recipe::{self, Task};
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
fn fits(graphs: &BTreeSet<String>, weapon: &str, set: &str) -> bool {
    let name = weapon.trim_start_matches("weapon_").replace('_', "");
    graphs.iter().any(|g| match variation(g) {
        None => true,
        Some((base, v)) if base.contains("locomotion") => set.to_lowercase() == v,
        Some((_, v)) => {
            let v = v.replace('_', "");
            name.contains(&v) || v.contains(&name)
        }
    })
}

/// What is seen of one resource index.
#[derive(Default)]
struct Seen {
    /// Sample tasks naming it.
    samples: usize,
    /// Weapons in hand (type ID and set) and on how many of its samples.
    weapons: BTreeMap<(String, String), usize>,
    /// Time steps from one tick to the next where it kept playing.
    steps: Vec<f32>,
    /// Its times.
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
    let table = learn_clip_table(&track, &rig.sets, rig.upper.as_ref(), rig.graphs.as_ref());
    // Per index, from the round's recipes; each player's last tick's samples by position, for the time steps.
    let mut seen: BTreeMap<u16, Seen> = BTreeMap::new();
    let mut last: BTreeMap<u32, (u32, Vec<Task>)> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            let Some(decoded) = p
                .rig
                .pose_recipe
                .as_ref()
                .and_then(|r| recipe::decode(r).ok())
            else {
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
            let previous = last
                .get(&p.controller)
                .filter(|(t, _)| *t + 1 == sample.tick)
                .map(|(_, tasks)| tasks);
            for (position, task) in decoded.tasks.iter().enumerate() {
                let Task::Sample { resource, time } = *task else {
                    continue;
                };
                let entry = seen.entry(resource).or_default();
                entry.samples += 1;
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
            last.insert(p.controller, (sample.tick, decoded.tasks));
        }
    }
    let tick = track.tick_interval;
    for (index, s) in &seen {
        let mut steps = s.steps.clone();
        steps.sort_by(f32::total_cmp);
        let implied = steps.get(steps.len() / 2).map(|r| tick / r);
        let mut times = s.times.clone();
        times.sort_by(f32::total_cmp);
        let still = if s.steps.is_empty() && !times.is_empty() {
            format!(" at {:.3}", times[times.len() / 2])
        } else {
            String::new()
        };
        // The weapon in hand on three quarters of its samples, else the set.
        let dominant = s
            .weapons
            .iter()
            .max_by_key(|(_, n)| **n)
            .filter(|(_, n)| **n * 4 >= s.samples * 3)
            .map(|((w, set), _)| (w.clone(), set.clone()));
        let mut sets: BTreeMap<&str, usize> = BTreeMap::new();
        for ((_, set), n) in &s.weapons {
            *sets.entry(set.as_str()).or_default() += n;
        }
        let dominant_set = sets
            .iter()
            .max_by_key(|(_, n)| **n)
            .filter(|(_, n)| **n * 4 >= s.samples * 3)
            .map(|(set, _)| set.to_string());
        let held = match (&dominant, &dominant_set) {
            (Some((w, _)), _) => w.trim_start_matches("weapon_").to_owned(),
            (None, Some(set)) => format!("{}s", set.to_lowercase()),
            (None, None) => "mixed weapons".to_owned(),
        };
        let matched = match table.clips.get(index) {
            Some((path, j)) => {
                let stem = path.rsplit('/').next().unwrap_or(path);
                let length = clips.get(path.as_str()).map_or(0.0, |c| c.0);
                format!("{stem} ({length:.2} s, Jaccard {j:.2})")
            }
            None => {
                // Candidates: what could play with the weapon (or set) in hand, its own graphs' first.
                let mut candidates: Vec<(&String, f32, bool)> = clips
                    .iter()
                    .filter(|(_, (_, graphs))| match (&dominant, &dominant_set) {
                        (Some((weapon, set)), _) => fits(graphs, weapon, set),
                        (None, Some(set)) => graphs.iter().any(|g| match variation(g) {
                            None => true,
                            Some((base, v)) => {
                                base.contains("locomotion") && v == set.to_lowercase()
                            }
                        }),
                        (None, None) => graphs.iter().any(|g| variation(g).is_none()),
                    })
                    .map(|(path, (length, graphs))| {
                        (path, *length, graphs.iter().all(|g| variation(g).is_some()))
                    })
                    .collect();
                candidates.sort_by(|a, b| {
                    let miss = |c: &(&String, f32, bool)| implied.map_or(0.0, |i| (c.1 - i).abs());
                    b.2.cmp(&a.2).then(miss(a).total_cmp(&miss(b)))
                });
                let top: Vec<String> = candidates
                    .iter()
                    .take(3)
                    .map(|(path, length, _)| {
                        format!(
                            "{} ({length:.2} s)",
                            path.rsplit('/').next().unwrap_or(path)
                        )
                    })
                    .collect();
                format!("no match; could be {}", top.join(", "))
            }
        };
        println!(
            "{index:4} {:6} samples, {} steps, length {}{still} — {held} — {matched}",
            s.samples,
            s.steps.len(),
            implied.map_or("?".to_owned(), |i| format!("{i:.2} s")),
        );
    }
    for (index, clip) in &table.dropped {
        let now = table.path(*index).map_or("nothing".to_owned(), |p| {
            p.rsplit('/').next().unwrap_or(p).to_owned()
        });
        println!(
            "dropped by the blends' structure: {index} {} (now {now})",
            clip.rsplit('/').next().unwrap_or(clip)
        );
    }
    println!(
        "table: {} of {} indices matched; they name {} of the round's {} sample tasks ({:.1} %)",
        table.clips.len(),
        seen.len(),
        table.samples.1,
        table.samples.0,
        table.samples.1 as f64 * 100.0 / table.samples.0.max(1) as f64
    );
}
