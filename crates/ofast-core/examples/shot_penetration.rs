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
//! With `OFAST_DOORS=1`, also round by round: how many hits after a wall start where a traced way out is, without
//! and with the rotating doors posed as the demo has them ([`Doors`]), and every hit whose line goes through a door
//! first, with whether CS2 counted the door (the hit starts past the eyes) or not.
//!
//! With `OFAST_FIXTURES=func_brush,prop_dynamic,…`, the map's model entities of those classes are walls too, placed
//! from its entity lump (`OFAST_HIDDEN=no` leaves out those that start disabled, `=only` keeps only them), and each
//! hit whose line goes through one is listed: on the 13 demos checked they made the counts worse.

use std::collections::HashMap;

use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::events::{EventValue, GameEvent};
use ofast_core::demo::players::{list_rounds, track_round};
use ofast_core::demo::shots::{
    EFFECT_DISPATCH, FIRE_BULLETS, PlayerHit, Shot, bullet_direction, decode_fire_bullets,
    decode_player_hit, hit_of, spread_offset,
};
use ofast_core::export::shots::{Doors, ShotData, Wall, damage_at, walls_through};
use ofast_core::game::Cs2Install;
use ofast_core::map::{Entity, Map, entity_transform};
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::{Crossing, WorldCollision};
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
    // As bullets see it (grates, railings and glass left out: as walls they explained no more of the demos' hits).
    let collision = WorldCollision::from_physics_where(&world, |a| a.stops_bullets());
    let items = content
        .read("scripts/items/items_game.txt")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let mut warnings = Vec::new();
    let shot_data = ShotData::load(&content, items.as_deref(), &mut warnings);
    // OFAST_FIXTURES=func_brush,prop_dynamic,…: the map's model entities of those classes in the bullets' way too;
    // OFAST_HIDDEN=no leaves out those that start disabled, =only keeps only them.
    let classes: Vec<String> = std::env::var("OFAST_FIXTURES")
        .map(|c| c.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let hidden = std::env::var("OFAST_HIDDEN").unwrap_or_default();
    let fixtures = if classes.is_empty() {
        Fixtures::default()
    } else {
        let entities = Map::load_entities(&content, &map).expect("entities load");
        Fixtures::load(
            &content,
            &entities,
            |e| {
                let off = e.flag("startdisabled") == Some(true);
                e.class_name()
                    .is_some_and(|c| classes.iter().any(|k| k == c))
                    && match hidden.as_str() {
                        "no" => !off,
                        "only" => off,
                        _ => true,
                    }
            },
            &mut warnings,
        )
    };
    let mut census: std::collections::BTreeMap<(String, bool), usize> = Default::default();
    for f in &fixtures.placed {
        *census.entry((f.class.clone(), f.hidden)).or_default() += 1;
    }
    println!("fixtures (class, hidden): {census:?}");
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
    // Bullets through a body: a hit whose start lies on an earlier hit's line just past its point (where the bullet
    // came out of that player).
    for first in &hits {
        let v: [f32; 3] = std::array::from_fn(|k| first.point[k] - first.start[k]);
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l <= 0.0 {
            continue;
        }
        let d = v.map(|c| c / l);
        for second in hits
            .iter()
            .filter(|h| h.tick == first.tick && h.point != first.point)
        {
            let v: [f32; 3] = std::array::from_fn(|k| second.start[k] - first.point[k]);
            let t: f32 = (0..3).map(|k| v[k] * d[k]).sum();
            let off = (0..3)
                .map(|k| (v[k] - d[k] * t).powi(2))
                .sum::<f32>()
                .sqrt();
            if off < 0.5 && t > 0.0 && t < 20.0 {
                println!("through a body: tick {} depth {t:.1}", first.tick);
            }
        }
    }
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
        let mut crossings: Vec<_> = collision
            .ray_crossings(shot.eye, direction, distance)
            .into_iter()
            .map(|c| (c, surfaces.get(c.surface as usize).copied().unwrap_or(0)))
            .collect();
        let met = fixtures.crossings(shot.eye, direction, distance);
        let named: Vec<String> = met
            .iter()
            .filter_map(|(c, _)| fixtures.fixture_of(c.shape))
            .map(|f| {
                format!(
                    "{} {}{}",
                    f.class,
                    f.name,
                    if f.hidden { " (hidden)" } else { "" }
                )
            })
            .collect();
        crossings.extend(met);
        let walls = walls_through(crossings, &shot_data);
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
        if !named.is_empty() {
            println!(
                "tick {:6}: through fixtures {named:?} (hit after a wall: {})",
                hit.tick,
                !hit_of(shot, hit)
            );
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

    // OFAST_DOORS=1: round by round, the hits after a wall again, with the rotating doors posed as the demo has them.
    if std::env::var("OFAST_DOORS").is_ok() {
        doors_check(&data, &content, &collision, surfaces, &shot_data);
    }
}

/// The hits that start past the shooter's eyes, round by round: how many start within half an inch of a traced way
/// out of a wall, without the doors and with them ([`Doors`], posed per tick from the round's samples).
fn doors_check(
    data: &[u8],
    content: &Content,
    collision: &WorldCollision,
    surfaces: &[u32],
    shot_data: &ShotData,
) {
    let paths = content.resource_paths("vmdl");
    let (mut after, mut world_only, mut with_doors) = (0, 0, 0);
    for info in list_rounds(data).expect("rounds") {
        let Ok(track) = track_round(data, info.round) else {
            continue;
        };
        let mut warnings = Vec::new();
        let doors = Doors::load(content, &track, &paths, &mut warnings);
        // Every hit whose bullet's line goes through a door before the hit's start: whether CS2 counted the door
        // (the hit starts past the eyes: a penetration) or not (from the eyes).
        for hit in track.player_hits.iter().filter(|h| h.start != h.point) {
            for shot in track
                .shots
                .iter()
                .map(|(_, s)| s)
                .filter(|s| s.tick == hit.tick)
            {
                let Some(weapon) = shot_data.weapon(shot.item) else {
                    continue;
                };
                for bullet in 0..weapon.bullets.max(1) {
                    let d = bullet_direction(
                        shot.angles,
                        spread_offset(shot.seed, bullet, shot.inaccuracy, shot.spread),
                    );
                    let along = |p: [f32; 3]| -> (f32, f32) {
                        let v: [f32; 3] = std::array::from_fn(|k| p[k] - shot.eye[k]);
                        let t: f32 = (0..3).map(|k| v[k] * d[k]).sum();
                        let off = (0..3).map(|k| (v[k] - d[k] * t).powi(2)).sum::<f32>();
                        (t, off.sqrt())
                    };
                    let ((start, start_off), (point, point_off)) =
                        (along(hit.start), along(hit.point));
                    if start_off > 0.5 || point_off > 0.5 || point <= start {
                        continue;
                    }
                    let through = walls_through(
                        doors.crossings(&track, shot.tick, shot.eye, d, point),
                        shot_data,
                    );
                    let Some(door) = through.first() else {
                        continue;
                    };
                    let from_eyes = hit_of(shot, hit);
                    println!(
                        "  door on the line: round {} tick {} door at {:.1}, {:.1} thick; the hit starts at {start:.1}, \
                         point {point:.1}: {}",
                        info.round,
                        hit.tick,
                        door.distance,
                        door.thickness,
                        if from_eyes {
                            "from the eyes (no penetration)"
                        } else {
                            "past the eyes (penetration)"
                        }
                    );
                }
            }
        }
        for hit in track.player_hits.iter().filter(|h| h.start != h.point) {
            for (_, shot) in track
                .shots
                .iter()
                .filter(|(_, s)| s.tick == hit.tick && !hit_of(s, hit))
            {
                let Some(weapon) = shot_data.weapon(shot.item) else {
                    continue;
                };
                for bullet in 0..weapon.bullets.max(1) {
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
                    let ((start, start_off), (_, point_off)) = (on(hit.start), on(hit.point));
                    if start_off > 0.5 || point_off > 0.5 || start <= 0.0 {
                        continue;
                    }
                    after += 1;
                    let world: Vec<_> = collision
                        .ray_crossings(shot.eye, d, start + 1.0)
                        .into_iter()
                        .map(|c| (c, surfaces.get(c.surface as usize).copied().unwrap_or(0)))
                        .collect();
                    let exits = |walls: &[ofast_core::export::shots::Wall]| {
                        walls
                            .iter()
                            .any(|w| (w.distance + w.thickness - start).abs() < 0.5)
                    };
                    let mut both = world.clone();
                    let door_crossings =
                        doors.crossings(&track, shot.tick, shot.eye, d, start + 1.0);
                    both.extend(door_crossings.iter().copied());
                    let found = exits(&walls_through(world, shot_data));
                    world_only += usize::from(found);
                    with_doors += usize::from(exits(&walls_through(both, shot_data)));
                    if !found {
                        // Where it came out, and the nearest door's hinge.
                        let at: [f32; 3] = std::array::from_fn(|k| shot.eye[k] + d[k] * start);
                        let sample = &track.samples[track
                            .samples
                            .partition_point(|s| s.tick < shot.tick)
                            .min(track.samples.len() - 1)];
                        let nearest = sample
                            .doors
                            .iter()
                            .map(|door| {
                                (
                                    (0..3)
                                        .map(|k| (door.position[k] - at[k]).powi(2))
                                        .sum::<f32>()
                                        .sqrt(),
                                    door.angles,
                                )
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0));
                        println!(
                            "  round {} tick {}: out at {:?}, {} door crossings on the line, nearest door {:?}",
                            info.round,
                            hit.tick,
                            at.map(|c| c.round()),
                            door_crossings.len(),
                            nearest
                        );
                        // What the world has along the line up to there (distance, in or out).
                        let along: Vec<String> = collision
                            .ray_crossings(shot.eye, d, start + 50.0)
                            .iter()
                            .map(|c| {
                                format!("{:.1}{}", c.distance, if c.entering { "+" } else { "-" })
                            })
                            .collect();
                        println!(
                            "    from the eyes, it starts at {start:.1}; the world's crossings: {along:?}"
                        );
                        // As map_raycast's camera (setpos: the eyes 64 below; setang along the bullet).
                        println!(
                            "    tick {} start {start:.1} map_raycast camera \"{:.1},{:.1},{:.1},{:.3},{:.3}\"",
                            hit.tick,
                            shot.eye[0],
                            shot.eye[1],
                            shot.eye[2] - 64.0,
                            -d[2].asin().to_degrees(),
                            d[1].atan2(d[0]).to_degrees()
                        );
                    }
                }
            }
        }
        if info.round == 1 {
            println!(
                "doors: {:?}, hulls and triangles per model {:?}",
                warnings,
                doors.counts()
            );
            if let Some(sample) = track.samples.first() {
                for door in &sample.doors {
                    println!(
                        "  door {:016x} at {:?} turned {:?}",
                        door.model, door.position, door.angles
                    );
                }
            }
        }
    }
    println!(
        "\ndoors: {after} hits start past the eyes; a traced way out within 0.5 in of the start: {world_only} without \
         the doors, {with_doors} with them"
    );
}

/// [`Crossing::shape`] of the first fixture ([`Fixtures`]): fixture `i` crosses as `FIXTURE_SHAPES + i`.
const FIXTURE_SHAPES: u32 = 1 << 31;

/// One of the map's model entities placed in the bullets' way ([`Fixtures`]).
#[derive(Debug, Clone, PartialEq)]
struct Fixture {
    /// Its class (`func_brush`, `prop_dynamic`…).
    class: String,
    /// Its `targetname` (empty without one).
    name: String,
    /// It starts disabled (`startdisabled`): not drawn (another game mode's prop or blocker).
    hidden: bool,
}

/// The map's model entities as walls (`OFAST_FIXTURES`): `func_brush` blockers, `prop_dynamic` props and the like,
/// models of their own, not in `world_physics`, each placed where the map's entity lump puts it (`origin`, `angles`,
/// `scales`: [`ofast_core::map::entity_transform`]) with its model's collision as bullets see it, all baked into one
/// collision in the world's space ([`WorldCollision::add_placed`]). Measured on the 13 demos, they are not walls to
/// CS2's bullets (docs/decisions/018): the export leaves them out, this tool keeps them to show it again.
#[derive(Debug, Default)]
struct Fixtures {
    collision: WorldCollision,
    /// The surface property hashes of every placed model, one table ([`WorldCollision::add_placed`]'s base).
    surfaces: Vec<u32>,
    /// What was placed, by index ([`Fixtures::fixture_of`]).
    placed: Vec<Fixture>,
}

impl Fixtures {
    /// Places the `entities` that `keep` accepts and whose model has collision that stops bullets; a model that
    /// cannot be read is left out with a warning, and so is an entity with a parent (`parentname`).
    fn load(
        content: &Content,
        entities: &[Entity],
        keep: impl Fn(&Entity) -> bool,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut out = Fixtures::default();
        let mut models: HashMap<String, Option<(PhysicsData, u32)>> = HashMap::new();
        for entity in entities {
            let Some(model) = entity.string("model") else {
                continue;
            };
            if !keep(entity) || entity.string("parentname").is_some_and(|p| !p.is_empty()) {
                continue;
            }
            let loaded = models.entry(model.to_owned()).or_insert_with(|| {
                let physics = content
                    .read_resource(model)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        let resource = Resource::parse(&bytes).map_err(|e| e.to_string())?;
                        PhysicsData::from_resource(&resource).map_err(|e| e.to_string())
                    });
                match physics {
                    Ok(Some(physics)) => {
                        let base = out.surfaces.len() as u32;
                        out.surfaces.extend(&physics.surfaces);
                        Some((physics, base))
                    }
                    Ok(None) => None,
                    Err(e) => {
                        warnings.push(format!("{model}: {e}: bullets pass it"));
                        None
                    }
                }
            });
            let Some((physics, base)) = loaded else {
                continue;
            };
            let before = out.collision.counts();
            let place = entity_transform(entity.origin(), entity.angles(), entity.scales());
            let shape = FIXTURE_SHAPES + out.placed.len() as u32;
            out.collision
                .add_placed(physics, |a| a.stops_bullets(), &place, shape, *base);
            if out.collision.counts() == before {
                continue;
            }
            out.placed.push(Fixture {
                class: entity.class_name().unwrap_or_default().to_owned(),
                name: entity.string("targetname").unwrap_or_default().to_owned(),
                hidden: entity.flag("startdisabled") == Some(true),
            });
        }
        out
    }

    /// Where a line from `from` along the unit `direction` crosses the fixtures within `reach`: each crossing, with
    /// its surface property's hash ([`walls_through`]).
    fn crossings(&self, from: [f32; 3], direction: [f32; 3], reach: f32) -> Vec<(Crossing, u32)> {
        self.collision
            .ray_crossings(from, direction, reach)
            .into_iter()
            .map(|c| {
                let hash = self.surfaces.get(c.surface as usize).copied().unwrap_or(0);
                (c, hash)
            })
            .collect()
    }

    /// The fixture a crossing's [`Crossing::shape`] is of, when it is one's.
    fn fixture_of(&self, shape: u32) -> Option<&Fixture> {
        shape
            .checked_sub(FIXTURE_SHAPES)
            .and_then(|i| self.placed.get(i as usize))
    }
}
