"""Build an Unreal Level Sequence from an OFAST replay file (one round of a CS2 demo) — no clicks needed.

Run with tools/unreal/run_ue.ps1:

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_demo.py \
        -Replay test-data/exports/demos/<demo>_r2.ofast-replay.json -Shot test-data/shots

Inputs (environment variables):
    OFAST_REPLAY     the replay file written by `ofast export-demo` (format: docs/formats/replay.md)
    OFAST_SHOT       optional folder for verification screenshots: the killer's view just before the first kills
    OFAST_POV_SHOTS  optional extra screenshots through players' eyes: "tick:controller,tick:controller" (into
                     OFAST_SHOT; the watcher's own body is hidden in that frame)
    OFAST_CLOSE_SHOTS optional close-ups of players, "tick:controller,…": from 1.3 m in front and to the right at
                     chest height, to inspect a pose and the weapons (into OFAST_SHOT)
    OFAST_VIEW_SHOTS optional screenshots from a camera placed as CS2's `getpos` prints it while spectating the demo,
                     "tick:x:y:z:pitch:yaw,…" (Source units and degrees; e.g. CS2's Chase Cam, to compare the same
                     view), with CS2's field of view; every living player is shown (into OFAST_SHOT)

The map must have been imported first (tools/unreal/import_map.py): the sequence is built in its level
/Game/OFAST/Maps/<map>. What the script makes, all rebuilt on every run:

- /Game/OFAST/Replays/<demo>_r<round>: a Level Sequence at the demo's tick rate (64 fps: one frame per tick),
  playback from the round's start (freeze time included) to its end;
- per player, when `ofast export-demo --players` wrote the animated agents next to the replay (listed in
  <demo>_r<round>.agents.json, one glTF per agent look): the player's agent as a skeletal mesh, playing the player's
  baked animation (`player_<controller>`: body, weapons in hand and holstered), its feet on the demo's positions and
  turned along the body yaw; otherwise a proxy (a capsule-sized cylinder in team colours: CT blue, T orange, with a
  "visor" at the eyes showing where the player faces) turned along the eye yaw. Actor label "CT <name>" /
  "T <name>"; not seen by the player's own first-person camera (decision 021: that camera owns the body, which is
  bOwnerNoSee, so any camera a cut uses shows it right), and hidden while dead unless the export baked the death as a
  ragdoll (the agents manifest lists the ticks of those deaths under `ragdolls`: the body falls and stays);
- per player: a first-person camera ("POV <name>") at the eyes, aimed as the cameras file has it (the eye angles
  turned by recoil, smoothed when the export was asked to), with CS2's default field of view (90 degrees at 4:3);
- per player, when the export wrote their first-person view (the agents manifest's `viewmodels`): the viewmodel
  (arms and the weapon in hand, `viewmodel_<controller>` baked from the demo's weapon states) on that camera at the
  player's own viewmodel offsets, drawn with Unreal's first-person rendering at the player's viewmodel FOV, seen
  only by that camera (bOnlyOwnerSee);
- per stretch a weapon lay in the world (the replay's `dropped_weapons`: a dead player's gun, a weapon dropped for
  a teammate), its model (the agents manifest's `dropped_weapons` glTFs) moved as the demo has it, with its full
  rotation (Rotator(roll=roll, pitch=-pitch, yaw=-yaw)), shown from the tick it was let go to the tick before it
  was picked up or the round ended;
- the map's rotating doors (the replay's `doors`), each the level's own door actor (found at its hinge) swung as the
  demo has it;
- the map's physics props (the replay's `props`: de_nuke's hard hats and mugs), each the level's own actor (found
  where the round starts it) moved as the demo has it;
- per thrown grenade (the replay's `grenades`), its model (the agents manifest's models) flying from the throw to its
  detonation, expiry or landing, keyed like the dropped weapons;
- per smoke whose cloud the export simulated, a Heterogeneous Volume drawing it from its atlas, its frame picked by
  the demo tick keyed in /Game/OFAST/Replays/MPC_Replay (smoke_volumes.py);
- per molotov or incendiary fire, CS2's flame sprites played by a material on instanced quads, one slot per sprite
  per flame, driven by the same demo tick, and a flickering point light; per HE grenade and flashbang going off, its
  fireball, smoke or flare sprites and a light pulse; a white flash over a blinded player's POV camera
  (grenade_effects.py; the sprites need the agents manifest's `effects`, written by `--players`);
- per gun shot (the agents manifest's `shots`), its muzzle flash (in the world, and in first person on the shooter's
  viewmodel) and light, its tracer when it draws one, and each bullet's impacts where it met the world (sprites by
  the surface's family, a bullet hole: both sides of each wall it went through) or a player (blood)
  (shot_effects.py);
- per shot of a gun that throws one, its case (the agents manifest's `brass` and `cases`): flown in the world from
  the agent's gun, and on the shooter's viewmodel for their own view (place_brass);
- the round's cameras from its cameras file (`<demo>_r<round>.cameras.json` next to the replay, written by
  `ofast export-demo`; replay_cameras.py): the shot cameras chosen for its kills and HLAE cameras, as CineCameraActors,
  and the camera cut track (by default each kill in turn through its killer's eyes, from the previous kill until half
  a second after this one, the way fragmovie editors cut a round);
- marked frames: freeze time end, every kill ("attacker > victim (weapon, HS)") and bomb events.

Conversions are the map import's: Source (x, y, z) inches -> Unreal (x, -y, z) x 2.54 cm, angles (pitch, yaw) ->
Rotator(pitch=-pitch, yaw=-yaw). Keys that a straight line through their neighbours already gives are dropped
(standing still, running straight), which keeps the sequence light; the tolerance is far below what shows.
"""

import json
import math
import os
import re
import sys
import time

import unreal

# Unreal runs this file without its folder on sys.path; the shared modules sit next to it.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gltf_specular  # noqa: E402
import grenade_effects  # noqa: E402
import replay_cameras  # noqa: E402
import shot_effects  # noqa: E402
import smoke_volumes  # noqa: E402
from replay_cameras import (  # noqa: E402
    ANGLE_TOLERANCE_DEG, POSITION_TOLERANCE_CM, TAG, UNITS_TO_CM, channels_by_name, key_visibility, reduce_keys,
    spawn_tagged, to_unreal, unwrap)

REPLAY_PATH = os.environ["OFAST_REPLAY"]
SHOT_DIR = os.environ.get("OFAST_SHOT", "")
POV_SHOTS = [tuple(int(v) for v in item.split(":")) for item in os.environ.get("OFAST_POV_SHOTS", "").split(",")
             if ":" in item]
CLOSE_SHOTS = [tuple(int(v) for v in item.split(":"))
               for item in os.environ.get("OFAST_CLOSE_SHOTS", "").split(",") if ":" in item]
VIEW_SHOTS = [(int(item.split(":")[0]),) + tuple(float(v) for v in item.split(":")[1:6])
              for item in os.environ.get("OFAST_VIEW_SHOTS", "").split(",") if item.count(":") == 5]
WIDTH, HEIGHT = 1920, 1080
# Player proxy: CS2's player hull is 32 x 32 x 72 units; the engine cylinder is 100 cm with its pivot in the middle.
HULL_SCALE = (32 * UNITS_TO_CM / 100, 32 * UNITS_TO_CM / 100, 72 * UNITS_TO_CM / 100)
HULL_HALF_HEIGHT_CM = 36 * UNITS_TO_CM
TEAM_COLOURS = {2: (1.0, 0.45, 0.05), 3: (0.1, 0.35, 1.0)}  # T orange, CT blue
TEAM_NAMES = {2: "T", 3: "CT"}
# CS2's default field of view: 90 degrees horizontally at 4:3, i.e. 106.26 degrees at 16:9 (as import_map.py).
HFOV_16_9 = replay_cameras.hfov_16_9(90.0)
# A map actor is the demo's door when its origin is within this of the door's closed position (its hinge).
DOOR_MATCH_CM = 2.0
# A map actor is the demo's physics prop when its origin is within this of the prop's first position.
PROP_MATCH_CM = 2.0
# A door is blown apart when an HE grenade goes off this close to its hinge (units) this few ticks after its entity's
# last tick (broken_doors; seen: 9 - 83 units, 1 tick).
DOOR_BLAST_REACH = 128.0
DOOR_BLAST_TICKS = 4
# How long a case thrown in the world is hidden in its shooter's own view, where the first-person case flies instead
# (export::brass::VIEW_LIFE): after that it lies in the world for every view.
CASE_VIEW_FLIGHT_S = 1.0
WARM_UP_TICKS = 300
SHOT_DELAY_S = 6.0  # as import_map.SHOT_DELAY_S: lets Lumen settle after each cut
SHOT_TIMEOUT_TICKS = 3000  # editor ticks a screenshot may take before it is given up
# SpecularFactor of CS2's character materials (csgo_character) that are not cloth-shaded: Interchange's 1, F0 0.08, not
# glTF's 0.04. CS2 adds a rim light to its agents that OFAST does not convert; without it they read darker than CS2's,
# more so at F0 0.04 (an SAS agent in the shade 0.60 of CS2 at 0.08, 0.48 at 0.04, before the cloth shading was
# converted; docs/formats/material.md, "Weapons' and agents' specular"). A stand-in. Cloth-shaded materials
# (KHR_materials_sheen) carry a specular texture instead (1 - their cloth mask, at F0 0.04 like CS2's).
CHARACTER_SPECULAR_FACTOR = 1.0

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary


def log(message):
    unreal.log("OFAST: " + message)


def euler_filter(rotations):
    """Unreal (roll, pitch, yaw) triples made continuous for linear keys. Each triple is turned to the nearest of its
    two equivalent forms, (r, p, y) and (r + 180, 180 - p, y + 180), each angle unwrapped next to the previous key.
    Unwrapping each angle on its own is not enough: CS2 keeps pitch within -90 ... 90, so a gun tumbling end over
    end through the vertical jumps yaw and roll by 180 degrees at once, and when the two unwrap opposite ways the gun
    spins a full turn between two ticks (seen in slow motion and sub-frame samples, not at whole ticks)."""
    def near(a, b):
        return b + ((a - b + 180.0) % 360.0 - 180.0)
    out = []
    for r, p, y in rotations:
        if out:
            pr, pp, py = out[-1]
            same = (near(r, pr), near(p, pp), near(y, py))
            flipped = (near(r + 180.0, pr), near(180.0 - p, pp), near(y + 180.0, py))
            r, p, y = min(same, flipped, key=lambda c: abs(c[0] - pr) + abs(c[1] - pp) + abs(c[2] - py))
        out.append((r, p, y))
    return out


def runs(frames, values):
    """Splits a per-tick list with None gaps (no pawn) into runs of consecutive known samples."""
    run = []
    for frame, value in zip(frames, values):
        if value is None:
            if run:
                yield run
            run = []
        else:
            run.append((frame, value))
    if run:
        yield run


def clean_level(map_name):
    level = "/Game/OFAST/Maps/" + map_name
    if not assets.does_asset_exist(level):
        raise RuntimeError("%s is not imported yet: run import_map.py for %s first" % (level, map_name))
    levels.load_level(level)
    old = [a for a in actors.get_all_level_actors() if TAG in a.tags]
    for actor in old:
        actors.destroy_actor(actor)
    log("level %s, removed %d old replay actors" % (level, len(old)))


def team_material(team):
    """A colour instance of the engine's basic shape material, one per team."""
    path = "/Game/OFAST/Replays/MI_Proxy_%s" % TEAM_NAMES.get(team, "Other")
    if assets.does_asset_exist(path):
        return assets.load_asset(path)
    folder, name = path.rsplit("/", 1)
    instance = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
    mel = unreal.MaterialEditingLibrary
    mel.set_material_instance_parent(instance, assets.load_asset("/Engine/BasicShapes/BasicShapeMaterial"))
    r, g, b = TEAM_COLOURS.get(team, (0.6, 0.6, 0.6))
    mel.set_material_instance_vector_parameter_value(instance, "Color", unreal.LinearColor(r, g, b, 1.0))
    assets.save_loaded_asset(instance)
    return instance


def spawn_proxy(player):
    """The player's stand-in: cylinder + visor (facing) + name, attached together."""
    team = player["team"]
    body = spawn_tagged(assets.load_asset("/Engine/BasicShapes/Cylinder"), "%s %s" % (TEAM_NAMES.get(team, "?"), player["name"]))
    body.set_actor_scale3d(unreal.Vector(*HULL_SCALE))
    body.static_mesh_component.set_material(0, team_material(team))
    body.static_mesh_component.set_collision_enabled(unreal.CollisionEnabled.NO_COLLISION)
    visor = spawn_tagged(assets.load_asset("/Engine/BasicShapes/Cube"), "%s visor" % player["name"])
    visor.set_actor_scale3d(unreal.Vector(0.25, 0.5, 0.12))
    visor.static_mesh_component.set_material(0, team_material(0))
    visor.static_mesh_component.set_collision_enabled(unreal.CollisionEnabled.NO_COLLISION)
    visor.attach_to_actor(body, "", unreal.AttachmentRule.KEEP_WORLD, unreal.AttachmentRule.KEEP_WORLD,
                          unreal.AttachmentRule.KEEP_WORLD, False)
    # Relative locations are in the body's local space, which its scale stretches: the cylinder mesh is 100 units
    # wide and tall, so x = 50 is its surface and z is divided by the height scale. The visor sits at the eyes
    # (64 units above the feet = 28 units above the middle).
    visor.set_actor_relative_location(
        unreal.Vector(50.0, 0.0, 28 * UNITS_TO_CM / HULL_SCALE[2]), False, False)
    return body, [visor]


def character_pipelines(fps):
    """The Interchange pipelines agent files import with: the glTF defaults, except that
    - all meshes of one skeleton combine into one skeletal mesh: the default glTF pipeline makes one skeletal mesh
      per glTF mesh, and the weapons and gloves an agent file holds (bones of its own skeleton) would become separate
      meshes the player's actor does not show;
    - animations are sampled at `fps`, the rate the export keyed them at (the demo's tick rate): left to guess,
      Interchange picks a standard rate and would blend between the ticks.
    The modified copy is saved as /Game/OFAST/Pipelines/GLTFCharacters."""
    path = "/Game/OFAST/Pipelines/GLTFCharacters"
    pipeline = assets.load_asset(path) if assets.does_asset_exist(path) else None
    if pipeline is None:
        pipeline = assets.duplicate_loaded_asset(
            assets.load_asset("/Interchange/Pipelines/DefaultGLTFAssetsPipeline"), path)
    changed = False
    behaviour = unreal.InterchangeCombineSkeletalMeshesBehavior.BY_SKELETON
    meshes = pipeline.get_editor_property("mesh_pipeline")
    if meshes.get_editor_property("combine_skeletal_meshes_behavior") != behaviour:
        meshes.set_editor_property("combine_skeletal_meshes_behavior", behaviour)
        changed = True
    animation = pipeline.get_editor_property("animation_pipeline")
    # Interchange measures the animation a hair longer than its last key (75.922 s for 4859 ticks of 1/64 s) and
    # refuses lengths between frames unless they snap: the keys stay on the ticks, the length gains the last frame.
    snap = unreal.InterchangeFrameAlignment.SNAP_TO_CLOSEST
    if (animation.get_editor_property("custom_bone_animation_sample_rate") != int(fps)
            or animation.get_editor_property("frame_alignment") != snap):
        animation.set_editor_property("use30_hz_to_bake_bone_animation", False)
        animation.set_editor_property("custom_bone_animation_sample_rate", int(fps))
        animation.set_editor_property("frame_alignment", snap)
        changed = True
    if changed:
        assets.save_loaded_asset(pipeline)
    return [unreal.SoftObjectPath(pipeline.get_path_name()),
            unreal.SoftObjectPath("/Interchange/Pipelines/DefaultGLTFPipeline.DefaultGLTFPipeline")]


def static_pipelines():
    """The Interchange pipelines the dropped weapons' files (rigid meshes, no skin) import with: the glTF defaults,
    except that all meshes of a file combine into one static mesh (the default makes one per glTF mesh, and the actor
    would show only one part of the weapon). Saved as /Game/OFAST/Pipelines/GLTFStaticCombined."""
    path = "/Game/OFAST/Pipelines/GLTFStaticCombined"
    pipeline = assets.load_asset(path) if assets.does_asset_exist(path) else None
    if pipeline is None:
        pipeline = assets.duplicate_loaded_asset(
            assets.load_asset("/Interchange/Pipelines/DefaultGLTFAssetsPipeline"), path)
    meshes = pipeline.get_editor_property("mesh_pipeline")
    behaviour = unreal.InterchangeCombineStaticMeshesBehavior.ALL
    if meshes.get_editor_property("combine_static_meshes_behavior") != behaviour:
        meshes.set_editor_property("combine_static_meshes_behavior", behaviour)
        assets.save_loaded_asset(pipeline)
    return [unreal.SoftObjectPath(pipeline.get_path_name()),
            unreal.SoftObjectPath("/Interchange/Pipelines/DefaultGLTFPipeline.DefaultGLTFPipeline")]


def set_gltf_specular(gltf_path, objects):
    """Sets SpecularFactor on the imported material instances that Interchange's glTF master draws without a specular
    texture, as import_map.py does for maps (gltf_specular.specular_factor): 0.25 on CS2's weapons (glTF
    specularFactor 0.5, F0 0.02), 0.5 on the rest (F0 0.04); CHARACTER_SPECULAR_FACTOR on the agents. Their
    cloth-shaded materials (KHR_materials_sheen, Interchange's sheen master) carry a specular texture, which the master
    applies itself (F0 0.04 off the cloth), so they are left as they are. Interchange
    leaves it at 1, F0 0.08. Instances are found through the meshes' material slots, which keep the glTF material
    names: Interchange shortens long asset names in the middle (weapon_kni_e_karambit), so matching by the instance's
    name missed some weapons."""
    with open(gltf_path, encoding="utf-8") as f:
        materials = {m["name"]: m for m in json.load(f).get("materials", [])}
    masters = {assets.load_asset(path).get_path_name()
               for path in (gltf_specular.INTERCHANGE_MASTER, gltf_specular.SHEEN_MASTER)}
    slots = []
    for o in objects:
        if isinstance(o, unreal.StaticMesh):
            slots += o.get_editor_property("static_materials")
        elif isinstance(o, unreal.SkeletalMesh):
            slots += o.get_editor_property("materials")
    found = {}
    for slot in slots:
        instance = slot.get_editor_property("material_interface")
        name = str(slot.get_editor_property("material_slot_name"))
        if isinstance(instance, unreal.MaterialInstanceConstant) and name in materials:
            found[instance.get_path_name()] = (instance, materials[name])
    count = characters = cloth = 0
    for instance, material in found.values():
        if not gltf_specular.takes_specular_factor(instance, masters):
            continue
        character = material.get("extras", {}).get("ofast", {}).get("shader") == "csgo_character.vfx"
        characters += character
        if "KHR_materials_sheen" in material.get("extensions", {}):
            # CS2's cloth shading: its sheen instead of a highlight on the cloth (the export's specular texture, which
            # the master applies itself, so such instances do not get here; without one, the glTF's factor).
            factor = gltf_specular.specular_factor(material)
            cloth += 1
        elif character:
            factor = CHARACTER_SPECULAR_FACTOR
        else:
            factor = gltf_specular.specular_factor(material)
        unreal.MaterialEditingLibrary.set_material_instance_scalar_parameter_value(instance, "SpecularFactor", factor)
        count += 1
    log("  SpecularFactor set on %d of %d glTF materials (%d on mesh slots), %d of them csgo_character (%d cloth)"
        % (count, len(materials), len(found), characters, cloth))


def agents_manifest(replay):
    """(folder, "<demo>_r<round>", manifest) of the agents manifest next to the replay, or None without one."""
    folder = os.path.dirname(REPLAY_PATH)
    base = "%s_r%d" % (replay["demo"], replay["round"])
    manifest_path = os.path.join(folder, base + ".agents.json")
    if not os.path.exists(manifest_path):
        return None
    with open(manifest_path, encoding="utf-8") as f:
        return folder, base, json.load(f)


def import_gltf(replay, path, look, pipelines):
    """Imports one glTF of the players export into its own folder (named after `look`) and returns the objects."""
    manager = unreal.InterchangeManager.get_interchange_manager_scripted()
    params = unreal.ImportAssetParameters()
    params.is_automated = True
    params.replace_existing = True
    params.override_pipelines = pipelines
    # A short folder: asset file paths must stay under Windows' 260 characters.
    destination = "/Game/OFAST/Replays/Agents/%s_r%d_%s" % (replay["demo"][-12:], replay["round"], look)
    # Fresh every run: Interchange keeps existing material instances on a re-import (decision 006), so a changed
    # material conversion would not show. The level's old replay actors are already gone.
    if assets.does_directory_exist(destination):
        assets.delete_directory(destination)
    objects = manager.import_asset(destination, unreal.InterchangeManager.create_source_data(path), params)
    set_gltf_specular(path, objects)
    # Interchange leaves imported assets in memory only: saved, the level's actors keep them when reopened.
    assets.save_directory(destination, only_if_is_dirty=True, recursive=True)
    return objects


def finish_key(finish):
    """A weapon's skin as a key: (kit, seed, wear, stickers, keychain and name tag), or None for none (the replay's
    and the manifest's `finish`; the extras as written, so two weapons with one paint and different stickers,
    keychains or names are two looks)."""
    if finish is None:
        return None
    extras = json.dumps({"stickers": finish.get("stickers", []), "keychain": finish.get("keychain"),
                         "name_tag": finish.get("name_tag")}, sort_keys=True)
    return (finish["kit"], finish["seed"], finish["wear"], extras)


def import_dropped_weapons(replay):
    """Imports the models of the weapons lying in the world (the agents manifest's `dropped_weapons`, one glTF per
    look and skin). Returns (model id, mesh groups, finish) -> mesh, as the replay's `dropped_weapons` name them."""
    loaded = agents_manifest(replay)
    if loaded is None:
        return {}
    folder, base, manifest = loaded
    pipelines = static_pipelines()
    meshes = {}
    for entry in manifest.get("dropped_weapons", []):
        path = os.path.join(folder, entry["file"])
        look = entry["file"][len(base) + 1:-len(".gltf")]  # dropped_<model file stem>_<mesh groups shown>
        objects = import_gltf(replay, path, look, pipelines)
        mesh = next((o for o in objects if isinstance(o, unreal.StaticMesh)), None)
        if mesh is None:
            log("dropped weapon %s: no mesh in %s" % (entry["model"], path))
            continue
        # Every mesh-group mask the replay uses for this file (grenades send every bit).
        for groups in entry["mesh_groups"]:
            meshes[(entry["model_id"], groups, finish_key(entry.get("finish")))] = mesh
    if meshes:
        log("dropped weapons: %d models" % len(meshes))
    return meshes


def import_cases(replay):
    """Imports the case models the round's brass throws (the agents manifest's `cases`, one glTF per model). Returns
    model path -> mesh."""
    loaded = agents_manifest(replay)
    if loaded is None:
        return {}
    folder, base, manifest = loaded
    pipelines = static_pipelines()
    meshes = {}
    for entry in manifest.get("cases", []):
        path = os.path.join(folder, entry["file"])
        look = entry["file"][len(base) + 1:-len(".gltf")]  # case_<model file stem>
        objects = import_gltf(replay, path, look, pipelines)
        mesh = next((o for o in objects if isinstance(o, unreal.StaticMesh)), None)
        if mesh is None:
            log("case %s: no mesh in %s" % (entry["model"], path))
            continue
        meshes[entry["model"]] = mesh
    return meshes


def place_brass(sequence, replay, meshes, end_frame, viewmodel_actors, hide_in_own_view, own_view_only):
    """One actor per case the round's guns threw (the agents manifest's `brass`, written by export::brass), its model
    keyed as the export flew it and shown from leaving the port to the end of its life (`still`: it lies at its last
    key). A case thrown from the gun the agent holds (`view` false) is in the world. Where its shooter's own view
    shows the first-person case instead (a shooter with a viewmodel), its flight (the first CASE_VIEW_FLIGHT_S) is an
    actor their own camera does not see (`hide_in_own_view(actor, controller)`), and the case lying after it another
    one, seen by every camera (CS2 draws the cases on the floor in first person too). One thrown from the viewmodel
    (`view` true) rides on the shooter's viewmodel actor, keyed in its space, drawn with Unreal's first-person
    rendering and seen only by their own camera (`own_view_only(actor, controller)`). Movable, as Unreal attaches
    only movable components to a moving actor and Movie Render Queue needs them to move. Returns the number of keys."""
    loaded = agents_manifest(replay)
    if loaded is None:
        return 0
    tick0 = replay["start_tick"]
    fps = round(1.0 / replay["tick_interval"])
    keys = placed = 0
    keep = unreal.AttachmentRule.KEEP_RELATIVE

    def spawn_case(mesh, label, points, parent, shown):
        nonlocal keys
        actor = spawn_tagged(mesh, label)
        component = actor.static_mesh_component
        component.set_mobility(unreal.ComponentMobility.MOVABLE)
        component.set_editor_property("cast_shadow", False)
        if parent is not None:
            actor.attach_to_actor(parent, "", keep, keep, keep, False)
            component.set_editor_property("first_person_primitive_type", unreal.FirstPersonPrimitiveType.FIRST_PERSON)
        binding = sequence.add_possessable(actor)
        frames = [k[0] - tick0 for k in points]
        locations = [to_unreal(k[1:4]) for k in points]
        # Source (pitch, yaw, roll) -> Rotator(roll=roll, pitch=-pitch, yaw=-yaw), as the dropped weapons'.
        rotations = euler_filter([(k[6], -k[4], -k[5]) for k in points])
        keys += key_transform(binding, frames, locations, rotations, (1.0, 1.0, 1.0), end_frame)
        first, last = shown
        key_visibility(binding, ([(0, False)] if first > 0 else []) + [(first, True), (last + 1, False)], end_frame)
        return actor

    for case in loaded[2].get("brass", []):
        mesh = meshes.get(case["model"])
        controller = case["controller"]
        viewmodel = viewmodel_actors.get(controller)
        parent = viewmodel if case["view"] else None
        first, last = case["first_tick"] - tick0, case["last_tick"] - tick0
        if mesh is None or (case["view"] and parent is None) or last < 0 or first >= end_frame:
            continue
        label = "%s case %d at %d" % ("view" if case["view"] else "world", controller, case["tick"])
        if case["view"]:
            own_view_only(spawn_case(mesh, label, case["keys"], parent, (first, last)), controller)
        elif viewmodel is None:
            spawn_case(mesh, label, case["keys"], None, (first, last))
        else:
            flight_end = first + round(CASE_VIEW_FLIGHT_S * fps)
            hide_in_own_view(spawn_case(mesh, label, case["keys"], None, (first, min(flight_end, last + 1) - 1)),
                             controller)
            if flight_end <= last:
                spawn_case(mesh, label + " lying", case["keys"], None, (flight_end, last))
        placed += 1
    if placed:
        log("brass: %d cases, %d keys" % (placed, keys))
    return keys


def place_dropped_weapons(sequence, replay, meshes, end_frame):
    """One actor per stretch a weapon lay in the world, keyed as the demo moves it and shown only then. Returns the
    number of keys."""
    keys, placed = place_keyed_models(
        sequence, replay, end_frame, replay.get("dropped_weapons", []),
        lambda s: meshes.get((s["model"], s["mesh_groups"], finish_key(s.get("finish")))),
        lambda s: "dropped %s #%d at %d" % (s["class"], s["entity"], s["first_tick"]))
    if placed:
        log("dropped weapons: %d placed, %d keys" % (placed, keys))
    return keys


def place_grenades(sequence, replay, meshes, end_frame):
    """One actor per thrown grenade (the replay's `grenades`): its model (the agents manifest's models, mesh groups
    0) flying along its keys from the throw to its last tick — an HE grenade's and a flashbang's detonation, a smoke's
    expiry, a molotov's landing. Returns the number of keys."""
    keys, placed = place_keyed_models(
        sequence, replay, end_frame, replay.get("grenades", []),
        lambda g: meshes.get((g["model"], 0, None)),
        lambda g: "grenade %s #%d at %d" % (g["kind"], g["entity"], g["first_tick"]))
    if placed:
        log("grenades: %d placed, %d keys" % (placed, keys))
    return keys


def place_keyed_models(sequence, replay, end_frame, stretches, mesh_of, label_of):
    """One static mesh actor per stretch (`first_tick`, `last_tick`, `keys` = [tick, x, y, z, pitch, yaw, roll]), keyed
    as the demo moves it and shown from its first tick to its last. Returns (keys, actors placed)."""
    tick0 = replay["start_tick"]
    keys = 0
    placed = 0
    for stretch in stretches:
        mesh = mesh_of(stretch)
        if mesh is None:
            continue
        first, last = stretch["first_tick"] - tick0, stretch["last_tick"] - tick0
        actor = spawn_tagged(mesh, label_of(stretch))
        binding = sequence.add_possessable(actor)
        points = stretch["keys"]
        frames = [k[0] - tick0 for k in points]
        locations = [to_unreal(k[1:4]) for k in points]
        # Source (pitch, yaw, roll) -> Rotator(roll=roll, pitch=-pitch, yaw=-yaw), filtered so a tumble never spins
        # the long way between two ticks.
        rotations = euler_filter([(k[6], -k[4], -k[5]) for k in points])
        keys += key_transform(binding, frames, locations, rotations, (1.0, 1.0, 1.0), end_frame)
        changes = [(0, first <= 0), (max(first, 0), True)]
        if last + 1 < end_frame:
            changes.append((last + 1, False))
        key_visibility(binding, changes, end_frame)
        placed += 1
    return keys, placed


def animate_doors(sequence, replay, end_frame):
    """The map's rotating doors swung as the demo has them (the replay's `doors`). Each door is the map import's own
    actor whose origin is the door's closed position (its hinge, the map entity's origin) and whose rotation is its
    closed rotation; it is keyed with the demo's rotation on the ticks it swung, keeping its location and scale, and
    takes no decals. Every stretch with the same closed position drives the same actor. A door an HE grenade blew
    apart is hidden from then on (broken_doors). A door that never swings nor breaks is left as the map has it.
    Returns the number of keys."""
    doors = replay.get("doors", [])
    if not doors:
        return 0
    tick0 = replay["start_tick"]
    by_hinge = {}
    closed_angles = {}
    for door in doors:
        hinge = tuple(door["closed_position"])
        by_hinge.setdefault(hinge, {}).update({k[0]: k for k in door["keys"]})
        closed_angles[hinge] = door["closed_angles"]
    broken_at = broken_doors(replay)
    candidates = [a for a in actors.get_all_level_actors()
                  if TAG not in a.tags and isinstance(a, unreal.StaticMeshActor)]
    keys = 0
    swung = 0
    broken = 0
    for hinge, by_tick in by_hinge.items():
        points = [by_tick[t] for t in sorted(by_tick)]
        pitch, yaw, roll = closed_angles[hinge]
        closed = unreal.Rotator(roll=roll, pitch=-pitch, yaw=-yaw)
        first = unreal.Rotator(roll=points[0][6], pitch=-points[0][4], yaw=-points[0][5])
        gone = broken_at.get(hinge)
        still = all(p[4:] == points[0][4:] for p in points) and angle_between(first, closed) < 1.0
        # Closed all round and never broken: the map already shows it so.
        if still and not gone:
            continue
        target = unreal.Vector(*to_unreal(hinge))
        near = [a for a in candidates if (a.get_actor_location() - target).length() < DOOR_MATCH_CM]
        # The door itself turns like its closed rotation (a frame or wall placed at the same point need not).
        matches = [a for a in near if angle_between(a.get_actor_rotation(), closed) < 1.0]
        if not matches:
            log("door at %s: no map actor there with its closed rotation (%d at the hinge): not swung"
                % (hinge, len(near)))
            continue
        frames = [p[0] - tick0 for p in points]
        rotations = euler_filter([(p[6], -p[4], -p[5]) for p in points])
        for actor in matches:
            # No decals: the bullets are traced without the doors, and a decal marks whatever lies in its box, so a
            # hole or scorch on a surface behind a door that swings would mark the door (CS2 marks the surface hit).
            actor.static_mesh_component.set_editor_property("receives_decals", False)
            binding = sequence.add_possessable(actor)
            if not still:
                location = actor.get_actor_location()
                scale = actor.get_actor_scale3d()
                keys += key_transform(binding, frames, [(location.x, location.y, location.z)] * len(frames),
                                      rotations, (scale.x, scale.y, scale.z), end_frame)
            if gone:
                key_visibility(binding, [(0, True), (gone - tick0, False)], end_frame)
        swung += not still
        broken += gone is not None
        log("door at %s: %s, %d keys, swings %.0f degrees%s" % (
            hinge, ", ".join(a.get_actor_label() for a in matches), len(points),
            max(abs(r[2] - rotations[0][2]) for r in rotations), ", broken at %d" % gone if gone else ""))
    if swung or broken:
        log("doors: %d of %d swung, %d broken" % (swung, len(by_hinge), broken))
    return keys


def animate_props(sequence, replay, end_frame):
    """The map's physics props moved as the demo has them (the replay's `props`: de_nuke's hanging hard hats and mugs,
    which bullets, blasts and players push about). CS2 makes every prop again where the map entity puts it (at the
    round's start, sometimes within it), so the entries with the same first key are one prop: the map import's own
    actors there with that rotation (every mesh node of its model), keyed with the entries' origins and rotations (a
    prop made again jumps back where the map has it), hidden after the last entry's last tick when the prop leaves
    before the round's end. A prop that never moves nor leaves is left as the map has it, and so is one with no actor
    there. Returns the number of keys."""
    props = replay.get("props", [])
    if not props:
        return 0
    tick0 = replay["start_tick"]
    end_tick = replay["end_tick"]
    # One prop per first key: its entries' keys by tick, and the last tick any of them was there.
    by_spawn = {}
    for prop in props:
        spawn = tuple(prop["keys"][0][1:])
        keyed, last = by_spawn.setdefault(spawn, ({}, 0))
        keyed.update({k[0]: k for k in prop["keys"]})
        by_spawn[spawn] = (keyed, max(last, prop["last_tick"]))
    candidates = [a for a in actors.get_all_level_actors()
                  if TAG not in a.tags and isinstance(a, unreal.StaticMeshActor)]
    keys = 0
    moved = 0
    for spawn, (keyed, last) in by_spawn.items():
        points = [keyed[t] for t in sorted(keyed)]
        leaves = last < end_tick
        if all(p[1:] == list(spawn) for p in points) and not leaves:
            continue
        start = unreal.Vector(*to_unreal(spawn[:3]))
        rotation = unreal.Rotator(roll=spawn[5], pitch=-spawn[3], yaw=-spawn[4])
        matches = [a for a in candidates if (a.get_actor_location() - start).length() < PROP_MATCH_CM
                   and angle_between(a.get_actor_rotation(), rotation) < 2.0]
        if not matches:
            log("prop at %s: no map actor there with its rotation: not moved" % (spawn[:3],))
            continue
        frames = [p[0] - tick0 for p in points]
        rotations = euler_filter([(p[6], -p[4], -p[5]) for p in points])
        for actor in matches:
            component = actor.static_mesh_component
            component.set_editor_property("mobility", unreal.ComponentMobility.MOVABLE)
            # No decals: they mark whatever lies in their box, and a hat landing in a scorch's would show it.
            component.set_editor_property("receives_decals", False)
            binding = sequence.add_possessable(actor)
            scale = actor.get_actor_scale3d()
            keys += key_transform(binding, frames, [to_unreal(p[1:4]) for p in points], rotations,
                                  (scale.x, scale.y, scale.z), end_frame)
            if leaves:
                key_visibility(binding, [(0, True), (last + 1 - tick0, False)], end_frame)
        moved += 1
    log("props: %d of %d moved or removed" % (moved, len(by_spawn)))
    return keys


def broken_doors(replay):
    """Hinge -> the tick a door was blown apart, for the replay's doors an HE grenade broke. CS2 removes the entity of a
    door a blast breaks and draws the doorway clear from then: on the Aurora vs FURIA de_nuke demo the A door's entity
    leaves the demo mid-round in each of rounds 1 - 8, every time one tick before an HE grenade goes off 9 - 83 units
    from its hinge, and players cross the doorway only after. Doors also leave at the round's restart (320 ticks before
    the next freeze time), unbroken: so a door counts as broken only when a blast follows its entity's last tick within
    DOOR_BLAST_TICKS, within DOOR_BLAST_REACH of its hinge."""
    blasts = [g["detonation"] for g in replay.get("grenades", []) if g["kind"] == "he" and g.get("detonation")]
    last_seen = {}
    for door in replay.get("doors", []):
        hinge = tuple(door["closed_position"])
        last_seen[hinge] = max(last_seen.get(hinge, 0), door["last_tick"])
    out = {}
    for hinge, last in last_seen.items():
        for blast in blasts:
            near = math.dist(blast["position"], hinge) <= DOOR_BLAST_REACH
            if near and 0 < blast["tick"] - last <= DOOR_BLAST_TICKS:
                out[hinge] = blast["tick"]
    return out


def angle_between(a, b):
    """Degrees between two Rotators (the angle of the rotation from one to the other)."""
    qa, qb = a.quaternion(), b.quaternion()
    dot = abs(qa.x * qb.x + qa.y * qb.y + qa.z * qb.z + qa.w * qb.w)
    return math.degrees(2.0 * math.acos(min(1.0, dot)))


def import_agents(replay):
    """Imports the players' animated agents written next to the replay by `ofast export-demo --players`, if any: the
    files <demo>_r<round>.agents.json lists, one per agent look. Returns controller -> (skeletal mesh,
    AnimSequence `player_<controller>` (import_player_file), the [first tick, last tick] of each death the animation
    holds as a ragdoll: the body stays visible there)."""
    loaded = agents_manifest(replay)
    if loaded is None:
        return {}
    folder, base, manifest = loaded
    pipelines = character_pipelines(manifest.get("fps", 30))
    agents = {}
    for entry in manifest["agents"]:
        path = os.path.join(folder, entry["file"])
        look = entry["file"][len(base) + 1:-len(".gltf")]  # <agent>_<mesh groups>[_<gloves>]
        meshes, animations = import_player_file(replay, path, look, pipelines, entry["players"])
        if not meshes:
            log("agents %s: no skeletal mesh in %s" % (look, path))
            continue
        rate = ""
        if animations:
            sample = next(iter(animations.values()))
            rate = ", %d frames over %.3f s" % (unreal.AnimationLibrary.get_num_frames(sample),
                                                sample.get_play_length())
        # Controller (as text) -> [[first tick, last tick], …] of each death baked as a ragdoll.
        ragdolls = entry.get("ragdolls", {})
        log("agents %s: %s with %d player animations%s, %d weapons, %d deaths as ragdolls"
            % (look, meshes[0].get_name(), len(animations), rate, len(entry.get("weapons", [])),
               sum(len(r) for r in ragdolls.values())))
        for controller in entry["players"]:
            if controller in animations:
                agents[controller] = (meshes[0], animations[controller], ragdolls.get(str(controller), []))
    return agents


def import_player_file(replay, path, look, pipelines, players):
    """Imports one glTF with an animation per player (an agent look, a viewmodel look). Returns (skeletal meshes,
    controller -> AnimSequence). Interchange names each AnimSequence <file><animation>, shortened in the middle when
    long ("aurora-_layer_10"): only the trailing `_<controller>` is sure to remain — except in a file with a single
    animation, which it names <file>_Anim (a look worn by one player only)."""
    objects = import_gltf(replay, path, look, pipelines)
    meshes = [o for o in objects if isinstance(o, unreal.SkeletalMesh)]
    sequences = [o for o in objects if isinstance(o, unreal.AnimSequence)]
    animations = {}
    for o in sequences:
        match = re.search(r"_(\d+)$", o.get_name())
        if match:
            animations[int(match.group(1))] = o
    if not animations and len(sequences) == 1 and len(players) == 1:
        animations[players[0]] = sequences[0]
    return meshes, animations


def import_viewmodels(replay):
    """Imports the players' first-person views (the agents manifest's `viewmodels`: one glTF per agent and glove
    skin, with an animation `viewmodel_<controller>` per player). Returns controller -> (skeletal mesh,
    AnimSequence)."""
    loaded = agents_manifest(replay)
    if loaded is None:
        return {}
    folder, base, manifest = loaded
    pipelines = character_pipelines(manifest.get("fps", 30))
    viewmodels = {}
    for entry in manifest.get("viewmodels", []):
        path = os.path.join(folder, entry["file"])
        look = entry["file"][len(base) + 1:-len(".gltf")]  # viewmodel_<agent>[_<gloves>]
        meshes, animations = import_player_file(replay, path, look, pipelines, entry["players"])
        if not meshes:
            log("viewmodel %s: no skeletal mesh in %s" % (look, path))
            continue
        log("viewmodel %s: %s with %d player animations, %d weapons"
            % (look, meshes[0].get_name(), len(animations), len(entry.get("weapons", []))))
        for controller in entry["players"]:
            if controller in animations:
                viewmodels[controller] = (meshes[0], animations[controller])
    return viewmodels


def viewmodel_fov_16_9(fov):
    """CS2's viewmodel_fov is horizontal at 4:3 like its world FOV: the horizontal FOV of a 16:9 picture."""
    return math.degrees(2 * math.atan(math.tan(math.radians(fov / 2)) * (16 / 9) / (4 / 3)))


def spawn_viewmodel(player, mesh, camera):
    """The player's viewmodel on their first-person camera, as CS2 draws it (decision 016): the rig's root is the
    camera, drawn with Unreal's first-person rendering at the player's viewmodel_fov (FirstPersonFieldOfView),
    without shadows. Its place on the camera (the player's offsets, and the hand) is keyed by key_viewmodel_hand."""
    settings = player.get("viewmodel") or {}
    actor = actors.spawn_actor_from_object(mesh, unreal.Vector(0.0, 0.0, 0.0))
    actor.tags = [TAG]
    actor.set_actor_label("VM %s" % player["name"])
    component = actor.skeletal_mesh_component
    component.set_editor_property("first_person_primitive_type", unreal.FirstPersonPrimitiveType.FIRST_PERSON)
    component.set_editor_property("cast_shadow", False)
    keep = unreal.AttachmentRule.KEEP_RELATIVE
    actor.attach_to_actor(camera, "", keep, keep, keep, False)
    lens = camera.camera_component
    lens.set_editor_property("enable_first_person_field_of_view", True)
    lens.set_editor_property("first_person_field_of_view", viewmodel_fov_16_9(settings.get("fov", 60.0)))
    return actor


def key_viewmodel_hand(binding, frames, hands, offset, end_frame, punch=None):
    """The viewmodel's transform on its camera: moved by the player's viewmodel_offset_x / _y / _z (inches in view
    space: right, forward, up; Unreal's camera looks along +X with +Y to the right), and mirrored from every tick the
    player holds it in the left hand (m_bLeftHanded, which the switch-hands key flips during a round), the sideways
    offset with it — CS2 mirrors the whole viewmodel (cl_righthand 0; the mirrored offset is not checked against CS2
    yet). Step keys: the hand changes from one tick to the next. Turned past the camera by recoil on the ticks
    `punch` (per frame: export::punch's (camera pitch, yaw, viewmodel pitch, yaw), or None) has it, linear keys, the
    yaw mirrored with the viewmodel."""
    right, forward, up = offset
    track = binding.add_track(unreal.MovieScene3DTransformTrack)
    section = track.add_section()
    section.set_range(0, end_frame)
    channels = channels_by_name(section)
    for name, value in (("Location.X", forward * UNITS_TO_CM), ("Location.Y", right * UNITS_TO_CM),
                        ("Location.Z", up * UNITS_TO_CM), ("Rotation.X", 0.0), ("Rotation.Y", 0.0),
                        ("Rotation.Z", 0.0), ("Scale.X", 1.0), ("Scale.Y", 1.0), ("Scale.Z", 1.0)):
        channels[name].set_default(value)
    constant = unreal.MovieSceneKeyInterpolation.CONSTANT
    previous = None
    for frame, left in zip(frames, hands):
        if left is None or left == previous:
            continue
        side = -1.0 if left else 1.0
        for name, value in (("Location.Y", side * right * UNITS_TO_CM), ("Scale.Y", side)):
            channels[name].add_key(unreal.FrameNumber(frame), value, 0.0, unreal.MovieSceneTimeUnit.DISPLAY_RATE,
                                   constant)
        previous = left
    # Recoil: Source's pitch and yaw negated, as the camera's are; keyed while turned, with a key of nothing on the
    # tick before and the tick after each stretch (a channel holds its first key's value before it, and a linear
    # segment between two stretches would drift).
    linear = unreal.MovieSceneKeyInterpolation.LINEAR
    turned = False
    left = False
    before = None
    for frame, hand, turn in zip(frames, hands, punch or []):
        left = left if hand is None else hand
        now = turn is not None and (abs(turn[2]) > 0.0 or abs(turn[3]) > 0.0)
        keyed = []
        if now and not turned and before is not None:
            keyed.append((before, 0.0, 0.0))
        if now or turned:
            keyed.append((frame, -turn[2], -turn[3]) if now else (frame, 0.0, 0.0))
        for at, pitch, yaw in keyed:
            for name, value in (("Rotation.Y", pitch), ("Rotation.Z", -yaw if left else yaw)):
                channels[name].add_key(unreal.FrameNumber(at), value, 0.0,
                                       unreal.MovieSceneTimeUnit.DISPLAY_RATE, linear)
        turned = now
        before = frame


def spawn_agent(player, mesh):
    """The player as the team's agent model (feet at the actor's origin, facing +X). It takes no decals: a blast's
    scorch (grenade_effects.SCORCH) would blacken the feet of a player walking over it."""
    actor = actors.spawn_actor_from_object(mesh, unreal.Vector(0.0, 0.0, 0.0))
    actor.skeletal_mesh_component.set_editor_property("receives_decals", False)
    actor.tags = [TAG]
    actor.set_actor_label("%s %s" % (TEAM_NAMES.get(player["team"], "?"), player["name"]))
    return actor


def key_animation(binding, animation, end_frame):
    """A skeletal animation track playing `animation` from the sequence's first frame (the round's first tick, where
    the baked animation starts too)."""
    track = binding.add_track(unreal.MovieSceneSkeletalAnimationTrack)
    section = track.add_section()
    section.set_range(0, end_frame)
    params = section.get_editor_property("params")
    params.set_editor_property("animation", animation)
    section.set_editor_property("params", params)


def spawn_pov_camera(player):
    camera = spawn_tagged(unreal.CameraActor, "POV %s" % player["name"])
    component = camera.camera_component
    component.set_editor_property("field_of_view", HFOV_16_9)
    component.set_editor_property("aspect_ratio", 16 / 9)
    component.set_editor_property("constrain_aspect_ratio", True)
    return camera


def key_transform(binding, frames, locations, rotations, scale, end_frame):
    """A transform track: location (cm) and rotation (Unreal degrees: (pitch, yaw), or (roll, pitch, yaw) for a full
    rotation) keyed linearly, after dropping keys a straight line already gives. `scale` is the constant scale (the
    track overrides the actor's)."""
    track = binding.add_track(unreal.MovieScene3DTransformTrack)
    section = track.add_section()
    section.set_range(0, end_frame)
    channels = channels_by_name(section)
    for axis, value in zip(("X", "Y", "Z"), scale):
        channels["Scale." + axis].set_default(value)
    channels["Rotation.X"].set_default(0.0)
    linear = unreal.MovieSceneKeyInterpolation.LINEAR
    count = 0
    for name, values, tolerance in (("Location", locations, POSITION_TOLERANCE_CM),
                                    ("Rotation", rotations, ANGLE_TOLERANCE_DEG)):
        if values is None:
            continue
        for run in runs(frames, values):
            # Rotation channels: roll (X), pitch (Y), yaw (Z); players key pitch and yaw only.
            axes = ("X", "Y", "Z") if name == "Location" or len(run[0][1]) == 3 else ("Y", "Z")
            for frame, value in reduce_keys([f for f, _ in run], [v for _, v in run], tolerance):
                for axis, v in zip(axes, value):
                    channels["%s.%s" % (name, axis)].add_key(unreal.FrameNumber(frame), v, 0.0,
                                                             unreal.MovieSceneTimeUnit.DISPLAY_RATE, linear)
                    count += 1
    return count


def life_changes(frames, alive):
    """(frame, visible) at every sample: shown while `alive` (a pawn, and alive or lying as a ragdoll)."""
    return list(zip(frames, alive))


def apply_owners(cameras, hidden_in_own_view, own_view_only):
    """Owner visibility (decision 021): each player's POV camera owns what their own view must not show (every
    primitive of those actors bOwnerNoSee: their body, their flashes in the world, the blood on them, their cases in
    flight) and what only it shows (bOnlyOwnerSee: their viewmodel and what rides on it), so Unreal hides them in that
    camera only, whatever camera a cut uses. Applied last, after the effects added their components. `cameras`:
    controller -> (camera, binding); the others: controller -> actors."""
    owned = 0
    for controller, (camera, _) in cameras.items():
        for group, flag in ((hidden_in_own_view.get(controller, []), "owner_no_see"),
                            (own_view_only.get(controller, []), "only_owner_see")):
            for actor in group:
                actor.set_owner(camera)
                for component in actor.get_components_by_class(unreal.PrimitiveComponent):
                    component.set_editor_property(flag, True)
                owned += 1
    log("owner visibility: %d actors on %d POV cameras" % (owned, len(cameras)))


def build(replay):
    tick0 = replay["start_tick"]
    ticks = replay["samples"]["ticks"]
    frames = [t - tick0 for t in ticks]
    end_frame = replay["end_tick"] - tick0 + 1
    fps = round(1.0 / replay["tick_interval"])
    name = "%s_r%d" % (replay["demo"], replay["round"])
    path = "/Game/OFAST/Replays/" + name
    cameras_path = replay_cameras.cameras_path(REPLAY_PATH)
    if not os.path.exists(cameras_path):
        raise RuntimeError("no cameras file %s (exported before M7?): export the round again" % cameras_path)
    cameras_file = replay_cameras.load(cameras_path)
    if assets.does_asset_exist(path):
        assets.delete_asset(path)
    sequence = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, "/Game/OFAST/Replays", unreal.LevelSequence, unreal.LevelSequenceFactoryNew())
    sequence.set_display_rate(unreal.FrameRate(fps, 1))
    sequence.set_playback_start(0)
    sequence.set_playback_end(end_frame)

    cameras = {}
    # Each player's body (frame, visible) changes: their muzzle flashes and the blood on them follow it
    # (shot_effects); and their viewmodel actor and its changes, which their first-person flashes and tracers follow.
    body_life = {}
    viewmodel_actors = {}
    viewmodel_life = {}
    # What each player's own view must not show, and what only it shows: owners set at the end (apply_owners).
    hidden_in_own_view = {}
    own_view_only = {}
    started = time.monotonic()
    keys = 0
    agents = import_agents(replay)
    viewmodels = import_viewmodels(replay)
    manifest = agents_manifest(replay)
    # Recoil on each player's viewmodel (export::punch): tick -> (camera pitch, yaw, viewmodel pitch, yaw), Source's;
    # the cameras file has the camera's part in the eyes already.
    punches = {int(c): {k[0]: k[1:] for k in keys}
               for c, keys in ((manifest[2] if manifest else {}).get("punch") or {}).items()}
    for player in replay["players"]:
        controller = player["controller"]
        track = replay["samples"]["players"][str(controller)]
        positions = [None if p is None else to_unreal(p) for p in track["position"]]
        known = [a for a in track["angles"] if a is not None]
        yaws = iter(unwrap([-a[1] for a in known]))
        pitches = iter([-a[0] for a in known])
        rotations = [None if a is None else (next(pitches), next(yaws)) for a in track["angles"]]
        # The first-person camera: the player's eyes from the cameras file (recoil and smoothing in them).
        camera = spawn_pov_camera(player)
        camera_binding = sequence.add_possessable(camera)
        keys += replay_cameras.key_eyes(camera_binding, cameras_file["eyes"].get(str(controller), []), tick0,
                                        end_frame)
        cameras[controller] = (camera, camera_binding)
        agent = agents.get(controller)
        if agent is not None:
            # The agent: feet on the positions, turned along the body yaw (the way CS2 draws the model).
            body = spawn_agent(player, agent[0])
            attached = []
            body_yaws = iter(unwrap([-y for y in track.get("body_yaw", []) if y is not None]))
            body_rotations = [None if y is None else (0.0, next(body_yaws)) for y in track["body_yaw"]]
            binding = sequence.add_possessable(body)
            keys += key_transform(binding, frames, positions, body_rotations, (1.0, 1.0, 1.0), end_frame)
            key_animation(binding, agent[1], end_frame)
        else:
            body, attached = spawn_proxy(player)
            # The cylinder's pivot is its middle: lift it by half the hull.
            centres = [None if p is None else (p[0], p[1], p[2] + HULL_HALF_HEIGHT_CM) for p in positions]
            body_rotations = [None if r is None else (0.0, r[1]) for r in rotations]
            binding = sequence.add_possessable(body)
            keys += key_transform(binding, frames, centres, body_rotations, HULL_SCALE, end_frame)
        # Shown while alive, and through each death the animation holds as a ragdoll (the body lies there).
        deaths = agent[2] if agent is not None else []
        alive = [(bool(a) or any(first <= t <= last for first, last in deaths)) and p is not None
                 for a, p, t in zip(track["alive"], positions, ticks)]
        changes = life_changes(frames, alive)
        body_life[controller] = changes
        for actor in [body] + attached:
            key_visibility(sequence.add_possessable(actor) if actor is not body else binding, changes, end_frame)
        hidden_in_own_view.setdefault(controller, []).extend([body] + attached)
        # The arms and the weapon in hand, on the camera, seen only through this player's eyes.
        viewmodel = viewmodels.get(controller)
        if viewmodel is not None:
            vm = spawn_viewmodel(player, viewmodel[0], camera)
            viewmodel_actors[controller] = vm
            vm_binding = sequence.add_possessable(vm)
            key_animation(vm_binding, viewmodel[1], end_frame)
            offset = (player.get("viewmodel") or {}).get("offset", [1.0, 1.0, -1.0])
            punch = punches.get(controller, {})
            key_viewmodel_hand(vm_binding, frames, track.get("left_handed", []), offset, end_frame,
                               [punch.get(t) for t in ticks])
            living = [bool(a) and p is not None for a, p in zip(track["alive"], positions)]
            vm_changes = life_changes(frames, living)
            viewmodel_life[controller] = vm_changes
            key_visibility(vm_binding, vm_changes, end_frame)
            own_view_only.setdefault(controller, []).append(vm)
    world_models = import_dropped_weapons(replay)
    keys += place_dropped_weapons(sequence, replay, world_models, end_frame)
    keys += place_grenades(sequence, replay, world_models, end_frame)
    # The demo tick the smokes, fires, blasts and flashed views read: keyed whatever the round has (a round without
    # a smoke still has its fires and flashes).
    smoke_volumes.key_replay_tick(sequence, replay, end_frame)
    smoke_volumes.place_smokes(sequence, replay, REPLAY_PATH, end_frame, spawn_tagged)
    grenade_effects.place_effects(sequence, replay, REPLAY_PATH, end_frame, manifest[2] if manifest else {},
                                  {controller: camera for controller, (camera, _) in cameras.items()}, spawn_tagged)
    own_views = replay_cameras.own_views(cameras_file)

    def hide(actor, binding, controller):
        key_visibility(binding, body_life.get(controller, []), end_frame)
        hidden_in_own_view.setdefault(controller, []).append(actor)

    def first_person(actor, binding, controller):
        key_visibility(binding, viewmodel_life.get(controller, []), end_frame)
        own_view_only.setdefault(controller, []).append(actor)

    def light(owner, binding, controller, in_first_person, component=False):
        replay_cameras.tag_light(owner, controller, in_first_person, component)
        key_visibility(binding, replay_cameras.light_changes(own_views.get(controller, []), in_first_person),
                       end_frame)

    shot_effects.place_shots(sequence, replay, REPLAY_PATH, end_frame, manifest[2] if manifest else {}, spawn_tagged,
                             hide, viewmodel_actors, first_person, light)
    keys += place_brass(sequence, replay, import_cases(replay), end_frame, viewmodel_actors,
                        lambda actor, c: hidden_in_own_view.setdefault(c, []).append(actor),
                        lambda actor, c: own_view_only.setdefault(c, []).append(actor))
    keys += animate_doors(sequence, replay, end_frame)
    keys += animate_props(sequence, replay, end_frame)
    # The shot and HLAE cameras, and the cuts.
    shots = []
    shot_actors = []
    for camera in cameras_file["cameras"]:
        actor = replay_cameras.spawn_shot_camera(camera)
        shot_actors.append((actor, camera))
        shots.append(replay_cameras.key_shot_camera(sequence, actor, camera, tick0, end_frame))
    cuts = replay_cameras.add_cuts(sequence, cameras_file, {c: b for c, (_, b) in cameras.items()}, shots)
    apply_owners(cameras, hidden_in_own_view, own_view_only)
    log("sequence %s: %d players (%d animated agents, %d viewmodels), %d frames at %d fps, %d keys in %.0f s; "
        "%d shot cameras, %d camera cuts"
        % (path, len(replay["players"]), len(agents), len(viewmodels), end_frame, fps, keys,
           time.monotonic() - started, len(shots), cuts))
    add_markers(sequence, replay)
    # The demo's clock, for a camera made here and written back (export_camera.py: ticks and HLAE's game time).
    for tag, value in (("OFAST_start_tick", tick0), ("OFAST_tick_interval", replay["tick_interval"]),
                       ("OFAST_server_tick_offset", cameras_file.get("server_tick_offset")),
                       ("OFAST_demo", replay["demo"]), ("OFAST_map", replay["map"]),
                       ("OFAST_round", replay["round"])):
        if value is not None:
            assets.set_metadata_tag(sequence, tag, str(value))
    assets.save_loaded_asset(sequence)
    return sequence, {controller: camera for controller, (camera, _) in cameras.items()}, shot_actors


def add_markers(sequence, replay):
    tick0 = replay["start_tick"]
    names = {p["controller"]: p["name"] for p in replay["players"]}
    marks = []
    if replay.get("freeze_end_tick"):
        marks.append((replay["freeze_end_tick"] - tick0, "freeze time ends"))
    for kill in replay["kills"]:
        flags = ", HS" if kill.get("headshot") else ""
        attacker = names.get(kill["attacker"], "world")
        marks.append((kill["tick"] - tick0, "%s > %s (%s%s)" % (attacker, names.get(kill["victim"], "?"),
                                                                kill["weapon"], flags)))
    for event in replay["events"]:
        if event["name"].startswith("bomb_") or event["name"] == "round_end":
            marks.append((event["tick"] - tick0, event["name"].replace("_", " ")))
    for frame, label in marks:
        mark = unreal.MovieSceneMarkedFrame()
        mark.set_editor_property("frame_number", unreal.FrameNumber(frame_to_tick_resolution(sequence, frame)))
        mark.set_editor_property("label", label)
        sequence.add_marked_frame(mark)
    log("markers: %d (freeze end, kills, bomb, round end)" % len(marks))


def frame_to_tick_resolution(sequence, frame):
    """Marked frames are stored in tick resolution, not display rate."""
    display = sequence.get_display_rate()
    resolution = sequence.get_tick_resolution()
    return int(round(frame * resolution.numerator * display.denominator / (resolution.denominator * display.numerator)))


def overview_camera(replay, tick):
    """A camera above the living players at `tick`, looking down at 55 degrees towards their middle."""
    index = replay["samples"]["ticks"].index(tick)
    points = []
    for track in replay["samples"]["players"].values():
        if track["alive"][index] and track["position"][index] is not None:
            points.append(to_unreal(track["position"][index]))
    if not points:
        return None
    lo = [min(p[k] for p in points) for k in range(3)]
    hi = [max(p[k] for p in points) for k in range(3)]
    centre = unreal.Vector(*[(a + b) / 2 for a, b in zip(lo, hi)])
    spread = max(hi[0] - lo[0], hi[1] - lo[1], 2000.0)
    location = centre + unreal.Vector(-spread * 0.45, 0.0, spread * 0.65)
    camera = spawn_tagged(unreal.CameraActor, "replay overview")
    camera.set_actor_location(location, False, False)
    camera.set_actor_rotation(unreal.MathLibrary.find_look_at_rotation(location, centre), False)
    camera.camera_component.set_editor_property("field_of_view", HFOV_16_9)
    return camera


def close_camera(replay, tick, controller):
    """A camera 1.3 m in front of a player and 0.5 m to his right at chest height (45 units above the feet),
    turned with his body yaw and looking at his chest: to inspect the pose and what he holds."""
    ticks = replay["samples"]["ticks"]
    index = min(range(len(ticks)), key=lambda i: abs(ticks[i] - tick))
    track = replay["samples"]["players"].get(str(controller))
    if track is None or track["position"][index] is None:
        return None
    x, y, z = track["position"][index]
    yaw = math.radians(track["body_yaw"][index] or 0.0)
    forward, right = (math.cos(yaw), math.sin(yaw)), (math.sin(yaw), -math.cos(yaw))
    chest = unreal.Vector(*to_unreal((x, y, z + 45)))
    eye = (x + 51 * forward[0] + 20 * right[0], y + 51 * forward[1] + 20 * right[1], z + 50)
    location = unreal.Vector(*to_unreal(eye))
    camera = spawn_tagged(unreal.CameraActor, "close %d at %d" % (controller, tick))
    camera.set_actor_location(location, False, False)
    camera.set_actor_rotation(unreal.MathLibrary.find_look_at_rotation(location, chest), False)
    camera.camera_component.set_editor_property("field_of_view", 60.0)
    return camera


def view_camera(tick, x, y, z, pitch, yaw):
    """A camera where CS2's `getpos` says its view is (Source units, degrees), with CS2's field of view."""
    camera = spawn_tagged(unreal.CameraActor, "view at %d" % tick)
    camera.set_actor_location(unreal.Vector(*to_unreal((x, y, z))), False, False)
    camera.set_actor_rotation(unreal.Rotator(roll=0.0, pitch=-pitch, yaw=-yaw), False)
    camera.camera_component.set_editor_property("field_of_view", HFOV_16_9)
    return camera


def verification_shots(sequence, replay, cameras, shot_actors=()):
    """Screenshots from the killer's eye 8 ticks before the first three kills, to compare with CS2 (demo_gototick,
    spec_player in first person), and one from above the players at the first kill."""
    tick0 = replay["start_tick"]
    names = {p["controller"]: p["name"] for p in replay["players"]}
    prefix = "%s_r%d" % (replay["demo"][-12:], replay["round"])
    shots = []
    for kill in replay["kills"][:3]:
        if kill["attacker"] not in cameras:
            continue
        tick = kill["tick"] - 8
        ascii_name = "".join(c for c in names[kill["attacker"]] if c.isascii() and c.isalnum())[:16] or "player"
        path = os.path.join(SHOT_DIR, "replay_%s_tick%d_%s.png" % (prefix, tick, ascii_name))
        shots.append((tick - tick0, cameras[kill["attacker"]], path))
    if replay["kills"]:
        tick = min(replay["samples"]["ticks"], key=lambda t: abs(t - replay["kills"][0]["tick"]))
        camera = overview_camera(replay, tick)
        if camera is not None:
            shots.append((tick - tick0, camera, os.path.join(SHOT_DIR, "replay_%s_overview.png" % prefix)))
    # Requested views through a player's eyes (to compare with CS2: demo_gototick, spec_player in first person).
    for tick, controller in POV_SHOTS:
        if controller in cameras:
            path = os.path.join(SHOT_DIR, "replay_%s_tick%d_pov%d.png" % (prefix, tick, controller))
            shots.append((tick - tick0, cameras[controller], path))
    for tick, controller in CLOSE_SHOTS:
        camera = close_camera(replay, tick, controller)
        if camera is not None:
            path = os.path.join(SHOT_DIR, "replay_%s_tick%d_close%d.png" % (prefix, tick, controller))
            shots.append((tick - tick0, camera, path))
    for number, view in enumerate(VIEW_SHOTS):
        path = os.path.join(SHOT_DIR, "replay_%s_tick%d_view%d.png" % (prefix, view[0], number))
        shots.append((view[0] - tick0, view_camera(*view), path))
    # Each shot camera half a second before the end of its keys (8 ticks before its kill), and at its first key.
    for number, (actor, camera) in enumerate(shot_actors):
        keys = camera["keys"]
        for tick in (int(keys[-1][0]) - 40, int(keys[0][0])):
            path = os.path.join(SHOT_DIR, "replay_%s_tick%d_shot%d_%s.png" % (prefix, tick, number, camera["kind"]))
            shots.append((tick - tick0, actor, path))
    if not shots:
        unreal.SystemLibrary.quit_editor()
        return
    unreal.LevelSequenceEditorBlueprintLibrary.open_level_sequence(sequence)
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 16")
    smoke_volumes.apply_cvars()
    for _, _, path in shots:
        if os.path.exists(path):
            os.remove(path)
    # The screenshot call can return no task, or a task already done, while a capture is still pending (seen
    # 2026-10-23: shots after the first came out missing), so each shot waits for its file to exist; one that never
    # comes is given up after SHOT_TIMEOUT_TICKS.
    state = {"ticks": 0, "index": 0, "requested_at": None}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] < WARM_UP_TICKS:
            return
        if state["requested_at"] is not None:
            path = shots[state["index"]][2]
            if os.path.exists(path):
                log("screenshot written: " + path)
            elif state["ticks"] - state["requested_at"] < SHOT_TIMEOUT_TICKS:
                return
            else:
                log("screenshot FAILED (none written): " + path)
            state["index"] += 1
            state["requested_at"] = None
        if state["index"] >= len(shots):
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.LevelSequenceEditorBlueprintLibrary.close_level_sequence()
            unreal.SystemLibrary.quit_editor()
            return
        frame, camera, path = shots[state["index"]]
        unreal.LevelSequenceEditorBlueprintLibrary.set_current_time(frame)
        unreal.LevelSequenceEditorBlueprintLibrary.force_update()
        unreal.AutomationLibrary.take_high_res_screenshot(WIDTH, HEIGHT, path, camera=camera, delay=SHOT_DELAY_S)
        state["requested_at"] = state["ticks"]

    handle = unreal.register_slate_post_tick_callback(tick)


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    with open(REPLAY_PATH, encoding="utf-8") as f:
        replay = json.load(f)
    if replay.get("format") != "ofast-replay" or replay.get("version") != 1:
        raise RuntimeError("%s is not an OFAST replay (version 1)" % REPLAY_PATH)
    clean_level(replay["map"])
    sequence, cameras, shot_actors = build(replay)
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world().get_path_name()
    if not world.startswith("/Game/"):
        raise RuntimeError("refusing to save " + world)
    levels.save_current_level()
    # The Studio's "Send to Unreal" takes this line as the sign the round was built.
    log("built %s" % sequence.get_path_name())
    if SHOT_DIR:
        verification_shots(sequence, replay, cameras, shot_actors)
    else:
        unreal.SystemLibrary.quit_editor()


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
