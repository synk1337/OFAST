"""Render a round's sequence with Movie Render Queue (M9, decision 023): a preset's picture, and the passes
compositing needs.

Run with tools/unreal/run_ue.ps1 (the editor renders in Play In Editor, so it needs a renderer: not -nullrhi), or the
Studio's "Render":

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/render_round.py \
        -Sequence /Game/OFAST/Replays/<demo>_r17 -Preset final -Ticks 154760,154860 -Shot renders

Inputs (environment variables):
    OFAST_SEQUENCE   the round's sequence (import_demo.py made it and stored the demo's clock and map on it)
    OFAST_PRESET     preview | final | 4k (PRESETS below); final by default
    OFAST_TICKS      optional "start,end": demo ticks to render (end not included); the whole round without it
    OFAST_SHOT       the folder the render's folder goes in: <sequence>_<preset>[_<start>-<end>]/

Every preset is also saved as a Movie Render Queue configuration, /Game/OFAST/Render/OFAST_<preset>, to pick in
Unreal's own Movie Render Queue window (Window > Cinematics > Movie Render Queue, then the job's settings).

What a render holds (docs/formats/render.md):
- the picture (PNG, and 16-bit EXR for final and 4k), at 60 frames a second whatever the sequence's 64; a folder
  per pass, and one EXR a frame with every pass as its layers;
- final and 4k: temporal samples (8: motion blur as a camera's shutter makes it), 32 warm-up frames (Lumen and the
  smokes settle), depth (Movie Render Queue's own WorldDepth material: the EXR's R channel, in centimetres), and a
  players layer: the frame with everything but the players cut out (stencil layer "Players": each player's body and
  viewmodel), over the rest of the world as a second layer ("DefaultLayer");
- the console variables the smokes need (smoke_volumes.SMOKE_CVARS).

Each player's own first-person camera still hides their body (decision 021): Movie Render Queue renders through the
cut's camera as the view actor, which bOwnerNoSee checks (MoviePipelineImagePassBase.cpp sets ViewInitOptions.ViewActor
to the player controller's view target).

Its log lines ("OFAST: ..."): progress ("N % done"), then "rendered <folder>: ok", or "FAILED: <why>".
"""

import os
import sys
import time

import unreal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import smoke_volumes  # noqa: E402
from replay_cameras import TAG, log  # noqa: E402

# Also listed in the Studio: apps/studio/src-tauri/src/unreal.rs (PRESETS) and apps/studio/src/names.ts.
PRESETS = {
    "preview": {"resolution": (1280, 720), "temporal": 1, "exr": False, "passes": False},
    "final": {"resolution": (1920, 1080), "temporal": 8, "exr": True, "passes": True},
    "4k": {"resolution": (3840, 2160), "temporal": 8, "exr": True, "passes": True},
}
OUTPUT_FPS = 60
WARM_UP_FRAMES = 32
FRAME_DIGITS = 5  # a long round runs past frame 9999 at 60 frames a second
PLAYERS_LAYER = "Players"
WORLD_DEPTH = "/MovieRenderPipeline/Materials/MovieRenderQueue_WorldDepth.MovieRenderQueue_WorldDepth"
CONFIG_FOLDER = "/Game/OFAST/Render"
LEVEL_TICKS = 200  # editor ticks between loading the level and starting the render

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary


def is_player(actor):
    """A player's body (an agent or a proxy and its visor) or viewmodel, as import_demo.py labels them."""
    label = actor.get_actor_label()
    return TAG in actor.tags and (label.startswith(("T ", "CT ", "VM ")) or label.endswith(" visor"))


def put_players_in_layer():
    layers = unreal.get_editor_subsystem(unreal.LayersSubsystem)
    players = [a for a in actors.get_all_level_actors() if is_player(a)]
    for actor in players:
        layers.add_actor_to_layer(actor, PLAYERS_LAYER)
    return len(players)


def make_config(name, preset):
    """The preset's Movie Render Queue configuration, saved as /Game/OFAST/Render/OFAST_<name>: no folder and no range
    of its own (a job sets them on its copy, render_into), so it suits any round in Unreal's own window."""
    path = "%s/OFAST_%s" % (CONFIG_FOLDER, name)
    if assets.does_asset_exist(path):
        assets.delete_asset(path)
    config = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        "OFAST_" + name, CONFIG_FOLDER, unreal.MoviePipelinePrimaryConfig, None)
    output = config.find_or_add_setting_by_class(unreal.MoviePipelineOutputSetting)
    # A folder per pass (what an editor imports as one image sequence); the EXR holds every pass as its layers, so its
    # pass name is empty and it lands in the folder itself.
    output.set_editor_property("file_name_format", "{render_pass}/{sequence_name}.{frame_number}")
    output.set_editor_property("zero_pad_frame_numbers", FRAME_DIGITS)
    output.set_editor_property("output_resolution", unreal.IntPoint(*preset["resolution"]))
    output.set_editor_property("use_custom_frame_rate", True)
    output.set_editor_property("output_frame_rate", unreal.FrameRate(OUTPUT_FPS, 1))
    deferred = config.find_or_add_setting_by_class(unreal.MoviePipelineDeferredPassBase)
    if preset["passes"]:
        depth = unreal.MoviePipelinePostProcessPass()
        depth.set_editor_property("enabled", True)
        depth.set_editor_property("name", "WorldDepth")
        depth.set_editor_property("material", unreal.load_asset(WORLD_DEPTH))
        # 16-bit halves: 1 cm steps to 20 m, 4 cm at 80 m, enough for fog and focus; 32-bit floats doubled the EXR
        # (its four channels are written whole, though only R is depth).
        depth.set_editor_property("high_precision_output", False)
        deferred.set_editor_property("additional_post_process_materials", [depth])
        deferred.set_editor_property("actor_layers", [unreal.ActorLayer(name=PLAYERS_LAYER)])
        deferred.set_editor_property("add_default_layer", True)
        deferred.set_editor_property("accumulator_includes_alpha", True)
    config.find_or_add_setting_by_class(unreal.MoviePipelineImageSequenceOutput_PNG)
    if preset["exr"]:
        # PIZ, the default: lossless, read by every compositing app, a fifth smaller than ZIP on these frames.
        config.find_or_add_setting_by_class(unreal.MoviePipelineImageSequenceOutput_EXR)
    aa = config.find_or_add_setting_by_class(unreal.MoviePipelineAntiAliasingSetting)
    aa.set_editor_property("temporal_sample_count", preset["temporal"])
    aa.set_editor_property("engine_warm_up_count", WARM_UP_FRAMES)
    aa.set_editor_property("render_warm_up_count", WARM_UP_FRAMES)
    cvars = config.find_or_add_setting_by_class(unreal.MoviePipelineConsoleVariableSetting)
    for command in smoke_volumes.SMOKE_CVARS:
        cvar, value = command.split()
        cvars.add_or_update_console_variable(cvar, float(value))
    config.find_or_add_setting_by_class(unreal.MoviePipelineGameOverrideSetting)
    assets.save_loaded_asset(config)
    return config


def render_into(job, folder, frames):
    """The job's own copy of its configuration: the output folder, and the range in the sequence's frames (or all)."""
    output = job.get_configuration().find_or_add_setting_by_class(unreal.MoviePipelineOutputSetting)
    output.set_editor_property("output_directory", unreal.DirectoryPath(folder))
    if frames is not None:
        output.set_editor_property("use_custom_playback_range", True)
        output.set_editor_property("custom_start_frame", frames[0])
        output.set_editor_property("custom_end_frame", frames[1])


def main():
    path = os.environ["OFAST_SEQUENCE"]
    name = os.environ.get("OFAST_PRESET") or "final"
    if name not in PRESETS:
        raise RuntimeError("preset %r: one of %s" % (name, ", ".join(PRESETS)))
    sequence = assets.load_asset(path)
    if sequence is None:
        raise RuntimeError("no sequence %s" % path)
    tick0 = assets.get_metadata_tag(sequence, "OFAST_start_tick")
    # The level it was built in (a round built before 2026-10-31 has only its map: the standard look's level).
    level = (assets.get_metadata_tag(sequence, "OFAST_level")
             or "/Game/OFAST/Maps/%s" % assets.get_metadata_tag(sequence, "OFAST_map"))
    if not tick0:
        raise RuntimeError("%s has no demo clock: send the round to Unreal again" % path)
    ticks = [int(t) for t in os.environ.get("OFAST_TICKS", "").split(",") if t.strip()]
    frames = (ticks[0] - int(tick0), ticks[1] - int(tick0)) if len(ticks) == 2 else None
    # A folder per range: frames are numbered from the sequence's start, so two ranges in one folder would interleave.
    folder = "%s_%s" % (path.rsplit("/", 1)[-1], name) + ("_%d-%d" % tuple(ticks) if frames else "")
    folder = os.path.join(os.environ["OFAST_SHOT"], folder)
    levels.load_level(level)
    players = put_players_in_layer()
    config = make_config(name, PRESETS[name])
    log("render %s, preset %s, frames %s, %d player actors in layer %r, into %s"
        % (path, name, frames or "all", players, PLAYERS_LAYER, folder))
    queue_subsystem = unreal.get_editor_subsystem(unreal.MoviePipelineQueueSubsystem)
    state = {"ticks": 0, "done": False, "percent": 0}

    def tick(_delta):
        state["ticks"] += 1
        if "executor" in state:
            # The finished event is the result; this is the fallback when it never comes (the editor would idle on).
            if not queue_subsystem.is_rendering():
                finished(None, None)
                return
            percent = int(state["job"].get_status_progress() * 100) // 10 * 10
            if percent > state["percent"]:
                state["percent"] = percent
                log("%d %% done" % percent)
            return
        if state["ticks"] < LEVEL_TICKS:
            return
        queue = queue_subsystem.get_queue()
        queue.delete_all_jobs()
        job = queue.allocate_new_job(unreal.MoviePipelineExecutorJob)
        # Full object paths (package.asset): the file names and the EXR metadata take the asset's name from them.
        job.set_editor_property("sequence", unreal.SoftObjectPath(object_path(path)))
        job.set_editor_property("map", unreal.SoftObjectPath(object_path(level)))
        job.set_editor_property("job_name", "OFAST %s" % name)
        job.set_configuration(config)
        render_into(job, folder, frames)
        executor = unreal.MoviePipelinePIEExecutor()
        executor.on_executor_finished_delegate.add_callable(finished)
        # Kept for the whole render: a Python callable bound to a delegate goes when its owner does.
        state["executor"] = executor
        state["job"] = job
        state["started"] = time.time()
        queue_subsystem.render_queue_with_executor_instance(executor)
        log("Movie Render Queue started")

    def finished(_executor, success):
        if state["done"]:
            return
        state["done"] = True
        unreal.unregister_slate_post_tick_callback(handle)
        if success is None:  # no finished event: judged by frames written by this render (the folder can be reused)
            success = any(os.path.getmtime(os.path.join(top, f)) >= state["started"]
                          for top, _, files in os.walk(folder) for f in files)
        # The Studio and run_ue.ps1 read these lines: the folder, or why not.
        if success:
            log("rendered %s: ok" % folder)
        else:
            log("FAILED: Movie Render Queue stopped before the end (see Unreal's log)")
        unreal.SystemLibrary.quit_editor()

    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    handle = unreal.register_slate_post_tick_callback(tick)


def object_path(package):
    """/Game/A/B -> /Game/A/B.B, the asset's full object path."""
    return "%s.%s" % (package, package.rsplit("/", 1)[-1])


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
