"""OFAST's Viewmodel panel: how the first-person gun is drawn, for every round of the project (the user's ask,
2026-11-01: a default viewmodel, changed in Unreal with its place). Docs: docs/user-guide/README.md, "The gun in first
person"; docs/formats/viewmodel.md.

- The settings, one for the project (<project>/Saved/OFAST/Viewmodel.json): whose viewmodel ("Same for everyone",
  the default, or "Each player's own", as they played), the field of view and the offsets for everyone (CS2's
  viewmodel_fov and viewmodel_offset_x / _y / _z, in CS2's numbers; CS2's own default to start with), the hand (as
  played, right, left), the gun's motion with the player (0 still, 1 CS2's: export::sway).
- Each round's own data (<project>/Saved/OFAST/Viewmodels/<demo>_r<round>.json, written by import_demo.py): its
  sample frames, and per player the viewmodel settings they played with, the frames they switched hands, the recoil
  on their viewmodel (export::punch) and the gun's motion (export::sway).
- key(sequence): each "VM <name>" viewmodel's transform on its camera keyed again from both (the offsets, the hand
  mirroring it, the recoil plus the motion), and each "POV <name>" camera's first-person field of view; by
  import_demo.py, before a render (render_round.py), when the panel opens a round keyed with other settings, and at
  each change in the panel.

The panel is an object of OfastViewmodel, a class made here in Python, opened in an editor tab of its own as the Look
and Cameras panels are; a Slate tick callback watches its fields and keys the open round once they are left alone.
"""

import json
import math
import os
import time

import unreal

from replay_cameras import UNITS_TO_CM, channels_by_name, log

SETTINGS_FORMAT = "ofast-viewmodel"
ROUND_FORMAT = "ofast-viewmodels"
VERSION = 1
SAME = "Same for everyone"
OWN = "Each player's own"
AS_PLAYED = "As played"
RIGHT = "Right"
LEFT = "Left"
# CS2's own default viewmodel (viewmodel_presetpos 1, "Desktop"): what a new install shows.
CS2_DEFAULT = {"fov": 68.0, "offset": [2.5, 0.0, -1.5]}
DEFAULTS = {"whose": SAME, "fov": CS2_DEFAULT["fov"], "offset": list(CS2_DEFAULT["offset"]), "hand": AS_PLAYED,
            "motion": 1.0}
# CS2's limits for viewmodel_fov and the offsets.
FOV_RANGE = (54.0, 68.0)
OFFSET_RANGE = (-2.5, 2.5)
OFFSET_Z_RANGE = (-2.0, 2.0)
# Where the round's settings were last keyed (to key it again when they change): the sequence's metadata tag.
KEYED_TAG = "OFAST_viewmodel"

assets = unreal.EditorAssetLibrary
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
sequencer = unreal.LevelSequenceEditorBlueprintLibrary


def viewmodel_fov_16_9(fov):
    """CS2's viewmodel_fov is horizontal at 4:3 like its world FOV: the horizontal FOV of a 16:9 picture."""
    return math.degrees(2 * math.atan(math.tan(math.radians(fov / 2)) * (16 / 9) / (4 / 3)))


# The settings and each round's data.


def saved(*parts):
    return os.path.join(unreal.Paths.project_saved_dir(), "OFAST", *parts)


def load_settings():
    """The project's viewmodel settings (DEFAULTS without a file, or for what the file lacks)."""
    settings = json.loads(json.dumps(DEFAULTS))
    path = saved("Viewmodel.json")
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            stored = json.load(f)
        if stored.get("format") == SETTINGS_FORMAT and stored.get("version") == VERSION:
            settings.update({k: v for k, v in stored.items() if k in DEFAULTS})
    return settings


def save_settings(settings):
    path = saved("Viewmodel.json")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(dict(settings, format=SETTINGS_FORMAT, version=VERSION), f, indent=1)


def round_file(sequence):
    return saved("Viewmodels", sequence.get_name() + ".json")


def save_round(sequence, frames, end_frame, players):
    """The round's data (import_demo.py): `frames` its sample frames, `players` [{"name", "fov", "offset", "hands":
    [[frame, left]] (each change), "punch": [[frame, pitch, yaw]], "sway": [[frame, pitch, yaw]]}] (Source's
    degrees: positive pitch down, positive yaw left)."""
    path = round_file(sequence)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump({"format": ROUND_FORMAT, "version": VERSION, "frames": frames, "end_frame": end_frame,
                   "players": players}, f)


def load_round(sequence):
    path = round_file(sequence)
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    return data if data.get("format") == ROUND_FORMAT and data.get("version") == VERSION else None


# Keying.


def key_viewmodel(binding, frames, end_frame, hands, offset, turns):
    """The viewmodel's transform on its camera: moved by `offset` (CS2's viewmodel_offset_x / _y / _z: inches right,
    forward and up; Unreal's camera looks along +X with +Y to the right), mirrored from every frame `hands` says it is
    in the left hand (None: unchanged) — CS2 mirrors the whole viewmodel, the sideways offset with it. Step keys: the
    hand changes from one tick to the next. Turned past the camera by `turns` (per frame: (pitch, yaw) Source's
    degrees, or None), linear keys, the yaw mirrored with the viewmodel."""
    for track in binding.find_tracks_by_type(unreal.MovieScene3DTransformTrack):
        binding.remove_track(track)
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
    # Source's pitch and yaw negated, as the camera's are; keyed while turned, with a key of nothing on the frame
    # before and the frame after each stretch (a channel holds its first key's value before it, and a linear segment
    # between two stretches would drift).
    linear = unreal.MovieSceneKeyInterpolation.LINEAR
    turned = False
    left = False
    before = None
    count = 0
    for frame, hand, turn in zip(frames, hands, turns):
        left = left if hand is None else hand
        now = turn is not None and (abs(turn[0]) > 0.0 or abs(turn[1]) > 0.0)
        keyed = []
        if now and not turned and before is not None:
            keyed.append((before, 0.0, 0.0))
        if now or turned:
            keyed.append((frame, -turn[0], -turn[1]) if now else (frame, 0.0, 0.0))
        for at, pitch, yaw in keyed:
            for name, value in (("Rotation.Y", pitch), ("Rotation.Z", -yaw if left else yaw)):
                channels[name].add_key(unreal.FrameNumber(at), value, 0.0, unreal.MovieSceneTimeUnit.DISPLAY_RATE,
                                       linear)
                count += 1
        turned = now
        before = frame
    return count


def hands_by_frame(frames, changes, hand):
    """Each frame's hand (True: left) from the player's changes, or the one `hand` forces."""
    if hand in (RIGHT, LEFT):
        return [hand == LEFT] * len(frames)
    out, at, left = [], 0, None
    for frame in frames:
        while at < len(changes) and changes[at][0] <= frame:
            left = changes[at][1]
            at += 1
        out.append(left)
    return out


def turns_by_frame(frames, punch, sway, motion):
    """Each frame's turn off the camera (pitch, yaw): the recoil's plus the motion's × `motion`; None where neither
    has a row (nothing)."""
    recoil = {row[0]: row[1:] for row in punch}
    moved = {row[0]: row[1:] for row in sway} if motion > 0.0 else {}
    out = []
    for frame in frames:
        a, b = recoil.get(frame), moved.get(frame)
        if a is None and b is None:
            out.append(None)
            continue
        a, b = a or (0.0, 0.0), b or (0.0, 0.0)
        out.append((a[0] + motion * b[0], a[1] + motion * b[1]))
    return out


def bindings_by_name(sequence):
    out = {}
    for binding in sequence.get_bindings():
        out.setdefault(str(binding.get_display_name()), binding)
    return out


def key(sequence, settings=None):
    """Every viewmodel of the round keyed with `settings` (the project's by default) and its cameras' field of view.
    Returns the number of viewmodels, or None for a round without its data (sent before 2026-11-01)."""
    data = load_round(sequence)
    if data is None:
        return None
    settings = settings or load_settings()
    bindings = bindings_by_name(sequence)
    cameras = {a.get_actor_label(): a for a in actors.get_all_level_actors() if isinstance(a, unreal.CameraActor)}
    frames, end_frame = data["frames"], data["end_frame"]
    count = 0
    for player in data["players"]:
        binding = bindings.get("VM %s" % player["name"])
        if binding is None:
            continue
        same = settings["whose"] != OWN
        fov = settings["fov"] if same else player["fov"]
        offset = settings["offset"] if same else player["offset"]
        key_viewmodel(binding, frames, end_frame, hands_by_frame(frames, player["hands"], settings["hand"]), offset,
                      turns_by_frame(frames, player["punch"], player["sway"], settings["motion"]))
        camera = cameras.get("POV %s" % player["name"])
        if camera is not None:
            camera.camera_component.set_editor_property("first_person_field_of_view", viewmodel_fov_16_9(fov))
        count += 1
    assets.set_metadata_tag(sequence, KEYED_TAG, json.dumps(settings, sort_keys=True))
    return count


def keyed_with(sequence):
    """The settings the round was last keyed with (as text), or ""."""
    return assets.get_metadata_tag(sequence, KEYED_TAG) or ""


# The panel.


def tip(text):
    return {"ToolTip": text}


def slider(label, low, high, text, clamp):
    return unreal.uproperty(float, meta={"Category": "Viewmodel", "DisplayName": label, "UIMin": str(low),
                                         "UIMax": str(high), "ClampMin": str(clamp[0]), "ClampMax": str(clamp[1]),
                                         **tip(text)})


# The Details panel lists a Python class's fields last first, so they are written here bottom up.
@unreal.uclass()
class OfastViewmodel(unreal.PrimaryDataAsset):
    motion = slider("Gun motion", 0.0, 2.0, "How much the gun moves with the player: bob while running, lowered "
                    "while running or falling, lag behind turns. 1 is CS2's; 0 a still gun.", (0.0, 3.0))
    hand = unreal.uproperty(str, meta={
        "Category": "Viewmodel", "DisplayName": "Hand", "GetOptions": "hand_options",
        **tip("Which hand holds the gun: as each player played (some play left-handed), or the same for everyone.")})
    offset_z = slider("Offset Z (up)", OFFSET_Z_RANGE[0], OFFSET_Z_RANGE[1], "viewmodel_offset_z: the gun higher "
                      "(+) or lower (−). CS2's default -1.5.", (-10.0, 10.0))
    offset_y = slider("Offset Y (forward)", OFFSET_RANGE[0], OFFSET_RANGE[1], "viewmodel_offset_y: the gun further "
                      "(+) or closer (−). CS2's default 0.", (-10.0, 10.0))
    offset_x = slider("Offset X (right)", OFFSET_RANGE[0], OFFSET_RANGE[1], "viewmodel_offset_x: the gun more to the "
                      "side (+) or toward the middle (−). CS2's default 2.5.", (-10.0, 10.0))
    fov = slider("Field of view", FOV_RANGE[0], FOV_RANGE[1], "viewmodel_fov: smaller makes the gun bigger. CS2's "
                 "default 68 (CS2 allows 54 to 68).", (30.0, 120.0))
    whose = unreal.uproperty(str, meta={
        "Category": "Viewmodel", "DisplayName": "Viewmodel", "GetOptions": "whose_options",
        **tip("Same for everyone: the field of view and offsets below, for every player. Each player's own: the "
              "settings each player had in the demo (the offsets and field of view below are then not used).")})

    @unreal.ufunction(ret=unreal.Array(str))
    def whose_options(self):
        return [SAME, OWN]

    @unreal.ufunction(ret=unreal.Array(str))
    def hand_options(self):
        return [AS_PLAYED, RIGHT, LEFT]

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Viewmodel", "DisplayName": "CS2's default",
                            **tip("Back to CS2's own default viewmodel: field of view 68, offsets 2.5, 0, -1.5, "
                                  "the hand as played, CS2's gun motion.")})
    def reset(self):
        load_fields(DEFAULTS)


FIELDS = {"whose": "whose", "fov": "fov", "offset_x": 0, "offset_y": 1, "offset_z": 2, "hand": "hand",
          "motion": "motion"}
# Keyed once the fields are left alone this long (a slider dragged keys the round once).
SETTLE = 0.4
_state = {"handle": None, "panel": None, "sequence": None, "last": None, "pending": None}


def panel():
    if _state["panel"] is None:
        _state["panel"] = unreal.new_object(OfastViewmodel, name="Viewmodel")
        load_fields(load_settings())
    return _state["panel"]


def load_fields(settings):
    p = _state["panel"]
    for name, field in FIELDS.items():
        value = settings["offset"][field] if isinstance(field, int) else settings[field]
        p.set_editor_property(name, value)


def fields_settings():
    p = panel()
    return {"whose": p.get_editor_property("whose") or SAME, "fov": p.get_editor_property("fov"),
            "offset": [p.get_editor_property(n) for n in ("offset_x", "offset_y", "offset_z")],
            "hand": p.get_editor_property("hand") or AS_PLAYED, "motion": p.get_editor_property("motion")}


def say(message):
    log("viewmodel: " + message)
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world()
    unreal.SystemLibrary.print_string(world, "OFAST: " + message, True, False, unreal.LinearColor(1.0, 0.75, 0.25, 1.0),
                                      6.0, "OFAST_viewmodel")


def apply_to_open_round(settings):
    sequence = sequencer.get_current_level_sequence()
    if sequence is None or not assets.get_metadata_tag(sequence, "OFAST_start_tick"):
        return
    count = key(sequence, settings)
    if count is None:
        say("This round was sent before the Viewmodel panel: send it to Unreal again to change its gun.")
        return
    sequencer.refresh_current_level_sequence()
    sequencer.force_update()
    say("Viewmodel of %d players changed." % count)


def _tick(_delta):
    if _state["panel"] is None:
        return
    try:
        sequence = sequencer.get_current_level_sequence()
        path = sequence.get_path_name() if sequence is not None else None
        settings = fields_settings()
        if path != _state["sequence"]:
            # Another round opened: keyed with the project's settings if it was keyed with others.
            _state["sequence"] = path
            if sequence is not None and keyed_with(sequence) != json.dumps(settings, sort_keys=True):
                apply_to_open_round(settings)
        if settings != _state["last"]:
            _state["last"] = settings
            _state["pending"] = time.monotonic()
        elif _state["pending"] is not None and time.monotonic() - _state["pending"] >= SETTLE:
            _state["pending"] = None
            save_settings(settings)
            apply_to_open_round(settings)
    except Exception as error:
        if str(error) != _state.get("error"):
            _state["error"] = str(error)
            log("viewmodel: FAILED: %s" % error)


def register():
    """OFAST > Viewmodel in the editor's menu bar (the OFAST menu made by ofast_look.register) and the tick callback
    that watches the panel; once a session."""
    if _state["handle"] is not None:
        return
    import ofast_look

    ofast_look.register()
    menus = unreal.ToolMenus.get()
    menu = menus.find_menu("LevelEditor.MainMenu.OFAST")
    if menu:
        entry = unreal.ToolMenuEntry(name="OFASTViewmodel", type=unreal.MultiBlockType.MENU_ENTRY)
        entry.set_label("Viewmodel…")
        entry.set_tool_tip("The gun in first person: field of view, offsets, hand, its motion. For every round.")
        entry.set_string_command(unreal.ToolMenuStringCommandType.PYTHON, "",
                                 "import ofast_viewmodel; ofast_viewmodel.open_panel()")
        menu.add_menu_entry("Viewmodel", entry)
        menus.refresh_all_widgets()
    _state["last"] = fields_settings()
    _state["handle"] = unreal.register_slate_post_tick_callback(_tick)
    log("viewmodel: panel ready (OFAST > Viewmodel)")


def open_panel():
    """Opens the Viewmodel panel (the project's settings)."""
    register()
    unreal.get_editor_subsystem(unreal.AssetEditorSubsystem).open_editor_for_assets([panel()])
    return panel()
