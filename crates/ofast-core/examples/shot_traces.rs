//! Developer tool: trace a demo's gun shots through the map's collision, and check the traces against the hits on
//! players the demo does record.
//!
//! ```bash
//! cargo run --release --example shot_traces -- path/to/match.dem [shots to print]
//! ```
//!
//! Each shot's first bullet ([`ofast_core::demo::shots`]) is traced through the map's world collision as bullets
//! see it ([`ofast_core::physics::CollisionAttribute::stops_bullets`]). A bullet that hit a player arrives as an
//! effect dispatch (packet message 400) starting at the shooter's eyes and ending where it met the player: the
//! world must not stop that bullet's trace before the player, except where the bullet went through a wall (CS2's
//! penetration, not traced yet). The tool counts both, and prints the first shots: where they ended.

use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::shots::{
    EFFECT_DISPATCH, FIRE_BULLETS, bullet_direction, decode_fire_bullets, decode_player_hit,
    hit_of, spread_offset,
};
use ofast_core::game::Cs2Install;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::WorldCollision;
use ofast_core::resource::Resource;

/// How far a bullet is traced (inches): CS2's rifles reach 8192.
const REACH: f32 = 8192.0;

/// A protobuf message's fields as text, `tag:value`, without its schema: varints as integers, fixed32 as an integer
/// and a float, length-delimited fields as their bytes' length (nested messages decoded the same way when they parse).
fn wire_fields(body: &[u8]) -> String {
    fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *b.get(*at)?;
            *at += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let Some(key) = varint(body, &mut at) else {
            break;
        };
        let (tag, kind) = (key >> 3, key & 7);
        let value = match kind {
            0 => varint(body, &mut at).map(|v| v.to_string()),
            5 => body.get(at..at + 4).map(|b| {
                at += 4;
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                format!("{bits}/{}", f32::from_bits(bits))
            }),
            1 => body.get(at..at + 8).map(|b| {
                at += 8;
                format!("{:?}", b)
            }),
            2 => varint(body, &mut at).and_then(|n| {
                let n = n as usize;
                let inner = body.get(at..at + n)?;
                at += n;
                Some(format!("{{{}}}", wire_fields(inner)))
            }),
            _ => None,
        };
        match value {
            Some(v) => out.push(format!("{tag}:{v}")),
            None => break,
        }
    }
    out.join(" ")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(args.first().expect("demo path")).expect("demo reads");
    let print: usize = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(10);
    let mut parser = DemoParser::new(&data).expect("demo header");
    let map = parser.header().map_name().to_owned();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &map).expect("map archive opens");
    let bytes = content
        .read_resource(&format!("maps/{map}/world_physics.vmdl"))
        .expect("world physics reads");
    let world = PhysicsData::from_resource(&Resource::parse(&bytes).expect("parses"))
        .expect("PHYS reads")
        .expect("a PHYS block");
    let collision = WorldCollision::from_physics_where(&world, |a| a.stops_bullets());

    parser.keep_messages(&[FIRE_BULLETS, EFFECT_DISPATCH]);
    let (mut shots, mut hits) = (Vec::new(), Vec::new());
    // OFAST_RAW=n: every field of the first n fire-bullets messages, as the wire has them.
    let mut raw: usize = std::env::var("OFAST_RAW")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    // OFAST_HITS=first,last: every field of the effect dispatches and the player_hurt events of those ticks.
    let hits_in: Option<(u32, u32)> = std::env::var("OFAST_HITS").ok().map(|v| {
        let (a, b) = v.split_once(',').expect("OFAST_HITS: first,last");
        (
            a.trim().parse().expect("tick"),
            b.trim().parse().expect("tick"),
        )
    });
    let within = |tick: u32| hits_in.is_some_and(|(a, b)| (a..=b).contains(&tick));
    while parser.next_frame().expect("frame").is_some() {
        for e in parser.take_events() {
            if e.name == "player_hurt" && within(e.tick) {
                println!("tick {}: player_hurt {:?}", e.tick, e.keys);
            }
        }
        for m in parser.take_messages() {
            if m.kind == FIRE_BULLETS && raw > 0 {
                raw -= 1;
                println!("tick {}: {}", m.tick, wire_fields(&m.body));
            }
            if m.kind == EFFECT_DISPATCH && within(m.tick) {
                println!("tick {}: effect {}", m.tick, wire_fields(&m.body));
            }
            if m.kind == FIRE_BULLETS {
                shots.extend(decode_fire_bullets(m.tick, &m.body));
            } else if let Some(hit) = decode_player_hit(
                m.tick,
                &m.body,
                parser
                    .string_tables()
                    .by_name(ofast_core::demo::shots::EFFECT_TABLE),
            ) {
                hits.push(hit);
            }
        }
    }
    println!(
        "{map}: {} shots, {} effect dispatches",
        shots.len(),
        hits.len()
    );
    // The names behind the dispatches' effect numbers (field 18).
    if hits_in.is_some()
        && let Some(table) = parser.string_tables().by_name("EffectDispatch")
    {
        let names: Vec<String> = table
            .entries
            .iter()
            .enumerate()
            .map(|(n, e)| format!("{n}:{}", e.key))
            .collect();
        println!("EffectDispatch: {}", names.join(" "));
    }

    let near: Option<Vec<f32>> = std::env::var("OFAST_NEAR").ok().map(|v| {
        v.split(',')
            .map(|c| c.trim().parse().expect("OFAST_NEAR: x,y,z,r"))
            .collect()
    });
    let started = std::time::Instant::now();
    let (mut checked, mut clear, mut blocked) = (0, 0, Vec::new());
    let mut off_line: Vec<f32> = Vec::new();
    for (n, shot) in shots.iter().enumerate() {
        let direction = bullet_direction(
            shot.angles,
            spread_offset(shot.seed, 0, shot.inaccuracy, shot.spread),
        );
        let hit = collision.ray_hit(shot.eye, direction, REACH);
        // OFAST_NEAR="x,y,z,r": the shots whose bullet ends within r of that point (to find which shot made a bullet
        // hole a CS2 screenshot shows).
        if let Some(near) = &near
            && let Some(h) = hit
        {
            let end: [f32; 3] = std::array::from_fn(|k| shot.eye[k] + direction[k] * h.distance);
            if (0..3)
                .map(|k| (end[k] - near[k]).powi(2))
                .sum::<f32>()
                .sqrt()
                <= near[3]
            {
                println!(
                    "near: tick {} pawn {} item {} from {:?} ends at {:?}",
                    shot.tick,
                    shot.pawn,
                    shot.item,
                    shot.eye.map(|c| c.round()),
                    end.map(|c| (c * 10.0).round() / 10.0)
                );
            }
        }
        if n < print {
            println!(
                "tick {:6} item {:3} from {:?}: {}",
                shot.tick,
                shot.item,
                shot.eye.map(|c| c.round()),
                hit.map_or("nothing within reach".to_owned(), |h| format!(
                    "world at {:.1} ({:?}, normal {:?})",
                    h.distance,
                    std::array::from_fn::<f32, 3, _>(
                        |k| (shot.eye[k] + direction[k] * h.distance).round()
                    ),
                    h.normal.map(|c| (c * 100.0).round() / 100.0)
                ))
            );
        }
        // The hit on a player this shot made, if any: same tick, starting at its eyes.
        let Some(on_player) = hits.iter().find(|h| hit_of(shot, h)).map(|h| h.point) else {
            continue;
        };
        checked += 1;
        let distance = (0..3)
            .map(|k| (on_player[k] - shot.eye[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        // How far the hit lies from the bullet's line.
        let along: f32 = (0..3)
            .map(|k| (on_player[k] - shot.eye[k]) * direction[k])
            .sum();
        let off = (0..3)
            .map(|k| (on_player[k] - shot.eye[k] - direction[k] * along).powi(2))
            .sum::<f32>()
            .sqrt();
        off_line.push(off);
        match hit {
            Some(h) if h.distance < distance - 1.0 => {
                blocked.push((shot.tick, shot.item, h.distance, distance));
            }
            _ => clear += 1,
        }
    }
    println!("traced in {:.2?}", started.elapsed());
    off_line.sort_by(f32::total_cmp);
    if !off_line.is_empty() {
        let at = |q: f32| off_line[((off_line.len() - 1) as f32 * q) as usize];
        println!(
            "player hits off their bullet's line (inches): median {:.3}, 95% {:.3}, max {:.3}",
            at(0.5),
            at(0.95),
            at(1.0)
        );
    }
    println!(
        "{checked} shots hit a player: {clear} with nothing in the way, {} through the world (wall bangs, or traces \
         wrongly stopped):",
        blocked.len()
    );
    for (tick, item, wall, player) in blocked {
        println!("  tick {tick:6} item {item:3}: world at {wall:.1}, player at {player:.1}");
    }
}
