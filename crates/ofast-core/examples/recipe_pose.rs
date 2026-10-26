//! Developer tool: one player's pose recipe on one tick, played ([`ofast_core::anim::recipe::RecipePlayer`]) and set
//! against OFAST's re-decided pose: each task with its clip's path and whether the clip is additive, then chosen
//! bones in model space (translation, and the bone's local scale) in three poses: the recipe's as played, the bake
//! over it (OFAST's aim and weapon snap, as the export plays them, without the aim punch) and the bake without it (as
//! with `OFAST_RECIPE=0`).
//!
//! ```bash
//! cargo run --release --example recipe_pose -- path/to/match.dem 5 XANTARES 34532 [bone,bone…]
//! ```
//!
//! The recipes are played from the round's start ([`ofast_core::anim::recipe::RecipePlayer::play_round`]), as the
//! export plays them.

use std::collections::BTreeMap;

use ofast_core::anim::Transform;
use ofast_core::anim::bake::{PlayerExtras, bake_player, frame_samples};
use ofast_core::anim::recipe::{RecipePlayer, clips_by_path, learn_clip_table};
use ofast_core::content::Content;
use ofast_core::demo::players::track_round;
use ofast_core::demo::recipe::{self, Task};
use ofast_core::export::players::load_player_rig;
use ofast_core::game::Cs2Install;
use ofast_core::model::{Affine, compose};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let demo = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args[1].parse().expect("round number");
    let name = &args[2];
    let at: u32 = args[3].parse().expect("tick");
    let bones: Vec<String> = args
        .get(4)
        .map_or("wpn,hand_R,wpnHand_R,hand_L,head_0".into(), String::clone)
        .split(',')
        .map(str::to_owned)
        .collect();
    let track = track_round(&demo, round).expect("round");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let rig = load_player_rig(&content, &track).expect("rig");
    let table = learn_clip_table(&track, &rig.sets, rig.upper.as_ref(), rig.graphs.as_ref());
    let clips = clips_by_path(&rig.sets, rig.upper.as_ref(), rig.graphs.as_ref());
    let controller = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .find(|p| &p.name == name)
        .map(|p| p.controller)
        .expect("player in the round");

    // The tick's tasks.
    let decoded = track
        .samples
        .iter()
        .find(|s| s.tick == at)
        .and_then(|s| s.players.iter().find(|p| p.controller == controller))
        .and_then(|p| p.rig.pose_recipe.as_ref())
        .and_then(|r| recipe::decode(r).ok());
    for (i, node) in decoded.iter().flat_map(|d| d.topology.iter().enumerate()) {
        let what = match decoded.as_ref().and_then(|d| d.tasks.get(i)) {
            Some(Task::Sample { resource, time }) => match table.path(*resource) {
                Some(path) => {
                    let additive = clips.get(path).is_some_and(|c| c.is_additive());
                    let stem = path.rsplit('/').next().unwrap_or(path);
                    format!(
                        "#{resource} {stem} @{time:.3}{}",
                        if additive { " (additive)" } else { "" }
                    )
                }
                None => format!("#{resource} unknown @{time:.3}"),
            },
            Some(other) => format!("{other:?}"),
            None => "not decoded".to_owned(),
        };
        println!("  {i}: {}({:?}) {what}", node.kind, node.inputs);
    }

    let (poses, _) = RecipePlayer::new(&table, &clips, &rig.masks, &rig.clip_skeleton)
        .play_round(&track, controller);
    let raw = poses.get(&at).cloned();
    let fps = 1.0 / track.tick_interval;
    let bake = |recipe_poses| {
        bake_player(
            &track,
            controller,
            &rig.sets,
            rig.upper.as_ref(),
            rig.graphs.as_ref(),
            fps,
            PlayerExtras {
                aim_punch: None,
                recipe_poses,
            },
        )
        .expect("bakes")
    };
    let frame = frame_samples(&track, fps)
        .iter()
        .position(|&(t, _)| t.round() as u32 == at)
        .expect("frame at the tick");
    let over = bake(Some(&poses))[frame].clone();
    let ofast = bake(None)[frame].clone();

    let parents = &rig.clip_skeleton.parents;
    let model = |pose: &[Transform]| {
        let mut world: Vec<Affine> = Vec::with_capacity(pose.len());
        for (i, bone) in pose.iter().enumerate() {
            let local = bone.to_affine();
            let w = match parents[i] {
                Some(p) => compose(&world[p], &local),
                None => local,
            };
            world.push(w);
        }
        world
    };
    let index: BTreeMap<&str, usize> = rig
        .clip_skeleton
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.as_str(), i))
        .collect();
    let sets = [
        ("recipe", raw.as_deref().map(model), raw.as_deref()),
        ("over", over.as_deref().map(model), over.as_deref()),
        ("ofast", ofast.as_deref().map(model), ofast.as_deref()),
    ];
    for bone in &bones {
        let Some(&i) = index.get(bone.as_str()) else {
            println!("{bone}: not in the skeleton");
            continue;
        };
        for (label, world, local) in &sets {
            match (world, local) {
                (Some(w), Some(l)) => println!(
                    "{bone:10} {label:6} model [{:7.2} {:7.2} {:7.2}] scale {:.3}",
                    w[i][0][3], w[i][1][3], w[i][2][3], l[i].scale
                ),
                _ => println!("{bone:10} {label:6} none"),
            }
        }
    }
}
