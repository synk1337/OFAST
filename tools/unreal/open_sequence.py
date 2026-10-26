"""Opens a round in the Unreal editor: its map's level, then its Level Sequence in Sequencer.

The Studio's "Open in Unreal" starts the editor (UnrealEditor.exe, with its window) with this script as a console
command, not through -ExecutePythonScript: that one quits the editor when the script ends, unless the script is kept
alive, and then its "Executing Python script" notification stays, whose Cancel quits the editor without asking to
save:

    UnrealEditor.exe OFASTSandbox.uproject -ExecCmds="py tools/unreal/open_sequence.py"

Inputs (environment variables):
    OFAST_LEVEL     the level to open, /Game/OFAST/Maps/<map>
    OFAST_SEQUENCE  the sequence to open, /Game/OFAST/Replays/<demo>_r<round> (import_demo.py makes it)
"""

import os

import unreal

LEVEL = os.environ.get("OFAST_LEVEL", "")
SEQUENCE = os.environ.get("OFAST_SEQUENCE", "")

# The script runs while the editor is still starting: the level opens a few frames later, the sequence once the
# level's assets are ready (opening it at once crashed the renderer: a mesh drawn before its vertex data was).
LEVEL_TICKS = 30
SEQUENCE_TICKS = 300


def log(message):
    unreal.log("OFAST: " + message)


def open_level():
    if LEVEL:
        unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).load_level(LEVEL)
    log("opened %s" % LEVEL)


def open_sequence():
    if not SEQUENCE:
        return
    sequence = unreal.load_asset(SEQUENCE)
    if sequence is None:
        log("FAILED: %s does not exist (send the round to Unreal again)" % SEQUENCE)
        return
    unreal.LevelSequenceEditorBlueprintLibrary.open_level_sequence(sequence)
    # The viewport shows the sequence's camera cuts (the players' views) rather than the editor's own camera.
    unreal.LevelSequenceEditorBlueprintLibrary.set_lock_camera_cut_to_viewport(True)
    log("opened %s" % SEQUENCE)


state = {"ticks": 0}


def tick(_delta):
    state["ticks"] += 1
    try:
        if state["ticks"] == LEVEL_TICKS:
            open_level()
        elif state["ticks"] == SEQUENCE_TICKS:
            unreal.unregister_slate_post_tick_callback(handle)
            open_sequence()
    except Exception as error:
        unreal.unregister_slate_post_tick_callback(handle)
        log("FAILED: %s" % error)


handle = unreal.register_slate_post_tick_callback(tick)
