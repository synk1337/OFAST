//! Developer tool: how a shotgun's pellets spread, checked against the pellets whose path a demo shows.
//!
//! ```bash
//! cargo run --release --example shot_pellets -- path/to/match.dem [rows to print]
//! ```
//!
//! A shotgun fires several pellets per shot (`m_nNumBullets`: the Nova 9, the XM1014 6, the MAG-7 and Sawed-Off 8).
//! Each hit on a player that starts at a shotgun shooter's eyes ([`hit_of`]) and each pane of glass broken on a shot's
//! tick lies on one pellet's path: the tool measures that pellet's offset from the shot's aim (as tangents along its
//! right and up vectors) and, under several rules for the pellets' spread, how near the nearest predicted pellet
//! comes (milliradians). Every shotgun in `scripts/weapons.vdata` has a fixed `m_nSpreadSeed`, which the other guns
//! leave at 0: a fixed pattern, as CS:GO's `weapon_accuracy_shotgun_spread_patterns` made it.

use std::collections::BTreeMap;

use ofast_core::demo::DemoParser;
use ofast_core::demo::shots::{
    EFFECT_DISPATCH, FIRE_BULLETS, MsgVector, PlayerHit, Shot, UniformRandom, angle_vectors,
    decode_fire_bullets, decode_player_hit, hit_of,
};
use prost::Message;

/// Packet message id of a breaking pane of glass (`examples/shot_spread.rs`).
const GLASS: i32 = 423;

#[derive(Clone, PartialEq, Message)]
struct Glass {
    /// Where the bullet met the pane.
    #[prost(message, optional, tag = "11")]
    point: Option<MsgVector>,
}

/// The shotguns: item definition index, pellets per shot, `m_nSpreadSeed` (from `scripts/weapons.vdata`).
const SHOTGUNS: [(u32, u32, i32); 4] = [
    (25, 6, 817_955),    // XM1014
    (27, 8, 19_899_236), // MAG-7
    (29, 8, 9_571_223),  // Sawed-Off
    (35, 9, 17_514),     // Nova
];

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A point's offset from the shot's aim as (right, up) tangents.
fn observed(shot: &Shot, point: [f32; 3]) -> [f32; 2] {
    let [f, r, u] = angle_vectors(shot.angles);
    let v: [f32; 3] = std::array::from_fn(|k| point[k] - shot.eye[k]);
    let along = dot(v, f);
    [dot(v, r) / along, dot(v, u) / along]
}

/// An offset from a fraction and an angle of a cone of `size`.
fn polar(fraction: f32, angle: f32, size: f32) -> [f32; 2] {
    [angle.cos() * fraction * size, angle.sin() * fraction * size]
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

/// Draws a fraction (0 – 1) and an angle (0 – 2π), in that order or the other.
fn draw(random: &mut UniformRandom, angle_first: bool) -> (f32, f32) {
    let tau = std::f32::consts::TAU;
    if angle_first {
        let a = random.float(0.0, tau);
        (random.float(0.0, 1.0), a)
    } else {
        let r = random.float(0.0, 1.0);
        (r, random.float(0.0, tau))
    }
}

/// The predicted offsets of all `pellets` of `shot` under rule `rule` (`pattern`: the gun's `m_nSpreadSeed`).
fn pellets(rule: usize, shot: &Shot, count: u32, pattern: i32) -> Vec<[f32; 2]> {
    let seed = shot.seed as i32;
    // The shot's inaccuracy, from its own stream (the first two draws), the same for every pellet.
    let shared = || {
        let mut random = UniformRandom::new(seed.wrapping_add(1));
        let (r, a) = draw(&mut random, false);
        polar(r, a, shot.inaccuracy)
    };
    (0..count)
        .map(|i| {
            let i = i as i32;
            match rule {
                // OFAST's now: every pellet its own stream, seed + 1 + pellet, inaccuracy then spread.
                0 => {
                    let mut random = UniformRandom::new(seed.wrapping_add(1).wrapping_add(i));
                    let (r1, a1) = draw(&mut random, false);
                    let a2 = random.float(0.0, std::f32::consts::TAU);
                    let r2 = random.float(0.0, 1.0);
                    add(polar(r1, a1, shot.inaccuracy), polar(r2, a2, shot.spread))
                }
                // The shot's inaccuracy for all; each pellet's spread from the pattern's stream, pattern + pellet.
                1 | 2 => {
                    let mut random = UniformRandom::new(pattern.wrapping_add(i));
                    let (r2, a2) = draw(&mut random, rule == 2);
                    add(shared(), polar(r2, a2, shot.spread))
                }
                // The same, the pattern's stream run on from pellet to pellet.
                3 | 4 => {
                    let mut random = UniformRandom::new(pattern);
                    let mut spread = (0.0, 0.0);
                    for _ in 0..=i {
                        spread = draw(&mut random, rule == 4);
                    }
                    add(shared(), polar(spread.0, spread.1, shot.spread))
                }
                // Each pellet's inaccuracy and spread from seed + 1 + pellet, the spread angle from the pattern.
                5 => {
                    let mut random = UniformRandom::new(seed.wrapping_add(1).wrapping_add(i));
                    let (r1, a1) = draw(&mut random, false);
                    let mut fixed = UniformRandom::new(pattern.wrapping_add(i));
                    let (r2, a2) = draw(&mut fixed, false);
                    add(polar(r1, a1, shot.inaccuracy), polar(r2, a2, shot.spread))
                }
                // CS:GO's loop with one density for both cones: r, θ0, θ1 from seed + 1 + pellet.
                6 => {
                    let mut random = UniformRandom::new(seed.wrapping_add(1).wrapping_add(i));
                    let (r, a1) = draw(&mut random, false);
                    let a2 = random.float(0.0, std::f32::consts::TAU);
                    add(polar(r, a1, shot.inaccuracy), polar(r, a2, shot.spread))
                }
                // The pattern alone (no inaccuracy), pattern + pellet, either order.
                7 | 8 => {
                    let mut fixed = UniformRandom::new(pattern.wrapping_add(i));
                    let (r2, a2) = draw(&mut fixed, rule == 8);
                    polar(r2, a2, shot.spread)
                }
                // Shared inaccuracy from the shot's stream; the pattern's points spread over a disc (√r).
                9 => {
                    let mut fixed = UniformRandom::new(pattern.wrapping_add(i));
                    let (r2, a2) = draw(&mut fixed, false);
                    add(shared(), polar(r2.sqrt(), a2, shot.spread))
                }
                // The shot's stream for the inaccuracy (two draws), then the pattern turned by a third draw.
                10 | 11 => {
                    let mut random = UniformRandom::new(seed.wrapping_add(1));
                    let (r1, a1) = draw(&mut random, false);
                    let turn = random.float(0.0, std::f32::consts::TAU);
                    let mut fixed = UniformRandom::new(pattern.wrapping_add(i));
                    let (r2, a2) = draw(&mut fixed, rule == 11);
                    add(
                        polar(r1, a1, shot.inaccuracy),
                        polar(r2, a2 + turn, shot.spread),
                    )
                }
                _ => [0.0; 2],
            }
        })
        .collect()
}

const RULES: [&str; 12] = [
    "own stream per pellet (OFAST now)",
    "shared inaccuracy, pattern + pellet (r a)",
    "shared inaccuracy, pattern + pellet (a r)",
    "shared inaccuracy, pattern stream on (r a)",
    "shared inaccuracy, pattern stream on (a r)",
    "own inaccuracy, pattern + pellet spread",
    "one density for both cones",
    "pattern only (r a)",
    "pattern only (a r)",
    "shared inaccuracy, pattern on a disc",
    "shared inaccuracy, pattern turned (r a)",
    "shared inaccuracy, pattern turned (a r)",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(args.first().expect("demo path")).expect("demo reads");
    let rows: usize = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(20);
    let mut parser = DemoParser::new(&data).expect("demo header");
    parser.keep_messages(&[FIRE_BULLETS, EFFECT_DISPATCH, GLASS]);
    let mut shots: Vec<Shot> = Vec::new();
    let mut hits: Vec<PlayerHit> = Vec::new();
    // Each pane of glass broken: the tick and where the pellet met it.
    let mut panes: Vec<(u32, [f32; 3])> = Vec::new();
    while parser.next_frame().expect("frame").is_some() {
        parser.take_events();
        for m in parser.take_messages() {
            if m.kind == FIRE_BULLETS {
                shots.extend(decode_fire_bullets(m.tick, &m.body));
            } else if m.kind == GLASS {
                if let Ok(glass) = Glass::decode(m.body.as_slice())
                    && let Some(point) = glass.point
                {
                    panes.push((m.tick, point.to_array()));
                }
            } else if let Some(hit) = decode_player_hit(m.tick, &m.body) {
                hits.push(hit);
            }
        }
    }
    hits.sort_by(|a, b| {
        (a.tick, a.point.map(f32::to_bits)).cmp(&(b.tick, b.point.map(f32::to_bits)))
    });
    hits.dedup();
    let gun = |shot: &Shot| SHOTGUNS.iter().find(|g| g.0 == shot.item).copied();
    let shotgun_shots: Vec<&Shot> = shots.iter().filter(|s| gun(s).is_some()).collect();
    println!(
        "{} shots, {} with a shotgun",
        shots.len(),
        shotgun_shots.len()
    );

    // Each pellet seen: its shot and its offset from the aim.
    let mut seen: Vec<(&Shot, [f32; 2], &str)> = Vec::new();
    for shot in &shotgun_shots {
        for hit in hits
            .iter()
            .filter(|h| h.start != h.point && hit_of(shot, h))
        {
            seen.push((shot, observed(shot, hit.point), "hit"));
        }
        for (_, point) in panes.iter().filter(|(t, _)| *t == shot.tick) {
            seen.push((shot, observed(shot, *point), "glass"));
        }
    }
    println!("{} pellets seen on shotgun shots", seen.len());
    let mut totals: BTreeMap<usize, [usize; 3]> = BTreeMap::new();
    for (k, (shot, offset, kind)) in seen.iter().enumerate() {
        let (_, count, pattern) = gun(shot).expect("a shotgun");
        let mut line = format!(
            "tick {:6} item {} seed {:10} inacc {:.4} spread {:.4} {kind:5} seen ({:7.2}, {:7.2}) mrad:",
            shot.tick,
            shot.item,
            shot.seed,
            shot.inaccuracy,
            shot.spread,
            offset[0] * 1e3,
            offset[1] * 1e3
        );
        for (rule, _) in RULES.iter().enumerate() {
            let best = pellets(rule, shot, count, pattern)
                .iter()
                .map(|p| (p[0] - offset[0]).hypot(p[1] - offset[1]) * 1e3)
                .fold(f32::INFINITY, f32::min);
            let t = totals.entry(rule).or_default();
            t[0] += 1;
            t[1] += usize::from(best < 0.05);
            t[2] += usize::from(best < 1.0);
            line += &format!(" {best:7.2}");
        }
        if k < rows {
            println!("{line}");
        }
    }
    println!("rule: pellets seen, within 0.05 mrad of a predicted pellet, within 1 mrad");
    for (rule, [all, exact, near]) in totals {
        println!("  {:46} {all:4} {exact:4} {near:4}", RULES[rule]);
    }
}
