//! Developer tool: a demo's rounds, scores, kill counts and highlights, as the Studio shows them.
//!
//! ```bash
//! cargo run --release --example demo_summary -- path/to/match.dem
//! # the summary as JSON (what the Studio receives)
//! cargo run --release --example demo_summary -- path/to/match.dem json
//! ```
//!
//! Without a path, the newest demo in CS2's `game/csgo/replays` folder is used.

use std::path::PathBuf;
use std::time::Instant;

use ofast_core::demo::summary::{HighlightKind, summarize};
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().map_or_else(newest_replay, PathBuf::from);
    let data = std::fs::read(&path).expect("demo readable");
    let start = Instant::now();
    let summary = summarize(&data).expect("demo summarized");
    if args.get(1).map(String::as_str) == Some("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("serializable")
        );
        return;
    }
    println!(
        "{}: {} on {}, {} rounds, {} players, read in {:.1} s",
        path.display(),
        summary.server,
        summary.map,
        summary.rounds.len(),
        summary.players.len(),
        start.elapsed().as_secs_f32()
    );
    let name = |i: Option<usize>| i.map_or("-", |i| summary.players[i].name.as_str());
    for r in &summary.rounds {
        println!(
            "round {:2}{} ticks {:6}–{:6} freeze end {:?}, decided {:?}: {:?} by {:?}, {} {} – {} {}, {} players, {} kills",
            r.round,
            if r.restarted { " (restarted)" } else { "" },
            r.start_tick,
            r.end_tick,
            r.freeze_end_tick,
            r.decided_tick,
            r.winner,
            r.reason,
            r.t.name,
            r.t.score,
            r.ct.score,
            r.ct.name,
            r.players.len(),
            r.kills.len()
        );
    }
    println!("\nhighlights:");
    for h in &summary.highlights {
        let what = match h.kind {
            HighlightKind::MultiKill => format!("{}k", h.count),
            HighlightKind::Clutch => format!("1v{} clutch", h.count),
            HighlightKind::Collateral => format!("{}-kill collateral", h.count),
            HighlightKind::SpecialKill => format!("{:?}", h.tags),
        };
        let round = &summary.rounds[h.round_index];
        let weapons: Vec<&str> = h
            .kills
            .iter()
            .map(|&k| round.kills[k].weapon.as_str())
            .collect();
        println!(
            "  round {:2} tick {:6}: {:16} {} ({})",
            round.round,
            h.start_tick,
            name(Some(h.player)),
            what,
            weapons.join(", ")
        );
    }
}
