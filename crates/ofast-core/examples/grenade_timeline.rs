//! Developer tool: the grenades and fires of a demo round as OFAST reads them ([`ofast_core::demo::grenades`]),
//! next to the round's grenade events (`grenade_thrown`, `*_detonate`, `smokegrenade_expired`, `inferno_*`), to check
//! them against each other and against CS2.
//!
//! ```bash
//! cargo run --release --example grenade_timeline -- path/to/match.dem 5
//! ```
//!
//! Per projectile: its kind, thrower, first and last tick in the demo, where it started and ended, bounces, and a
//! smoke's cloud (start tick, position, colour, voxel updates seen). Per fire: start tick, lifetime, the most flames
//! at once and when its entity left.

use std::collections::BTreeMap;

use ofast_core::demo::grenades::GrenadeKind;
use ofast_core::demo::players::track_round;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let track = track_round(&data, round).expect("round");
    let names: BTreeMap<u32, String> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .map(|p| (p.controller, p.name.clone()))
        .collect();

    // entity → (kind, thrower, first tick, last tick, first position, last position, bounces, smoke, voxel updates)
    #[allow(clippy::type_complexity)]
    let mut grenades: BTreeMap<
        u32,
        (
            GrenadeKind,
            Option<u32>,
            u32,
            u32,
            [f32; 3],
            [f32; 3],
            u32,
            Option<(u32, [f32; 3], [f32; 3])>,
            Vec<i64>,
        ),
    > = BTreeMap::new();
    #[allow(clippy::type_complexity)]
    let mut fires: BTreeMap<u32, (Option<u32>, f32, usize, u32, u32, bool)> = BTreeMap::new();
    for sample in &track.samples {
        for g in &sample.grenades {
            let entry = grenades.entry(g.entity).or_insert((
                g.kind,
                g.thrower,
                sample.tick,
                sample.tick,
                g.position,
                g.position,
                0,
                None,
                Vec::new(),
            ));
            entry.3 = sample.tick;
            entry.5 = g.position;
            entry.6 = g.bounces;
            if let Some(s) = g.smoke {
                entry.7.get_or_insert((s.start_tick, s.position, s.color));
                if entry.8.last() != Some(&s.voxel_update) {
                    entry.8.push(s.voxel_update);
                }
            }
        }
        for f in &sample.fires {
            let entry = fires.entry(f.entity).or_insert((
                f.start_tick,
                f.lifetime,
                0,
                sample.tick,
                sample.tick,
                f.incendiary,
            ));
            entry.2 = entry.2.max(f.flames.iter().filter(|fl| fl.burning).count());
            entry.4 = sample.tick;
        }
    }
    let fmt = |p: [f32; 3]| format!("({:.0}, {:.0}, {:.0})", p[0], p[1], p[2]);
    println!("grenades:");
    for (entity, (kind, thrower, first, last, from, to, bounces, smoke, updates)) in &grenades {
        let who = thrower
            .and_then(|t| names.get(&t))
            .map_or("?", String::as_str);
        let cloud = smoke.map_or(String::new(), |(start, pos, color)| {
            format!(
                ", cloud from {start} at {} colour {:?}, {} voxel updates ({:?} … {:?})",
                fmt(pos),
                color,
                updates.len(),
                updates.first(),
                updates.last()
            )
        });
        println!(
            "  #{entity} {:9} by {who:10} ticks {first}–{last}: {} → {}, {bounces} bounces{cloud}",
            kind.name(),
            fmt(*from),
            fmt(*to)
        );
    }
    println!("fires:");
    for (entity, (start, lifetime, most, first, last, incendiary)) in &fires {
        println!(
            "  #{entity} {} start {start:?}, lifetime {lifetime} s, up to {most} flames, entity ticks {first}–{last}",
            if *incendiary { "incendiary" } else { "molotov" }
        );
    }
    // Each player's flash fields (`m_flFlashDuration`, `m_flFlashMaxAlpha`) on every tick they change.
    println!("flashes (tick, player, duration s, max alpha):");
    let mut last: BTreeMap<u32, (f32, f32)> = BTreeMap::new();
    for sample in &track.samples {
        for p in &sample.players {
            let now = (p.rig.flash_duration, p.rig.flash_max_alpha);
            if last
                .insert(p.controller, now)
                .is_some_and(|before| before != now)
            {
                println!("  {:6} {} {:.2} {:.0}", sample.tick, p.name, now.0, now.1);
            }
        }
    }
    println!("events:");
    for e in &track.events {
        if e.name.contains("grenade")
            || e.name.contains("flashbang")
            || e.name.starts_with("inferno")
            || e.name == "player_blind"
        {
            let who = e
                .controller("userid")
                .and_then(|c| names.get(&c))
                .map_or("", String::as_str);
            let id = e
                .int("entityid")
                .map_or(String::new(), |i| format!(" #{i}"));
            let weapon = e.string("weapon").unwrap_or("");
            // A blinding: how long (seconds).
            let blind = match e.get("blind_duration") {
                Some(ofast_core::demo::events::EventValue::Float(d)) => format!(" blind {d:.2} s"),
                _ => String::new(),
            };
            println!("  {:6} {}{id} {who} {weapon}{blind}", e.tick, e.name);
        }
    }
}
