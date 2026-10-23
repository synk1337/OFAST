//! The players of a demo round as animated CS2 agents with their weapons, for Unreal (M4.4, M4.5).
//!
//! Players who look the same share one glTF, next to the round's replay file:
//! `<demo>_r<round>_<agent>_<mesh groups>[_<gloves>].gltf`. A look is the agent model (the pawn's `m_hModel`,
//! resolved by resource id over CS2's models; the team's default agent when it is unknown), its mesh groups (a CT
//! agent shows or hides a defuse kit) and the glove skin. Each file holds ([`super::gltf::export_character`]):
//!
//! - the agent, skinned, with the meshes of those mesh groups;
//! - the gloves' world model (`items_game.txt` → `model_world`) bone-merged onto it, when the look has glove skins;
//! - one bone per weapon the look's players carry, with the weapon's world model (its own model: a knife skin's
//!   blade, the CS:GO-era mesh some gun skins need) rigidly on it ([`super::loadout`]);
//! - one animation per player, `player_<controller>` (the controller index of the replay file), keyed on every
//!   tick ([`animation_fps`]) from the round's first tick to its last: CS2's locomotion and weapon layers baked from
//!   the demo's rig inputs ([`crate::anim::bake`]) and retargeted onto the agent, plus every weapon bone on the hand,
//!   on its holster, or scaled to 0; from each death on, the body falling as the agent's ragdoll ([`super::deaths`]).
//!
//! The weapons lying in the world (a dead player's gun, a dropped one: the replay's `dropped_weapons`) get one glTF
//! per look, the weapon model alone: `<demo>_r<round>_dropped_<model>_<mesh groups>.gltf`.
//!
//! `<demo>_r<round>.agents.json` lists the files, their looks, their players, the deaths baked as ragdolls and the
//! dropped weapons' models (`docs/formats/replay.md`);
//! `tools/unreal/import_demo.py` plays each animation on its player's actor, which the replay's positions and body
//! yaw move.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::json;

use super::ExportError;
use super::deaths::bake_deaths;
use super::gltf::{
    Binding, BoneAnimation, MergedModel, export_character, export_model_static, load_model,
    resource_data,
};
use super::loadout::{
    AgentMounts, Place, Slot, WeaponFrames, WeaponLook, carried_places, hide_changes, key_places,
    skeleton_with_slots, slot_target,
};
use crate::anim::aim::AimBones;
use crate::anim::bake::{
    ActionGraph, GraphRig, LocomotionSets, UpperBody, WeaponAnimation, WeaponSet, bake_player,
    frame_samples,
};
use crate::anim::graph::Graph;
use crate::anim::layers::BoneMask;
use crate::anim::weapon::WeaponClips;
use crate::anim::{
    BoneConstraints, Clip, Locomotion, NmSkeleton, Retargeter, Transform, matrix_to_quat,
};
use crate::content::Content;
use crate::demo::players::{COUNTER_TERRORISTS, PlayerSample, RoundTrack};
use crate::items::{item_names, item_values};
use crate::model::{Affine, Model, compose, invert_rigid};
use crate::physics::PhysicsData;
use crate::physics::ragdoll::WorldCollision;
use crate::resource::Resource;

/// Agent model of Counter-Terrorists whose own is unknown.
pub const CT_AGENT: &str = "agents/models/ctm_sas/ctm_sas.vmdl";
/// Agent model of Terrorists whose own is unknown.
pub const T_AGENT: &str = "agents/models/tm_phoenix/tm_phoenix.vmdl";
/// Key rate of a round's player animations: the demo's tick rate (64), one key per tick. Keys then fall on the
/// ticks the sequence shows (`import_demo.py` plays it at the tick rate and imports the animations at this rate),
/// so a weapon switch happens on its tick: at 30 keys per second (M4.4) the weapon vanished for two frames, 66 ms,
/// around every switch, and Unreal's blend between keys an eighth of a tick apart smeared each change.
pub fn animation_fps(track: &RoundTrack) -> f32 {
    (1.0 / track.tick_interval).round().max(1.0)
}

/// What [`export_players`] wrote.
#[derive(Debug, Clone)]
pub struct PlayersReport {
    /// The glTF files, one per look.
    pub files: Vec<PathBuf>,
    /// The manifest listing them.
    pub manifest: PathBuf,
    /// Players animated.
    pub players: usize,
    /// Weapon bones over all files.
    pub weapons: usize,
    /// Deaths baked as ragdolls ([`super::deaths`]).
    pub deaths: usize,
    /// Weapon models written for the weapons lying in the world (`<demo>_r<round>_dropped_…`).
    pub dropped: usize,
    /// One line per death: who, when, how far the body went and when it lay still.
    pub falls: Vec<String>,
    /// Key rate of the animations ([`animation_fps`]).
    pub fps: f32,
    /// Frames per animation.
    pub frames: usize,
    /// Problems that did not stop the export.
    pub warnings: Vec<String>,
}

/// A set's locomotion graph (`worldmodel_locomotion.vnmgraph+rifle`…).
fn locomotion_graph(set: WeaponSet) -> String {
    format!(
        "animation/graphs/worldmodel/worldmodel_locomotion.vnmgraph+{}.vnmgraph",
        set.name()
    )
}

/// Reads CS2's three locomotion graphs and their clips.
pub fn load_locomotion(content: &Content) -> Result<LocomotionSets, ExportError> {
    let load = |set: WeaponSet| -> Result<Locomotion, ExportError> {
        let graph = resource_data(content, &locomotion_graph(set))?;
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

/// How a group of players looks: what one glTF holds besides their animations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Look {
    /// Agent model path (`.vmdl`, no `_c`).
    agent: String,
    /// The agent's mesh groups shown (0 = the model's default).
    mesh_groups: u64,
    /// Glove world model path, when the players wear glove skins.
    gloves: Option<String>,
}

/// What every look's export shares.
struct Round<'a> {
    content: &'a Content,
    track: &'a RoundTrack,
    sets: LocomotionSets,
    clip_skeleton: NmSkeleton,
    upper: Option<UpperBody>,
    /// CS2's graphs for the graph-evaluated layers (starts, jumps, actions, planting, defusing…).
    graphs: Option<GraphRig>,
    /// Model resource id → path.
    models: HashMap<u64, String>,
    /// Key rate of the animations.
    fps: f32,
    /// For each animation frame: its demo tick and sample index.
    frames: Vec<(f32, usize)>,
    /// The map's solid collision, for the bodies of the dead (`None` when it could not be read).
    collision: Option<WorldCollision>,
}

/// The solid collision of `map` (`maps/<map>/world_physics.vmdl`), or why it cannot be read (the map's archive is
/// not open, or the file is broken).
fn load_world_collision(content: &Content, map: &str) -> Result<WorldCollision, String> {
    let path = format!("maps/{map}/world_physics.vmdl");
    let bytes = content
        .read_resource(&path)
        .map_err(|e| format!("{path}: {e} (is the map's archive open?)"))?;
    let resource = Resource::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let physics = PhysicsData::from_resource(&resource)
        .map_err(|e| format!("{path}: {e}"))?
        .ok_or_else(|| format!("{path}: no PHYS block"))?;
    Ok(WorldCollision::from_physics(&physics))
}

/// An agent's ragdoll (its `PHYS` block); `None` when it has none.
fn load_ragdoll(content: &Content, agent: &str) -> Option<PhysicsData> {
    let bytes = content.read_resource(agent).ok()?;
    let resource = Resource::parse(&bytes).ok()?;
    PhysicsData::from_resource(&resource).ok()?
}

/// Everything the bake reads from CS2 for a round's players ([`load_player_rig`]).
pub struct PlayerRig {
    /// The three locomotion sets.
    pub sets: LocomotionSets,
    /// The clip skeleton (`worldmodel.vnmskel`).
    pub clip_skeleton: NmSkeleton,
    /// The upper-body layers' data, when readable.
    pub upper: Option<UpperBody>,
    /// The graphs of the graph-evaluated layers, when readable.
    pub graphs: Option<GraphRig>,
    /// `items_game.txt`, when readable.
    pub items_text: Option<String>,
    /// Problems that leave layers out.
    pub warnings: Vec<String>,
}

/// Reads what [`bake_player`] needs for a round's players: the locomotion sets, the upper-body data for the weapons
/// they hold, and the graphs ([`GraphRig`]). A missing part leaves its layers out, with a warning.
pub fn load_player_rig(content: &Content, track: &RoundTrack) -> Result<PlayerRig, ExportError> {
    let sets = load_locomotion(content)?;
    let skeleton_path = sets.rifle.clips[0].1.skeleton.clone();
    let skeleton_data = resource_data(content, &skeleton_path)?;
    let clip_skeleton = NmSkeleton::parse(&skeleton_data);
    let mut warnings = Vec::new();
    let items_text = match content.read(ITEMS_GAME) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => {
            warnings.push(format!(
                "{ITEMS_GAME}: {e}: no weapon layers, holsters or glove skins"
            ));
            None
        }
    };
    let (mut upper, sources) = match load_upper_body(
        content,
        track,
        items_text.as_deref(),
        &skeleton_data,
        &clip_skeleton,
        &mut warnings,
    )? {
        Some((upper, sources)) => (Some(upper), sources),
        None => (None, HashMap::new()),
    };
    let graphs = match load_graph_rig(
        content,
        upper.as_mut(),
        &sources,
        &skeleton_data,
        &clip_skeleton,
    ) {
        Ok(rig) => Some(rig),
        Err(e) => {
            warnings.push(format!(
                "animation graphs: {e}: no starts, turns, jumps, ladders, actions, planting or defusing"
            ));
            None
        }
    };
    Ok(PlayerRig {
        sets,
        clip_skeleton,
        upper,
        graphs,
        items_text,
        warnings,
    })
}

/// Writes the round's players as animated agents into `out_dir` (see the module docs). `demo_name` names the files
/// like the replay file (`<demo>_r<round>_…`).
pub fn export_players(
    content: &Content,
    track: &RoundTrack,
    demo_name: &str,
    out_dir: &Path,
) -> Result<PlayersReport, ExportError> {
    let PlayerRig {
        sets,
        clip_skeleton,
        upper,
        graphs,
        items_text,
        mut warnings,
    } = load_player_rig(content, track)?;
    let glove_models = items_text
        .as_deref()
        .map(|text| item_values(text, "model_world"))
        .unwrap_or_default();
    let collision = load_world_collision(content, &track.map_name)
        .inspect_err(|e| {
            warnings.push(format!(
                "no collision for {}: {e}: the dead fall onto a floor where they last stood",
                track.map_name
            ))
        })
        .ok();
    let round = Round {
        content,
        track,
        sets,
        clip_skeleton,
        upper,
        graphs,
        models: content.resource_paths("vmdl"),
        fps: animation_fps(track),
        frames: frame_samples(track, animation_fps(track)),
        collision,
    };

    // Each player's look, from the last sample where they were alive (else their last one).
    let mut last: BTreeMap<u32, &PlayerSample> = BTreeMap::new();
    for p in track.samples.iter().flat_map(|s| &s.players) {
        if p.alive || last.get(&p.controller).is_none_or(|q| !q.alive) {
            last.insert(p.controller, p);
        }
    }
    let mut groups: BTreeMap<Look, Vec<u32>> = BTreeMap::new();
    for (&controller, p) in &last {
        // Mesh-group bits belong to one model: a default agent gets its own default groups.
        let (agent, mesh_groups) = match round
            .models
            .get(&p.appearance.model)
            .filter(|path| path.starts_with("agents/models/"))
        {
            Some(path) => (path.clone(), p.appearance.mesh_groups),
            None => {
                if p.appearance.model != 0 {
                    warnings.push(format!(
                        "player {controller}: model {:016x} is not a known agent: the team's default",
                        p.appearance.model
                    ));
                }
                let agent = if p.team == COUNTER_TERRORISTS {
                    CT_AGENT
                } else {
                    T_AGENT
                };
                (agent.to_owned(), 0)
            }
        };
        let gloves = (p.appearance.gloves != 0)
            .then(|| glove_models.get(&p.appearance.gloves).cloned())
            .flatten();
        if p.appearance.gloves != 0 && gloves.is_none() {
            warnings.push(format!(
                "player {controller}: gloves item {} has no world model: the agent's own",
                p.appearance.gloves
            ));
        }
        let look = Look {
            agent,
            mesh_groups,
            gloves,
        };
        groups.entry(look).or_default().push(controller);
    }

    let mut report = PlayersReport {
        files: Vec::new(),
        manifest: out_dir.join(format!("{demo_name}_r{}.agents.json", track.round)),
        players: 0,
        weapons: 0,
        deaths: 0,
        dropped: 0,
        falls: Vec::new(),
        fps: round.fps,
        frames: round.frames.len(),
        warnings,
    };
    let mut manifest = Vec::new();
    for (look, players) in &groups {
        let mut name = format!(
            "{demo_name}_r{}_{}_{}",
            track.round,
            stem(&look.agent),
            look.mesh_groups
        );
        if let Some(gloves) = &look.gloves {
            name = format!("{name}_{}", stem(gloves));
        }
        let (written, weapons, falls) = export_look(&round, look, players, out_dir, &name)?;
        report.deaths += falls.len();
        // Each player's baked deaths: the ticks of their first and last frames.
        let mut ragdolls: BTreeMap<String, Vec<[f32; 2]>> = BTreeMap::new();
        for (controller, fall) in &falls {
            let last = round.frames[fall.end_frame - 1].0;
            ragdolls
                .entry(controller.to_string())
                .or_default()
                .push([fall.tick, last]);
        }
        for (controller, fall) in falls {
            let name = last.get(&controller).map_or("?", |p| p.name.as_str());
            let at = format!(
                "({:.0}, {:.0}, {:.0})",
                fall.rest[0], fall.rest[1], fall.rest[2]
            );
            // How far the body's centre went, and how much of it down (lying down alone is about 0.7 m).
            let moved = format!(
                "{:.1} m ({:.1} m down)",
                fall.distance() * 0.0254,
                (fall.start[2] - fall.rest[2]) * 0.0254
            );
            report.falls.push(match fall.frames_to_rest {
                Some(frames) => format!(
                    "{name} (tick {:.0}): the body moved {moved} and lay still after {:.1} s at {at}",
                    fall.tick,
                    frames as f32 / round.fps
                ),
                None => format!(
                    "{name} (tick {:.0}): the body moved {moved}, still moving at the round's end at {at}",
                    fall.tick
                ),
            });
        }
        manifest.push(json!({
            "file": format!("{name}.gltf"),
            "model": look.agent,
            "mesh_groups": look.mesh_groups,
            "gloves": look.gloves,
            "players": players,
            "ragdolls": ragdolls,
            "weapons": weapons.iter().map(|s| json!({
                "model": s.look.model,
                "mesh_groups": s.look.mesh_groups,
                "instance": s.instance,
            })).collect::<Vec<_>>(),
        }));
        report.files.push(written.gltf);
        report.warnings.extend(written.warnings);
        report.players += players.len();
        report.weapons += weapons.len();
    }
    let dropped = export_dropped(&round, demo_name, out_dir, &mut report);
    std::fs::write(
        &report.manifest,
        serde_json::to_string_pretty(&json!({
            "format": "ofast-agents",
            "version": 1,
            "demo": demo_name,
            "round": track.round,
            "fps": round.fps,
            "agents": manifest,
            "dropped_weapons": dropped,
        }))
        .expect("JSON serializes"),
    )?;
    Ok(report)
}

/// Writes one glTF per look of the weapons lying in the world during the round (the replay's `dropped_weapons`):
/// `<demo>_r<round>_dropped_<model file stem>_<mesh groups shown>.gltf`, the weapon model alone in its mesh groups
/// and rigid (a static mesh: it does not animate), so Unreal can place it where the demo has it. Returns the
/// manifest entries: file, model path, model id, and every mesh-group mask the replay names that shows this file
/// (grenades send every bit, which shows the model's default groups). A model that cannot be read is left out with
/// a warning, like a carried weapon's.
fn export_dropped(
    round: &Round<'_>,
    demo_name: &str,
    out_dir: &Path,
    report: &mut PlayersReport,
) -> Vec<serde_json::Value> {
    let mut looks: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for w in round.track.samples.iter().flat_map(|s| &s.dropped) {
        looks.entry(w.model).or_default().insert(w.mesh_groups);
    }
    let mut entries = Vec::new();
    let mut names = HashSet::new();
    for (id, masks) in looks {
        let Some(path) = round.models.get(&id) else {
            report.warnings.push(format!(
                "dropped weapon model {id:016x} is not a known model: not shown"
            ));
            continue;
        };
        let model = match load_model(round.content, path) {
            Ok(model) => model,
            Err(e) => {
                report
                    .warnings
                    .push(format!("dropped weapon {path}: {e}: not shown"));
                continue;
            }
        };
        // One file per set of groups shown, named by it: several masks can show the same.
        let mut by_shown: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for mask in masks {
            by_shown
                .entry(shown_groups(&model, mask))
                .or_default()
                .push(mask);
        }
        for (shown, masks) in by_shown {
            let mut name = format!(
                "{demo_name}_r{}_dropped_{}_{shown}",
                round.track.round,
                stem(path)
            );
            // Another model with the same file stem: keep the files apart.
            if !names.insert(name.clone()) {
                name = format!("{name}_{id:016x}");
                names.insert(name.clone());
            }
            match export_model_static(round.content, &model, shown, out_dir, &name) {
                Ok(written) => {
                    entries.push(json!({
                        "file": format!("{name}.gltf"),
                        "model": path,
                        "model_id": format!("{id:016x}"),
                        "mesh_groups": masks,
                    }));
                    report.files.push(written.gltf);
                    report.warnings.extend(written.warnings);
                    report.dropped += 1;
                }
                Err(e) => report
                    .warnings
                    .push(format!("dropped weapon {path}: {e}: not shown")),
            }
        }
    }
    entries
}

/// What [`export_look`] returns: the glTF export's report, the weapon slots it holds, and each death baked as a
/// ragdoll (the player's controller, the fall).
type LookExport = (
    super::gltf::ExportReport,
    Vec<Slot>,
    Vec<(u32, super::deaths::DeathSummary)>,
);

/// Writes one look's glTF ([`LookExport`]).
fn export_look(
    round: &Round<'_>,
    look: &Look,
    players: &[u32],
    out_dir: &Path,
    file_name: &str,
) -> Result<LookExport, ExportError> {
    let content = round.content;
    let track = round.track;
    let agent = load_model(content, &look.agent)?;
    let ragdoll = load_ragdoll(content, &look.agent);
    let mut mesh_groups = shown_groups(&agent, look.mesh_groups);
    let mounts = AgentMounts::of(&agent);
    let retargeter = Retargeter::new(&agent.skeleton, &round.clip_skeleton);
    // The agent's twist bones and shoulder blades, placed after each pose as CS2 does. Its glove skin, merged by
    // bone name, follows them.
    let constraints = BoneConstraints::of(&agent);
    let mut warnings: Vec<String> = constraints
        .skipped
        .iter()
        .filter(|s| !s.contains("CBoneConstraintDotToMorph"))
        .map(|s| format!("{}: constraint {s}: not applied", look.agent))
        .collect();

    // Bodies, and each carried weapon's slot and place, frame by frame.
    let weapon_look = |w: &crate::demo::players::CarriedWeapon| {
        Some(WeaponLook {
            model: round.models.get(&w.model)?.clone(),
            mesh_groups: w.mesh_groups,
        })
    };
    let kind = |item: u32| {
        let weapon = round.upper.as_ref()?.weapons.get(&item)?;
        Some((weapon.weapon.as_str(), weapon.category.as_str()))
    };
    let mut bodies = Vec::new();
    let mut places: Vec<Vec<Vec<(Slot, Place)>>> = Vec::new();
    let mut slots: BTreeSet<Slot> = BTreeSet::new();
    for &controller in players {
        let poses = bake_player(
            track,
            controller,
            &round.sets,
            round.upper.as_ref(),
            round.graphs.as_ref(),
            round.fps,
        )?;
        let carried: Vec<Vec<(Slot, Place)>> = round
            .frames
            .iter()
            .zip(&poses)
            .map(|(&(tick, index), pose)| {
                let player = track.samples[index]
                    .players
                    .iter()
                    .find(|p| p.controller == controller && p.alive);
                match (pose, player) {
                    (Some(_), Some(p)) => carried_places(
                        &p.weapons,
                        in_hand(p, tick, track.tick_interval),
                        weapon_look,
                        kind,
                    ),
                    _ => Vec::new(),
                }
            })
            .collect();
        slots.extend(carried.iter().flatten().map(|(slot, _)| slot.clone()));
        bodies.push(poses);
        places.push(carried);
    }

    // The weapon models; a slot whose model cannot be read is left out.
    let mut weapon_models: BTreeMap<String, (Model, WeaponFrames)> = BTreeMap::new();
    for slot in &slots {
        if weapon_models.contains_key(&slot.look.model) {
            continue;
        }
        match load_model(content, &slot.look.model) {
            Ok(model) => match WeaponFrames::of(&model) {
                Some(frames) => {
                    weapon_models.insert(slot.look.model.clone(), (model, frames));
                }
                None => warnings.push(format!("{}: no `weapon` bone: not shown", slot.look.model)),
            },
            Err(e) => warnings.push(format!("{}: {e}: not shown", slot.look.model)),
        }
    }
    let slots: Vec<Slot> = slots
        .into_iter()
        .filter(|s| weapon_models.contains_key(&s.look.model))
        .collect();

    let skeleton = skeleton_with_slots(&agent, &mounts, slots.len());
    let first_slot_bone = agent.skeleton.bones.len();
    let bind = skeleton.world_transforms();
    let mut merged = Vec::new();
    for (n, slot) in slots.iter().enumerate() {
        let (model, frames) = &weapon_models[&slot.look.model];
        let bone = first_slot_bone + n;
        merged.push(MergedModel {
            label: format!("w{n}_{}", stem(&slot.look.model)),
            model,
            mesh_groups: shown_groups(model, slot.look.mesh_groups),
            binding: Binding::Rigid {
                bone,
                placement: compose(&bind[bone], &invert_rigid(&frames.weapon_bone)),
            },
        });
    }
    let gloves = match &look.gloves {
        Some(path) => match load_model(content, path) {
            Ok(model) => Some(model),
            Err(e) => {
                warnings.push(format!("gloves {path}: {e}: the agent's own"));
                None
            }
        },
        None => None,
    };
    match &gloves {
        Some(model) => merged.push(MergedModel {
            label: "gloves".into(),
            model,
            mesh_groups: model.default_mesh_group_mask,
            binding: Binding::ByName,
        }),
        // No glove skin to merge: the agent's own gloves, even if the pawn asked to hide them.
        None => mesh_groups = with_own_gloves(&agent, mesh_groups),
    }

    // One animation per player: the agent's bones, then the weapon bones.
    let mut animations = Vec::new();
    let mut falls = Vec::new();
    for ((&controller, poses), carried) in players.iter().zip(bodies).zip(&places) {
        let poses = hold_gaps(poses, &round.clip_skeleton.reference);
        let mut worlds: Vec<Vec<Affine>> = poses
            .iter()
            .map(|pose| {
                let mut world = retargeter.world(pose);
                constraints.apply(&mut world);
                world
            })
            .collect();
        // Each death: the body falls as the agent's ragdoll and stays.
        if let Some(ragdoll) = &ragdoll {
            let deaths = bake_deaths(
                track,
                controller,
                &round.frames,
                round.fps,
                &agent,
                ragdoll,
                round.collision.as_ref(),
                &mut worlds,
            );
            for summary in &deaths.summaries {
                falls.push((controller, *summary));
                // The twist bones and shoulder blades follow the fallen limbs (only there: a living pose already
                // has them, and a point constraint would move a second time).
                for world in &mut worlds[summary.first_frame..summary.end_frame] {
                    constraints.apply(world);
                }
            }
        }
        // Each weapon bone, frame by frame: where it is keyed (relative to the root) and whether it shows.
        let slot_keys: Vec<Vec<(Affine, bool)>> = slots
            .iter()
            .map(|slot| {
                let frames_of = &weapon_models[&slot.look.model].1;
                let target = |k: usize, place: Place| {
                    let world = &worlds[k];
                    slot_target(world, &mounts, frames_of, place)
                        .map(|t| compose(&invert_rigid(&world[0]), &t))
                };
                // A place the agent or weapon cannot show (no such attachment) counts as hidden.
                let places: Vec<Place> = (0..worlds.len())
                    .map(|k| {
                        let place = place_of(carried.get(k), slot);
                        if target(k, place).is_some() {
                            place
                        } else {
                            Place::Hidden
                        }
                    })
                    .collect();
                key_places(&hide_changes(&places))
                    .into_iter()
                    .enumerate()
                    .map(|(k, (place, shown))| {
                        let local = target(k, place)
                            .or_else(|| target(k, Place::Hand))
                            .unwrap_or_else(|| {
                                let b = &skeleton.bones[first_slot_bone];
                                crate::model::affine(b.rotation, b.position)
                            });
                        (local, shown)
                    })
                    .collect()
            })
            .collect();
        let mut frames = Vec::with_capacity(poses.len());
        let mut scales = Vec::with_capacity(poses.len());
        for (k, world) in worlds.iter().enumerate() {
            let mut locals = retargeter.locals_of(world);
            let mut frame_scales = vec![1.0; skeleton.bones.len()];
            for (n, keys) in slot_keys.iter().enumerate() {
                let (local, shown) = keys[k];
                if !shown {
                    frame_scales[first_slot_bone + n] = 0.0;
                }
                locals.push((
                    [local[0][3], local[1][3], local[2][3]],
                    matrix_to_quat(&local),
                ));
            }
            frames.push(locals);
            scales.push(frame_scales);
        }
        animations.push(BoneAnimation {
            name: format!("player_{controller}"),
            frames,
            fps: round.fps,
            scales,
        });
    }

    let mut written = export_character(
        content,
        &agent,
        mesh_groups,
        &skeleton,
        &merged,
        &animations,
        out_dir,
        file_name,
    )?;
    written.warnings.extend(warnings);
    Ok((written, slots, falls))
}

/// Where `slot`'s weapon is in one frame's list (hidden when the player does not carry it).
fn place_of(carried: Option<&Vec<(Slot, Place)>>, slot: &Slot) -> Place {
    carried
        .and_then(|list| list.iter().find(|(s, _)| s == slot))
        .map_or(Place::Hidden, |(_, place)| *place)
}

/// The mesh groups to show: `mask` as the entity sent it, or the model's default when it sent none (0) or all bits
/// (knives and grenades send every bit).
fn shown_groups(model: &Model, mask: u64) -> u64 {
    if mask == 0 || mask == u64::MAX {
        model.default_mesh_group_mask
    } else {
        mask
    }
}

/// `mask` with every `…_hide_default_gloves` choice of the agent swapped for the `…_default` one of the same body
/// group (`first_or_third_person_@1_#&thirdperson_hide_default_gloves` → `…_@0_#&thirdperson_default`).
fn with_own_gloves(agent: &Model, mask: u64) -> u64 {
    let mut out = mask;
    for (i, group) in agent.mesh_groups.iter().enumerate() {
        let Some(choice) = group.split("#&").nth(1) else {
            continue;
        };
        if out & (1 << i) == 0 || !choice.ends_with("_hide_default_gloves") {
            continue;
        }
        let own = choice.replace("_hide_default_gloves", "_default");
        if let Some(j) = agent
            .mesh_groups
            .iter()
            .position(|g| g.split("#&").nth(1) == Some(own.as_str()))
        {
            out = (out & !(1 << i)) | (1 << j);
        }
    }
    out
}

/// How long a weapon being drawn stays in its holster, seconds: the weapons layer's blend into `Deploy` (0.1 s in
/// every weapon graph), after which the draw clip holds the hand at the holster (`draw_ak` starts with `wpn` on the
/// back) and takes the weapon from there. CS2 too shows a rifle still slung on the tick its draw begins.
const DRAW_FROM_HOLSTER: f32 = 0.1;

/// The entity of the weapon in a player's hand at demo tick `tick` (fractional): the active weapon, except during
/// the first [`DRAW_FROM_HOLSTER`] of its draw (gameplay state 11, from its timestamp), when nothing is in hand yet.
fn in_hand(player: &PlayerSample, tick: f32, interval: f32) -> Option<u32> {
    let weapon = player.rig.weapon.as_ref()?;
    let drawing = weapon.anim_state == 11
        && weapon
            .state_tick
            .is_some_and(|start| (tick - start as f32) * interval < DRAW_FROM_HOLSTER);
    (!drawing).then_some(weapon.entity)
}

/// A model path's file name without its extension (`agents/models/ctm_sas/ctm_sas.vmdl` → `ctm_sas`).
fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file)
}

/// The main graph of third-person animation.
const MAIN_GRAPH: &str = "animation/graphs/worldmodel/worldmodel.vnmgraph";
/// CS2's item definitions.
const ITEMS_GAME: &str = "scripts/items/items_game.txt";

/// Where each weapon's weapons-layer actions are, by item: its state in the main graph's `Weapons/State/Weapons`
/// and the graph that state plays, if any ([`WeaponClips::action_state`], [`WeaponClips::action_graph`]).
type ActionSources = HashMap<u32, (Option<usize>, Option<String>)>;

/// The upper-body layers' data for every weapon the round's players hold or carry: names from `items_game.txt`,
/// clips found in the graphs ([`WeaponClips`]; their category also picks a carried weapon's holster), the
/// `UpperBody` masks and the aim's bones, and where each weapon's actions are ([`load_graph_rig`] loads them).
/// `None` (with a warning) when one of them cannot be read: the players then get the locomotion alone, and their
/// weapons show only in hand.
fn load_upper_body(
    content: &Content,
    track: &RoundTrack,
    items_text: Option<&str>,
    skeleton_data: &crate::kv3::Value,
    skeleton: &NmSkeleton,
    warnings: &mut Vec<String>,
) -> Result<Option<(UpperBody, ActionSources)>, ExportError> {
    let (Some(mask), Some(aim)) = (
        BoneMask::from_skeleton(skeleton_data, skeleton, "UpperBody"),
        AimBones::find(skeleton),
    ) else {
        warnings
            .push("clip skeleton without an UpperBody mask or aim bones: locomotion only".into());
        return Ok(None);
    };
    let Some(text) = items_text else {
        return Ok(None);
    };
    let names = item_names(text);
    let graph = |path: &str| -> Result<Graph, ExportError> {
        Ok(Graph::parse(&resource_data(content, path)?)?)
    };
    let main = graph(MAIN_GRAPH)?;
    let clip = |path: &String| -> Result<Clip, ExportError> {
        Clip::parse(&resource_data(content, path)?).map_err(|source| ExportError::Clip {
            path: path.clone(),
            source,
        })
    };
    let pair = |paths: &[String]| -> Result<Option<[Clip; 2]>, ExportError> {
        Ok(match paths {
            [] => None,
            [only] => Some([clip(only)?, clip(only)?]),
            // Standing first, crouched second.
            _ => {
                let stand = &paths[0];
                let crouch = paths.iter().find(|p| p.contains("crouch")).unwrap_or(stand);
                Some([clip(stand)?, clip(crouch)?])
            }
        })
    };
    let items: BTreeSet<u32> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .flat_map(|p| {
            p.rig
                .weapon
                .as_ref()
                .map(|w| w.item)
                .into_iter()
                .chain(p.weapons.iter().map(|w| w.item))
        })
        .collect();
    let mut weapons = HashMap::new();
    let mut sources = ActionSources::new();
    for item in items {
        let Some(name) = names.get(&item) else {
            warnings.push(format!("weapon item {item}: not in {ITEMS_GAME}"));
            continue;
        };
        let clips = WeaponClips::find(&main, name, |path| {
            graph(path).map_err(|e| crate::anim::AnimError::Graph(format!("{path}: {e}")))
        })?;
        let Some(category) = clips.category.clone() else {
            warnings.push(format!("weapon {name}: not in the animation graph"));
            continue;
        };
        sources.insert(item, (clips.action_state, clips.action_graph.clone()));
        weapons.insert(
            item,
            WeaponAnimation {
                weapon: name.clone(),
                category,
                idle: pair(&clips.idle)?,
                shoot: clips.shoot.first().map(clip).transpose()?,
                actions: None,
            },
        );
    }
    let upper = UpperBody {
        weapons,
        mask,
        ladder_mask: BoneMask::from_skeleton(skeleton_data, skeleton, "UpperBody_Ladder"),
        aim,
        parents: skeleton.parents.clone(),
    };
    Ok(Some((upper, sources)))
}

/// CS2's graphs for the graph-evaluated layers ([`GraphRig`]): the three locomotion graphs whole, the main graph's
/// planting, defuse and body-additive states, and each weapon's actions (its own graph, or its equipment state in
/// the main graph), which this also sets on `upper`'s weapons. The masks come from the clip skeleton.
fn load_graph_rig(
    content: &Content,
    upper: Option<&mut UpperBody>,
    sources: &ActionSources,
    skeleton_data: &crate::kv3::Value,
    skeleton: &NmSkeleton,
) -> Result<GraphRig, ExportError> {
    let mut load_graph = |path: &str| -> Result<Graph, ExportError> {
        Ok(Graph::parse(&resource_data(content, path)?)?)
    };
    let mut load_clip = |path: &str| -> Result<Clip, ExportError> {
        Clip::parse(&resource_data(content, path)?).map_err(|source| ExportError::Clip {
            path: path.to_owned(),
            source,
        })
    };
    let mask = |id: &str| BoneMask::from_skeleton(skeleton_data, skeleton, id);
    let mut rig = GraphRig {
        locomotion: WeaponSet::ALL.map(locomotion_graph),
        main: MAIN_GRAPH.to_owned(),
        defuse_mask: mask("Defuse"),
        defuse_grenade_mask: mask("Defuse_Grenade"),
        breathing_mask: mask("Breathing"),
        flashed_mask: mask("Flashed"),
        ..GraphRig::default()
    };
    for path in rig.locomotion.clone() {
        rig.library
            .load(&path, None, &mut load_graph, &mut load_clip)?;
    }
    let main = rig.library.load_graph(MAIN_GRAPH, &mut load_graph)?;
    let node = |path: &str| main.find(path);
    rig.planting = node("SM0/Planting");
    rig.defuse = node("Defuse/Defuse");
    rig.jump_start = node("BodyAdditives/JumpAdditiveStart");
    rig.jump_land = node("BodyAdditives/JumpAdditiveLand");
    rig.breathing = node("BodyAdditives/Breathing");
    rig.body_flinch = node("BodyFlinch/Flinch_WPNs");
    rig.head_flinch = node("HeadFlinch/Flinch_WPNs");
    rig.fire_flinch = node("FireFlinch/OnFire_WPNs");
    rig.flashed = node("flashed/flashed");
    for main_node in [
        rig.planting,
        rig.defuse,
        rig.jump_start,
        rig.jump_land,
        rig.breathing,
        rig.body_flinch,
        rig.head_flinch,
        rig.fire_flinch,
        rig.flashed,
    ]
    .into_iter()
    .flatten()
    {
        rig.library
            .load(MAIN_GRAPH, Some(main_node), &mut load_graph, &mut load_clip)?;
    }
    if let Some(upper) = upper {
        for (item, weapon) in &mut upper.weapons {
            weapon.actions = match sources.get(item) {
                Some((_, Some(graph))) => {
                    rig.library
                        .load(graph, None, &mut load_graph, &mut load_clip)?;
                    rig.library
                        .graph(graph)
                        .and_then(|g| g.root)
                        .map(|node| ActionGraph {
                            graph: graph.clone(),
                            node,
                        })
                }
                Some((Some(state), None)) => {
                    rig.library
                        .load(MAIN_GRAPH, Some(*state), &mut load_graph, &mut load_clip)?;
                    Some(ActionGraph {
                        graph: MAIN_GRAPH.to_owned(),
                        node: *state,
                    })
                }
                _ => None,
            };
        }
    }
    Ok(rig)
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

    #[test]
    fn stems_and_shown_groups() {
        assert_eq!(stem("agents/models/ctm_sas/ctm_sas.vmdl"), "ctm_sas");
        assert_eq!(stem("weapon_rif_ak47.vmdl"), "weapon_rif_ak47");
        // The SAS agent's groups: what a pawn wearing glove skins sends, and the agent's own gloves back.
        let agent = Model {
            meshes: Vec::new(),
            mesh_groups: [
                "defusekit_@0_#&defusekit_hidden",
                "defusekit_@1_#&defusekit_visible",
                "first_or_third_person_@0_#&thirdperson_default",
                "first_or_third_person_@1_#&thirdperson_hide_default_gloves",
            ]
            .map(String::from)
            .to_vec(),
            mesh_group_masks: Vec::new(),
            default_mesh_group_mask: 5,
            lod_masks: Vec::new(),
            external_meshes: Vec::new(),
            skeleton: Default::default(),
            remapping_table: Vec::new(),
            remapping_starts: Vec::new(),
            key_values_text: String::new(),
            reference_pose: None,
        };
        assert_eq!(with_own_gloves(&agent, 0b1001), 0b0101);
        assert_eq!(with_own_gloves(&agent, 0b0110), 0b0110);
        assert_eq!(shown_groups(&agent, 0), 5);
        assert_eq!(shown_groups(&agent, u64::MAX), 5);
        assert_eq!(shown_groups(&agent, 6), 6);
    }
}
