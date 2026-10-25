//! Developer tool: check how OFAST rebuilds a shot's spread against the bullets whose true path a demo shows.
//!
//! ```bash
//! cargo run --release --example shot_spread -- path/to/match.dem [count]
//! ```
//!
//! Glass a bullet breaks arrives as its own message (packet message 423), with the point where the bullet met the
//! pane (field 11) and its direction (field 12): an exact point on the bullet's path. For each such pane, the shot
//! of the same tick whose aim passes nearest is the one that broke it; the tool prints the offset the bullet really
//! had from the shot's aim (as tangents along the shot's right and up vectors, in milliradians) next to what
//! [`ofast_core::demo::shots::spread_offset`] gives from the shot's seed, under a few variants of the seeding and of
//! the order of the random draws (the variant that matches to the last digit is CS2's).

use ofast_core::demo::DemoParser;
use ofast_core::demo::shots::{
    FIRE_BULLETS, MsgVector, Shot, UniformRandom, angle_vectors, decode_fire_bullets, spread_offset,
};
use prost::Message;

/// Packet message id of a breaking pane of glass (inferred from the demos: it arrives on the tick of a shot, at a
/// point on the shot's path).
const GLASS: i32 = 423;

#[derive(Clone, PartialEq, Message)]
struct Glass {
    /// Where the bullet met the pane.
    #[prost(message, optional, tag = "11")]
    point: Option<MsgVector>,
    /// The bullet's direction.
    #[prost(message, optional, tag = "12")]
    direction: Option<MsgVector>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The bullet's offset from the shot's aim as (right, up) tangents.
fn observed(shot: &Shot, point: [f32; 3]) -> [f32; 2] {
    let [f, r, u] = angle_vectors(shot.angles);
    let v: [f32; 3] = std::array::from_fn(|k| point[k] - shot.eye[k]);
    let along = dot(v, f);
    [dot(v, r) / along, dot(v, u) / along]
}

/// Four draws in a given order: (fraction of inaccuracy, its angle, fraction of spread, its angle); the spread
/// scaled by `spread_scale`.
fn variant(seed: i32, order: [usize; 4], spread_scale: f32, shot: &Shot) -> [f32; 2] {
    let mut random = UniformRandom::new(seed);
    let mut draws = [0.0f32; 4];
    for slot in order {
        let (low, high) = if slot % 2 == 0 {
            (0.0, 1.0)
        } else {
            (0.0, std::f32::consts::TAU)
        };
        draws[slot] = random.float(low, high);
    }
    let (i, s) = (
        draws[0] * shot.inaccuracy,
        draws[2] * shot.spread * spread_scale,
    );
    [
        draws[1].cos() * i + draws[3].cos() * s,
        draws[1].sin() * i + draws[3].sin() * s,
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(args.first().expect("demo path")).expect("demo reads");
    let count: usize = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(20);
    let mut parser = DemoParser::new(&data).expect("demo header");
    parser.keep_messages(&[FIRE_BULLETS, GLASS]);
    let (mut shots, mut panes) = (Vec::new(), Vec::new());
    while parser.next_frame().expect("frame").is_some() {
        parser.take_events();
        for m in parser.take_messages() {
            if m.kind == FIRE_BULLETS {
                shots.extend(decode_fire_bullets(m.tick, &m.body));
            } else if let Ok(glass) = Glass::decode(m.body.as_slice())
                && let Some(point) = glass.point
            {
                panes.push((m.tick, point.to_array()));
            }
        }
    }
    println!("{} shots, {} panes of glass", shots.len(), panes.len());

    // (name, draw order, seed offset, spread scale)
    let variants: [(&str, [usize; 4], i32, f32); 7] = [
        ("r a r a +0", [0, 1, 2, 3], 0, 1.0),
        ("a r a r +1", [1, 0, 3, 2], 1, 1.0),
        ("r a r a +1", [0, 1, 2, 3], 1, 1.0),
        ("r a r a +1 no spread", [0, 1, 2, 3], 1, 0.0),
        ("r a r a +1 spread/2", [0, 1, 2, 3], 1, 0.5),
        ("r a a r +1", [0, 1, 3, 2], 1, 1.0),
        ("r a r a +1 spread x2", [0, 1, 2, 3], 1, 2.0),
    ];
    let mut shown = 0;
    let mut best = std::collections::BTreeMap::<String, (usize, f32)>::new();
    for (tick, point) in &panes {
        // The shot of that tick whose aim passes nearest the pane, if within its inaccuracy and spread.
        let Some((shot, seen)) = shots
            .iter()
            .filter(|s| s.tick == *tick)
            .map(|s| (s, observed(s, *point)))
            .filter(|(s, o)| o[0].hypot(o[1]) <= (s.inaccuracy + s.spread) * 1.05 + 1e-4)
            .min_by(|a, b| a.1[0].hypot(a.1[1]).total_cmp(&b.1[0].hypot(b.1[1])))
        else {
            continue;
        };
        let distance = (0..3)
            .map(|k| (point[k] - shot.eye[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        let mut line = format!(
            "tick {tick:6} d {distance:5.0} item {:3} mode {} seed {:10} inacc {:.4} spread {:.4}  seen ({:7.3}, {:7.3}) mrad",
            shot.item,
            shot.mode,
            shot.seed,
            shot.inaccuracy,
            shot.spread,
            seen[0] * 1e3,
            seen[1] * 1e3
        );
        let rule = spread_offset(shot.seed, 0, shot.inaccuracy, shot.spread);
        let error = (rule[0] - seen[0]).hypot(rule[1] - seen[1]) * 1e3;
        let entry = best
            .entry("OFAST's spread_offset".to_owned())
            .or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += error;
        line += &format!(
            " | OFAST: {error:6.3} [{:?} {:?} {:?} {:?} {:?}]",
            shot.angles, shot.inaccuracy, shot.spread, seen[0], seen[1]
        );
        for (name, order, offset, spread_scale) in variants {
            let p = variant(
                (shot.seed as i32).wrapping_add(offset),
                order,
                spread_scale,
                shot,
            );
            let error = (p[0] - seen[0]).hypot(p[1] - seen[1]) * 1e3;
            let entry = best.entry(name.to_owned()).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += error;
            line += &format!(" | {name}: {error:6.3}");
        }
        if shown < count {
            println!("{line}");
            shown += 1;
        }
    }
    println!("\nmean error (mrad) per variant:");
    for (key, (n, total)) in best {
        println!("  {key:16} {:8.4} over {n}", total / n as f32);
    }
}
