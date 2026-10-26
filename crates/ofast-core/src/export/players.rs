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
//! The players' first-person views (their arms and the weapon in hand, as CS2 draws them in a POV shot) get one
//! glTF per agent and glove skin: `<demo>_r<round>_viewmodel_<agent>[_<gloves>].gltf` ([`super::viewmodel`]).
//!
//! CS2's sprite sheets for the grenades' effects are written next to them too ([`super::effects`]).
//!
//! `<demo>_r<round>.agents.json` lists the files, their looks, their players, the deaths baked as ragdolls, the
//! dropped weapons' models, the viewmodels and the effect textures (`docs/formats/replay.md`);
//! `tools/unreal/import_demo.py` plays each animation on its player's actor, which the replay's positions and body
//! yaw move.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::json;

use super::ExportError;
use super::brass::{brass_json, load_casings};
use super::deaths::bake_deaths;
use super::effects::{blasts_json, export_effect_textures, fire_walls_json};
use super::gltf::{
    Binding, BoneAnimation, MergedModel, export_character, export_model_static, load_model,
    resource_data,
};
use super::loadout::{
    AgentMounts, Place, Slot, WeaponFrames, WeaponLook, carried_places, hide_changes, key_places,
    skeleton_with_slots, slot_target,
};
use super::punch::{punch_json, recoil_by_tick};
use super::shots::{
    Body, Capsule, Doors, ShotData, earlier_holes_json, export_decals, hitbox_check, shots_json,
};
use super::viewmodel::{export_viewmodel_look, load_viewmodel_rig};
use crate::anim::aim::AimBones;
use crate::anim::bake::{
    ActionGraph, GraphRig, LocomotionSets, UpperBody, WeaponAnimation, WeaponSet, bake_player,
    frame_samples,
};
use crate::anim::graph::Graph;
use crate::anim::layers::BoneMask;
use crate::anim::viewmodel::ARMS_MODEL;
use crate::anim::weapon::WeaponClips;
use crate::anim::{
    BoneConstraints, Clip, Locomotion, NmSkeleton, Retargeter, Transform, matrix_to_quat,
};
use crate::content::Content;
use crate::demo::players::{COUNTER_TERRORISTS, PlayerSample, RoundTrack};
use crate::items::{item_names, item_values};
use crate::model::{Affine, HitboxShape, Model, compose, invert_rigid};
use crate::physics::ragdoll::WorldCollision;
use crate::physics::{CollisionAttribute, PhysicsData};
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
    /// First-person viewmodel files written (`<demo>_r<round>_viewmodel_…`, [`super::viewmodel`]).
    pub viewmodels: usize,
    /// One line per death: who, when, how far the body went and when it lay still.
    pub falls: Vec<String>,
    /// How the export fits what the demo recorded (one line each: the hitboxes against the hits, [`hitbox_check`]).
    pub checks: Vec<String>,
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
    /// The map's world physics (`None` when it could not be read).
    world: Option<PhysicsData>,
    /// Its solid collision, for the bodies of the dead.
    collision: Option<WorldCollision>,
    /// Each player's aim punch (pitch, yaw) by tick, which their body's aim follows ([`super::punch`]).
    aim_punch: BTreeMap<u32, HashMap<u32, [f32; 2]>>,
}

/// The world physics of `map` (`maps/<map>/world_physics.vmdl`), or why it cannot be read (the map's archive is not
/// open, or the file is broken).
fn load_world_physics(content: &Content, map: &str) -> Result<PhysicsData, String> {
    let path = format!("maps/{map}/world_physics.vmdl");
    let bytes = content
        .read_resource(&path)
        .map_err(|e| format!("{path}: {e} (is the map's archive open?)"))?;
    let resource = Resource::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    PhysicsData::from_resource(&resource)
        .map_err(|e| format!("{path}: {e}"))?
        .ok_or_else(|| format!("{path}: no PHYS block"))
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
    let world = load_world_physics(content, &track.map_name)
        .inspect_err(|e| {
            warnings.push(format!(
                "no collision for {}: {e}: the dead fall onto a floor where they last stood, bullets meet no walls",
                track.map_name
            ))
        })
        .ok();
    let collision = world.as_ref().map(WorldCollision::from_physics);
    let shot_data = ShotData::load(content, items_text.as_deref(), &mut warnings);
    let recoil = recoil_by_tick(track, &shot_data);
    let aim_punch = recoil
        .iter()
        .map(|(&controller, ticks)| (controller, ticks.iter().map(|r| (r.tick, r.aim)).collect()))
        .collect();
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
        world,
        collision,
        aim_punch,
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
        viewmodels: 0,
        falls: Vec::new(),
        checks: Vec::new(),
        fps: round.fps,
        frames: round.frames.len(),
        warnings,
    };
    let mut manifest = Vec::new();
    let mut muzzles: Vec<Muzzle> = Vec::new();
    let mut bodies: Vec<Body> = Vec::new();
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
        let (written, weapons, falls, look_muzzles, look_bodies) =
            export_look(&round, look, players, out_dir, &name)?;
        muzzles.extend(look_muzzles);
        bodies.extend(look_bodies);
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
    let (viewmodels, view_muzzles) =
        export_viewmodels(&round, &groups, demo_name, out_dir, &mut report);
    let effects = export_effect_textures(content, out_dir, &mut report.warnings);
    let blasts = blasts_json(track, round.collision.as_ref());
    let fire_walls = fire_walls_json(track, round.collision.as_ref());
    let bullet_collision = round
        .world
        .as_ref()
        .map(|w| WorldCollision::from_physics_where(w, CollisionAttribute::stops_bullets));
    // By tick, as the bullets look them up.
    bodies.sort_by_key(|b| b.tick);
    let doors = Doors::load(content, track, &round.models, &mut report.warnings);
    let (shots, decal_groups) = shots_json(
        track,
        bullet_collision.as_ref(),
        round.world.as_ref(),
        Some(&doors),
        &bodies,
        &muzzles,
        &view_muzzles,
        &shot_data,
    );
    report.checks.push(hitbox_check(track, &bodies));
    let (earlier_holes, earlier_groups) = earlier_holes_json(
        track,
        bullet_collision.as_ref(),
        round.world.as_ref(),
        &shot_data,
    );
    let decal_groups: std::collections::BTreeSet<String> =
        decal_groups.union(&earlier_groups).cloned().collect();
    let decals = export_decals(content, &decal_groups, out_dir, &mut report.warnings);
    let gun_models: BTreeSet<&str> = track
        .shots
        .iter()
        .filter_map(|(_, s)| shot_data.weapon(s.item)?.model.as_deref())
        .collect();
    let casings = load_casings(content, gun_models, &mut report.warnings);
    let (brass, case_models) = brass_json(
        track,
        &muzzles,
        &view_muzzles,
        &shot_data,
        &casings,
        round.collision.as_ref(),
    );
    let cases = export_cases(&round, &case_models, demo_name, out_dir, &mut report);
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
            "viewmodels": viewmodels,
            "effects": effects,
            "blasts": blasts,
            "fires": fire_walls,
            "shots": shots,
            "earlier_holes": earlier_holes,
            "decals": decals,
            "brass": brass,
            "punch": punch_json(&recoil),
            "cases": cases,
        }))
        .expect("JSON serializes"),
    )?;
    Ok(report)
}

/// Writes the players' first-person views ([`super::viewmodel`]): one glTF per agent and glove skin of the looks
/// in `groups`, `<demo>_r<round>_viewmodel_<agent>[_<gloves>].gltf`. Returns the manifest entries (file, agent,
/// gloves, players, and the weapon looks it holds), and where the barrel was at each shot in first person
/// ([`super::viewmodel::ViewmodelExport`]). Without the upper-body data (no weapon names) or CS2's viewmodel files
/// there is none, with a warning; a look that fails is left out with a warning too.
fn export_viewmodels(
    round: &Round<'_>,
    groups: &BTreeMap<Look, Vec<u32>>,
    demo_name: &str,
    out_dir: &Path,
    report: &mut PlayersReport,
) -> (Vec<serde_json::Value>, Vec<Muzzle>) {
    let Some(upper) = &round.upper else {
        report
            .warnings
            .push("no weapon names: no first-person views".into());
        return (Vec::new(), Vec::new());
    };
    let mut weapons: Vec<(u32, String, String)> = upper
        .weapons
        .iter()
        .map(|(&item, w)| (item, w.weapon.clone(), w.category.clone()))
        .collect();
    weapons.sort();
    let rig = match load_viewmodel_rig(round.content, &weapons, &mut report.warnings) {
        Ok(rig) => rig,
        Err(e) => {
            report
                .warnings
                .push(format!("viewmodel graphs: {e}: no first-person views"));
            return (Vec::new(), Vec::new());
        }
    };
    let arms = match load_model(round.content, ARMS_MODEL) {
        Ok(model) => model,
        Err(e) => {
            report
                .warnings
                .push(format!("{ARMS_MODEL}: {e}: no first-person views"));
            return (Vec::new(), Vec::new());
        }
    };
    // The defuse kit makes no difference in first person: one file per agent and gloves.
    let mut looks: BTreeMap<(&str, Option<&str>), Vec<u32>> = BTreeMap::new();
    for (look, players) in groups {
        looks
            .entry((look.agent.as_str(), look.gloves.as_deref()))
            .or_default()
            .extend(players);
    }
    let mut entries = Vec::new();
    let mut muzzles = Vec::new();
    for ((agent, gloves), mut players) in looks {
        players.sort_unstable();
        let mut name = format!(
            "{demo_name}_r{}_viewmodel_{}",
            round.track.round,
            stem(agent)
        );
        if let Some(gloves) = gloves {
            name = format!("{name}_{}", stem(gloves));
        }
        match export_viewmodel_look(
            round.content,
            &rig,
            &arms,
            round.track,
            round.fps,
            &round.models,
            agent,
            gloves,
            &players,
            out_dir,
            &name,
        ) {
            Ok((written, weapons, look_muzzles)) => {
                muzzles.extend(look_muzzles);
                entries.push(json!({
                    "file": format!("{name}.gltf"),
                    "model": agent,
                    "gloves": gloves,
                    "players": players,
                    "weapons": weapons.iter().map(|w| json!({
                        "model": w.model,
                        "mesh_groups": w.mesh_groups,
                    })).collect::<Vec<_>>(),
                }));
                report.files.push(written.gltf);
                report.warnings.extend(written.warnings);
                report.viewmodels += 1;
            }
            Err(e) => report
                .warnings
                .push(format!("viewmodel {name}: {e}: not written")),
        }
    }
    (entries, muzzles)
}

/// Writes one glTF per case model the round's brass uses ([`super::brass`]): `<demo>_r<round>_case_<model file
/// stem>.gltf`, rigid like a dropped weapon's. Returns the manifest's `cases`: file and model path. A model that cannot
/// be read is left out with a warning (its cases are not drawn).
fn export_cases(
    round: &Round<'_>,
    models: &BTreeSet<String>,
    demo_name: &str,
    out_dir: &Path,
    report: &mut PlayersReport,
) -> Vec<serde_json::Value> {
    let mut entries = Vec::new();
    for path in models {
        let name = format!("{demo_name}_r{}_case_{}", round.track.round, stem(path));
        let written = load_model(round.content, path).and_then(|model| {
            let shown = shown_groups(&model, 0);
            export_model_static(round.content, &model, shown, out_dir, &name)
        });
        match written {
            Ok(written) => {
                entries.push(json!({ "file": format!("{name}.gltf"), "model": path }));
                report.files.push(written.gltf);
                report.warnings.extend(written.warnings);
            }
            Err(e) => report.warnings.push(format!("case {path}: {e}: not shown")),
        }
    }
    entries
}

/// Writes one glTF per look of the weapons lying in the world during the round (the replay's `dropped_weapons`) and
/// of the thrown grenades in flight (its `grenades`, mesh-group mask 0: the model's default groups):
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
    // Thrown grenades fly as their own model in its default mesh groups (mask 0).
    for g in round
        .track
        .samples
        .iter()
        .flat_map(|s| &s.grenades)
        .filter(|g| g.model != 0)
    {
        looks.entry(g.model).or_default().insert(0);
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

/// What [`export_look`] returns: the glTF export's report, the weapon slots it holds, each death baked as a
/// ragdoll (the player's controller, the fall), where the barrel was at each of the players' shots, and the
/// players' hitboxes on the ticks of the round's hits near them.
type LookExport = (
    super::gltf::ExportReport,
    Vec<Slot>,
    Vec<(u32, super::deaths::DeathSummary)>,
    Vec<Muzzle>,
    Vec<Body>,
);

/// How near a hit (inches) a player's origin must be, on the tick their hitboxes are posed for it
/// ([`HIT_POSE_TICKS`] before the hit), for them to be posed ([`Body`]): a player's height and then some.
const BODY_REACH: f32 = 150.0;

/// The hitbox set bullets hit on CS2's agents ([`crate::model::Hitbox::set`]).
const HITBOX_SET: &str = "cstrike";

/// How many ticks before a hit's tick its player's hitboxes are posed ([`Body`]): the shot is fired about a tick
/// before its message ([`crate::demo::shots::shot_times`]), and CS2's server checks it against the victim where the
/// shooter saw them, earlier still (its lag compensation). Fitted on the demos' hits (`hitbox_check`, the export's
/// `check:` line) over 10 rounds of 8 demos: posed 3 ticks back, 353 of 505 hit points lie within an inch of a
/// hitbox's surface (2 back 336, on the hit's own tick 172 of the 413 with a body near; 351 before the body's aim
/// took the aim punch, [`crate::anim::bake::BODY_AIM_PUNCH`]), and where a bullet went through a body its way out
/// lies within 3 inches of the next hit's start for 32 of 40 (the 8 inches of [`super::shots::PLAYER_DEPTH`]: 22).
const HIT_POSE_TICKS: f32 = 3.0;

/// A player's hitboxes in the world on frame `world` (the agent's posed bones, model space) with the player's
/// actor transform `actor`: each of `hitboxes` (bone index, shape) on its bone.
fn posed_hitboxes(
    world: &[Affine],
    actor: &Affine,
    hitboxes: &[(usize, HitboxShape)],
) -> Vec<Capsule> {
    let apply = |m: &Affine, p: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| (0..3).map(|k| m[i][k] * p[k]).sum::<f32>() + m[i][3])
    };
    hitboxes
        .iter()
        .filter_map(|&(bone, shape)| {
            let on = compose(actor, world.get(bone)?);
            let (a, b, radius) = match shape {
                HitboxShape::Capsule { a, b, radius } => (a, b, radius),
                HitboxShape::Sphere { center, radius } => (center, center, radius),
                // A box as the capsule round its long axis would be another shape: none of CS2's agents has one.
                HitboxShape::Box { .. } => return None,
            };
            Some(Capsule {
                a: apply(&on, a),
                b: apply(&on, b),
                radius,
            })
        })
        .collect()
}

/// Where a shot left the barrel of the gun in the shooter's hand at the shot's tick: the weapon model's
/// `muzzle_flash` attachment as the exported agent shows it (third person, in the world) or as their viewmodel does
/// (first person, in the viewmodel rig's space: [`super::viewmodel::ViewmodelExport`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Muzzle {
    /// The shooter's controller.
    pub controller: u32,
    /// The shot's tick.
    pub tick: u32,
    /// The muzzle (Source axes, inches; in the world, or in the viewmodel rig's space).
    pub position: [f32; 3],
    /// Along the barrel (unit).
    pub direction: [f32; 3],
    /// The gun's ejection port (`shell_eject`) in the same space, where a fired case is thrown from
    /// ([`super::brass`]), on the shot's tick and each of the [`EJECT_TICKS`] − 1 after it (a case leaves the port
    /// up to 0.04 s after the shot, and the gun moves meanwhile); `None` when the gun has none (or is not in hand).
    pub eject: [Option<Affine>; EJECT_TICKS],
}

/// How many ticks of a gun's ejection port [`Muzzle::eject`] holds, from the shot's: CS2's casing systems leave the
/// port 0 (shotguns) or 0.04 s after the shot, 2.56 ticks at 64 per second.
pub const EJECT_TICKS: usize = 4;

/// The muzzle and the ejection port (if any) of the gun in hand at one frame: `world` is the agent's posed bones
/// (model space), `carried` the frame's weapons and their places, `actor` the player's actor transform. `None`
/// without a gun in hand.
fn muzzle_at(
    world: &[Affine],
    carried: Option<&Vec<(Slot, Place)>>,
    mounts: &AgentMounts,
    weapon_models: &BTreeMap<String, (Model, WeaponFrames)>,
    actor: &Affine,
) -> Option<(Affine, Option<Affine>)> {
    let (slot, _) = carried?.iter().find(|(_, place)| *place == Place::Hand)?;
    let (_, frames) = weapon_models.get(&slot.look.model)?;
    let weapon_bone = compose(actor, &slot_target(world, mounts, frames, Place::Hand)?);
    Some((
        compose(&weapon_bone, &frames.muzzle?),
        frames.shell_eject.map(|e| compose(&weapon_bone, &e)),
    ))
}

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
            round.aim_punch.get(&controller).unwrap_or(&HashMap::new()),
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

    // The agent's hitboxes on their bones, and the bullets' hit points by tick with the frame their hitboxes are
    // posed in (HIT_POSE_TICKS before; the other effect dispatches, impacts and knives, start where they end).
    let hitboxes: Vec<(usize, HitboxShape)> = agent
        .hitboxes(HITBOX_SET)
        .into_iter()
        .filter_map(|h| Some((agent.skeleton.find(&h.bone)?, h.shape)))
        .collect();
    let mut hits: BTreeMap<u32, Vec<[f32; 3]>> = BTreeMap::new();
    for hit in track.player_hits.iter().filter(|h| h.start != h.point) {
        hits.entry(hit.tick).or_default().push(hit.point);
    }
    let hits: Vec<(u32, usize, Vec<[f32; 3]>)> = hits
        .into_iter()
        .filter(|_| !hitboxes.is_empty())
        .filter_map(|(tick, points)| {
            let k = round
                .frames
                .partition_point(|&(t, _)| t < tick as f32 - HIT_POSE_TICKS - 0.5);
            (k < round.frames.len()).then_some((tick, k, points))
        })
        .collect();

    // One animation per player: the agent's bones, then the weapon bones.
    let mut animations = Vec::new();
    let mut falls = Vec::new();
    let mut muzzles = Vec::new();
    let mut hit_bodies = Vec::new();
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
        // Each shot: where the barrel was in the frame of its tick.
        for (_, shot) in track.shots.iter().filter(|(c, _)| *c == controller) {
            let Some(k) = round
                .frames
                .iter()
                .position(|&(tick, _)| tick >= shot.tick as f32 - 0.5)
            else {
                continue;
            };
            // The gun's muzzle and port in frame `k`, with the player's actor there.
            let at = |k: usize| {
                let sample = round.frames.get(k)?.1;
                let player = track.samples[sample]
                    .players
                    .iter()
                    .find(|p| p.controller == controller && p.alive)?;
                let actor = super::deaths::actor(player);
                muzzle_at(
                    worlds.get(k)?,
                    carried.get(k),
                    &mounts,
                    &weapon_models,
                    &actor,
                )
            };
            if let Some((m, _)) = at(k) {
                muzzles.push(Muzzle {
                    controller,
                    tick: shot.tick,
                    position: [m[0][3], m[1][3], m[2][3]],
                    direction: [m[0][0], m[1][0], m[2][0]],
                    eject: std::array::from_fn(|d| at(k + d).and_then(|(_, e)| e)),
                });
            }
        }
        // Each tick with hits near the living player: their hitboxes as the shooter saw them (HIT_POSE_TICKS).
        for &(tick, k, ref points) in &hits {
            let Some(player) = track.samples[round.frames[k].1]
                .players
                .iter()
                .find(|p| p.controller == controller && p.alive)
            else {
                continue;
            };
            let near = points.iter().any(|p| {
                (0..3)
                    .map(|i| (p[i] - player.position[i]).powi(2))
                    .sum::<f32>()
                    < BODY_REACH * BODY_REACH
            });
            if near && let Some(world) = worlds.get(k) {
                hit_bodies.push(Body {
                    controller,
                    tick,
                    hitboxes: posed_hitboxes(world, &super::deaths::actor(player), &hitboxes),
                });
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
    Ok((written, slots, falls, muzzles, hit_bodies))
}

/// Where `slot`'s weapon is in one frame's list (hidden when the player does not carry it).
fn place_of(carried: Option<&Vec<(Slot, Place)>>, slot: &Slot) -> Place {
    carried
        .and_then(|list| list.iter().find(|(s, _)| s == slot))
        .map_or(Place::Hidden, |(_, place)| *place)
}

/// The mesh groups to show: `mask` as the entity sent it, or the model's default when it sent none (0) or all bits
/// (knives and grenades send every bit).
pub(super) fn shown_groups(model: &Model, mask: u64) -> u64 {
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
pub(super) fn stem(path: &str) -> &str {
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
