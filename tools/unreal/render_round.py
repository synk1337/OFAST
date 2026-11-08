"""Render a round's sequence with Movie Render Queue (M9, decision 023): a preset's picture, and the passes
compositing needs.

Run with tools/unreal/run_ue.ps1 (the editor renders in Play In Editor, so it needs a renderer: not -nullrhi), or the
Studio's "Render":

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/render_round.py \
        -Sequence /Game/OFAST/Replays/<demo>_r17 -Preset 1080p -Ticks 154760,154860 -Shot renders

Inputs (environment variables):
    OFAST_SEQUENCE   the round's sequence (import_demo.py made it and stored the demo's clock and map on it)
    OFAST_PRESET     720p | 1080p | 2k | 4k (PRESETS below, the picture's size); 1080p by default
    OFAST_TICKS      optional "start,end": demo ticks to render (end not included); the whole round without it
    OFAST_FPS        optional frames a second (FPS_RANGE), OUTPUT_FPS without it: 250 or more for slow motion made
                     in the editing software (Vegas, Premiere), which then has a real frame for every slowed one
    OFAST_MOTION_BLUR  "0" for sharp frames, without motion blur (velocity edits: the editor ramps the speed, then
                     blends frames or adds blur); motion blur without it
    OFAST_SHOT       the folder the render's folder goes in: <sequence>_<preset>[_<fps>fps][_sharp][_<start>-<end>]/
    OFAST_POWER      optional full | balanced | low (POWER below): how much of the graphics card the render takes;
                     full without it
    OFAST_PLAYERS_LAYER  "1" for the players layer: each frame rendered twice more, the players alone and the world
                     without them; without it the picture and its depth only
    OFAST_SCREEN_PERCENTAGE  optional: the share of the picture's size Unreal shades at (10 to 100), instead of the one
                     that fits the graphics card (fit_card)
    OFAST_OUTPUT     optional images | prores (OUTPUTS below): PNG (and EXR) frames, or one Apple ProRes 4444 video
                     (.mov) per pass; images without it

Every preset is also saved as a Movie Render Queue configuration, /Game/OFAST/Render/OFAST_<preset>, to pick in
Unreal's own Movie Render Queue window (Window > Cinematics > Movie Render Queue, then the job's settings).

What a render holds (docs/formats/render.md):
- the picture (PNG and 16-bit EXR), at 60 frames a second (or OFAST_FPS) whatever the sequence's
  64: everything in a round moves smoothly between the demo's ticks (keys interpolated, the effects and smokes
  functions of a fractional tick), so 250 frames a second are 250 different pictures; a folder per pass, and one EXR
  a frame with every pass as its layers;
- sharp frames (OFAST_MOTION_BLUR=0): the frame's samples spatial instead of temporal (the edges as smooth, every
  sample at the frame's own moment) and Unreal's own motion blur off (SHARP_CVARS), set on the job's copy only;
- at every size: temporal samples (8: motion blur as a camera's shutter makes it), 32 warm-up frames (Lumen and the
  smokes settle), depth (Movie Render Queue's own WorldDepth material: the EXR's R channel, in centimetres), and, when
  asked (OFAST_PLAYERS_LAYER), a players layer: the frame with everything but the players cut out (stencil layer
  "Players": each player's body and viewmodel), over the rest of the world as a second layer ("DefaultLayer"), each a
  render of its own;
- the picture's size kept, Unreal shading it smaller when the graphics card's memory would not hold it (fit_card):
  TSR scales it up from the frame's samples;
- as images, or as videos (OFAST_OUTPUT prores): one Apple ProRes 4444 .mov a pass, every camera cut in it, the
  players layer's with their transparency; no depth (an 8-bit picture of centimetres clips: the EXR holds it);
- the console variables the smokes need (smoke_volumes.SMOKE_CVARS);
- the round's cinematics and focus keys (the Cameras panel's, ofast_cameras.py), made again from their file and cut
  in, and its depth of field's blur limits;
- the first-person gun as the Viewmodel panel's settings say (ofast_viewmodel.py).

Each player's own first-person camera still hides their body (decision 021): Movie Render Queue renders through the
cut's camera as the view actor, which bOwnerNoSee checks (MoviePipelineImagePassBase.cpp sets ViewInitOptions.ViewActor
to the player controller's view target).

Its log lines ("OFAST: ..."): progress ("N % done"), then "rendered <folder>: ok", or "FAILED: <why>".
"""

import os
import re
import sys
import time

import unreal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ofast_cameras  # noqa: E402
import ofast_look  # noqa: E402
import ofast_viewmodel  # noqa: E402
import smoke_volumes  # noqa: E402
import tonemapper  # noqa: E402
from replay_cameras import TAG, log, progress  # noqa: E402

# For the Studio's progress bar: the run's share before Movie Render Queue renders (the level loading, the job set
# up, LEVEL_TICKS); its frames take the rest.
SETUP_SHARE = 0.15

# The picture's sizes (the user's ask, 2026-11-08: 720p, 1080p, 2K and 4K instead of preview, final and 4k), each
# rendered alike: SAMPLES a frame, PNG and EXR, depth. 2K is 2560 x 1440, as games and YouTube editors say it. Also
# listed in the Studio: apps/studio/src-tauri/src/unreal.rs (PRESETS) and apps/studio/src/names.ts.
PRESETS = {
    "720p": {"resolution": (1280, 720)},
    "1080p": {"resolution": (1920, 1080)},
    "2k": {"resolution": (2560, 1440)},
    "4k": {"resolution": (3840, 2160)},
}
SAMPLES = 8
# What a render writes (OFAST_OUTPUT; also listed in unreal.rs, OUTPUTS, and names.ts): image sequences, or Apple ProRes
# 4444 videos (Unreal's AppleProResMedia plugin, enabled in the project): visually lossless, 4:4:4 at 12 bits with
# an alpha channel, the master format editors import (VEGAS Pro, Premiere, After Effects, Resolve), about the PNGs'
# size. Unreal's other "uncompressed" video (Avid, .mxf) is 8-bit 4:2:2 without alpha: less than the PNGs hold.
OUTPUTS = ("images", "prores")
OUTPUT_FPS = 60
# The frame rates a render may ask for (OFAST_FPS); also checked by the Studio (unreal.rs, FPS_RANGE).
FPS_RANGE = (10, 1000)
WARM_UP_FRAMES = 32
FRAME_DIGITS = 5  # a long round runs past frame 9999 at 60 frames a second
FRAME_DIGITS_FAST = 6  # and past 99999 above 600 (a round with its 2 s after a death runs up to about 160 s)
# Sharp frames: Unreal's velocity motion blur off (with one temporal sample it would be the only blur left).
SHARP_CVARS = {"r.MotionBlurQuality": 0.0}
PLAYERS_LAYER = "Players"
WORLD_DEPTH = "/MovieRenderPipeline/Materials/MovieRenderQueue_WorldDepth.MovieRenderQueue_WorldDepth"
CONFIG_FOLDER = "/Game/OFAST/Render"
LEVEL_TICKS = 200  # editor ticks between loading the level and starting the render
# The share of the time the render works (OFAST_POWER, the Studio's "Power"). Movie Render Queue renders flat out,
# with no frame rate cap (its own time step, MoviePipeline.cpp), so below "full" the editor rests: each of its frames
# (a sample of an output frame) owes a rest as long as it took times (1 - share) / share, and once REST_S are owed the
# game thread sleeps them off. Holding back the next frame idles the graphics card too, long enough for it to clock
# down and for other programs to have it (rests after each sample, a few tens of milliseconds, fell in the gaps the
# card already had waiting for the samples to be read back: 4K at 50 %, 2026-11-06, 11 % slower and the same 126 W).
# The Studio also starts Unreal at a lower CPU priority then (unreal.rs, Power).
POWER = {"full": 1.0, "balanced": 0.75, "low": 0.5}
REST_S = 0.5
# The longest rest at once (s): a frame that stalled (shaders compiling) is not doubled.
MAX_REST_S = 5.0

# Graphics memory. A render that outgrows the card's own memory spills into the PC's: every frame then takes minutes
# instead of seconds and the whole PC crawls (a 4K render with the players layer on a 12 GB RTX 5070, 2026-11-05: 11 GB
# spilled, 64 of the PC's 65 GB committed, 1 to 4 minutes a frame). What render_round does about it (decision 027):
# - the editor's own copy of the world, which Play In Editor keeps beside the one rendered, drawn unlit (unlit_editor);
# - TSR's history at the picture's size (Unreal's cinematic quality keeps it at twice: 1 GB more a view at 4K, and the
#   frame's 8 samples already smooth it);
# - the shadow page pool at SHADOW_PAGES[0] for a 1080p picture, more for a bigger one, at most SHADOW_PAGES[1] (the
#   editor's 16384, DefaultEngine.ini: de_nuke's 174 lights). de_nuke round 17 at 1080p asked for 5659 pages (4096
#   overflowed by 1563, 2026-11-05);
# - the picture shaded smaller where the card would not hold it (fit_card), TSR scaling it up.
TSR_CVARS = {"r.TSR.History.ScreenPercentage": 100.0}
SHADOW_PAGES = (8192, 16384)
SHADOW_PAGES_MPIXELS = 2.07  # the pixels SHADOW_PAGES[0] holds (a 1080p picture), millions
# The card's memory a render takes (MB), fitted to de_nuke round 17 (the heaviest map) on a 12 GB RTX 5070 with the
# above (2026-11-06): 1080p 10.3 GB; 4K shaded at 50 % 10.7 GB, at 67 % 11.7, at 100 % 13.0 (1.2 GB of it spilled,
# 3.4 s a frame); with the players layer at 75 % (2026-11-08) 1.4 GB more a layer. FIT_BASE_MB whatever the size (the
# map, the players, the editor and Windows' desktop, about 1 GB), FIT_MB_PER_OUTPUT_MPIXEL a million pixels of the
# picture, FIT_MB_PER_MPIXEL a million pixels shaded, for each layer of the players layer FIT_MB_PER_LAYER and
# FIT_MB_PER_LAYER_MPIXEL a million pixels (its own view: Lumen's radiance cache and histories, mostly whatever the
# size; the frame's work memory is shared), and the shadow pages' FIT_MB_PER_SHADOW_PAGE (a page and its HZB).
FIT_BASE_MB = 8636
FIT_MB_PER_OUTPUT_MPIXEL = 67
FIT_MB_PER_MPIXEL = 173
FIT_MB_PER_LAYER = 1100
FIT_MB_PER_LAYER_MPIXEL = 250
FIT_MB_PER_SHADOW_PAGE = 0.143
# Left free on the card for the user's other programs, beyond the desktop FIT_BASE_MB holds.
FIT_RESERVE_MB = 500
# The shares of the picture's size Unreal may shade at (r.ScreenPercentage), the largest that fits first; never under
# about 1280 x 720 (FIT_MIN_MPIXELS), which a smaller card does not hold much better.
SCREEN_PERCENTAGES = (100, 85, 75, 67, 60, 50, 40, 33)
FIT_MIN_MPIXELS = 0.9
LAYERS = 2  # the players layer: the players alone and the world without them

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


def video_memory_mb():
    """The graphics card's own memory (MB), from Unreal's log when it picked the card ("Found D3D12 adapter N", "Adapter
    has M MB of dedicated video memory", "Chosen D3D12 Adapter Id = N"); None if the log does not say."""
    folder = unreal.Paths.convert_relative_path_to_full(unreal.Paths.project_log_dir())
    logs = [os.path.join(folder, f) for f in os.listdir(folder) if f.endswith(".log") and "-backup-" not in f]
    if not logs:
        return None
    adapters, adapter = {}, None
    with open(max(logs, key=os.path.getmtime), encoding="utf-8", errors="replace") as lines:
        for line in lines:
            found = re.search(r"Found D3D12 adapter (\d+):", line)
            if found:
                adapter = int(found.group(1))
                continue
            memory = re.search(r"Adapter has (\d+)MB of dedicated video memory", line)
            if memory and adapter is not None:
                adapters.setdefault(adapter, int(memory.group(1)))
                continue
            chosen = re.search(r"Chosen D3D12 Adapter Id = (\d+)", line)
            if chosen:
                return adapters.get(int(chosen.group(1)))
    return None


def shadow_pages(resolution, percent):
    """The shadow page pool for a picture of `resolution` shaded at `percent` (SHADOW_PAGES): as many pages as its
    pixels ask for, at least the first, at most the second."""
    mpixels = resolution[0] * resolution[1] / 1e6 * (percent / 100.0) ** 2
    low, high = SHADOW_PAGES
    return int(min(high, max(low, round(low * mpixels / SHADOW_PAGES_MPIXELS / 1024) * 1024)))


def need_mb(resolution, layer, percent):
    """The card's memory (MB) a render of a `resolution` picture shaded at `percent` takes, with the players layer if
    `layer` (FIT_*)."""
    mpixels = resolution[0] * resolution[1] / 1e6
    shaded = mpixels * (percent / 100.0) ** 2
    return (FIT_BASE_MB + FIT_MB_PER_OUTPUT_MPIXEL * mpixels + FIT_MB_PER_MPIXEL * shaded
            + ((FIT_MB_PER_LAYER + FIT_MB_PER_LAYER_MPIXEL * shaded) * LAYERS if layer else 0)
            + FIT_MB_PER_SHADOW_PAGE * shadow_pages(resolution, percent))


def fit_card(resolution, layer, card_mb):
    """The share of the picture's size (a percentage) Unreal shades at so that the render fits a card with `card_mb` of
    its own memory (need_mb), and whether it does: the largest of SCREEN_PERCENTAGES that fits, else the smallest that
    still shades FIT_MIN_MPIXELS (or the whole picture, if smaller); all of it when the card's memory is unknown."""
    mpixels = resolution[0] * resolution[1] / 1e6
    smallest = 100
    for percent in SCREEN_PERCENTAGES:
        if card_mb is None or need_mb(resolution, layer, percent) <= card_mb - FIT_RESERVE_MB:
            return percent, True
        if mpixels * (percent / 100.0) ** 2 >= FIT_MIN_MPIXELS:
            smallest = percent
    return smallest, False


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
    depth = unreal.MoviePipelinePostProcessPass()
    depth.set_editor_property("enabled", True)
    depth.set_editor_property("name", "WorldDepth")
    depth.set_editor_property("material", unreal.load_asset(WORLD_DEPTH))
    # 16-bit halves: 1 cm steps to 20 m, 4 cm at 80 m, enough for fog and focus; 32-bit floats doubled the EXR
    # (its four channels are written whole, though only R is depth).
    depth.set_editor_property("high_precision_output", False)
    deferred.set_editor_property("additional_post_process_materials", [depth])
    config.find_or_add_setting_by_class(unreal.MoviePipelineImageSequenceOutput_PNG)
    # PIZ, the default: lossless, read by every compositing app, a fifth smaller than ZIP on these frames.
    config.find_or_add_setting_by_class(unreal.MoviePipelineImageSequenceOutput_EXR)
    aa = config.find_or_add_setting_by_class(unreal.MoviePipelineAntiAliasingSetting)
    aa.set_editor_property("temporal_sample_count", SAMPLES)
    aa.set_editor_property("engine_warm_up_count", WARM_UP_FRAMES)
    aa.set_editor_property("render_warm_up_count", WARM_UP_FRAMES)
    cvars = config.find_or_add_setting_by_class(unreal.MoviePipelineConsoleVariableSetting)
    for command in smoke_volumes.SMOKE_CVARS:
        cvar, value = command.split()
        cvars.add_or_update_console_variable(cvar, float(value))
    for cvar, value in TSR_CVARS.items():
        cvars.add_or_update_console_variable(cvar, value)
    # Picked in Unreal's own Movie Render Queue window, the configuration switches CS2's look's tone mapper to a
    # linear picture for the render and back after it, as render_round does (tonemapper.movie_render).
    cvars.set_editor_property("start_console_commands", ["py import tonemapper; tonemapper.movie_render(True)"])
    cvars.set_editor_property("end_console_commands", ["py import tonemapper; tonemapper.movie_render(False)"])
    config.find_or_add_setting_by_class(unreal.MoviePipelineGameOverrideSetting)
    assets.save_loaded_asset(config)
    return config


def render_into(job, folder, frames, fps, blur, cvars, layer, output_kind):
    """The job's own copy of its configuration: the output folder, the frame rate, the range in the sequence's
    frames (or all: Movie Render Queue takes the range in the sequence's own frames, one a demo tick, whatever the
    output's rate), the console variables `cvars` (the round's depth of field's blur limits, the size Unreal shades
    at and the shadow pages that fit the card), without motion blur (`blur` False) the SAMPLES spatial
    (SHARP_CVARS), with `layer` the players layer, and for `output_kind` "prores" videos instead of images."""
    config = job.get_configuration()
    settings = config.find_or_add_setting_by_class(unreal.MoviePipelineConsoleVariableSetting)
    for cvar, value in dict(cvars, **({} if blur else SHARP_CVARS)).items():
        settings.add_or_update_console_variable(cvar, value)
    deferred = config.find_or_add_setting_by_class(unreal.MoviePipelineDeferredPassBase)
    if layer:
        deferred.set_editor_property("actor_layers", [unreal.ActorLayer(name=PLAYERS_LAYER)])
        deferred.set_editor_property("add_default_layer", True)
        deferred.set_editor_property("accumulator_includes_alpha", True)
    if output_kind == "prores":
        for kind in (unreal.MoviePipelineImageSequenceOutput_PNG, unreal.MoviePipelineImageSequenceOutput_EXR):
            image = config.find_setting_by_class(kind)
            if image is not None:
                config.remove_setting(image)
        # The depth pass would be a video of clipped centimetres.
        deferred.set_editor_property("additional_post_process_materials", [])
        prores = config.find_or_add_setting_by_class(unreal.MoviePipelineAppleProResOutput)
        # 4444, not 4444 XQ: Unreal 5.8 writes XQ frames with their alpha but labels the file 24-bit
        # (AppleProResEncoder.cpp, GetBitDepth: 32 for ProRes_4444 only), so editors drop the players layer's alpha.
        prores.set_editor_property("codec", unreal.AppleProResEncoderCodec.PRO_RES_4444)
        prores.set_editor_property("include_audio", False)
    output = config.find_or_add_setting_by_class(unreal.MoviePipelineOutputSetting)
    if output_kind == "prores":
        # One file a pass (Movie Render Queue drops a video's frame numbers), every camera cut in it, in the folder.
        output.set_editor_property("file_name_format", "{sequence_name}_{render_pass}")
    output.set_editor_property("output_directory", unreal.DirectoryPath(folder))
    output.set_editor_property("output_frame_rate", unreal.FrameRate(fps, 1))
    output.set_editor_property("zero_pad_frame_numbers", FRAME_DIGITS_FAST if fps > 600 else FRAME_DIGITS)
    if frames is not None:
        output.set_editor_property("use_custom_playback_range", True)
        output.set_editor_property("custom_start_frame", frames[0])
        output.set_editor_property("custom_end_frame", frames[1])
    if not blur:
        aa = config.find_or_add_setting_by_class(unreal.MoviePipelineAntiAliasingSetting)
        aa.set_editor_property("temporal_sample_count", 1)
        aa.set_editor_property("spatial_sample_count", SAMPLES)


def main():
    path = os.environ["OFAST_SEQUENCE"]
    name = os.environ.get("OFAST_PRESET") or "1080p"
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
    fps = int(os.environ.get("OFAST_FPS") or OUTPUT_FPS)
    if not FPS_RANGE[0] <= fps <= FPS_RANGE[1]:
        raise RuntimeError("%d frames a second: from %d to %d" % (fps, *FPS_RANGE))
    blur = os.environ.get("OFAST_MOTION_BLUR") != "0"
    power = os.environ.get("OFAST_POWER") or "full"
    if power not in POWER:
        raise RuntimeError("power %r: one of %s" % (power, ", ".join(POWER)))
    share = POWER[power]
    preset = PRESETS[name]
    layer = os.environ.get("OFAST_PLAYERS_LAYER") == "1"
    output_kind = os.environ.get("OFAST_OUTPUT") or "images"
    if output_kind not in OUTPUTS:
        raise RuntimeError("output %r: one of %s" % (output_kind, ", ".join(OUTPUTS)))
    # The size Unreal shades at, to fit the card (fit_card), and the shadow pages that size asks for: set before the
    # level loads too, so that the editor's own copy of the world gets no bigger pool if it draws lit before
    # unlit_editor.
    card_mb = video_memory_mb()
    percent, fits = fit_card(preset["resolution"], layer, card_mb)
    if os.environ.get("OFAST_SCREEN_PERCENTAGE"):
        percent = int(os.environ["OFAST_SCREEN_PERCENTAGE"])
        if not 10 <= percent <= 100:
            raise RuntimeError("screen percentage %d: from 10 to 100" % percent)
        fits = card_mb is None or need_mb(preset["resolution"], layer, percent) <= card_mb - FIT_RESERVE_MB
    pages = shadow_pages(preset["resolution"], percent)
    unreal.SystemLibrary.execute_console_command(None, "r.Shadow.Virtual.MaxPhysicalPages %d" % pages)
    fitted = {"r.ScreenPercentage": float(percent), "r.Shadow.Virtual.MaxPhysicalPages": float(pages)}
    # A folder per rate, blur and range: frames are numbered from the sequence's start, so two ranges (or two rates) in
    # one folder would interleave, and a sharp render would overwrite a blurred one.
    folder = ("%s_%s" % (path.rsplit("/", 1)[-1], name) + ("_%dfps" % fps if fps != OUTPUT_FPS else "")
              + ("" if blur else "_sharp") + ("_prores" if output_kind == "prores" else "")
              + ("_%d-%d" % tuple(ticks) if frames else ""))
    folder = os.path.join(os.environ["OFAST_SHOT"], folder)
    progress(0.0, SETUP_SHARE)
    levels.load_level(level)
    unlit_editor()
    # The Look panel's settings (ofast_look.py), even if the level was not saved after they were changed; the Cameras
    # panel's cinematics, from their file, likewise (ofast_cameras.py).
    ofast_look.apply_level_look(sequence=sequence)
    # CS2's look's tone mapper hands Movie Render Queue a linear picture, which it encodes for the files (else the
    # frames came out washed out: tonemapper.py, OUTPUT). Not saved: the editor quits after the render.
    tonemapper.movie_render(True)
    ofast_cameras.restore(sequence)
    # The Viewmodel panel's settings (ofast_viewmodel.py), whichever round they were last changed on.
    ofast_viewmodel.key(sequence)
    players = put_players_in_layer()
    config = make_config(name, preset)
    log("render %s, preset %s, %d frames a second, %s, frames %s, %s, %s, power %s, into %s"
        % (path, name, fps, "motion blur" if blur else "sharp", frames or "all",
           "%d player actors in layer %r" % (players, PLAYERS_LAYER) if layer else "no players layer",
           "ProRes 4444" if output_kind == "prores" else "images", power, folder))
    shaded = (round(preset["resolution"][0] * percent / 100.0), round(preset["resolution"][1] * percent / 100.0))
    log("graphics card: %s MB, the render needs about %d MB: shaded at %d %% (%dx%d), %d shadow pages"
        % (card_mb if card_mb is not None else "unknown", need_mb(preset["resolution"], layer, percent), percent,
           shaded[0], shaded[1], pages))
    # The Studio keeps "warning:" lines on screen (unreal.rs, render).
    if not fits:
        log("warning: This render needs more graphics memory than the card's %d MB, even shaded at %dx%d: it will "
            "spill into the PC's memory and be slow%s." % (card_mb, shaded[0], shaded[1],
                                                           " (the players layer takes most of it)" if layer else ""))
    elif percent < 100 and card_mb is not None:
        log("warning: Shaded at %dx%d and scaled up to %dx%d by Unreal's TSR, to fit the graphics card's %d MB."
            % (shaded[0], shaded[1], preset["resolution"][0], preset["resolution"][1], card_mb))
    queue_subsystem = unreal.get_editor_subsystem(unreal.MoviePipelineQueueSubsystem)
    # "last": when the previous frame's callback ran (before it rested), "rested": how long it rested, "owed": the rest
    # owed so far.
    state = {"ticks": 0, "done": False, "percent": 0, "bar": 0, "last": None, "rested": 0.0, "owed": 0.0}

    def rest():
        """Below full power, the rest the frame just done owes, slept off once REST_S are owed (POWER): the frame's own
        time is the time since the last callback, less what that callback slept."""
        now = time.perf_counter()
        slept = 0.0
        if state["last"] is not None:
            state["owed"] += max(0.0, now - state["last"] - state["rested"]) * (1.0 - share) / share
            if state["owed"] >= REST_S:
                slept = min(MAX_REST_S, state["owed"])
                state["owed"] = 0.0
                time.sleep(slept)
        state["last"] = now
        state["rested"] = slept

    def tick(_delta):
        state["ticks"] += 1
        if "executor" in state:
            # The finished event is the result; this is the fallback when it never comes (the editor would idle on).
            if not queue_subsystem.is_rendering():
                finished(None, None)
                return
            done = state["job"].get_status_progress()
            percent = int(done * 100) // 10 * 10
            if percent > state["percent"]:
                state["percent"] = percent
                log("%d %% done" % percent)
            # The bar, by the percent: from there to the next.
            if int(done * 100) > state["bar"]:
                state["bar"] = int(done * 100)
                step = (1.0 - SETUP_SHARE) / 100
                progress(SETUP_SHARE + step * state["bar"], SETUP_SHARE + step * min(state["bar"] + 1, 100))
            if share < 1.0:
                rest()
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
        render_into(job, folder, frames, fps, blur,
                    dict(ofast_cameras.blur_limits(ofast_cameras.load_record(sequence)), **fitted), layer, output_kind)
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
        quit_editor()

    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    handle = unreal.register_slate_post_tick_callback(tick)


def object_path(package):
    """/Game/A/B -> /Game/A/B.B, the asset's full object path."""
    return "%s.%s" % (package, package.rsplit("/", 1)[-1])


# The editor's active viewport's view mode while it rendered (unlit_editor), to give back before it quits.
EDITOR_VIEW_MODE = []


def unlit_editor():
    """Draws the editor's active viewport unlit for the render. Play In Editor keeps the editor's own copy of the
    world, and its viewport kept drawing it lit during the render: a second shadow page pool, Lumen scene and distance
    fields (de_nuke round 17 at 1080p, 2026-11-06: 6.3 GB of resources instead of 8.5, a 96 s render in 58 s). Unlit,
    that copy builds none. Only the active viewport: the others of the editor's layout are orthographic, unlit
    already (their own view mode, which setting every viewport would overwrite), and one call can give it back as it
    was (the editor saves its viewports' modes when it quits)."""
    EDITOR_VIEW_MODE.append(unreal.AutomationLibrary.get_editor_active_viewport_view_mode())
    unreal.AutomationLibrary.set_editor_active_viewport_view_mode(unreal.ViewModeIndex.VMI_UNLIT)


def quit_editor():
    """Gives the active viewport its view mode back (unlit_editor), then quits the editor."""
    if EDITOR_VIEW_MODE and EDITOR_VIEW_MODE[0] != unreal.ViewModeIndex.VMI_UNKNOWN:
        unreal.AutomationLibrary.set_editor_active_viewport_view_mode(EDITOR_VIEW_MODE[0])
    unreal.SystemLibrary.quit_editor()


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    quit_editor()
