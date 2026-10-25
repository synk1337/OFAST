//! Developer tool: check bullets through walls (CS2's penetration) against what a demo recorded of each hit.
//!
//! ```bash
//! cargo run --release --example shot_penetration -- path/to/match.dem [rows to print]
//! ```
//!
//! For every bullet that hit a player (an effect dispatch, [`ofast_core::demo::shots::PlayerHit`]) the tool finds
//! its shot and pellet, the `bullet_damage` event of that tick whose direction is the bullet's (`num_penetrations`:
//! the walls it went through; `distance`) and the victim's `player_hurt` (damage to health and armour, hit group),
//! traces the bullet's line through the map's collision as bullets see it to the hit
//! ([`WorldCollision::ray_crossings`]: every surface it went into or came out of), and prints:
//!
//! - how often the first surface a bullet from a player's eyes meets is a front (the side a mesh triangle's winding
//!   faces): the winding is outward when it nearly always is;
//! - per hit, the walls the trace crosses (their thickness and surfaces) against `num_penetrations`;
//! - the damage [`ofast_core::export::shots::damage_at`] predicts for the hit against the event's: without walls the
//!   range falloff, hit groups and armour; through walls the penetration's losses.
//!
//! Needs a demo with `bullet_damage` and `player_hurt` (a local match demo: tournament demos lack `bullet_damage`).

use std::collections::HashMap;

use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::events::{EventValue, GameEvent};
use ofast_core::demo::shots::{
    EFFECT_DISPATCH, FIRE_BULLETS, PlayerHit, Shot, bullet_direction, decode_fire_bullets,
    decode_player_hit, hit_of, spread_offset,
};
use ofast_core::export::shots::{ShotData, Wall, damage_at, walls_on};
use ofast_core::game::Cs2Install;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::WorldCollision;
use ofast_core::resource::Resource;

fn float(e: &GameEvent, key: &str) -> Option<f32> {
    match e.get(key)? {
        EventValue::Float(v) => Some(*v),
        EventValue::Int(v) => Some(*v as f32),
        _ => None,
    }
}

/// One hit as the demo has it, and as OFAST traces it.
struct Row {
    tick: u32,
    weapon: String,
    distance: f32,
    demo_distance: Option<f32>,
    penetrations: Option<i64>,
    walls: Vec<Wall>,
    hitgroup: i64,
    dmg_health: i64,
    dmg_armor: i64,
    health_after: i64,
    armor_before: i64,
    predicted: Option<(i64, i64)>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(args.first().expect("demo path")).expect("demo reads");
    let print: usize = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(40);
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
    let items = content
        .read("scripts/items/items_game.txt")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let mut warnings = Vec::new();
    let shot_data = ShotData::load(&content, items.as_deref(), &mut warnings);
    for w in &warnings {
        println!("warning: {w}");
    }

    parser.keep_messages(&[FIRE_BULLETS, EFFECT_DISPATCH]);
    let (mut shots, mut hits, mut events): (Vec<Shot>, Vec<PlayerHit>, Vec<GameEvent>) =
        (Vec::new(), Vec::new(), Vec::new());
    while parser.next_frame().expect("frame").is_some() {
        events.extend(
            parser
                .take_events()
                .into_iter()
                .filter(|e| e.name == "bullet_damage" || e.name == "player_hurt"),
        );
        for m in parser.take_messages() {
            if m.kind == FIRE_BULLETS {
                shots.extend(decode_fire_bullets(m.tick, &m.body));
            } else if let Some(hit) = decode_player_hit(m.tick, &m.body) {
                hits.push(hit);
            }
        }
    }
    let mut by_tick: HashMap<u32, Vec<&GameEvent>> = HashMap::new();
    for e in &events {
        by_tick.entry(e.tick).or_default().push(e);
    }
    // OFAST_TICK=n: everything the demo has of that tick's shots.
    if let Some(tick) = std::env::var("OFAST_TICK")
        .ok()
        .and_then(|t| t.parse::<u32>().ok())
    {
        for s in shots.iter().filter(|s| s.tick == tick) {
            println!("shot {s:?}");
        }
        for h in hits.iter().filter(|h| h.tick == tick) {
            println!("dispatch {h:?}");
        }
        for e in by_tick.get(&tick).into_iter().flatten() {
            println!("event {} {:?}", e.name, e.keys);
        }
    }
    println!(
        "{map}: {} shots, {} effect dispatches, {} bullet_damage, {} player_hurt",
        shots.len(),
        hits.len(),
        events.iter().filter(|e| e.name == "bullet_damage").count(),
        events.iter().filter(|e| e.name == "player_hurt").count()
    );
    for name in ["bullet_damage", "player_hurt"] {
        if let Some(e) = events.iter().find(|e| e.name == name) {
            println!("{name} keys: {:?}", e.keys);
        }
    }

    // The winding: the first crossing of each shot's first bullet (the eyes are never inside a wall).
    let (mut fronts, mut backs) = (0, 0);
    for shot in &shots {
        let direction = bullet_direction(
            shot.angles,
            spread_offset(shot.seed, 0, shot.inaccuracy, shot.spread),
        );
        if let Some(first) = collision.ray_crossings(shot.eye, direction, 8192.0).first() {
            if first.entering {
                fronts += 1;
            } else {
                backs += 1;
            }
        }
    }
    println!(
        "first surface met from the eyes: {fronts} fronts, {backs} backs ({:.2} % fronts)",
        100.0 * fronts as f32 / (fronts + backs).max(1) as f32
    );

    // Some hits come in two dispatches (the same tick, start and point): one each.
    hits.sort_by(|a, b| {
        (a.tick, a.point.map(f32::to_bits)).cmp(&(b.tick, b.point.map(f32::to_bits)))
    });
    hits.dedup();
    // A hit's dispatch starts at the shooter's eyes, or where the bullet came out of the last wall it went through;
    // the other dispatches on a bullet's line start where they end (another effect).
    hits.retain(|h| h.start != h.point);
    let surfaces = world.surfaces.as_slice();
    // The shot and pellet whose line a hit's start and point both lie on: its direction, the distances of both.
    let line_of = |hit: &PlayerHit| {
        shots
            .iter()
            .filter(|s| s.tick == hit.tick)
            .find_map(|shot| {
                let weapon = shot_data.weapon(shot.item)?;
                (0..weapon.bullets.max(1)).find_map(|bullet| {
                    let d = bullet_direction(
                        shot.angles,
                        spread_offset(shot.seed, bullet, shot.inaccuracy, shot.spread),
                    );
                    let on = |p: [f32; 3]| {
                        let v: [f32; 3] = std::array::from_fn(|k| p[k] - shot.eye[k]);
                        let t: f32 = (0..3).map(|k| v[k] * d[k]).sum();
                        let off = (0..3)
                            .map(|k| (v[k] - d[k] * t).powi(2))
                            .sum::<f32>()
                            .sqrt();
                        (t, off)
                    };
                    let ((ts, os), (tp, op)) = (on(hit.start), on(hit.point));
                    (os < 0.5 && op < 0.5 && ts >= 0.0 && tp > ts)
                        .then_some((shot, weapon, d, ts, tp))
                })
            })
    };
    let mut rows = Vec::new();
    let mut exits = Vec::new();
    for hit in &hits {
        let Some((shot, weapon, direction, start, distance)) = line_of(hit) else {
            continue;
        };
        let walls = walls_on(
            &collision, surfaces, &shot_data, shot.eye, direction, distance,
        );
        // Where the trace has the bullet come out of the last wall, against where the demo has it (a hit after a
        // wall).
        if !hit_of(shot, hit) {
            let exit = walls
                .last()
                .map_or(f32::INFINITY, |w| w.distance + w.thickness - start);
            exits.push(exit);
            if exit.abs() > 0.5 {
                // Not out of a traced wall: out of a player hit before it on the line?
                let before: Vec<String> = hits
                    .iter()
                    .filter(|h| h.tick == hit.tick && h.point != hit.point)
                    .filter_map(|h| {
                        let v: [f32; 3] = std::array::from_fn(|k| h.point[k] - shot.eye[k]);
                        let t: f32 = (0..3).map(|k| v[k] * direction[k]).sum();
                        let off = (0..3)
                            .map(|k| (v[k] - direction[k] * t).powi(2))
                            .sum::<f32>()
                            .sqrt();
                        (off < 0.5).then(|| format!("{t:.2}"))
                    })
                    .collect();
                println!(
                    "tick {:6} item {:3}: starts {start:.2}, ends {distance:.2}, traced exit {exit:.2} before the \
                     start; hits on the line at {before:?}",
                    hit.tick, shot.item
                );
            }
        }
        let tick_events = by_tick.get(&hit.tick).cloned().unwrap_or_default();
        let bullet = tick_events.iter().find(|e| {
            e.name == "bullet_damage"
                && [
                    float(e, "damage_dir_x"),
                    float(e, "damage_dir_y"),
                    float(e, "damage_dir_z"),
                ]
                .iter()
                .zip(direction)
                .map(|(a, b)| a.unwrap_or(0.0) * b)
                .sum::<f32>()
                    > 0.9999
        });
        let weapon_name = shot_data
            .items
            .get(&shot.item)
            .map(|n| n.trim_start_matches("weapon_").to_owned())
            .unwrap_or_default();
        let hurt = match bullet {
            Some(b) => tick_events.iter().find(|e| {
                e.name == "player_hurt"
                    && e.controller("userid") == b.controller("victim")
                    && e.controller("attacker") == b.controller("attacker")
            }),
            // Without `bullet_damage` (tournament demos): the tick's only hurt by this weapon, when this is the
            // tick's only hit.
            None => {
                let hurts: Vec<_> = tick_events
                    .iter()
                    .filter(|e| e.name == "player_hurt" && e.string("weapon") == Some(&weapon_name))
                    .collect();
                let tick_hits = hits
                    .iter()
                    .filter(|h| h.tick == hit.tick && h.point != hit.point)
                    .count()
                    + 1;
                (hurts.len() == 1 && tick_hits == 1).then(|| hurts[0])
            }
        };
        let int = |e: Option<&&GameEvent>, key: &str| e.and_then(|e| e.int(key)).unwrap_or(-1);
        let (hitgroup, dmg_health, dmg_armor) = (
            int(hurt, "hitgroup"),
            int(hurt, "dmg_health"),
            int(hurt, "dmg_armor"),
        );
        let (health_after, armor_after) = (int(hurt, "health"), int(hurt, "armor"));
        let armor_before = armor_after + dmg_armor.max(0);
        // A head hit on armour without armour damage: no helmet.
        let helmet = dmg_armor > 0 || hitgroup != 1;
        let predicted = hurt.and_then(|_| {
            let d = damage_at(weapon, &walls, distance)?;
            Some(ofast_core::export::shots::to_health(
                weapon,
                d,
                hitgroup as u32,
                armor_before.max(0) as f32,
                helmet,
            ))
        });
        rows.push(Row {
            tick: hit.tick,
            weapon: shot_data.items.get(&shot.item).cloned().unwrap_or_default(),
            distance,
            demo_distance: bullet.and_then(|b| float(b, "distance")),
            penetrations: bullet.and_then(|b| b.int("num_penetrations")),
            walls,
            hitgroup,
            dmg_health,
            dmg_armor,
            health_after,
            armor_before,
            predicted: predicted.map(|(h, a)| (h as i64, a as i64)),
        });
    }

    // Hits after a wall: where the trace's last wall ends against where the demo's hit starts.
    exits.sort_by(f32::total_cmp);
    let within = |d: f32| exits.iter().filter(|e| e.abs() <= d).count();
    println!(
        "\n{} hits start past the eyes (after a wall): the traced wall's exit within 0.1 in of the start {}, 0.5 in {}, \
         2 in {}; no wall traced {}",
        exits.len(),
        within(0.1),
        within(0.5),
        within(2.0),
        exits.iter().filter(|e| !e.is_finite()).count()
    );
    for e in exits.iter().filter(|e| e.abs() > 0.5) {
        print!("{e:.2} ");
    }
    println!();

    // Walls: OFAST's count against the demo's.
    let mut counts: HashMap<(usize, i64), usize> = HashMap::new();
    for row in &rows {
        *counts
            .entry((row.walls.len(), row.penetrations.unwrap_or(-1)))
            .or_default() += 1;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort();
    println!(
        "\n{} hits; (walls traced, num_penetrations): count",
        rows.len()
    );
    for ((walls, demo), n) in counts {
        println!("  ({walls}, {demo}): {n}");
    }

    // Damage: the hits that did not kill (a kill's damage is cut to the health left).
    let (mut exact, mut near, mut off) = ([0; 2], [0; 2], [0; 2]);
    let mut printed = 0;
    for row in rows.iter().filter(|r| r.health_after > 0) {
        let Some((health, armor)) = row.predicted else {
            continue;
        };
        let through = usize::from(!row.walls.is_empty());
        let miss = (health - row.dmg_health)
            .abs()
            .max((armor - row.dmg_armor).abs());
        match miss {
            0 => exact[through] += 1,
            1 => near[through] += 1,
            _ => off[through] += 1,
        }
        if (miss > 1 || !row.walls.is_empty()) && printed < print {
            printed += 1;
            println!(
                "tick {:6} {:18} {:7.1} (demo {:7.1}) hg {} armour {:3}: demo {:3}/{:2}, OFAST {:3}/{:2}; demo \
                 penetrations {:?}, walls {:?}",
                row.tick,
                row.weapon,
                row.distance,
                row.demo_distance.unwrap_or(-1.0),
                row.hitgroup,
                row.armor_before,
                row.dmg_health,
                row.dmg_armor,
                health,
                armor,
                row.penetrations,
                row.walls
                    .iter()
                    .map(|w| {
                        let name = |hash: u32| {
                            shot_data
                                .surfaces
                                .get(&hash)
                                .map_or("?", |(n, _)| n.as_str())
                        };
                        format!(
                            "{:.2} in at {:.0} ({} {} {:.2} → {} {} {:.2})",
                            w.thickness,
                            w.distance,
                            name(w.enter_surface),
                            w.enter.material,
                            w.enter.distance,
                            name(w.exit_surface),
                            w.exit.material,
                            w.exit.distance
                        )
                    })
                    .collect::<Vec<_>>()
            );
        }
    }
    println!(
        "\nnon-lethal hits, damage (health / armour) as the demo's: without walls {} exact, {} within 1, {} off; \
         through walls {} exact, {} within 1, {} off",
        exact[0], near[0], off[0], exact[1], near[1], off[1]
    );

    // Each single wall of one surface: the penetration modifiers that give the demo's damage exactly.
    println!(
        "\nsingle walls: surface, thickness, the file's modifier, the modifiers that give the demo's damage"
    );
    for row in rows.iter().filter(|r| {
        r.health_after > 0
            && r.walls.len() == 1
            && r.walls[0].enter_surface == r.walls[0].exit_surface
            && r.walls[0].enter.material != "W"
    }) {
        let Some(weapon) = shot_data
            .weapons
            .get(&row.weapon)
            .filter(|_| row.dmg_health >= 0)
        else {
            continue;
        };
        let fits: Vec<f32> = (1..=300)
            .map(|k| k as f32 / 100.0)
            .filter(|&m| {
                let mut wall = row.walls[0].clone();
                wall.enter.distance = m;
                wall.exit.distance = m;
                damage_at(weapon, &[wall], row.distance).is_some_and(|d| {
                    let helmet = row.dmg_armor > 0 || row.hitgroup != 1;
                    let (h, a) = ofast_core::export::shots::to_health(
                        weapon,
                        d,
                        row.hitgroup as u32,
                        row.armor_before as f32,
                        helmet,
                    );
                    (h as i64, a as i64) == (row.dmg_health, row.dmg_armor)
                })
            })
            .collect();
        let wall = &row.walls[0];
        println!(
            "  {:16} {:5.2} in, file {:.2}: fits {}",
            shot_data
                .surfaces
                .get(&wall.enter_surface)
                .map_or("?", |(n, _)| n.as_str()),
            wall.thickness,
            wall.enter.distance,
            match (fits.first(), fits.last()) {
                (Some(lo), Some(hi)) => format!("{lo:.2} – {hi:.2}"),
                _ => "none".to_owned(),
            }
        );
    }
}
