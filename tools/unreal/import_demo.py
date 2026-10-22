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
  "T <name>"; hidden while dead and while the camera looks through the player's own eyes;
- per player: a first-person camera at the eye (feet + the networked eye height), aimed along pitch and yaw, with
  CS2's default field of view (90 degrees at 4:3);
- a camera cut track that follows the killer of each kill in turn (from the previous kill until half a second after
  this one), the way fragmovie editors cut a round;
- marked frames: freeze time end, every kill ("attacker > victim (weapon, HS)") and bomb events.

Conversions are the map import's: Source (x, y, z) inches -> Unreal (x, -y, z) x 2.54 cm, angles (pitch, yaw) ->
Rotator(pitch=-pitch, yaw=-yaw). Keys that a straight line through their neighbours already gives are dropped
(standing still, running straight), which keeps the sequence light; the tolerance is far below what shows.
"""

import json
import math
import os
import re
import time

import unreal

REPLAY_PATH = os.environ["OFAST_REPLAY"]
SHOT_DIR = os.environ.get("OFAST_SHOT", "")
POV_SHOTS = [tuple(int(v) for v in item.split(":")) for item in os.environ.get("OFAST_POV_SHOTS", "").split(",")
             if ":" in item]
CLOSE_SHOTS = [tuple(int(v) for v in item.split(":"))
               for item in os.environ.get("OFAST_CLOSE_SHOTS", "").split(",") if ":" in item]
VIEW_SHOTS = [(int(item.split(":")[0]),) + tuple(float(v) for v in item.split(":")[1:6])
              for item in os.environ.get("OFAST_VIEW_SHOTS", "").split(",") if item.count(":") == 5]
WIDTH, HEIGHT = 1920, 1080
TAG = unreal.Name("OFAST_replay")
UNITS_TO_CM = 2.54
# Player proxy: CS2's player hull is 32 x 32 x 72 units; the engine cylinder is 100 cm with its pivot in the middle.
HULL_SCALE = (32 * UNITS_TO_CM / 100, 32 * UNITS_TO_CM / 100, 72 * UNITS_TO_CM / 100)
HULL_HALF_HEIGHT_CM = 36 * UNITS_TO_CM
TEAM_COLOURS = {2: (1.0, 0.45, 0.05), 3: (0.1, 0.35, 1.0)}  # T orange, CT blue
TEAM_NAMES = {2: "T", 3: "CT"}
# CS2's default field of view: 90 degrees horizontally at 4:3, i.e. 106.26 degrees at 16:9 (as import_map.py).
HFOV_16_9 = math.degrees(2 * math.atan(math.tan(math.radians(45)) * (16 / 9) / (4 / 3)))
# Key reduction tolerances: 0.2 cm and 0.1 degree.
POSITION_TOLERANCE_CM = 0.2
ANGLE_TOLERANCE_DEG = 0.1
# Camera cuts: how long after a kill the killer's view stays on.
CUT_AFTER_KILL_S = 0.5
WARM_UP_TICKS = 300
SHOT_DELAY_S = 6.0  # as import_map.SHOT_DELAY_S: lets Lumen settle after each cut

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary


def log(message):
    unreal.log("OFAST: " + message)


def to_unreal(position):
    x, y, z = position
    return (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM)


def unwrap(angles):
    """Makes a degree sequence continuous (359 -> 361 instead of 359 -> 1), so keys never spin the long way."""
    out = []
    for a in angles:
        if out:
            a = out[-1] + ((a - out[-1] + 180.0) % 360.0 - 180.0)
        out.append(a)
    return out


def reduce_keys(frames, values, tolerance):
    """Drops keys that linear interpolation between the kept neighbours reproduces within `tolerance`. Keeps the
    first and last key of every run (values are lists of floats of equal length)."""
    if len(frames) <= 2:
        return list(zip(frames, values))
    kept = [0]
    for i in range(1, len(frames) - 1):
        a, b = kept[-1], i + 1
        t = (frames[i] - frames[a]) / (frames[b] - frames[a])
        if any(abs(values[a][k] + (values[b][k] - values[a][k]) * t - values[i][k]) > tolerance
               for k in range(len(values[i]))):
            kept.append(i)
    kept.append(len(frames) - 1)
    return [(frames[i], values[i]) for i in kept]


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


def spawn_tagged(cls_or_mesh, label, location=(0.0, 0.0, 0.0)):
    if isinstance(cls_or_mesh, unreal.StaticMesh):
        actor = actors.spawn_actor_from_object(cls_or_mesh, unreal.Vector(*location))
    else:
        actor = actors.spawn_actor_from_class(cls_or_mesh, unreal.Vector(*location))
    actor.tags = [TAG]
    actor.set_actor_label(label)
    return actor


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


def apply_specular_factors(gltf_path, objects):
    """Sets the glTF master's SpecularFactor on the imported material instances whose glTF material carries
    KHR_materials_specular (CS2's weapon materials: 0.5, a 2 % reflectance for non-metals). Interchange creates the
    parameter but leaves it at 1."""
    with open(gltf_path, encoding="utf-8") as f:
        materials = json.load(f).get("materials", [])
    factors = {m["name"]: m["extensions"]["KHR_materials_specular"].get("specularFactor", 1.0)
               for m in materials if "KHR_materials_specular" in m.get("extensions", {})}
    count = 0
    for o in objects:
        if isinstance(o, unreal.MaterialInstanceConstant) and o.get_name() in factors:
            unreal.MaterialEditingLibrary.set_material_instance_scalar_parameter_value(
                o, "SpecularFactor", factors[o.get_name()])
            count += 1
    if count:
        log("  specular factor set on %d of %d weapon materials" % (count, len(factors)))


def import_agents(replay):
    """Imports the players' animated agents written next to the replay by `ofast export-demo --players`, if any: the
    files <demo>_r<round>.agents.json lists, one per agent look. Returns controller -> (skeletal mesh,
    AnimSequence); Interchange names each AnimSequence <file><animation>, so the controller is read from its
    `player_<controller>` ending (shortened in the middle when long, so only `_<controller>` is sure to remain) —
    except in a file with a single animation, which it names <file>_Anim."""
    folder = os.path.dirname(REPLAY_PATH)
    base = "%s_r%d" % (replay["demo"], replay["round"])
    manifest_path = os.path.join(folder, base + ".agents.json")
    if not os.path.exists(manifest_path):
        return {}
    with open(manifest_path, encoding="utf-8") as f:
        manifest = json.load(f)
    manager = unreal.InterchangeManager.get_interchange_manager_scripted()
    pipelines = character_pipelines(manifest.get("fps", 30))
    agents = {}
    for entry in manifest["agents"]:
        path = os.path.join(folder, entry["file"])
        look = entry["file"][len(base) + 1:-len(".gltf")]  # <agent>_<mesh groups>[_<gloves>]
        params = unreal.ImportAssetParameters()
        params.is_automated = True
        params.replace_existing = True
        params.override_pipelines = pipelines
        # A short folder: asset file paths must stay under Windows' 260 characters.
        destination = "/Game/OFAST/Replays/Agents/%s_r%d_%s" % (replay["demo"][-12:], replay["round"], look)
        # Fresh every run: Interchange keeps existing material instances on a re-import (decision 006), so a
        # changed material conversion would not show. The level's old replay actors are already gone.
        if assets.does_directory_exist(destination):
            assets.delete_directory(destination)
        objects = manager.import_asset(destination, unreal.InterchangeManager.create_source_data(path), params)
        apply_specular_factors(path, objects)
        # Interchange leaves imported assets in memory only: saved, the level's actors keep them when reopened.
        assets.save_directory(destination, only_if_is_dirty=True, recursive=True)
        meshes = [o for o in objects if isinstance(o, unreal.SkeletalMesh)]
        animations = {}
        sequences = [o for o in objects if isinstance(o, unreal.AnimSequence)]
        for o in sequences:
            # Interchange shortens long names in the middle ("aurora-_layer_10"): only the trailing number is safe.
            match = re.search(r"_(\d+)$", o.get_name())
            if match:
                animations[int(match.group(1))] = o
        # A file with one animation gets it named <file>_Anim instead (a look worn by one player only).
        if not animations and len(sequences) == 1 and len(entry["players"]) == 1:
            animations[entry["players"][0]] = sequences[0]
        if not meshes:
            log("agents %s: no skeletal mesh in %s" % (look, path))
            continue
        rate = ""
        if animations:
            sample = next(iter(animations.values()))
            rate = ", %d frames over %.3f s" % (unreal.AnimationLibrary.get_num_frames(sample),
                                                sample.get_play_length())
        log("agents %s: %s with %d player animations%s, %d weapons"
            % (look, meshes[0].get_name(), len(animations), rate, len(entry.get("weapons", []))))
        for controller in entry["players"]:
            if controller in animations:
                agents[controller] = (meshes[0], animations[controller])
    return agents


def spawn_agent(player, mesh):
    """The player as the team's agent model (feet at the actor's origin, facing +X)."""
    actor = actors.spawn_actor_from_object(mesh, unreal.Vector(0.0, 0.0, 0.0))
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


def channels_by_name(section):
    return {str(c.channel_name): c for c in section.get_all_channels()}


def key_transform(binding, frames, locations, rotations, scale, end_frame):
    """A transform track: location (cm) and rotation (pitch, yaw in Unreal degrees) keyed linearly, after dropping
    keys a straight line already gives. `scale` is the constant scale (the track overrides the actor's)."""
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
        axes = ("X", "Y", "Z") if name == "Location" else ("Y", "Z")  # rotation: pitch (Y), yaw (Z)
        for run in runs(frames, values):
            for frame, value in reduce_keys([f for f, _ in run], [v for _, v in run], tolerance):
                for axis, v in zip(axes, value):
                    channels["%s.%s" % (name, axis)].add_key(unreal.FrameNumber(frame), v, 0.0,
                                                             unreal.MovieSceneTimeUnit.DISPLAY_RATE, linear)
                    count += 1
    return count


def key_visibility(binding, changes, end_frame):
    """A visibility track from (frame, visible) pairs: one key per change."""
    track = binding.add_track(unreal.MovieSceneVisibilityTrack)
    track.set_property_name_and_path("bHiddenInGame", "bHiddenInGame")
    section = track.add_section()
    section.set_range(0, end_frame)
    channel = section.get_all_channels()[0]
    previous = None
    for frame, value in changes:
        if value != previous:
            channel.add_key(unreal.FrameNumber(frame), value)
            previous = value


def visibility_changes(frames, alive, own_view):
    """(frame, visible) at every sample and every camera-cut boundary: shown while alive with a pawn, except while
    the camera looks through this player's own eyes (in first person CS2 does not draw your own body either; here
    the visor and name would block the view). `own_view` lists the (start, end) frame ranges of those cuts."""
    boundaries = sorted({f for r in own_view for f in r} | set(frames))
    changes = []
    sample = 0
    for frame in boundaries:
        while sample + 1 < len(frames) and frames[sample + 1] <= frame:
            sample += 1
        watching = any(start <= frame < end for start, end in own_view)
        changes.append((frame, alive[sample] and not watching))
    return changes


def carve(ranges, frames):
    """`ranges` of (start, end) frames without the single frames in `frames`."""
    out = []
    for start, end in ranges:
        cuts = sorted(f for f in frames if start <= f < end)
        for f in cuts:
            if f > start:
                out.append((start, f))
            start = f + 1
        if start < end:
            out.append((start, end))
    return out


def camera_cut_plan(replay, controllers, end_frame, fps):
    """(start, end, controller) per cut: for each kill in turn, the killer's view from the previous cut until half a
    second after the kill (the last one runs to the end of the round)."""
    tick0 = replay["start_tick"]
    kills = [k for k in replay["kills"] if k["attacker"] in controllers]
    plan = []
    start = 0
    for i, kill in enumerate(kills):
        last = i == len(kills) - 1
        end = end_frame if last else min(end_frame, kill["tick"] - tick0 + round(CUT_AFTER_KILL_S * fps))
        if end > start:
            plan.append((start, end, kill["attacker"]))
            start = end
    return plan


def build(replay):
    tick0 = replay["start_tick"]
    ticks = replay["samples"]["ticks"]
    frames = [t - tick0 for t in ticks]
    end_frame = replay["end_tick"] - tick0 + 1
    fps = round(1.0 / replay["tick_interval"])
    name = "%s_r%d" % (replay["demo"], replay["round"])
    path = "/Game/OFAST/Replays/" + name
    if assets.does_asset_exist(path):
        assets.delete_asset(path)
    sequence = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, "/Game/OFAST/Replays", unreal.LevelSequence, unreal.LevelSequenceFactoryNew())
    sequence.set_display_rate(unreal.FrameRate(fps, 1))
    sequence.set_playback_start(0)
    sequence.set_playback_end(end_frame)

    cameras = {}
    started = time.monotonic()
    keys = 0
    cuts = camera_cut_plan(replay, {p["controller"] for p in replay["players"]}, end_frame, fps)
    agents = import_agents(replay)
    for player in replay["players"]:
        track = replay["samples"]["players"][str(player["controller"])]
        positions = [None if p is None else to_unreal(p) for p in track["position"]]
        known = [a for a in track["angles"] if a is not None]
        yaws = iter(unwrap([-a[1] for a in known]))
        pitches = iter([-a[0] for a in known])
        rotations = [None if a is None else (next(pitches), next(yaws)) for a in track["angles"]]
        agent = agents.get(player["controller"])
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
        alive = [bool(a) and p is not None for a, p in zip(track["alive"], positions)]
        own_view = [(start, end) for start, end, who in cuts if who == player["controller"]]
        # Frames other cameras screenshot (close-ups, other players' eyes) show this player even inside his own cut.
        shown = [t - tick0 for t, _ in CLOSE_SHOTS] + [view[0] - tick0 for view in VIEW_SHOTS] + \
            [t - tick0 for t, who in POV_SHOTS if who != player["controller"]]
        own_view = carve(own_view, shown)
        # Frames shown through this player's eyes by the POV screenshots: hide the body there too.
        own_view += [(t - tick0, t - tick0 + 1) for t, who in POV_SHOTS if who == player["controller"]]
        changes = visibility_changes(frames, alive, own_view)
        for actor in [body] + attached:
            key_visibility(sequence.add_possessable(actor) if actor is not body else binding, changes, end_frame)
        camera = spawn_pov_camera(player)
        eyes = [None if p is None or h is None else (p[0], p[1], p[2] + h * UNITS_TO_CM)
                for p, h in zip(positions, track["eye_height"])]
        camera_binding = sequence.add_possessable(camera)
        keys += key_transform(camera_binding, frames, eyes, rotations, (1.0, 1.0, 1.0), end_frame)
        cameras[player["controller"]] = (camera, camera_binding)
    log("sequence %s: %d players (%d animated agents), %d frames at %d fps, %d keys in %.0f s"
        % (path, len(replay["players"]), len(agents), end_frame, fps, keys,
           time.monotonic() - started))
    add_camera_cuts(sequence, cuts, cameras)
    add_markers(sequence, replay)
    assets.save_loaded_asset(sequence)
    return sequence, {controller: camera for controller, (camera, _) in cameras.items()}


def add_camera_cuts(sequence, cuts, cameras):
    """The camera cut track from camera_cut_plan. `cameras`: controller -> (camera actor, its binding)."""
    if not cuts:
        return
    track = sequence.add_track(unreal.MovieSceneCameraCutTrack)
    for start, end, controller in cuts:
        section = track.add_section()
        section.set_range(start, end)
        section.set_camera_binding_id(sequence.get_binding_id(cameras[controller][1]))
    log("camera cuts: %d (killer's view, kill by kill)" % len(cuts))


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


def verification_shots(sequence, replay, cameras):
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
    if not shots:
        unreal.SystemLibrary.quit_editor()
        return
    unreal.LevelSequenceEditorBlueprintLibrary.open_level_sequence(sequence)
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 16")
    state = {"ticks": 0, "index": 0, "task": None}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] < WARM_UP_TICKS:
            return
        task = state["task"]
        if task is not None and not task.is_task_done():
            return
        if task is not None:
            log("screenshot written: " + shots[state["index"]][2])
            state["index"] += 1
        if state["index"] >= len(shots):
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.LevelSequenceEditorBlueprintLibrary.close_level_sequence()
            unreal.SystemLibrary.quit_editor()
            return
        frame, camera, path = shots[state["index"]]
        unreal.LevelSequenceEditorBlueprintLibrary.set_current_time(frame)
        unreal.LevelSequenceEditorBlueprintLibrary.force_update()
        state["task"] = unreal.AutomationLibrary.take_high_res_screenshot(WIDTH, HEIGHT, path, camera=camera,
                                                                           delay=SHOT_DELAY_S)

    handle = unreal.register_slate_post_tick_callback(tick)


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    with open(REPLAY_PATH, encoding="utf-8") as f:
        replay = json.load(f)
    if replay.get("format") != "ofast-replay" or replay.get("version") != 1:
        raise RuntimeError("%s is not an OFAST replay (version 1)" % REPLAY_PATH)
    clean_level(replay["map"])
    sequence, cameras = build(replay)
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world().get_path_name()
    if not world.startswith("/Game/"):
        raise RuntimeError("refusing to save " + world)
    levels.save_current_level()
    if SHOT_DIR:
        verification_shots(sequence, replay, cameras)
    else:
        unreal.SystemLibrary.quit_editor()


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
