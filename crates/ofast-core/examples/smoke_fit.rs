//! Developer tool: fits OFAST's smoke simulation (`demo::smoke::Simulation`) to CS2's, by the bricks of world geometry
//! the server sent: each one the frame CS2's simulation first needed it. For a set of settings it reports how many
//! bricks OFAST's simulation needs in the same frame, how far off the others are, and the bricks it needs that CS2's
//! never did. `dump` lists one smoke's emitters and brick arrivals instead.
//!
//! ```bash
//! cargo run --release --example smoke_fit -- path/to/match.dem 5,9 fit
//! cargo run --release --example smoke_fit -- path/to/match.dem 5 dump 484
//! ```

use ofast_core::demo::players::track_round;
use ofast_core::demo::smoke::{self, BRICK, Simulation, SmokeSettings, VoxelFrame};
use rayon::prelude::*;

/// How one set of settings did over every smoke.
#[derive(Debug, Default, Clone, Copy)]
struct Score {
    /// Bricks sent after frame 0.
    sent: usize,
    /// Of those, needed by OFAST in the same frame.
    exact: usize,
    /// … within 2 frames.
    near: usize,
    /// … never needed by OFAST.
    missed: usize,
    /// Frames OFAST needed bricks before CS2, summed.
    early: usize,
    /// Frames OFAST needed bricks after CS2, summed.
    late: usize,
    /// Bricks OFAST needed that CS2 never sent.
    extra: usize,
}

impl Score {
    /// One number to minimise. Early is nearly harmless (the simulation waits for CS2's brick, so early frames count a
    /// quarter); late frames count, a missed brick as 20 late frames, a brick CS2 never needed as 2.
    fn cost(&self) -> usize {
        self.late + self.early / 4 + 20 * self.missed + 2 * self.extra
    }

    fn add(self, b: Score) -> Score {
        Score {
            sent: self.sent + b.sent,
            exact: self.exact + b.exact,
            near: self.near + b.near,
            missed: self.missed + b.missed,
            early: self.early + b.early,
            late: self.late + b.late,
            extra: self.extra + b.extra,
        }
    }
}

/// Runs one smoke and scores it.
fn score(frames: &[VoxelFrame], settings: SmokeSettings) -> Score {
    let mut sim = Simulation::new(settings);
    let mut sent = vec![None; 512];
    for f in frames {
        for b in &f.bricks {
            sent[b.index as usize] = Some(f.number);
        }
        sim.step(f);
    }
    let mut out = Score::default();
    for (&cs2, &ours) in sent.iter().zip(sim.needed()) {
        match (cs2, ours) {
            (Some(0), _) | (None, None) => {}
            (Some(s), Some(o)) => {
                out.sent += 1;
                let d = s.abs_diff(o) as usize;
                out.exact += (d == 0) as usize;
                out.near += (d <= 2) as usize;
                out.early += s.saturating_sub(o) as usize;
                out.late += o.saturating_sub(s) as usize;
            }
            (Some(_), None) => {
                out.sent += 1;
                out.missed += 1;
            }
            (None, Some(_)) => out.extra += 1,
        }
    }
    out
}

fn total(smokes: &[Vec<VoxelFrame>], settings: SmokeSettings) -> Score {
    smokes
        .iter()
        .map(|f| score(f, settings))
        .fold(Score::default(), Score::add)
}

fn dump(frames: &[VoxelFrame]) {
    let mut voxels: Vec<String> = frames
        .iter()
        .flat_map(|f| f.emitters.iter())
        .map(|e| format!("{},{},{}", e.voxel[0], e.voxel[1], e.voxel[2]))
        .collect();
    voxels.sort();
    println!("  emitters: {}", voxels.join(" "));
    let sim_settings = SmokeSettings::default();
    let mut sim = Simulation::new(sim_settings);
    for f in frames {
        sim.step(f);
        if f.number < 12 || f.number % 10 == 0 {
            // Voxels with density, the largest, and their extent.
            let grid = sim.grid();
            let (mut lo, mut hi, mut n, mut max) = ([99usize; 3], [0usize; 3], 0, 0f32);
            for (k, &d) in grid.iter().enumerate() {
                if d > 0.0 {
                    let c = [k % 32, k / 32 % 32, k / 1024];
                    for a in 0..3 {
                        lo[a] = lo[a].min(c[a]);
                        hi[a] = hi[a].max(c[a]);
                    }
                    n += 1;
                    max = max.max(d);
                }
            }
            // How many voxels hold at least 2, 5, 10, 25, 50 % of the emitters' cap.
            let levels: Vec<usize> = [0.02, 0.05, 0.1, 0.25, 0.5]
                .iter()
                .map(|l| grid.iter().filter(|&&d| d > l * sim_settings.cap).count())
                .collect();
            println!(
                "  step {}: {n} voxels, max {max:.1}, extent {lo:?} – {hi:?}, at 2/5/10/25/50 % of the cap {levels:?}",
                f.number
            );
        }
    }
    // The masks of the bricks under and around the emitters, layer by layer (z up), rows y, columns x: `#` solid.
    for b in frames[0].bricks.iter().filter(|b| b.solid != 0).take(12) {
        let c = b.coordinates();
        println!(
            "  brick ({},{},{}):",
            c[0] * BRICK,
            c[1] * BRICK,
            c[2] * BRICK
        );
        for z in 0..4 {
            let rows: Vec<String> = (0..4)
                .map(|y| {
                    (0..4)
                        .map(|x| if b.is_solid([x, y, z]) { '#' } else { '.' })
                        .collect()
                })
                .collect();
            println!("    z{z}: {}", rows.join(" "));
        }
    }
    for f in frames {
        if f.bricks.is_empty() {
            continue;
        }
        let mut list: Vec<([usize; 3], u32, Option<u16>)> = f
            .bricks
            .iter()
            .map(|b| {
                (
                    b.coordinates(),
                    b.solid.count_ones(),
                    sim.needed()[b.index as usize],
                )
            })
            .collect();
        list.sort();
        let text: Vec<String> = list
            .iter()
            .map(|(c, s, o)| {
                format!(
                    "({},{},{}){s}{}",
                    c[0] * BRICK,
                    c[1] * BRICK,
                    c[2] * BRICK,
                    o.map_or(" ofast-never".into(), |o| format!(" ofast@{o}"))
                )
            })
            .collect();
        println!("  frame {} tick {}: {}", f.number, f.tick, text.join(" "));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let rounds: Vec<u32> = args
        .get(1)
        .map_or("1", String::as_str)
        .split(',')
        .map(|r| r.parse().expect("round number"))
        .collect();
    let mode = args.get(2).map_or("fit", String::as_str);
    let mut smokes = Vec::new();
    for round in rounds {
        let track = track_round(&data, round).expect("round");
        for log in &track.smoke_logs {
            let frames = smoke::frames(log);
            if mode == "dump" {
                if args.get(3).is_some_and(|e| e.parse() == Ok(log.entity)) {
                    println!(
                        "smoke #{} round {round}: {} frames",
                        log.entity,
                        frames.len()
                    );
                    dump(&frames);
                }
                continue;
            }
            smokes.push(frames);
        }
    }
    if mode == "dump" {
        return;
    }
    println!("{} smokes", smokes.len());
    // Axis-order check: an emitter must sit in an open voxel. Count emitters in solid voxels when the bricks'
    // coordinates and the masks' bits are read in each order of the axes.
    for order in [
        [0, 1, 2],
        [1, 0, 2],
        [0, 2, 1],
        [2, 1, 0],
        [1, 2, 0],
        [2, 0, 1],
    ] {
        let (mut solid, mut known, mut all) = (0, 0, 0);
        for frames in &smokes {
            let mut bricks = std::collections::HashMap::new();
            for f in frames {
                for b in &f.bricks {
                    let c = b.coordinates();
                    bricks.insert([c[order[0]], c[order[1]], c[order[2]]], b.solid);
                }
            }
            for e in frames.iter().flat_map(|f| f.emitters.iter()) {
                all += 1;
                let v = e.voxel.map(|v| v as usize);
                let Some(&mask) = bricks.get(&[v[0] / 4, v[1] / 4, v[2] / 4]) else {
                    continue;
                };
                known += 1;
                let l = [v[0] % 4, v[1] % 4, v[2] % 4];
                // The bit, with the same axis order inside the brick.
                let mut inv = [0; 3];
                for (k, &o) in order.iter().enumerate() {
                    inv[o] = l[k];
                }
                solid += (mask >> smoke::morton(inv) & 1) as usize;
            }
        }
        println!(
            "axis order {order:?}: {solid} of {known} emitters in known bricks are solid ({all} emitters)"
        );
    }
    // Connectivity check: for each brick sent after frame 0, the face neighbours already sent by then, and across each
    // shared face how many voxel pairs are open on both sides (a way in for smoke that moves face to face).
    if mode == "connect" {
        let (mut no_way, mut solid_way, mut open_way) = (0, 0, 0);
        for frames in &smokes {
            let mut sent = std::collections::HashMap::new();
            for f in frames {
                for b in &f.bricks {
                    sent.insert(b.coordinates(), (f.number, b.solid));
                }
            }
            for (&c, &(frame, mask)) in &sent {
                if frame == 0 {
                    continue;
                }
                let mut best = None::<usize>;
                for axis in 0..3 {
                    for delta in [-1i32, 1] {
                        let v = c[axis] as i32 + delta;
                        if !(0..8).contains(&v) {
                            continue;
                        }
                        let mut n = c;
                        n[axis] = v as usize;
                        let Some(&(nf, nmask)) = sent.get(&n) else {
                            continue;
                        };
                        if nf > frame {
                            continue;
                        }
                        // Voxel pairs across the face: in this brick at the face (local 0 or 3 on `axis`), in the
                        // neighbour on the other side.
                        let mut open = 0;
                        for a in 0..4 {
                            for b in 0..4 {
                                let mut here = [0usize; 3];
                                let others: Vec<usize> = (0..3).filter(|&k| k != axis).collect();
                                here[others[0]] = a;
                                here[others[1]] = b;
                                let mut there = here;
                                here[axis] = if delta < 0 { 0 } else { 3 };
                                there[axis] = if delta < 0 { 3 } else { 0 };
                                let solid_here = mask >> smoke::morton(here) & 1 != 0;
                                let solid_there = nmask >> smoke::morton(there) & 1 != 0;
                                open += (!solid_here && !solid_there) as usize;
                            }
                        }
                        best = Some(best.map_or(open, |b: usize| b.max(open)));
                    }
                }
                match best {
                    None => no_way += 1,
                    Some(0) => solid_way += 1,
                    Some(_) => open_way += 1,
                }
            }
        }
        println!(
            "bricks after frame 0: {open_way} have an open face to a brick sent by then, {solid_way} only faces \
             blocked by solid voxels, {no_way} no face neighbour sent by then"
        );
        // The most a cloud moving face to face could reach: each step, a flood from every reached voxel through
        // the open voxels of the bricks sent so far (`through_solid`: through solid voxels too); a brick is needed
        // when the flood touches one of its voxels.
        for through_solid in [false, true] {
            let (mut missed, mut early, mut exact) = (0, 0, 0);
            for frames in &smokes {
                let mut solid: Vec<Option<u64>> = vec![None; 512];
                let mut reached = vec![false; 32768];
                let mut needed: Vec<Option<u16>> = vec![None; 512];
                let mut sent: Vec<Option<u16>> = vec![None; 512];
                let mut stack: Vec<u32> = Vec::new();
                for f in frames {
                    for b in &f.bricks {
                        solid[b.index as usize] = Some(b.solid);
                        sent[b.index as usize] = Some(f.number);
                    }
                    for e in &f.emitters {
                        let code = smoke::morton(e.voxel.map(|v| v as usize));
                        if !reached[code as usize] {
                            reached[code as usize] = true;
                        }
                    }
                    stack.extend((0..32768u32).filter(|&c| reached[c as usize]));
                    while let Some(code) = stack.pop() {
                        let c = smoke::demorton(code, 5);
                        for axis in 0..3 {
                            for delta in [-1i32, 1] {
                                let v = c[axis] as i32 + delta;
                                if !(0..32).contains(&v) {
                                    continue;
                                }
                                let mut n = c;
                                n[axis] = v as usize;
                                let nc = smoke::morton(n);
                                if reached[nc as usize] {
                                    continue;
                                }
                                let brick = (nc >> 6) as usize;
                                needed[brick].get_or_insert(f.number);
                                let open = match solid[brick] {
                                    None => false,
                                    Some(m) => through_solid || m >> (nc & 63) & 1 == 0,
                                };
                                if open {
                                    reached[nc as usize] = true;
                                    stack.push(nc);
                                }
                            }
                        }
                    }
                }
                for (s, n) in sent.iter().zip(&needed) {
                    match (s, n) {
                        (Some(0), _) => {}
                        (Some(s), Some(n)) => {
                            early += (n < s) as usize;
                            exact += (n == s) as usize;
                        }
                        (Some(_), None) => missed += 1,
                        _ => {}
                    }
                }
            }
            println!(
                "flood{}: of the bricks sent after frame 0, {missed} never touched, {early} touched before CS2 sent \
                 them, {exact} on the frame",
                if through_solid {
                    " through solid voxels"
                } else {
                    ""
                }
            );
        }
        return;
    }
    let base = SmokeSettings::default();
    let b = total(&smokes, base);
    println!("default: {b:?} cost {}", b.cost());
    // The weights add up to 1 (only the decay takes smoke away), so the cloud's size is what the emitters feed
    // against the decay of 0.5. `own` is what a voxel keeps, `bias` how much more it pulls from above than below.
    // Searched first (2026-10-23) over own 0 – 0.9, bias 0 – 0.75, feed 15 – 480, cap 50 – unbounded, burst 60 –
    // 60,000, a different decay while fading, and needing a brick only when smoke would enter it: none beat the default
    // by much. This grid refines around it.
    let mut grid = Vec::new();
    for own in [0.5, 0.6, 0.7] {
        for bias in [0.15, 0.25, 0.35] {
            for cap in [150.0, 250.0, 400.0] {
                for burst in [1500.0, 3000.0, 6000.0] {
                    let side = (1.0 - own) / 6.0;
                    grid.push(SmokeSettings {
                        own,
                        side,
                        above: side * (1.0 + bias),
                        below: side * (1.0 - bias),
                        cap,
                        burst,
                        ..base
                    });
                }
            }
        }
    }
    let mut results: Vec<(SmokeSettings, Score)> = grid
        .into_par_iter()
        .map(|s| (s, total(&smokes, s)))
        .collect();
    results.sort_by_key(|(_, s)| s.cost());
    for (s, score) in results.iter().take(10) {
        println!(
            "own {} side {:.3} above {:.3} below {:.3} feed {} cap {} burst {}: {score:?} cost {}",
            s.own,
            s.side,
            s.above,
            s.below,
            s.feed,
            s.cap,
            s.burst,
            score.cost()
        );
    }
}
