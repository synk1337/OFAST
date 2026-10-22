//! Developer tool: print every player's position, eye angles and view offset for one round of a demo, as CSV.
//!
//! ```bash
//! cargo run --release --example demo_round -- path/to/match.dem 1 > round1.csv
//! ```
//!
//! Columns: `tick,steamid,name,team,alive,x,y,z,pitch,yaw,view_offset_x,view_offset_y,view_offset_z`, then the rig
//! inputs (`velocity_x/y/z,on_ground,duck_amount,walking,scoped,defusing,on_ladder,last_jump_tick,
//! last_landing_tick,weapon_item,weapon_class,weapon_anim_state,reloading,body_yaw,weapon_state_tick,throw_strength,flash_duration,flash_max_alpha`;
//! empty when absent). x, y, z are
//! the feet;
//! the first-person camera is at x + view_offset_x, y + view_offset_y, z + view_offset_z. Newer columns come last so
//! that scripts written for the older ones keep working. Without a path, the newest demo
//! in CS2's `game/csgo/replays` folder is used; the round defaults to 1.

use std::io::Write;
use std::path::PathBuf;

use ofast_core::demo::players::track_round;
use ofast_core::game::Cs2Install;

fn newest_replay() -> PathBuf {
    let install = Cs2Install::locate().expect("CS2 install found");
    std::fs::read_dir(install.replays_dir())
        .expect("replays folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "dem"))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
        .expect("a .dem file in the replays folder")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .filter(|a| !a.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(newest_replay);
    let round: u32 = args
        .next()
        .map(|r| r.parse().expect("round number"))
        .unwrap_or(1);

    let data = std::fs::read(&path).expect("demo reads");
    let track = track_round(&data, round).expect("round tracks");
    eprintln!(
        "{} round {}: ticks {}..={} ({} samples, {:.1} s)",
        track.map_name,
        track.round,
        track.start_tick,
        track.end_tick,
        track.samples.len(),
        (track.end_tick - track.start_tick) as f32 * track.tick_interval
    );

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    writeln!(
        out,
        "tick,steamid,name,team,alive,x,y,z,pitch,yaw,view_offset_x,view_offset_y,view_offset_z,\
         velocity_x,velocity_y,velocity_z,on_ground,duck_amount,walking,scoped,defusing,on_ladder,\
         last_jump_tick,last_landing_tick,weapon_item,weapon_class,weapon_anim_state,reloading,body_yaw,weapon_state_tick,throw_strength,flash_duration,flash_max_alpha"
    )
    .unwrap();
    // An absent value (no jump yet, no weapon) is an empty column.
    let text = |value: Option<String>| value.unwrap_or_default();
    for sample in &track.samples {
        for p in &sample.players {
            let rig = &p.rig;
            let weapon = rig.weapon.as_ref();
            writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                sample.tick,
                p.steam_id,
                p.name.replace(',', " "),
                p.team,
                p.alive,
                p.position[0],
                p.position[1],
                p.position[2],
                p.pitch,
                p.yaw,
                p.view_offset[0],
                p.view_offset[1],
                p.view_offset[2],
                rig.velocity[0],
                rig.velocity[1],
                rig.velocity[2],
                rig.on_ground,
                rig.duck_amount,
                rig.walking,
                rig.scoped,
                rig.defusing,
                rig.on_ladder,
                text(rig.last_jump_tick.map(|t| t.to_string())),
                text(rig.last_landing_tick.map(|t| t.to_string())),
                text(weapon.map(|w| w.item.to_string())),
                text(weapon.map(|w| w.class.clone())),
                text(weapon.map(|w| w.anim_state.to_string())),
                text(weapon.map(|w| w.reloading.to_string())),
                rig.body_yaw,
                text(weapon.and_then(|w| w.state_tick).map(|t| t.to_string())),
                text(weapon.map(|w| w.throw_strength.to_string())),
                rig.flash_duration,
                rig.flash_max_alpha,
            )
            .unwrap();
        }
    }
}
