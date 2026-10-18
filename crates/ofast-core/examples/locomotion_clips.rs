//! Developer tool: measure CS2's locomotion clips — how fast each one moves, from its feet.
//!
//! ```bash
//! cargo run --release --example locomotion_clips -- rifle
//! ```
//!
//! Player clips carry no root motion: the character runs on the spot. The speed a clip was made for shows in its
//! feet instead: while a foot is planted it slides backwards under the body at exactly that speed. For each of the
//! set's idle, walk, run and crouch clips (8 directions) this prints the duration and the speed and direction of the
//! planted foot (the lower ankle, each frame), which `anim::locomotion` uses to play strides without sliding.

use ofast_core::anim::{Clip, NmSkeleton};
use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::model::compose;
use ofast_core::resource::{BlockKind, Resource};

fn data(content: &Content, path: &str) -> Value {
    let bytes = content.read_resource(path).expect("resource reads");
    Resource::parse(&bytes)
        .expect("resource parses")
        .kv3(BlockKind::DATA)
        .expect("DATA block")
        .root
}

fn main() {
    let set = std::env::args().nth(1).unwrap_or_else(|| "rifle".into());
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let folder = format!("animation/anims/world/{set}/_default_{set}");
    let mut names = vec![format!("idle_{set}"), format!("idle_crouch_{set}")];
    for gait in ["walk", "run", "crouch"] {
        for dir in ["n", "ne", "e", "se", "s", "sw", "w", "nw"] {
            names.push(format!("{gait}_{dir}_{set}"));
        }
    }
    let mut skeleton: Option<NmSkeleton> = None;
    println!("clip                     frames  seconds  planted-foot speed  direction  stride");
    for name in names {
        let path = format!("{folder}/{name}.vnmclip");
        let Ok(bytes) = content.read_resource(&path) else {
            println!("{name:<24} missing");
            continue;
        };
        let root = Resource::parse(&bytes)
            .expect("parses")
            .kv3(BlockKind::DATA)
            .expect("DATA")
            .root;
        let clip = Clip::parse(&root).expect("clip decodes");
        let skeleton =
            skeleton.get_or_insert_with(|| NmSkeleton::parse(&data(&content, &clip.skeleton)));
        let ankles = ["ankle_L", "ankle_R"].map(|b| skeleton.find(b).expect("ankle bone"));

        // Model-space ankle positions per frame.
        let positions: Vec<[[f32; 3]; 2]> = (0..clip.frames)
            .map(|f| {
                let pose = clip.decode_frame(f).expect("frame");
                let mut world = Vec::with_capacity(pose.len());
                for (i, bone) in pose.iter().enumerate() {
                    let local = bone.to_affine();
                    let w = match skeleton.parents[i] {
                        Some(p) => compose(&world[p], &local),
                        None => local,
                    };
                    world.push(w);
                }
                ankles.map(|a| [world[a][0][3], world[a][1][3], world[a][2][3]])
            })
            .collect();
        // A foot is planted while its ankle is within 1.5 inches of the lowest an ankle gets in the clip (a run has
        // flight phases with both feet up); its velocity between two planted frames, averaged.
        let fps = clip.fps();
        let lowest = positions
            .iter()
            .flat_map(|p| p.iter().map(|a| a[2]))
            .fold(f32::INFINITY, f32::min);
        let (mut vx, mut vy, mut n) = (0.0f32, 0.0f32, 0usize);
        for pair in positions.windows(2) {
            for (a, b) in pair[0].iter().zip(&pair[1]) {
                if a[2] < lowest + 1.5 && b[2] < lowest + 1.5 {
                    vx += (b[0] - a[0]) * fps;
                    vy += (b[1] - a[1]) * fps;
                    n += 1;
                }
            }
        }
        let (vx, vy) = if n > 0 {
            (vx / n as f32, vy / n as f32)
        } else {
            (0.0, 0.0)
        };
        // The body moves the opposite way to the planted foot.
        let speed = vx.hypot(vy);
        let direction = (-vy).atan2(-vx).to_degrees();
        println!(
            "{name:<24} {:>6}  {:>7.3}  {:>12.1} u/s  {:>8.1}°  {:>6.1}",
            clip.frames,
            clip.duration,
            speed,
            direction,
            speed * clip.duration
        );
    }
}
