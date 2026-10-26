//! Developer tool: which clip each pose-recipe resource index stands for, found by co-occurrence. For every player
//! moving on the ground whose recipe starts with a locomotion blend-space triangle (tasks: sample, sample, blend of
//! the two, sample, blend: three clips at one time, [`ofast_core::demo::recipe`]), the triangle's three resource
//! indices are set against the locomotion clips OFAST's own bake blends for that tick. Each index lists the clips
//! with the highest summed blend weight over the ticks it was seen (and on how many ticks each came with it).
//!
//! ```bash
//! cargo run --release --example recipe_map -- path/to/match.dem 5
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ofast_core::anim::bake::{WeaponSet, body_relative};
use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::entity::handle_to_index;
use ofast_core::demo::players::track_round;
use ofast_core::demo::recipe::{self, RecipePaths, Task};
use ofast_core::export::players::load_player_rig;
use ofast_core::game::Cs2Install;

/// Each player's locomotion triangle (three resource indices) by (tick, name) over `first ..= last`.
fn triangles(data: &[u8], first: u32, last: u32) -> HashMap<(u32, String), [u16; 3]> {
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
            let Ok(decoded) = recipe::decode(&r) else {
                continue;
            };
            // Only a list that starts as the triangle: 1, 1, 7(0, 1), 1, 7(2, 3).
            let kinds: Vec<(u8, &[u8])> = decoded
                .topology
                .iter()
                .take(5)
                .map(|t| (t.kind, t.inputs.as_slice()))
                .collect();
            if kinds != [(1, &[][..]), (1, &[]), (7, &[0, 1]), (1, &[]), (7, &[2, 3])] {
                continue;
            }
            let resource = |i: usize| match decoded.tasks.get(i) {
                Some(Task::Sample { resource, .. }) => Some(*resource),
                _ => None,
            };
            if let (Some(a), Some(b), Some(c)) = (resource(0), resource(1), resource(3)) {
                out.insert((tick, name.clone()), [a, b, c]);
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let track = track_round(&data, round).expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let rig = load_player_rig(&content, &track).expect("rig");
    let first = track.samples.first().map_or(0, |s| s.tick);
    let last = track.samples.last().map_or(0, |s| s.tick);
    let triangles = triangles(&data, first, last);
    // Resource index → clip path → (summed weight, ticks seen together).
    let mut seen: BTreeMap<u16, BTreeMap<String, (f32, u32)>> = BTreeMap::new();
    let mut ticks: BTreeMap<u16, u32> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            let speed = p.rig.velocity[0].hypot(p.rig.velocity[1]);
            if !p.alive || !p.rig.on_ground || speed < 40.0 {
                continue;
            }
            let Some(indices) = triangles.get(&(sample.tick, p.name.clone())) else {
                continue;
            };
            let item = p.rig.weapon.as_ref().map(|w| w.item);
            let set = rig
                .upper
                .as_ref()
                .and_then(|u| u.weapons.get(&item.unwrap_or(0)))
                .map_or(WeaponSet::for_item(item), |w| {
                    WeaponSet::for_category(&w.category)
                });
            let locomotion = rig.sets.get(set);
            let blend = locomotion.blend(
                body_relative(p.rig.velocity, p.rig.body_yaw),
                p.rig.duck_amount,
            );
            for &index in indices.iter().collect::<BTreeSet<_>>() {
                *ticks.entry(index).or_default() += 1;
                for &(clip, weight) in &blend.clips {
                    let entry = seen
                        .entry(index)
                        .or_default()
                        .entry(locomotion.clips[clip].0.clone())
                        .or_default();
                    entry.0 += weight;
                    entry.1 += 1;
                }
            }
        }
    }
    for (index, clips) in &seen {
        let mut ranked: Vec<(&String, &(f32, u32))> = clips.iter().collect();
        ranked.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
        let top: Vec<String> = ranked
            .iter()
            .take(3)
            .map(|(path, (score, n))| {
                let stem = path.rsplit('/').next().unwrap_or(path);
                format!("{stem} {score:.1}/{n}")
            })
            .collect();
        println!("{index:4} ({:4} ticks): {}", ticks[index], top.join(", "));
    }
}
