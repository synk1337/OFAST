//! Developer tool: the voxel logs of a round's smokes (`m_VoxelFrameData`), read frame by frame by the layout
//! documented in `docs/formats/demo.md` ("Smoke voxels"), to check that layout against the data: whether the frames
//! chain to exactly the bytes in use, the frame numbers count up, and what the payloads hold.
//!
//! ```bash
//! cargo run --release --example smoke_log_probe -- path/to/match.dem 5 [entity]
//! ```

use ofast_core::demo::players::track_round;

/// Reads `u16` little-endian at `at`.
fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let only: Option<u32> = args.get(2).map(|e| e.parse().expect("entity"));
    let track = track_round(&data, round).expect("round");
    for log in &track.smoke_logs {
        if only.is_some_and(|e| e != log.entity) {
            continue;
        }
        let (first_tick, _) = log.snapshots.first().cloned().unwrap_or_default();
        let (last_tick, last) = log.snapshots.last().cloned().unwrap_or_default();
        println!(
            "smoke #{} (serial {}): {} snapshots, ticks {first_tick}–{last_tick}, last {} bytes",
            log.entity,
            log.serial,
            log.snapshots.len(),
            last.len()
        );
        // Frames: u16 frame number, u16 payload length, payload.
        let mut at = 0usize;
        let mut numbers = Vec::new();
        let mut emitters = 0usize;
        let mut bricks = 0usize;
        let mut holes = 0usize;
        let mut fading_from = None;
        let mut problems = Vec::new();
        while at + 4 <= last.len() {
            let number = u16_at(&last, at).unwrap();
            let length = u16_at(&last, at + 2).unwrap() as usize;
            let payload = &last[at + 4..(at + 4 + length).min(last.len())];
            if payload.len() < length {
                problems.push(format!("frame {number} runs past the end"));
                break;
            }
            numbers.push(number);
            // u8 fading, u8 flags, [emitters], [bricks], u8 holes × 20 bytes.
            let (fading, flags) = (payload.first().copied(), payload.get(1).copied());
            if fading == Some(1) && fading_from.is_none() {
                fading_from = Some(number);
            }
            let mut p = 2usize;
            if flags.is_some_and(|f| f & 1 != 0) {
                let n = payload.get(p).copied().unwrap_or(0) as usize;
                emitters += n;
                if only.is_some() && n > 0 {
                    for k in 0..n {
                        let e = &payload[p + 1 + 8 * k..p + 9 + 8 * k];
                        let hold = f32::from_le_bytes([e[4], e[5], e[6], e[7]]);
                        println!(
                            "    frame {number}: emitter at voxel ({}, {}, {}) level {} hold {hold}",
                            e[0], e[1], e[2], e[3]
                        );
                    }
                }
                p += 1 + 8 * n;
            }
            if flags.is_some_and(|f| f & 2 != 0) {
                let n = u16_at(payload, p).unwrap_or(0) as usize;
                bricks += n;
                if only.is_some() && n > 0 {
                    let list: Vec<String> = (0..n)
                        .map(|k| {
                            let r = p + 2 + 10 * k;
                            let index = u16_at(payload, r).unwrap_or(0);
                            let mask =
                                u64::from_le_bytes(payload[r + 2..r + 10].try_into().unwrap());
                            format!("{index}:{}", mask.count_ones())
                        })
                        .collect();
                    println!(
                        "    frame {number}: {n} bricks (index:solid voxels) {}",
                        list.join(" ")
                    );
                }
                p += 2 + 10 * n;
            }
            let n = payload.get(p).copied().unwrap_or(0) as usize;
            holes += n;
            if only.is_some() && n > 0 {
                for k in 0..n {
                    let h = &payload[p + 1 + 20 * k..(p + 21 + 20 * k).min(payload.len())];
                    println!("    frame {number}: hole record {h:02x?}");
                }
            }
            p += 1 + 20 * n;
            if p != payload.len() {
                problems.push(format!(
                    "frame {number}: payload {} bytes, layout reads {p}",
                    payload.len()
                ));
            }
            at += 4 + length;
        }
        if at != last.len() {
            problems.push(format!("{} bytes left after the frames", last.len() - at));
        }
        let consecutive = numbers.windows(2).all(|w| w[1] == w[0].wrapping_add(1));
        println!(
            "  {} frames ({:?} … {:?}), consecutive: {consecutive}, emitters {emitters}, bricks {bricks}, holes {holes}, fading from frame {fading_from:?}",
            numbers.len(),
            numbers.first(),
            numbers.last()
        );
        // How the log grew: bytes per snapshot (first few, last few) against the ticks.
        let sizes: Vec<String> = log
            .snapshots
            .iter()
            .take(4)
            .chain(log.snapshots.iter().rev().take(3).rev())
            .map(|(t, b)| format!("{t}:{}", b.len()))
            .collect();
        println!("  growth (tick:bytes): {}", sizes.join(" "));
        for p in problems.iter().take(5) {
            println!("  PROBLEM {p}");
        }
    }
}
