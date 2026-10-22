//! Developer tool: what each player holds and carries at some ticks of a round, with model paths.
//!
//! ```bash
//! cargo run --release --example round_weapons -- path/to/match.dem 5 25824,26508
//! ```

use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::game::Cs2Install;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args[1].parse().expect("round");
    let ticks: Vec<u32> = args[2]
        .split(',')
        .map(|t| t.parse().expect("tick"))
        .collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let models = Content::game(&install)
        .expect("archives")
        .resource_paths("vmdl");
    let name = |id: u64| {
        models
            .get(&id)
            .map(|p| {
                p.rsplit('/')
                    .next()
                    .unwrap_or(p)
                    .trim_end_matches(".vmdl")
                    .to_owned()
            })
            .unwrap_or_else(|| format!("{id:016x}"))
    };
    let track = track_round(&data, round).expect("round");
    for tick in ticks {
        let Some(sample) = track.samples.iter().find(|s| s.tick >= tick) else {
            continue;
        };
        println!("tick {}:", sample.tick);
        for p in &sample.players {
            let active = p.rig.weapon.as_ref();
            println!(
                "  {:2} {:12} team {} alive {} agent {} groups {} | hand {:?} state {:?} | carries {}",
                p.controller,
                p.name,
                p.team,
                p.alive,
                name(p.appearance.model),
                p.appearance.mesh_groups,
                active.map(|w| (w.entity, w.item)),
                active.map(|w| w.anim_state),
                p.weapons
                    .iter()
                    .map(|w| format!("{}#{}:{}", name(w.model), w.entity, w.mesh_groups & 0xff))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
}
