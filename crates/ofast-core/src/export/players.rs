//! The players of a demo round as animated CS2 agents, for Unreal (M4.4).
//!
//! One glTF per team, next to the round's replay file: `<demo>_r<round>_ct.gltf` and `<demo>_r<round>_t.gltf`. Each
//! holds the team's agent model, skinned ([`super::gltf`]), and one animation per player of that team, named
//! `player_<controller>` (the controller index of the replay file), keyed at [`PLAYER_ANIMATION_FPS`] from the
//! round's first tick to its last. The animations are CS2's ground locomotion baked from the demo's rig inputs
//! ([`crate::anim::bake`]) and retargeted onto the agent's skeleton; `tools/unreal/import_demo.py` plays them on
//! the players' actors, which the replay file's positions and body yaw move.
//!
//! Every CT uses the SAS agent and every T the Phoenix agent for now; each player's own agent, weapon and knife come
//! with M4.5.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::ExportError;
use super::gltf::{BoneAnimation, export_model_animated, resource_data};
use crate::anim::bake::{LocomotionSets, WeaponSet, bake_player};
use crate::anim::{Clip, Locomotion, NmSkeleton, Retargeter, Transform};
use crate::content::Content;
use crate::demo::players::RoundTrack;

/// Agent model every Counter-Terrorist wears for now.
pub const CT_AGENT: &str = "agents/models/ctm_sas/ctm_sas.vmdl_c";
/// Agent model every Terrorist wears for now.
pub const T_AGENT: &str = "agents/models/tm_phoenix/tm_phoenix.vmdl_c";
/// Key rate of the players' animations: one Unreal's glTF import recognises (the replay's 64 ticks per second are
/// not), fine enough for a run cycle of 0.73 s.
pub const PLAYER_ANIMATION_FPS: f32 = 30.0;

/// What [`export_players`] wrote.
#[derive(Debug, Clone)]
pub struct PlayersReport {
    /// The glTF files, one per team that had players.
    pub files: Vec<PathBuf>,
    /// Players animated.
    pub players: usize,
    /// Frames per animation.
    pub frames: usize,
    /// Problems that did not stop the export.
    pub warnings: Vec<String>,
}

/// Reads CS2's three locomotion graphs and their clips.
pub fn load_locomotion(content: &Content) -> Result<LocomotionSets, ExportError> {
    let load = |set: WeaponSet| -> Result<Locomotion, ExportError> {
        let graph = resource_data(
            content,
            &format!(
                "animation/graphs/worldmodel/worldmodel_locomotion.vnmgraph+{}.vnmgraph",
                set.name()
            ),
        )?;
        Locomotion::from_graph(&graph, |path: &str| -> Result<Clip, ExportError> {
            Clip::parse(&resource_data(content, path)?).map_err(|source| ExportError::Clip {
                path: path.to_owned(),
                source,
            })
        })
    };
    Ok(LocomotionSets {
        rifle: load(WeaponSet::Rifle)?,
        pistol: load(WeaponSet::Pistol)?,
        knife: load(WeaponSet::Knife)?,
    })
}

/// Writes the round's players as animated agents into `out_dir` (see the module docs). `demo_name` names the files
/// like the replay file (`<demo>_r<round>_ct.gltf`).
pub fn export_players(
    content: &Content,
    track: &RoundTrack,
    demo_name: &str,
    out_dir: &Path,
) -> Result<PlayersReport, ExportError> {
    let sets = load_locomotion(content)?;
    let skeleton_path = sets.rifle.clips[0].1.skeleton.clone();
    let clip_skeleton = NmSkeleton::parse(&resource_data(content, &skeleton_path)?);

    // Each player's team: the last one seen in the round.
    let mut teams: BTreeMap<u32, u32> = BTreeMap::new();
    for p in track.samples.iter().flat_map(|s| &s.players) {
        teams.insert(p.controller, p.team);
    }
    let mut report = PlayersReport {
        files: Vec::new(),
        players: 0,
        frames: 0,
        warnings: Vec::new(),
    };
    for (team, suffix, agent) in [(3, "ct", CT_AGENT), (2, "t", T_AGENT)] {
        let players: Vec<u32> = teams
            .iter()
            .filter(|&(_, &t)| t == team)
            .map(|(&c, _)| c)
            .collect();
        if players.is_empty() {
            continue;
        }
        let file_name = format!("{demo_name}_r{}_{suffix}", track.round);
        let mut frames_written = 0;
        let written =
            export_model_animated(content, agent, out_dir, Some(&file_name), |model, _| {
                let retargeter = Retargeter::new(&model.skeleton, &clip_skeleton);
                let mut animations = Vec::new();
                for &controller in &players {
                    let poses = bake_player(track, controller, &sets, PLAYER_ANIMATION_FPS)
                        .map_err(ExportError::Anim)?;
                    let poses = hold_gaps(poses, &clip_skeleton.reference);
                    frames_written = poses.len();
                    animations.push(BoneAnimation {
                        name: format!("player_{controller}"),
                        frames: poses.iter().map(|pose| retargeter.locals(pose)).collect(),
                        fps: PLAYER_ANIMATION_FPS,
                    });
                }
                Ok(animations)
            })?;
        report.files.push(written.gltf);
        report.warnings.extend(written.warnings);
        report.players += players.len();
        report.frames = frames_written;
    }
    Ok(report)
}

/// Fills the frames without a pose (no living pawn) with the last pose before them — or, before the first, the
/// first pose (the reference pose if the player never lived). Unreal hides the player there anyway; holding keeps
/// the keys from jumping.
fn hold_gaps(poses: Vec<Option<Vec<Transform>>>, reference: &[Transform]) -> Vec<Vec<Transform>> {
    let first = poses
        .iter()
        .flatten()
        .next()
        .cloned()
        .unwrap_or_else(|| reference.to_vec());
    let mut last = first;
    poses
        .into_iter()
        .map(|pose| {
            if let Some(pose) = pose {
                last = pose;
            }
            last.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32) -> Vec<Transform> {
        vec![Transform {
            translation: [x, 0.0, 0.0],
            ..Transform::IDENTITY
        }]
    }

    #[test]
    fn gaps_hold_the_pose_next_to_them() {
        let held = hold_gaps(
            vec![None, Some(pose(1.0)), None, Some(pose(2.0)), None],
            &pose(9.0),
        );
        let xs: Vec<f32> = held.iter().map(|p| p[0].translation[0]).collect();
        assert_eq!(xs, [1.0, 1.0, 1.0, 2.0, 2.0]);
        let never = hold_gaps(vec![None, None], &pose(9.0));
        assert_eq!(never[1][0].translation[0], 9.0);
    }
}
