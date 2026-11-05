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
//!
//! Why an index goes unmatched (how recipe version 3's dead pawns, silent reloads and plants were found):
//!
//! - `OFAST_INDEX=598,257`: per index, its player-ticks (alive and dead), the weapons and gameplay states
//!   (`m_iWeaponGameplayAnimState`) in hand, the clips OFAST bakes on its ticks with their Jaccard index against it,
//!   the blends it feeds, and how its starts (its time going back) lie against the player's shots;
//!   `OFAST_STARTS=n` adds its first n starts with the body-relative velocity (forward, left) 8 ticks before, at and
//!   8 after, and what OFAST bakes then; `OFAST_DEAD=1` prints some recipes of dead players.
//! - `OFAST_TIMELINE=name,from,to`: one player's ticks: the body-relative velocity, CS2's samples (`index@time`,
//!   `=clip` where the table names it) and the clips OFAST bakes.

use std::collections::{BTreeMap, BTreeSet};

use ofast_core::anim::Clip;
use ofast_core::anim::bake::WeaponSet;
use ofast_core::anim::graph::Graph;
use ofast_core::anim::recipe::{baked_clips, learn_clip_table};
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
    // Recipes on consecutive ticks of a player, and how many of them repeat the previous server tick (not sent anew).
    let mut repeats = (0usize, 0usize);
    let mut server: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
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
            if let Some(&(tick, then)) = server.get(&p.controller)
                && tick + 1 == sample.tick
            {
                repeats.0 += 1;
                repeats.1 += usize::from(then == decoded.tick);
            }
            server.insert(p.controller, (sample.tick, decoded.tick));
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
    println!(
        "recipes on a player's consecutive ticks: {}, {} of them repeating the previous server tick",
        repeats.0, repeats.1
    );
    // `OFAST_TIMELINE=name,from,to`: one player's ticks: body-relative velocity, CS2's samples (index@time, the
    // table's clip if any) and the clips OFAST bakes.
    if let Ok(spec) = std::env::var("OFAST_TIMELINE") {
        let parts: Vec<&str> = spec.split(',').collect();
        let (name, from, to) = (
            parts[0],
            parts[1].parse::<u32>().expect("from"),
            parts[2].parse::<u32>().expect("to"),
        );
        let baked = baked_clips(&track, &rig.sets, rig.upper.as_ref(), rig.graphs.as_ref());
        let stem = |c: &str| {
            c.rsplit('/')
                .next()
                .unwrap_or(c)
                .trim_end_matches(".vnmclip")
                .to_owned()
        };
        for sample in track
            .samples
            .iter()
            .filter(|s| (from..=to).contains(&s.tick))
        {
            let Some(p) = sample.players.iter().find(|p| p.name == name) else {
                continue;
            };
            let (sin, cos) = p.rig.body_yaw.to_radians().sin_cos();
            let v = p.rig.velocity;
            let tasks = p
                .rig
                .pose_recipe
                .as_ref()
                .and_then(|r| recipe::decode(r).ok())
                .map(|d| d.tasks)
                .unwrap_or_default();
            let cs2: Vec<String> = tasks
                .iter()
                .filter_map(|t| match t {
                    Task::Sample { resource, time } => Some(format!(
                        "{resource}@{time:.2}{}",
                        table
                            .path(*resource)
                            .map_or(String::new(), |p| format!("={}", stem(p)))
                    )),
                    _ => None,
                })
                .collect();
            let ours: Vec<String> = baked
                .get(&(sample.tick, p.controller))
                .map(|c| c.iter().map(|c| stem(c)).collect())
                .unwrap_or_default();
            println!(
                "{} ({:4.0},{:4.0}) | CS2 {} | OFAST {}",
                sample.tick,
                v[0] * cos + v[1] * sin,
                -v[0] * sin + v[1] * cos,
                cs2.join(" "),
                ours.join(" ")
            );
        }
    }
    // `OFAST_DUMP=file.tsv`: every living player-tick with a recipe, one line each: tick, controller, name, world
    // velocity, body yaw, on the ground, walking, duck amount, on a ladder, the weapon set, CS2's samples
    // (`index@time`, `=clip` where the table names it, `,` between) and the clips OFAST bakes (`,` between).
    if let Ok(path) = std::env::var("OFAST_DUMP") {
        use std::io::Write;
        let baked = baked_clips(&track, &rig.sets, rig.upper.as_ref(), rig.graphs.as_ref());
        let stem = |c: &str| {
            c.rsplit('/')
                .next()
                .unwrap_or(c)
                .trim_end_matches(".vnmclip")
                .to_owned()
        };
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).expect("dump file"));
        for sample in &track.samples {
            for p in sample.players.iter().filter(|p| p.alive) {
                let Some(decoded) = p
                    .rig
                    .pose_recipe
                    .as_ref()
                    .and_then(|r| recipe::decode(r).ok())
                else {
                    continue;
                };
                let cs2: Vec<String> = decoded
                    .tasks
                    .iter()
                    .filter_map(|t| match t {
                        Task::Sample { resource, time } => Some(format!(
                            "{resource}@{time:.3}{}",
                            table
                                .path(*resource)
                                .map_or(String::new(), |p| format!("={}", stem(p)))
                        )),
                        _ => None,
                    })
                    .collect();
                let ours: Vec<String> = baked
                    .get(&(sample.tick, p.controller))
                    .map(|c| c.iter().map(|c| stem(c)).collect())
                    .unwrap_or_default();
                let item = p.rig.weapon.as_ref().map(|w| w.item);
                let set = rig
                    .upper
                    .as_ref()
                    .and_then(|u| u.weapons.get(&item.unwrap_or(0)))
                    .map_or(WeaponSet::for_item(item), |w| {
                        WeaponSet::for_category(&w.category)
                    });
                let v = p.rig.velocity;
                writeln!(
                    out,
                    "{}\t{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{}\t{}\t{:.2}\t{}\t{set:?}\t{}\t{}",
                    sample.tick,
                    p.controller,
                    p.name,
                    v[0],
                    v[1],
                    v[2],
                    p.rig.body_yaw,
                    u8::from(p.rig.on_ground),
                    u8::from(p.rig.walking),
                    p.rig.duck_amount,
                    u8::from(p.rig.on_ladder),
                    cs2.join(","),
                    ours.join(",")
                )
                .expect("dump writes");
            }
        }
    }
    if let Ok(list) = std::env::var("OFAST_INDEX") {
        let indices: Vec<u16> = list
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let baked = baked_clips(&track, &rig.sets, rig.upper.as_ref(), rig.graphs.as_ref());
        for index in indices {
            explain(&track, &baked, &rig, index);
        }
    }
}

/// `OFAST_INDEX`: what goes with one index on its player-ticks: the weapons in hand, the clips OFAST bakes (with
/// their Jaccard index against it), how its time restarts against the player's shots, and the blends it feeds.
fn explain(
    track: &ofast_core::demo::players::RoundTrack,
    baked: &std::collections::HashMap<(u32, u32), BTreeSet<&str>>,
    rig: &ofast_core::export::players::PlayerRig,
    index: u16,
) {
    let mut ticks = 0usize;
    let mut weapons: BTreeMap<String, usize> = BTreeMap::new();
    // The weapon's gameplay state (`m_iWeaponGameplayAnimState`) and reload flag on its ticks.
    let mut states: BTreeMap<(i64, bool), usize> = BTreeMap::new();
    let mut with: BTreeMap<&str, usize> = BTreeMap::new();
    let mut clip_ticks: BTreeMap<&str, usize> = BTreeMap::new();
    let mut feeds: BTreeMap<String, usize> = BTreeMap::new();
    let mut alive = (0usize, 0usize);
    // Restarts (its time going back, or it starting) and how far each is from the player's nearest shot.
    let mut restarts: Vec<Option<i64>> = Vec::new();
    let mut last: BTreeMap<u32, (u32, f32)> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            if let Some(clips) = baked.get(&(sample.tick, p.controller)) {
                for &c in clips {
                    *clip_ticks.entry(c).or_default() += 1;
                }
            }
            let Some(decoded) = p
                .rig
                .pose_recipe
                .as_ref()
                .and_then(|r| recipe::decode(r).ok())
            else {
                continue;
            };
            let at: Vec<usize> = decoded
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| matches!(t, Task::Sample { resource, .. } if *resource == index))
                .map(|(k, _)| k)
                .collect();
            let Some(&first) = at.first() else {
                continue;
            };
            ticks += 1;
            if p.alive {
                alive.0 += 1;
            } else {
                alive.1 += 1;
                if std::env::var_os("OFAST_DEAD").is_some() && alive.1 % 200 == 1 {
                    println!(
                        "  dead: {} tick {} recipe tick {} pos {:?} tasks {:?}",
                        p.name, sample.tick, decoded.tick, p.position, decoded.tasks
                    );
                }
            }
            let item = p.rig.weapon.as_ref().map(|w| w.item);
            let name = rig
                .upper
                .as_ref()
                .and_then(|u| u.weapons.get(&item.unwrap_or(0)))
                .map_or_else(
                    || format!("item {}", item.map_or(-1, i64::from)),
                    |w| w.weapon.trim_start_matches("weapon_").to_owned(),
                );
            *weapons.entry(name).or_default() += 1;
            if let Some(w) = &p.rig.weapon {
                *states.entry((w.anim_state, w.reloading)).or_default() += 1;
            }
            if let Some(clips) = baked.get(&(sample.tick, p.controller)) {
                for &c in clips {
                    *with.entry(c).or_default() += 1;
                }
            }
            // The task that uses it: its kind, which input, weight and mask.
            for (k, node) in decoded.topology.iter().enumerate() {
                if let Some(slot) = node.inputs.iter().position(|&i| usize::from(i) == first) {
                    let what = match decoded.tasks.get(k) {
                        Some(Task::Blend { kind, mask, .. }) => {
                            format!(
                                "blend {kind} input {slot}{}",
                                if mask.is_empty() { "" } else { " masked" }
                            )
                        }
                        _ => format!("type {} input {slot}", node.kind),
                    };
                    *feeds.entry(what).or_default() += 1;
                }
            }
            let Task::Sample { time, .. } = decoded.tasks[first] else {
                continue;
            };
            let previous = last
                .get(&p.controller)
                .filter(|(t, _)| *t + 1 == sample.tick);
            if previous.is_none_or(|&(_, then)| time < then) {
                let shot = track
                    .shots
                    .iter()
                    .filter(|(c, _)| *c == p.controller)
                    .map(|(_, s)| i64::from(s.tick) - i64::from(sample.tick))
                    .min_by_key(|d| d.abs());
                restarts.push(shot);
                // `OFAST_STARTS=n`: the first n starts, with the body-relative velocity (forward, left) 8 ticks
                // before, at the start and 8 after, and what OFAST bakes then.
                let shown = std::env::var("OFAST_STARTS")
                    .ok()
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);
                if restarts.len() <= shown {
                    let at = |t: u32| {
                        let s = track.samples.iter().find(|s| s.tick == t)?;
                        let q = s.players.iter().find(|q| q.controller == p.controller)?;
                        let (sin, cos) = q.rig.body_yaw.to_radians().sin_cos();
                        let v = q.rig.velocity;
                        Some((
                            v[0] * cos + v[1] * sin,
                            -v[0] * sin + v[1] * cos,
                            q.rig.walking,
                            q.rig.duck_amount,
                        ))
                    };
                    let show = |t: u32| {
                        at(t).map_or("-".to_owned(), |(f, l, w, d)| {
                            format!(
                                "({f:4.0},{l:4.0}{}{})",
                                if w { " walk" } else { "" },
                                if d > 0.0 {
                                    format!(" duck {d:.1}")
                                } else {
                                    String::new()
                                }
                            )
                        })
                    };
                    let stems = |t: u32| -> String {
                        baked
                            .get(&(t, p.controller))
                            .map(|c| {
                                c.iter()
                                    .filter(|c| !c.contains("idle_") && !c.contains("breathing"))
                                    .map(|c| {
                                        c.rsplit('/')
                                            .next()
                                            .unwrap_or(c)
                                            .trim_end_matches(".vnmclip")
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .unwrap_or_default()
                    };
                    println!(
                        "  start {} tick {}: {} → {} → {}; baked {} | +4: {} | +12: {}",
                        p.name,
                        sample.tick,
                        show(sample.tick.saturating_sub(8)),
                        show(sample.tick),
                        show(sample.tick + 8),
                        stems(sample.tick),
                        stems(sample.tick + 4),
                        stems(sample.tick + 12)
                    );
                }
            }
            last.insert(p.controller, (sample.tick, time));
        }
    }
    println!(
        "\nindex {index}: {ticks} player-ticks ({} alive, {} dead)",
        alive.0, alive.1
    );
    let mut weapons: Vec<_> = weapons.into_iter().collect();
    weapons.sort_by_key(|w| std::cmp::Reverse(w.1));
    let top: Vec<String> = weapons
        .iter()
        .take(8)
        .map(|(w, n)| format!("{w} {n}"))
        .collect();
    println!("  in hand: {}", top.join(", "));
    let mut states: Vec<_> = states.into_iter().collect();
    states.sort_by_key(|s| std::cmp::Reverse(s.1));
    let top: Vec<String> = states
        .iter()
        .take(6)
        .map(|((s, r), n)| format!("{s}{} {n}", if *r { " reloading" } else { "" }))
        .collect();
    println!("  weapon states: {}", top.join(", "));
    let mut with: Vec<(&str, usize)> = with.into_iter().collect();
    with.sort_by_key(|w| std::cmp::Reverse(w.1));
    for (clip, n) in with.iter().take(10) {
        let either = ticks + clip_ticks[clip] - n;
        println!(
            "  baked with it: {:44} on {n:5} of its ticks ({:3.0} %), Jaccard {:.2}",
            clip.rsplit('/').next().unwrap_or(clip),
            *n as f64 * 100.0 / ticks.max(1) as f64,
            *n as f64 / either.max(1) as f64
        );
    }
    let mut feeds: Vec<_> = feeds.into_iter().collect();
    feeds.sort_by_key(|f| std::cmp::Reverse(f.1));
    let top: Vec<String> = feeds
        .iter()
        .take(5)
        .map(|(w, n)| format!("{w} ({n})"))
        .collect();
    println!("  feeds: {}", top.join(", "));
    let near = |d: i64| {
        restarts
            .iter()
            .filter(|s| s.is_some_and(|s| s.abs() <= d))
            .count()
    };
    let mut offsets: BTreeMap<i64, usize> = BTreeMap::new();
    for s in restarts.iter().flatten().filter(|s| s.abs() <= 4) {
        *offsets.entry(*s).or_default() += 1;
    }
    println!(
        "  starts {}: a shot of the player's within 0 ticks {}, 2 ticks {}, 8 ticks {}; by offset (shot tick − start) {:?}",
        restarts.len(),
        near(0),
        near(2),
        near(8),
        offsets
    );
}
