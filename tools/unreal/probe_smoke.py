"""Probe: the smoke volumes' look, without re-importing the round. Run after import_demo.py (the level keeps its smoke
volumes and their material instances):

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/probe_smoke.py -Shot test-data/shots/probe_smoke \
        -Replay test-data/exports/demos_m6/<demo>_r5.ofast-replay.json -Name "34848:10,36839:10"

It rebuilds the smoke material from smoke_volumes.py (in place: the instances keep it), sets every smoke instance's
parameters from the replay again, rebuilds the sprite materials and layer instances of grenade_effects.py (fires,
blasts, flashes: their look, not the sprites' per-instance data, which only an import places) and shot_effects.py
(the shots' layers and bullet holes; the muzzle lights are moved and lit as the sequence keys them), then for each
"tick:controller" of OFAST_NAME sets MPC_Replay.ReplayTick to the tick, the effects' lights to what the sequence keys
there, and screenshots through that player's eyes (feet + eye height, CS2's 90 degrees at 4:3), the way the sequence's POV
cameras see it. A view can also be "tick:x:y:z:pitch:yaw" (Source units, degrees, as CS2's getpos prints it), or
"tick:controller:seq": through that player's POV camera in the round's sequence, played to that tick, with what the
sequence shows there (the viewmodel and its first-person muzzle flashes, where the import keyed that player's view:
a tick of its -PovShots). With such a view, the sequence plays the other views to their ticks too.

`-Only diagnose:<smoke entity>` takes, at the first view, one shot per test material on that smoke's volume instead:
constant (a constant extinction: does the Heterogeneous Volume draw?), grid (extinction where the smoke material's
grid formula puts the point inside the grid: are Origin and Size right?), ball (an opaque 3 m ball on the grid's
centre: does it draw opaque?), anywhere (the smoke at extinction 50, threshold 0: where is any density?).
"""

import json
import math
import os
import sys

import unreal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import grenade_effects  # noqa: E402
import shot_effects  # noqa: E402
import smoke_volumes  # noqa: E402

SHOT_DIR = os.environ["OFAST_SHOT"]
REPLAY_PATH = os.environ["OFAST_REPLAY"]
VIEWS = [item.split(":") for item in os.environ["OFAST_NAME"].split(",") if item]
ONLY = os.environ.get("OFAST_ONLY", "")
UNITS_TO_CM = 2.54
WARM_UP_TICKS = 200
# CS2's default field of view, 90 degrees at 4:3, horizontally at 16:9.
HFOV_16_9 = math.degrees(2 * math.atan(math.tan(math.radians(45)) * (16 / 9) / (4 / 3)))
GRID_HLSL = r"""
float3 g = float3(WorldPos.x - Origin.x, Origin.y - WorldPos.y, WorldPos.z - Origin.z) / Size;
return all(g >= 0.0) && all(g <= 1.0) ? 1.0 : 0.0;
"""
BALL_HLSL = r"""
float3 g = float3(WorldPos.x - Origin.x, Origin.y - WorldPos.y, WorldPos.z - Origin.z) / Size;
return length((g - 0.5) * Size) < 300.0 ? 100.0 : 0.0;
"""

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary
mel = unreal.MaterialEditingLibrary


def log(message):
    unreal.log("OFAST: " + message)


def test_material(name, hlsl):
    """A transient Volume material: extinction 0.01 x (1, or the Custom node `hlsl` over WorldPos, Origin, Size)."""
    path = "/Game/OFAST/Probe/" + name
    if assets.does_asset_exist(path):
        assets.delete_asset(path)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, "/Game/OFAST/Probe", unreal.Material, unreal.MaterialFactoryNew())
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_VOLUME)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_ADDITIVE)
    material.set_editor_property("used_with_heterogeneous_volumes", True)
    extinction = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -400, 200)
    extinction.set_editor_property("r", 0.01)
    source = extinction
    if hlsl:
        custom = mel.create_material_expression(material, unreal.MaterialExpressionCustom, -400, 0)
        custom.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT1)
        custom.set_editor_property("code", hlsl)
        inputs = []
        for input_name in ("WorldPos", "Origin", "Size"):
            custom_input = unreal.CustomInput()
            custom_input.set_editor_property("input_name", input_name)
            inputs.append(custom_input)
        custom.set_editor_property("inputs", inputs)
        mel.connect_material_expressions(
            mel.create_material_expression(material, unreal.MaterialExpressionWorldPosition, -800, -200), "",
            custom, "WorldPos")
        origin = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -800, -100)
        origin.set_editor_property("parameter_name", "Origin")
        mel.connect_material_expressions(origin, "", custom, "Origin")
        size = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -800, 0)
        size.set_editor_property("parameter_name", "Size")
        mel.connect_material_expressions(size, "", custom, "Size")
        product = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -150, 100)
        mel.connect_material_expressions(custom, "", product, "A")
        mel.connect_material_expressions(extinction, "", product, "B")
        source = product
    mel.connect_material_property(source, "", unreal.MaterialProperty.MP_SUBSURFACE_COLOR)
    white = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -400, -200)
    white.set_editor_property("constant", unreal.LinearColor(0.8, 0.8, 0.8, 1.0))
    mel.connect_material_property(white, "", unreal.MaterialProperty.MP_BASE_COLOR)
    mel.recompile_material(material)
    return material


def view_camera(replay, view):
    """A camera for a view of VIEWS: (tick, camera actor, name)."""
    if len(view) == 3 and view[2] == "seq":
        tick, controller = int(view[0]), int(view[1])
        name = next(p["name"] for p in replay["players"] if p["controller"] == controller)
        camera = next((a for a in actors.get_all_level_actors()
                       if isinstance(a, unreal.CameraActor) and a.get_actor_label() == "POV %s" % name), None)
        if camera is None:
            raise RuntimeError("no POV camera for %s in the level (run import_demo.py)" % name)
        return tick, camera, "tick%d_seq%d" % (tick, controller)
    if len(view) == 2:
        tick, controller = int(view[0]), view[1]
        index = replay["samples"]["ticks"].index(tick)
        track = replay["samples"]["players"][controller]
        if track["position"][index] is None:
            raise RuntimeError("player %s has no pawn at tick %d" % (controller, tick))
        x, y, z = track["position"][index]
        z += track["eye_height"][index]
        pitch, yaw = track["angles"][index]
        name = "tick%d_pov%s" % (tick, controller)
    else:
        tick = int(view[0])
        x, y, z, pitch, yaw = (float(v) for v in view[1:6])
        name = "tick%d_view" % tick
    camera = actors.spawn_actor_from_class(unreal.CameraActor, unreal.Vector(
        x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
    camera.set_actor_rotation(unreal.Rotator(roll=0.0, pitch=-pitch, yaw=-yaw), False)
    camera.camera_component.set_editor_property("field_of_view", HFOV_16_9)
    return tick, camera, name


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    with open(REPLAY_PATH, encoding="utf-8") as f:
        replay = json.load(f)
    levels.load_level("/Game/OFAST/Maps/" + replay["map"])
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world()
    collection = assets.load_asset(smoke_volumes.REPLAY_COLLECTION)
    material = smoke_volumes.smoke_material()
    by_label = {a.get_actor_label(): a for a in actors.get_all_level_actors()
                if isinstance(a, unreal.HeterogeneousVolume)}
    volumes = {}
    for grenade in smoke_volumes.smokes_of(replay):
        actor = by_label.get(smoke_volumes.smoke_label(grenade))
        if actor is None:
            log("smoke #%d: no volume in the level (run import_demo.py)" % grenade["entity"])
            continue
        component = actor.get_editor_property("heterogeneous_volume_component")
        instance = component.get_material(0)
        if not isinstance(instance, unreal.MaterialInstanceConstant):
            instance = assets.load_asset("%s/%s" % (smoke_volumes.smoke_folder(replay),
                                                    smoke_volumes.instance_name(grenade)))
        smoke_volumes.update_instance(instance, grenade)
        # The atlas again too: a re-export may have changed it.
        texture = smoke_volumes.import_smoke_texture(replay, REPLAY_PATH, grenade["smoke"]["volume"]["texture"])
        mel.set_material_instance_texture_parameter_value(instance, "Atlas", texture)
        mel.update_material_instance(instance)
        component.set_material(0, instance)
        volumes[grenade["entity"]] = (component, instance)
    log("smoke material rebuilt; %d smoke instances updated" % len(volumes))
    # The fires' material and layers too, from grenade_effects.py and the agents manifest's flipbooks.
    manifest_path = os.path.join(os.path.dirname(REPLAY_PATH), "%s_r%d.agents.json" % (replay["demo"],
                                                                                      replay["round"]))
    agents = {}
    if os.path.exists(manifest_path):
        with open(manifest_path, encoding="utf-8") as f:
            agents = json.load(f)
        effects = agents.get("effects", [])
        layers = grenade_effects.update_layers(replay, REPLAY_PATH, effects)
        # The shots' layers and bullet holes too (shot_effects.py; their look, not where they are).
        layers.update(grenade_effects.update_layers(replay, REPLAY_PATH, effects, shot_effects.shot_layers(), False))
        shot_effects.hole_instances(REPLAY_PATH, agents.get("decals", {}))
        log("fire material rebuilt; %d layers updated" % len(layers))
    muzzle_keys = shot_effects.light_frames(agents.get("shots", []), 0, 10 ** 9)
    names = {p["controller"]: p["name"] for p in replay["players"]}

    lights = {a.get_actor_label(): a for a in actors.get_all_level_actors() if isinstance(a, unreal.PointLight)}
    cameras = [view_camera(replay, view) for view in VIEWS]
    # Views through the sequence's cameras: the round's sequence open, played to each view's tick.
    in_sequence = any(len(view) == 3 and view[2] == "seq" for view in VIEWS)
    if in_sequence:
        sequence = assets.load_asset("/Game/OFAST/Replays/%s_r%d" % (replay["demo"], replay["round"]))
        unreal.LevelSequenceEditorBlueprintLibrary.open_level_sequence(sequence)
    # (file name, tick, camera, material for the diagnosed volume or None)
    shots = [("probe_smoke_%s.png" % name, tick, camera, None) for tick, camera, name in cameras]
    if ONLY.startswith("diagnose:"):
        entity = int(ONLY.split(":")[1])
        component, own = volumes[entity]
        tick, camera, name = cameras[0]

        def with_grid(test):
            instance = unreal.MaterialLibrary.create_dynamic_material_instance(world, test)
            instance.set_vector_parameter_value("Origin", mel.get_material_instance_vector_parameter_value(own, "Origin"))
            instance.set_scalar_parameter_value("Size", mel.get_material_instance_scalar_parameter_value(own, "Size"))
            return instance

        anywhere = unreal.MaterialLibrary.create_dynamic_material_instance(world, own)
        anywhere.set_scalar_parameter_value("Threshold", 0.0)
        anywhere.set_scalar_parameter_value("Extinction", 50.0)
        tests = [("constant", test_material("M_ProbeConstant", None)),
                 ("grid", with_grid(test_material("M_ProbeGrid", GRID_HLSL))),
                 ("ball", with_grid(test_material("M_ProbeBall", BALL_HLSL))), ("anywhere", anywhere)]
        shots = [("probe_smoke%d_%s_%s.png" % (entity, name, test), tick, camera, (component, m)) for test, m in tests]
    paths = [os.path.join(SHOT_DIR, shot[0]) for shot in shots]
    for path in paths:
        if os.path.exists(path):
            os.remove(path)
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 16")
    smoke_volumes.apply_cvars()
    # The screenshot call can return no task while a capture is pending (seen 2026-10-23), so each shot waits for its
    # file instead: set the tick (and test material), let it settle, request the shot, wait until the PNG exists.
    state = {"ticks": 0, "index": -1, "requested_at": None, "set_at": 0}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] < WARM_UP_TICKS:
            return
        index = state["index"]
        if index >= 0:
            if state["requested_at"] is None:
                if state["ticks"] - state["set_at"] < 60:
                    return
                unreal.AutomationLibrary.take_high_res_screenshot(1920, 1080, paths[index], camera=shots[index][2],
                                                                  delay=4.0)
                state["requested_at"] = state["ticks"]
                return
            if not os.path.exists(paths[index]):
                return
            log("screenshot written: " + paths[index])
        state["index"] = index = index + 1
        if index >= len(shots):
            unreal.unregister_slate_post_tick_callback(handle)
            if in_sequence:
                unreal.LevelSequenceEditorBlueprintLibrary.close_level_sequence()
            unreal.SystemLibrary.quit_editor()
            return
        _, at, _, test = shots[index]
        # With the sequence open, every view is played to its tick (the players stand where they are then).
        if in_sequence:
            unreal.LevelSequenceEditorBlueprintLibrary.set_current_time(at - replay["start_tick"])
            unreal.LevelSequenceEditorBlueprintLibrary.force_update()
        unreal.MaterialLibrary.set_scalar_parameter_value(world, collection, "ReplayTick", float(at))
        # The fires', blasts' and flashes' lights as the sequence keys them at that tick (without the fires' noise).
        for fire in replay.get("fires", []):
            light = lights.get(grenade_effects.light_label(fire))
            if light is not None:
                component = light.get_editor_property("light_component")
                component.set_light_color(unreal.LinearColor(*grenade_effects.LIGHT_COLOR, 1.0))
                burning = fire["start_tick"] <= at <= fire["end_tick"]
                component.set_editor_property("intensity", grenade_effects.light_intensity(
                    fire, at, grenade_effects.tick_rate_of(replay)) if burning else 0.0)
        for grenade in grenade_effects.bursts_of(replay):
            light = lights.get(grenade_effects.burst_light_label(grenade))
            if light is not None:
                pulse = grenade_effects.BURSTS[grenade["kind"]][1]
                light.get_editor_property("light_component").set_editor_property(
                    "intensity", grenade_effects.pulse_intensity(pulse, at - grenade["detonation"]["tick"]))
        # The muzzle lights where and as bright as the sequence keys them at that tick (its keys are in ticks here).
        for controller, keys in muzzle_keys.items():
            light = lights.get("muzzle light %s" % names.get(controller, controller))
            if light is None:
                continue
            earlier = [k for k in sorted(keys, key=lambda k: k[0]) if k[0] <= at]
            place = next((p for _, p, _ in reversed(earlier) if p), None)
            if place is not None:
                light.set_actor_location(unreal.Vector(place[0] * UNITS_TO_CM, -place[1] * UNITS_TO_CM,
                                                       place[2] * UNITS_TO_CM), False, False)
            light.get_editor_property("light_component").set_editor_property(
                "intensity", earlier[-1][2] if earlier and earlier[-1][0] == at else 0.0)
        if test is not None:
            test[0].set_material(0, test[1])
        state["set_at"] = state["ticks"]
        state["requested_at"] = None

    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
