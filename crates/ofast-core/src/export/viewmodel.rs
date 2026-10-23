//! The players' first-person view for Unreal (M5, decision 016): CS2's viewmodel baked from the demo
//! ([`crate::anim::viewmodel`]), one glTF per look next to the round's other files,
//! `<demo>_r<round>_viewmodel_<agent>[_<gloves>].gltf`. A look is the agent and its glove skin; each file holds:
//!
//! - the arms rig: the bones of `weapons/models/shared/arms/weapon_arms.vmdl`, whose bind pose is the agents' (to
//!   0.003 inch) and whose `root_motion` is the camera;
//! - the agent's own first-person meshes, bone-merged by name: its arms with their default gloves
//!   (`firstperson_default`), or with a glove skin only the sleeves (`firstperson_hide_default_gloves`) and the glove
//!   model's `firstperson_default`;
//! - every weapon the look's players hold, skinned onto its own copy of its model's bones (`w<n>_<bone>`, the root
//!   under `wpn`), so that the clips' secondary animations move its bolt, magazine and silencer;
//! - one animation per player, `viewmodel_<controller>`, keyed like the body's ([`super::players::animation_fps`]):
//!   the arms with their twist bones, the weapon in hand on `wpn` with its own bones, every other weapon scaled to
//!   0 — and the whole rig scaled to 0 while the player has no viewmodel (dead, or holding a weapon without one).
//!
//! `tools/unreal/import_demo.py` attaches each to its player's first-person camera at the player's viewmodel
//! offsets, drawn with Unreal's first-person rendering at the player's viewmodel FOV.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::ExportError;
use super::gltf::{
    Binding, BoneAnimation, ExportReport, MergedModel, export_character, load_model, resource_data,
};
use super::loadout::{WeaponFrames, WeaponLook};
use super::players::{shown_groups, stem};
use crate::anim::bake::frame_samples;
use crate::anim::eval::Library;
use crate::anim::graph::Graph;
use crate::anim::layers::BoneMask;
use crate::anim::viewmodel::{
    VIEWMODEL_GRAPH, ViewWeapon, ViewmodelFrame, ViewmodelRig, bake_viewmodel,
};
use crate::anim::weapon::{referenced_graph_under, weapon_state};
use crate::anim::{BoneConstraints, Clip, NmSkeleton, Retargeter, matrix_to_quat};
use crate::content::Content;
use crate::demo::players::RoundTrack;
use crate::kv3::Value;
use crate::model::{Affine, Bone, Model, compose, invert_rigid};

/// The viewmodel clips' skeleton, when the graph does not name it.
const VIEWMODEL_SKELETON: &str = "animation/skeletons/characters/viewmodel.vnmskel";

/// Reads CS2's viewmodel for the weapons `weapons` (item definition index, type ID, category ID): the viewmodel
/// graph and skeleton with every mask it defines (for the arms and for each weapon skeleton), each weapon's own graph
/// with its clips, and the skeleton its clips animate besides the arms. A weapon the graph does not know is left out
/// with a warning (its holder shows no viewmodel).
pub fn load_viewmodel_rig(
    content: &Content,
    weapons: &[(u32, String, String)],
    warnings: &mut Vec<String>,
) -> Result<ViewmodelRig, ExportError> {
    let mut load_graph = |path: &str| -> Result<Graph, ExportError> {
        Ok(Graph::parse(&resource_data(content, path)?)?)
    };
    let mut load_clip = |path: &str| -> Result<Clip, ExportError> {
        Clip::parse(&resource_data(content, path)?).map_err(|source| ExportError::Clip {
            path: path.to_owned(),
            source,
        })
    };
    let graph_data = resource_data(content, VIEWMODEL_GRAPH)?;
    let skeleton_path = graph_data
        .get("m_skeleton")
        .and_then(Value::as_str)
        .unwrap_or(VIEWMODEL_SKELETON)
        .to_owned();
    let skeleton_data = resource_data(content, &skeleton_path)?;
    let skeleton = NmSkeleton::parse(&skeleton_data);
    let mut library = Library::new();
    for id in BoneMask::ids(&skeleton_data) {
        if let Some(mask) = BoneMask::from_skeleton(&skeleton_data, &skeleton, &id) {
            library.insert_mask(None, &id, mask);
        }
    }

    // Where each weapon's actions are: its own graph (the root), or its state in the viewmodel graph.
    let main = library.load_graph(VIEWMODEL_GRAPH, &mut load_graph)?;
    let machine = main.find("WPN");
    let mut sources: Vec<(u32, &str, &str, String, Option<usize>)> = Vec::new();
    for (item, name, category) in weapons {
        let ids = |parameter: &str| match parameter {
            "weapon_category" => Some(category.clone()),
            "weapon_type" => Some(name.clone()),
            _ => None,
        };
        let Some(state) = machine.and_then(|m| weapon_state(main, m, &ids)) else {
            warnings.push(format!(
                "weapon {name}: not in the viewmodel graph: no first-person view"
            ));
            continue;
        };
        match referenced_graph_under(main, state) {
            Some(graph) => sources.push((*item, name, category, graph, None)),
            None => sources.push((
                *item,
                name,
                category,
                VIEWMODEL_GRAPH.to_owned(),
                Some(state),
            )),
        }
    }

    // Each weapon skeleton once: its bones and its weights in every mask.
    let mut weapon_skeletons: HashMap<String, NmSkeleton> = HashMap::new();
    let mut out = HashMap::new();
    for (item, name, category, graph, state) in sources {
        library.load(&graph, state, &mut load_graph, &mut load_clip)?;
        let Some(node) = state.or_else(|| library.graph(&graph).and_then(|g| g.root)) else {
            warnings.push(format!(
                "weapon {name}: {graph} has no root: no first-person view"
            ));
            continue;
        };
        // The skeleton the weapon's clips animate besides the arms (the first of its graph's clips that has one
        // says it; for the C4 and the Healthshot, the first under their state).
        let secondary = library.graph(&graph).and_then(|g| {
            let clips: Vec<String> = match state {
                Some(node) => g.clips_under(node).into_iter().map(|(c, _)| c).collect(),
                None => (0..g.nodes.len())
                    .filter_map(|n| g.clip(n).map(|(c, _)| c.to_owned()))
                    .collect(),
            };
            clips.into_iter().find_map(|clip| {
                library
                    .clip(&clip)?
                    .secondary
                    .first()
                    .map(|s| s.skeleton.clone())
            })
        });
        let skeleton = match secondary {
            Some(path) => {
                if !weapon_skeletons.contains_key(&path) {
                    let data = resource_data(content, &path)?;
                    let bones = NmSkeleton::parse(&data);
                    for id in BoneMask::ids(&skeleton_data) {
                        if let Some(mask) =
                            BoneMask::from_secondary(&skeleton_data, &id, &path, &bones)
                        {
                            library.insert_mask(Some(&path), &id, mask);
                        }
                    }
                    weapon_skeletons.insert(path.clone(), bones);
                }
                Some((path.clone(), weapon_skeletons[&path].clone()))
            }
            None => None,
        };
        out.insert(
            item,
            ViewWeapon {
                weapon: name.to_owned(),
                category: category.to_owned(),
                graph,
                node,
                skeleton,
            },
        );
    }
    Ok(ViewmodelRig {
        library,
        skeleton,
        weapons: out,
    })
}

/// The bit of the mesh group whose choice is `choice` (`firstperson_default`), if the model has one.
fn group_bit(model: &Model, choice: &str) -> Option<u64> {
    model
        .mesh_groups
        .iter()
        .position(|g| g.split("#&").nth(1) == Some(choice))
        .map(|i| 1u64 << i)
}

/// One weapon model on the viewmodel: its look, its model, where its bones start in the skeleton, and how its
/// animation bones (the item's weapon skeleton) map onto the model's.
struct Slot {
    look: WeaponLook,
    model: Model,
    first: usize,
    /// The model's bones from the weapon skeleton's poses, when the item has one.
    retarget: Option<(Retargeter, Vec<Option<usize>>)>,
}

/// What [`export_viewmodel_look`] wrote: the glTF export's report, and the weapon looks it holds.
pub type ViewmodelExport = (ExportReport, Vec<WeaponLook>);

/// One player's baked frames and the weapon look in hand at each: `(controller, frames, looks)`.
type Baked = (u32, Vec<Option<ViewmodelFrame>>, Vec<Option<WeaponLook>>);

/// Writes one viewmodel look's glTF (see the module docs): `agent` with glove skin `gloves` for `players`, the
/// round's frames at `fps`. `models` names the weapon models by resource id; `arms` is [`ARMS_MODEL`].
///
/// [`ARMS_MODEL`]: crate::anim::viewmodel::ARMS_MODEL
#[allow(clippy::too_many_arguments)]
pub fn export_viewmodel_look(
    content: &Content,
    rig: &ViewmodelRig,
    arms: &Model,
    track: &RoundTrack,
    fps: f32,
    models: &HashMap<u64, String>,
    agent: &str,
    gloves: Option<&str>,
    players: &[u32],
    out_dir: &Path,
    file_name: &str,
) -> Result<ViewmodelExport, ExportError> {
    let mut warnings = Vec::new();
    let agent_model = load_model(content, agent)?;
    let glove_model = match gloves {
        Some(path) => match load_model(content, path) {
            Ok(model) => Some(model),
            Err(e) => {
                warnings.push(format!(
                    "gloves {path}: {e}: the agent's own in first person"
                ));
                None
            }
        },
        None => None,
    };
    let own = group_bit(&agent_model, "firstperson_default");
    let sleeves = group_bit(&agent_model, "firstperson_hide_default_gloves");
    let glove_groups = glove_model
        .as_ref()
        .and_then(|g| group_bit(g, "firstperson_default"));
    let (agent_groups, glove_groups) = match (glove_groups, sleeves) {
        (Some(g), Some(s)) => (s, Some(g)),
        _ => (own.unwrap_or(0), None),
    };
    if agent_groups == 0 {
        warnings.push(format!("{agent}: no first-person meshes: the weapon alone"));
    }

    // Every player's frames, and the weapon look in hand at each.
    let frames_at = frame_samples(track, fps);
    let mut baked: Vec<Baked> = Vec::new();
    for &controller in players {
        let frames = bake_viewmodel(track, controller, rig, fps)?;
        let looks = frames_at
            .iter()
            .map(|&(_, index)| {
                let player = track.samples[index]
                    .players
                    .iter()
                    .find(|p| p.controller == controller && p.alive)?;
                let active = player.rig.weapon.as_ref()?;
                let carried = player.weapons.iter().find(|w| w.entity == active.entity)?;
                Some(WeaponLook {
                    model: models.get(&carried.model)?.clone(),
                    mesh_groups: carried.mesh_groups,
                })
            })
            .collect();
        baked.push((controller, frames, looks));
    }

    // One slot per weapon look in hand, with the weapon skeleton of the first item seen with it.
    let mut items_of: BTreeMap<WeaponLook, u32> = BTreeMap::new();
    for (_, frames, looks) in &baked {
        for (frame, look) in frames.iter().zip(looks) {
            if let (Some(frame), Some(look)) = (frame, look) {
                items_of.entry(look.clone()).or_insert(frame.item);
            }
        }
    }
    let mut skeleton = arms.skeleton.clone();
    let wpn = arms
        .skeleton
        .find("wpn")
        .ok_or_else(|| ExportError::Character(format!("{}: no `wpn` bone", file_name)))?;
    let bind = arms.skeleton.world_transforms();
    let mut slots: Vec<Slot> = Vec::new();
    let mut placements: Vec<Affine> = Vec::new();
    for (look, item) in items_of {
        let model = match load_model(content, &look.model) {
            Ok(model) => model,
            Err(e) => {
                warnings.push(format!("{}: {e}: not shown in first person", look.model));
                continue;
            }
        };
        let Some(frames) = WeaponFrames::of(&model) else {
            warnings.push(format!(
                "{}: no `weapon` bone: not shown in first person",
                look.model
            ));
            continue;
        };
        // The model's `weapon` bone on `wpn`: its bones under `wpn`, the roots placed relative to it.
        let first = skeleton.bones.len();
        let to_weapon = invert_rigid(&frames.weapon_bone);
        let model_world = model.skeleton.world_transforms();
        for (b, bone) in model.skeleton.bones.iter().enumerate() {
            let (parent, position, rotation) = match bone.parent {
                Some(p) => (first + p, bone.position, bone.rotation),
                None => {
                    let local = compose(&to_weapon, &model_world[b]);
                    (
                        wpn,
                        [local[0][3], local[1][3], local[2][3]],
                        matrix_to_quat(&local),
                    )
                }
            };
            skeleton.bones.push(Bone {
                name: format!("w{}_{}", slots.len(), bone.name),
                parent: Some(parent),
                position,
                rotation,
                flags: 0,
            });
        }
        placements.push(compose(&bind[wpn], &to_weapon));
        let retarget = rig
            .weapons
            .get(&item)
            .and_then(|w| w.skeleton.as_ref())
            .map(|(_, anim)| {
                let by_name = model
                    .skeleton
                    .bones
                    .iter()
                    .map(|b| anim.find(&b.name))
                    .collect();
                (Retargeter::new(&model.skeleton, anim), by_name)
            });
        slots.push(Slot {
            look,
            model,
            first,
            retarget,
        });
    }

    // The meshes: the agent's first-person groups and the gloves by bone name, each weapon on its own bones.
    let mut merged = vec![MergedModel {
        label: "fp".into(),
        model: &agent_model,
        mesh_groups: agent_groups,
        binding: Binding::ByName,
    }];
    if let (Some(model), Some(groups)) = (&glove_model, glove_groups) {
        merged.push(MergedModel {
            label: "gloves".into(),
            model,
            mesh_groups: groups,
            binding: Binding::ByName,
        });
    }
    for (n, slot) in slots.iter().enumerate() {
        merged.push(MergedModel {
            label: format!("w{n}_{}", stem(&slot.look.model)),
            model: &slot.model,
            mesh_groups: shown_groups(&slot.model, slot.look.mesh_groups),
            binding: Binding::Offset {
                first: slot.first,
                placement: placements[n],
            },
        });
    }

    // The animations.
    let retargeter = Retargeter::new(&arms.skeleton, &rig.skeleton);
    let constraints = BoneConstraints::of(arms);
    let bind_locals: Vec<([f32; 3], [f32; 4])> = skeleton
        .bones
        .iter()
        .map(|b| (b.position, b.rotation))
        .collect();
    let mut animations = Vec::new();
    for (controller, frames, looks) in &baked {
        let mut keys = Vec::with_capacity(frames.len());
        let mut scales = Vec::with_capacity(frames.len());
        let mut last_arms: Option<Vec<([f32; 3], [f32; 4])>> = None;
        for (frame, look) in frames.iter().zip(looks) {
            let mut locals = bind_locals.clone();
            let mut frame_scales = vec![1.0f32; skeleton.bones.len()];
            match frame {
                Some(frame) => {
                    let mut world = retargeter.world(&frame.arms);
                    constraints.apply(&mut world);
                    let arm_locals = retargeter.locals_of(&world);
                    locals[..arm_locals.len()].copy_from_slice(&arm_locals);
                    last_arms = Some(arm_locals);
                    for slot in &slots {
                        let model_bones = &slot.model.skeleton.bones;
                        let bones = slot.first..slot.first + model_bones.len();
                        if look.as_ref() != Some(&slot.look) {
                            // Every root of the weapon (they all hang under `wpn`) scaled to 0.
                            for (b, bone) in model_bones.iter().enumerate() {
                                if bone.parent.is_none() {
                                    frame_scales[slot.first + b] = 0.0;
                                }
                            }
                            continue;
                        }
                        if let (Some((retarget, by_name)), Some(pose)) =
                            (&slot.retarget, &frame.weapon)
                        {
                            let mut world = retarget.world(pose);
                            // Bones the clips do not move: a root stays where the bind pose has it under `wpn`,
                            // the others follow their parent at bind pose (again, from such a root).
                            for (b, bone) in model_bones.iter().enumerate() {
                                if by_name[b].is_some() {
                                    continue;
                                }
                                let (t, q) = bind_locals[slot.first + b];
                                let local = crate::model::affine(q, t);
                                world[b] = match bone.parent {
                                    None => local,
                                    Some(p) => compose(&world[p], &local),
                                };
                            }
                            for (b, local) in retarget.locals_of(&world).into_iter().enumerate() {
                                locals[bones.start + b] = local;
                                if let Some(a) = by_name[b] {
                                    frame_scales[bones.start + b] = pose[a].scale;
                                }
                            }
                        }
                    }
                }
                None => {
                    // No viewmodel: the rig hidden, held where it last was.
                    if let Some(arm_locals) = &last_arms {
                        locals[..arm_locals.len()].copy_from_slice(arm_locals);
                    }
                    frame_scales[0] = 0.0;
                }
            }
            keys.push(locals);
            scales.push(frame_scales);
        }
        animations.push(BoneAnimation {
            name: format!("viewmodel_{controller}"),
            frames: keys,
            fps,
            scales,
        });
    }

    // The arms model's own mesh (a placeholder CS2 never draws) stays out.
    let mut base = arms.clone();
    base.meshes.clear();
    let mut written = export_character(
        content,
        &base,
        0,
        &skeleton,
        &merged,
        &animations,
        out_dir,
        file_name,
    )?;
    written.warnings.extend(warnings);
    let looks = slots.into_iter().map(|s| s.look).collect();
    Ok((written, looks))
}
