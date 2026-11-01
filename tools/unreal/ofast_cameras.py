"""OFAST's Cameras panel: cinematic shots of a round made in Unreal without knowing Sequencer (the user's choice,
2026-10-31: "People like to do their own cinematics", most do not know Unreal's camera or render system). It works as
HLAE's campath does: fly the view to where the camera should be at a moment of the round, add a point, move on in
time, add the next; OFAST makes a smooth camera move through the points and cuts to it in the round. Docs:
docs/user-guide/README.md, "Making a cinematic"; docs/formats/cinematics.md.

- A cinematic is a CineCameraActor in the round's level ("Cinematic 1", tagged OFAST_replay, so it goes with its
  round as the round's other actors do, and OFAST_cinematic), bound in the round's sequence.
- Its points are what the user placed: a demo tick, where the view was and which way it looked (Unreal's
  centimetres and degrees) and its field of view. They are kept in a file per round,
  <project>/Saved/OFAST/Cinematics/<demo>_r<round>.json, and the camera's keys are made from them whenever they change
  (build): one key a tick, along a smooth curve through the points (spline), so it plays smoothly in slow motion.
  The file is what counts: a round sent to Unreal again, or its cameras changed, gets its cinematics back (restore,
  called by import_demo.py and import_cameras.py), and a render makes them again from it (render_round.py), even if
  Unreal was closed without saving. Keys changed by hand in Sequencer are made again from the points at the next
  change.
- Options, each cinematic its own: keep a player in frame (the camera turns to them), follow a player (the points are
  kept as places round the player, so the camera moves with them, smoothed), focus on that player (depth of field).
- The round's cuts: its cameras file's cuts (replay_cameras.add_cuts keeps them on the sequence, CUTS_TAG), with each
  cinematic of two points or more cut in from its first point to its last (a later one over an earlier one); the
  muzzle lights' visibility is keyed again for them (replay_cameras.key_lights).

The panel is an object of OfastCameras, a class made here in Python, opened in an editor tab of its own as the Look
panel is (ofast_look.py): its fields are the settings, its functions marked CallInEditor its buttons. A Slate tick
callback (register) watches the fields and the playhead. Opened by the Studio's "Open in Unreal" (open_sequence.py)
and from the editor's menu bar: OFAST > Cameras.
"""

import bisect
import json
import math
import os

import unreal

import replay_cameras

CINEMATIC_TAG = unreal.Name("OFAST_cinematic")
LABEL = "Cinematic %d"
FORMAT = "ofast-cinematics"
VERSION = 1
NOBODY = "nobody"
# The view's field of view when a point is added: horizontal degrees, Unreal's viewport's own.
DEFAULT_FOV = 90.0
FOV_RANGE = (5.0, 150.0)
# Depth of field as the Look panel counts it: f-stop = DOF_RANGE ** (1 - amount) (0.5: about f/4.7).
DOF_RANGE = 22.0
DEFAULT_DOF = 0.5
# Where the camera aims and focuses on a player: above an agent's feet (its actor's origin), above a stand-in's
# middle (a cylinder's).
AIM_ABOVE_FEET_CM = 115.0
AIM_ABOVE_MIDDLE_CM = 25.0
# Smoothing of a player's path, in ticks either side: what a followed camera rides on (a drone does not jump with the
# player), and what a camera keeping them in frame aims at.
FOLLOW_SMOOTHING = 24
AIM_SMOOTHING = 6
# "Preview" starts this many ticks before the cinematic (64: a second).
PREVIEW_LEAD = 64
# A point is "at the playhead" within this many ticks of it.
SAME_POINT = 0.5

actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary
sequencer = unreal.LevelSequenceEditorBlueprintLibrary


def log(message):
    unreal.log("OFAST: " + message)


# The round's cinematics.


def sequence_path(sequence):
    return sequence.get_path_name().split(".")[0]


def record_file(sequence):
    """The round's cinematics file: <project>/Saved/OFAST/Cinematics/<demo>_r<round>.json."""
    return os.path.join(unreal.Paths.project_saved_dir(), "OFAST", "Cinematics", sequence.get_name() + ".json")


def load_record(sequence):
    """The round's cinematics ({"format", "version", "sequence", "cinematics": [...]}; none without a file)."""
    path = record_file(sequence)
    record = None
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            record = json.load(f)
        if record.get("format") != FORMAT or record.get("version") != VERSION:
            log("cameras: %s is not an OFAST cinematics file (version %d): left as it is" % (path, VERSION))
            record = None
    return record or {"format": FORMAT, "version": VERSION, "sequence": sequence_path(sequence), "cinematics": []}


def save_record(sequence, record):
    path = record_file(sequence)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(record, f, indent=1)


def new_cinematic(record):
    used = {c["name"] for c in record["cinematics"]}
    number = next(n for n in range(1, len(used) + 2) if LABEL % n not in used)
    cinematic = {"name": LABEL % number, "keep_in_frame": "", "follow": "", "focus": False,
                 "depth_of_field": DEFAULT_DOF, "points": []}
    record["cinematics"].append(cinematic)
    return cinematic


def start_tick(sequence):
    tag = assets.get_metadata_tag(sequence, "OFAST_start_tick")
    if not tag:
        raise RuntimeError("%s was not made by OFAST (no OFAST_start_tick): send the round to Unreal again"
                           % sequence.get_name())
    return int(tag)


def tick_interval(sequence):
    return float(assets.get_metadata_tag(sequence, "OFAST_tick_interval") or 1.0 / 64.0)


def frames_of(sequence, cinematic):
    """The cinematic's points' frames in the sequence (one a tick), in time order."""
    tick0 = start_tick(sequence)
    return sorted(p["tick"] - tick0 for p in cinematic["points"])


def cut_range(sequence, cinematic):
    """The frames the cinematic is cut to, (start, end) with the end left out, or None below two points."""
    frames = frames_of(sequence, cinematic)
    if len(frames) < 2:
        return None
    return int(round(frames[0])), int(round(frames[-1])) + 1


# A smooth curve through the points, and a player's path.


def spline(times, values):
    """A smooth curve through (times[i], values[i]) (values: lists of floats of one length): cubic Hermite pieces,
    each point's slope that of the parabola through it and its two neighbours (Catmull-Rom for unequal spacing), the
    end points' that of their one neighbour; held before the first time and after the last."""
    n = len(times)
    width = len(values[0])
    secants = [[(values[i + 1][k] - values[i][k]) / (times[i + 1] - times[i]) for k in range(width)]
               for i in range(n - 1)]
    slopes = []
    for i in range(n):
        if n == 1:
            slopes.append([0.0] * width)
        elif i == 0:
            slopes.append(secants[0])
        elif i == n - 1:
            slopes.append(secants[-1])
        else:
            before, after = times[i] - times[i - 1], times[i + 1] - times[i]
            slopes.append([(secants[i - 1][k] * after + secants[i][k] * before) / (before + after)
                           for k in range(width)])

    def at(t):
        if t <= times[0]:
            return list(values[0])
        if t >= times[-1]:
            return list(values[-1])
        i = min(bisect.bisect_right(times, t) - 1, n - 2)
        h = times[i + 1] - times[i]
        s = (t - times[i]) / h
        a, b, c, d = 2 * s ** 3 - 3 * s ** 2 + 1, s ** 3 - 2 * s ** 2 + s, -2 * s ** 3 + 3 * s ** 2, s ** 3 - s ** 2
        return [a * values[i][k] + b * h * slopes[i][k] + c * values[i + 1][k] + d * h * slopes[i + 1][k]
                for k in range(width)]

    return at


def bindings_by_name(sequence):
    out = {}
    for binding in sequence.get_bindings():
        out.setdefault(str(binding.get_display_name()), binding)
    return out


def key_frame(key):
    time = key.get_time(unreal.MovieSceneTimeUnit.DISPLAY_RATE)
    return time.frame_number.value + time.sub_frame


def player_actors():
    """The round's players' bodies (agents, or stand-ins): label -> actor ("T molodoy", "CT Wicadia")."""
    out = {}
    for actor in actors.get_all_level_actors():
        label = actor.get_actor_label()
        if (replay_cameras.TAG in actor.tags and label.split(" ", 1)[0] in ("T", "CT") and " " in label
                and isinstance(actor, (unreal.SkeletalMeshActor, unreal.StaticMeshActor))):
            out[label] = actor
    return out


def player_path(sequence, label, smoothing):
    """frame -> where to aim at the player `label` ([x, y, z] cm, their chest), from their body's keys in the
    sequence, averaged over `smoothing` ticks either side; None without that player."""
    if not label:
        return None
    actor = player_actors().get(label)
    binding = bindings_by_name(sequence).get(label)
    tracks = binding.find_tracks_by_type(unreal.MovieScene3DTransformTrack) if binding else []
    if actor is None or not tracks:
        log("cameras: no player %s in this round" % label)
        return None
    channels = replay_cameras.channels_by_name(tracks[0].get_sections()[0])
    lift = AIM_ABOVE_FEET_CM if isinstance(actor, unreal.SkeletalMeshActor) else AIM_ABOVE_MIDDLE_CM
    curves = []
    for axis in ("X", "Y", "Z"):
        keys = channels["Location." + axis].get_keys()
        curves.append(([key_frame(k) for k in keys], [k.get_value() for k in keys]))

    def raw(frame):
        out = []
        for frames, values in curves:
            if not frames:
                out.append(0.0)
                continue
            i = bisect.bisect_right(frames, frame)
            if i == 0:
                out.append(values[0])
            elif i == len(frames):
                out.append(values[-1])
            else:
                s = (frame - frames[i - 1]) / (frames[i] - frames[i - 1])
                out.append(values[i - 1] + (values[i] - values[i - 1]) * s)
        return out

    def smooth(frame):
        samples = [raw(frame + k) for k in range(-smoothing, smoothing + 1)]
        middle = [sum(s[k] for s in samples) / len(samples) for k in range(3)]
        return [middle[0], middle[1], middle[2] + lift]

    return smooth


def look_at(origin, target):
    """(pitch, yaw) in degrees from `origin` towards `target` (Unreal's: pitch up, yaw from +X towards +Y)."""
    dx, dy, dz = (target[k] - origin[k] for k in range(3))
    return math.degrees(math.atan2(dz, math.hypot(dx, dy))), math.degrees(math.atan2(dy, dx))


def bake(sequence, cinematic):
    """The cinematic's camera, a row a tick from its first point to its last (and on each point): (frame, location,
    (pitch, yaw, roll), field of view, focus distance or None)."""
    tick0 = start_tick(sequence)
    points = sorted(cinematic["points"], key=lambda p: p["tick"])
    if not points:
        return []
    times = [p["tick"] - tick0 for p in points]
    frames = sorted(set([times[0], times[-1]] + list(range(math.floor(times[0]) + 1, math.ceil(times[-1])))))
    follow = player_path(sequence, cinematic.get("follow"), FOLLOW_SMOOTHING)
    aim = player_path(sequence, cinematic.get("keep_in_frame"), AIM_SMOOTHING)
    focus = aim or (player_path(sequence, cinematic.get("follow"), AIM_SMOOTHING) if follow else None)
    if follow:
        places = spline(times, [[p["location"][k] - follow(t)[k] for k in range(3)] for p, t in zip(points, times)])
    else:
        places = spline(times, [p["location"] for p in points])
    # Each angle turned the short way from the point before.
    pitches = replay_cameras.unwrap([p["rotation"][0] for p in points])
    yaws = replay_cameras.unwrap([p["rotation"][1] for p in points])
    rolls = replay_cameras.unwrap([p["rotation"][2] for p in points])
    turns = spline(times, [list(r) for r in zip(pitches, yaws, rolls)])
    fovs = spline(times, [[p["fov"]] for p in points])
    rows = []
    for frame in frames:
        place = places(frame)
        if follow:
            under = follow(frame)
            place = [place[k] + under[k] for k in range(3)]
        pitch, yaw, roll = turns(frame)
        if aim:
            pitch, yaw = look_at(place, aim(frame))
        distance = None
        if cinematic.get("focus") and focus:
            target = focus(frame)
            distance = math.sqrt(sum((target[k] - place[k]) ** 2 for k in range(3)))
        rows.append([frame, place, [pitch, yaw, roll], fovs(frame)[0], distance])
    # Aimed yaws made continuous too.
    for row, yaw in zip(rows, replay_cameras.unwrap([r[2][1] for r in rows])):
        row[2][1] = yaw
    return rows


# The cinematic's camera in the level and the sequence.


def focal_length(fov):
    """The focal length (mm) giving a horizontal field of view `fov` across the 16:9 sensor."""
    return replay_cameras.SENSOR_MM[0] / 2 / math.tan(math.radians(fov) / 2)


def field_of_view(focal):
    return math.degrees(2 * math.atan(replay_cameras.SENSOR_MM[0] / 2 / focal))


def fstop(amount):
    return max(DOF_RANGE ** (1.0 - amount), 1.0)


def cinematic_actors():
    return {a.get_actor_label(): a for a in actors.get_all_level_actors() if CINEMATIC_TAG in a.tags}


def spawn_camera(name, location, rotation):
    actor = actors.spawn_actor_from_class(unreal.CineCameraActor, unreal.Vector(*location), unreal.Rotator(
        roll=rotation[2], pitch=rotation[0], yaw=rotation[1]))
    actor.set_actor_label(name)
    actor.tags = [replay_cameras.TAG, CINEMATIC_TAG]
    component = actor.get_cine_camera_component()
    filmback = component.get_editor_property("filmback")
    filmback.set_editor_property("sensor_width", replay_cameras.SENSOR_MM[0])
    filmback.set_editor_property("sensor_height", replay_cameras.SENSOR_MM[1])
    component.set_editor_property("filmback", filmback)
    lens = component.get_editor_property("lens_settings")
    lens.set_editor_property("min_focal_length", focal_length(FOV_RANGE[1]))
    lens.set_editor_property("max_focal_length", focal_length(FOV_RANGE[0]))
    lens.set_editor_property("min_f_stop", 1.0)
    component.set_editor_property("lens_settings", lens)
    return actor


def remove_bindings(sequence, name):
    for binding in sequence.get_bindings():
        if str(binding.get_display_name()) == name:
            for child in binding.get_child_possessables():
                child.remove()
            binding.remove()


def keyed_end(sequence):
    """The last frame the round is keyed to (its cameras' cuts end there), from the cuts kept on the sequence."""
    tag = assets.get_metadata_tag(sequence, replay_cameras.CUTS_TAG)
    if tag:
        return json.loads(tag)["end_frame"]
    return sequence.get_playback_end()


def build(sequence, cinematic, actor=None):
    """The cinematic's camera (made if the level has none) keyed from its points, in a binding of its own (made
    anew: the cuts are made again after, update_cuts). Returns the actor."""
    rows = bake(sequence, cinematic)
    actor = actor or cinematic_actors().get(cinematic["name"])
    if actor is None:
        first = rows[0] if rows else [0, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]]
        actor = spawn_camera(cinematic["name"], first[1], first[2])
    remove_bindings(sequence, cinematic["name"])
    binding = sequence.add_possessable(actor)
    component = actor.get_cine_camera_component()
    focusing = bool(rows) and rows[0][4] is not None
    focus = component.get_editor_property("focus_settings")
    focus.set_editor_property("focus_method",
                              unreal.CameraFocusMethod.MANUAL if focusing else unreal.CameraFocusMethod.DISABLE)
    component.set_editor_property("focus_settings", focus)
    component.set_editor_property("current_aperture", fstop(cinematic.get("depth_of_field", DEFAULT_DOF)))
    if not rows:
        return actor
    end = max(keyed_end(sequence), int(math.ceil(rows[-1][0])) + 1)
    channels = replay_cameras.transform_section(binding, end)
    cubic = unreal.MovieSceneKeyInterpolation.AUTO
    for frame, place, (pitch, yaw, roll), _fov, _focus in rows:
        for axis, value in zip(("X", "Y", "Z"), place):
            replay_cameras.add_key(channels["Location." + axis], frame, value, cubic)
        replay_cameras.add_key(channels["Rotation.X"], frame, roll, cubic)
        replay_cameras.add_key(channels["Rotation.Y"], frame, pitch, cubic)
        replay_cameras.add_key(channels["Rotation.Z"], frame, yaw, cubic)
    lens = sequence.add_possessable(component)
    for name, path, values in (("CurrentFocalLength", "CurrentFocalLength", [focal_length(r[3]) for r in rows]),
                               ("ManualFocusDistance", "FocusSettings.ManualFocusDistance",
                                [r[4] for r in rows] if focusing else None)):
        if values is None:
            continue
        track = lens.add_track(unreal.MovieSceneFloatTrack)
        track.set_property_name_and_path(name, path)
        section = track.add_section()
        section.set_range(0, end)
        channel = section.get_all_channels()[0]
        for frame, (value,) in replay_cameras.reduce_keys([r[0] for r in rows], [(v,) for v in values], 0.01):
            replay_cameras.add_key(channel, frame, value, cubic)
    return actor


def base_cuts(sequence):
    """The round's own cuts ({"end_frame", "cuts"}: replay_cameras.add_cuts); for a round built before they were
    kept, the cut track as it is (its muzzle lights then not keyed again: no player is known for its cuts)."""
    tag = assets.get_metadata_tag(sequence, replay_cameras.CUTS_TAG)
    if tag:
        return json.loads(tag)
    names = binding_names(sequence)
    cuts = []
    for track in sequence.find_tracks_by_type(unreal.MovieSceneCameraCutTrack):
        for section in track.get_sections():
            name = cut_camera(section, names)
            if name and not name.startswith("Cinematic "):
                cuts.append({"start": section.get_start_frame(), "end": section.get_end_frame(), "camera": name})
    kept = {"end_frame": sequence.get_playback_end(), "cuts": sorted(cuts, key=lambda c: c["start"])}
    assets.set_metadata_tag(sequence, replay_cameras.CUTS_TAG, json.dumps(kept))
    log("cameras: %s keeps its %d cuts as they were (built before 2026-11-01: send it again for its muzzle lights)"
        % (sequence.get_name(), len(cuts)))
    return kept


def binding_names(sequence):
    """Binding Guid (as text) -> its name. Binding ids neither compare nor print in Python; their Guids' text does."""
    return {b.get_id().export_text(): str(b.get_display_name()) for b in sequence.get_bindings()}


def cut_camera(section, names):
    """The name of the camera a camera cut section cuts to (`names`: binding_names), or None."""
    return names.get(section.get_camera_binding_id().get_editor_property("guid").export_text())


def carve(cuts, start, end):
    """`cuts` with the frames from `start` to `end` taken out (cut short, or in two)."""
    out = []
    for cut in cuts:
        if cut["end"] <= start or cut["start"] >= end:
            out.append(cut)
            continue
        if cut["start"] < start:
            out.append(dict(cut, end=start))
        if cut["end"] > end:
            out.append(dict(cut, start=end))
    return out


def update_cuts(sequence, record):
    """The camera cut track: the round's own cuts with each cinematic cut in, in the order they start (a later one
    over an earlier one); the muzzle lights keyed again for them. Returns the number of cuts."""
    base = base_cuts(sequence)
    cuts = list(base["cuts"])
    ranges = [(cut_range(sequence, c), c["name"]) for c in record["cinematics"]]
    for (start, end), name in sorted((r, n) for r, n in ranges if r):
        cuts = carve(cuts, start, end) + [{"start": start, "end": end, "camera": name}]
    cuts.sort(key=lambda c: c["start"])
    count = replay_cameras.cut_track(sequence, cuts)
    controllers = sorted({c["eyes"] for c in base["cuts"] if "eyes" in c})
    replay_cameras.key_lights(sequence, replay_cameras.views_of(cuts), controllers, base["end_frame"])
    return count


def restore(sequence):
    """The round's cinematics as its file says (import_demo.py, import_cameras.py, render_round.py): each camera made
    if missing and keyed again, those no longer in the file removed, the cuts made again. Returns the number of
    cinematics; nothing is changed for a round without a cinematics file."""
    if not os.path.exists(record_file(sequence)):
        return 0
    record = load_record(sequence)
    names = {c["name"] for c in record["cinematics"]}
    for label, actor in cinematic_actors().items():
        if label not in names:
            remove_bindings(sequence, label)
            actors.destroy_actor(actor)
    for cinematic in record["cinematics"]:
        build(sequence, cinematic)
    cuts = update_cuts(sequence, record)
    log("cameras: %d cinematics of %s made again, %d cuts" % (len(record["cinematics"]), sequence.get_name(), cuts))
    return len(record["cinematics"])


# The panel.


def tip(text):
    return {"ToolTip": text}


# The Details panel lists a Python class's fields last first, so they are written here bottom up: Players, Points,
# Cinematic.
@unreal.uclass()
class OfastCameras(unreal.PrimaryDataAsset):
    depth_of_field = unreal.uproperty(float, meta={
        "Category": "Players", "DisplayName": "Depth of field", "UIMin": "0.0", "UIMax": "1.0", "ClampMin": "0.0",
        "ClampMax": "1.0", "EditCondition": "focus",
        **tip("How strong the blur is away from the player in focus: 0.5 about f/4.7, 0.8 about f/1.9.")})
    focus = unreal.uproperty(bool, meta={
        "Category": "Players", "DisplayName": "Focus on them",
        **tip("Depth of field: sharp on the player kept in frame (or followed), the rest blurred.")})
    follow = unreal.uproperty(str, meta={
        "Category": "Players", "DisplayName": "Follow", "GetOptions": "player_options",
        **tip("The camera moves with this player, as a drone would: each point is kept as a place round them.")})
    keep_in_frame = unreal.uproperty(str, meta={
        "Category": "Players", "DisplayName": "Keep in frame", "GetOptions": "player_options",
        **tip("The camera turns to keep this player in the middle of the picture (the points' turns are left out).")})
    field_of_view = unreal.uproperty(float, meta={
        "Category": "Points", "DisplayName": "Field of view", "UIMin": "20.0", "UIMax": "120.0",
        "ClampMin": str(FOV_RANGE[0]), "ClampMax": str(FOV_RANGE[1]),
        **tip("How wide the camera sees at the next point you add, degrees across: less zooms in. 90 is the "
              "viewport's own.")})
    points = unreal.uproperty(str, meta={
        "Category": "Points", "DisplayName": "Points", "MultiLine": "true", "EditCondition": "false",
        **tip("The cinematic's points (times from the round's start), its cut, where the playhead is.")})
    cinematic = unreal.uproperty(str, meta={
        "Category": "Cinematic", "DisplayName": "Cinematic", "GetOptions": "cinematic_options",
        **tip("The cinematic you are making: its points and settings below.")})

    @unreal.ufunction(ret=unreal.Array(str))
    def cinematic_options(self):
        return [c["name"] for c in _state["record"]["cinematics"]] if _state["record"] else []

    @unreal.ufunction(ret=unreal.Array(str))
    def player_options(self):
        return [NOBODY] + sorted(player_actors())

    # Buttons show in alphabetical order (of their names and labels alike, here): hence "Earlier" and "Later" point.
    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Cinematic", "DisplayName": "New cinematic",
                            **tip("A new camera where the view is now, its first point at the playhead; you fly "
                                  "it (hold the right mouse button, W A S D, Q and E).")})
    def new_cinematic(self):
        act(add_cinematic)

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Cinematic", "DisplayName": "Remove cinematic",
                            **tip("Removes this cinematic: its camera, its points, its cut.")})
    def remove_cinematic(self):
        act(drop_cinematic)

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Points", "DisplayName": "Add point here",
                            **tip("The camera is where the view is now at the playhead's moment (the point there "
                                  "is replaced). Then move the playhead, fly, add the next.")})
    def add_point(self):
        act(add_point)

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Points", "DisplayName": "Earlier point",
                            **tip("Moves the playhead to the point before it.")})
    def earlier_point(self):
        act(lambda: go_to_point(-1))

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Points", "DisplayName": "Later point",
                            **tip("Moves the playhead to the point after it.")})
    def later_point(self):
        act(lambda: go_to_point(1))

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Points", "DisplayName": "Remove point here",
                            **tip("Removes the point at the playhead (Earlier point and Later point go to one).")})
    def remove_point(self):
        act(remove_point)

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Preview", "DisplayName": "Preview",
                            **tip("Plays the round through its cuts from a second before this cinematic (Space "
                                  "stops it).")})
    def preview(self):
        act(preview)

    @unreal.ufunction(meta={"CallInEditor": "true", "Category": "Preview", "DisplayName": "Fly the camera",
                            **tip("The view becomes this cinematic's camera again, to move it at the playhead.")})
    def fly(self):
        act(fly)


# The panel's object, the open sequence and its cinematics, what the fields held at the last tick.
_state = {"handle": None, "panel": None, "sequence": None, "record": None, "last": None, "frame": None}
# What a button did, shown in the level viewport for a few seconds (one message at a time).
MESSAGE_SECONDS = 6.0
MESSAGE_COLOUR = unreal.LinearColor(1.0, 0.75, 0.25, 1.0)


def panel():
    if _state["panel"] is None:
        _state["panel"] = unreal.new_object(OfastCameras, name="Cameras")
        _state["panel"].set_editor_property("field_of_view", DEFAULT_FOV)
        _state["panel"].set_editor_property("depth_of_field", DEFAULT_DOF)
    return _state["panel"]


def act(work):
    """Runs a button's work; what went wrong is said in the panel."""
    try:
        if current_sequence() is None:
            say("Open a round first (the Studio's Open in Unreal).")
            return
        work()
    except Exception as error:
        say("That failed: %s" % error)
        log("cameras: FAILED: %s" % error)
    show()


def say(message):
    """Says what a button did in the level viewport (the panel's own rows would move with a message of changing
    length) and in the log."""
    if not message:
        return
    log("cameras: " + message)
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world()
    unreal.SystemLibrary.print_string(world, "OFAST: " + message, True, False, MESSAGE_COLOUR, MESSAGE_SECONDS,
                                      "OFAST_cameras")


def current_sequence():
    """The sequence open in Sequencer, if it is an OFAST round (its cinematics loaded when it changes)."""
    sequence = sequencer.get_current_level_sequence()
    if sequence is None or not assets.get_metadata_tag(sequence, "OFAST_start_tick"):
        _state.update(sequence=None, record=None)
        return None
    path = sequence_path(sequence)
    if _state["sequence"] != path:
        _state.update(sequence=path, record=load_record(sequence), frame=None)
        names = [c["name"] for c in _state["record"]["cinematics"]]
        if any(name not in cinematic_actors() for name in names):
            # Unreal closed without saving the level: the cameras made again from the file.
            restore(sequence)
        load_fields(next(iter(names), ""))
    return sequence


def selected():
    """The cinematic the panel shows, or None."""
    name = panel().get_editor_property("cinematic")
    record = _state["record"]
    return next((c for c in record["cinematics"] if c["name"] == name), None) if record else None


def load_fields(name):
    """The panel's fields set from the cinematic `name` (or its defaults without one)."""
    p = panel()
    cinematic = next((c for c in (_state["record"] or {}).get("cinematics", []) if c["name"] == name), None)
    p.set_editor_property("cinematic", cinematic["name"] if cinematic else "")
    p.set_editor_property("keep_in_frame", (cinematic or {}).get("keep_in_frame") or NOBODY)
    p.set_editor_property("follow", (cinematic or {}).get("follow") or NOBODY)
    p.set_editor_property("focus", bool((cinematic or {}).get("focus")))
    p.set_editor_property("depth_of_field", (cinematic or {}).get("depth_of_field", DEFAULT_DOF))
    _state["last"] = fields()


def fields():
    p = panel()
    return {name: p.get_editor_property(name)
            for name in ("cinematic", "keep_in_frame", "follow", "focus", "depth_of_field", "field_of_view")}


def player_field(value):
    return "" if value in ("", NOBODY) else value


def playhead():
    return sequencer.get_current_time()


def clock(sequence, frame):
    seconds = frame * tick_interval(sequence)
    return "%d:%05.2f" % (seconds // 60, seconds % 60)


def view():
    """Where the view is and which way it looks: (location, (pitch, yaw, roll))."""
    piloted = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).get_pilot_level_actor()
    if piloted is not None:
        location, rotation = piloted.get_actor_location(), piloted.get_actor_rotation()
    else:
        info = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_level_viewport_camera_info()
        if not info:
            raise RuntimeError("no level viewport to take the view from")
        location, rotation = info[-2], info[-1]
    return [location.x, location.y, location.z], [rotation.pitch, rotation.yaw, rotation.roll]


def pilot(actor):
    """The viewport becomes the camera (moving the view moves it), out of the cuts' lock."""
    sequencer.set_lock_camera_cut_to_viewport(False)
    unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).pilot_level_actor(actor)


def changed(sequence, cinematic, actor=None):
    """Keys the cinematic again, cuts again, saves its file; Sequencer shows it."""
    actor = build(sequence, cinematic, actor)
    update_cuts(sequence, _state["record"])
    save_record(sequence, _state["record"])
    sequencer.refresh_current_level_sequence()
    sequencer.force_update()
    return actor


def add_cinematic():
    sequence = current_sequence()
    cinematic = new_cinematic(_state["record"])
    location, rotation = view()
    actor = spawn_camera(cinematic["name"], location, rotation)
    load_fields(cinematic["name"])
    cinematic["points"].append(point(sequence, location, rotation))
    changed(sequence, cinematic, actor)
    pilot(actor)
    say("%s made, its first point at %s. Fly the camera, move the playhead, add the next point."
        % (cinematic["name"], clock(sequence, playhead())))


def point(sequence, location, rotation):
    return {"tick": start_tick(sequence) + playhead(), "location": location, "rotation": rotation,
            "fov": panel().get_editor_property("field_of_view")}


def add_point():
    sequence = current_sequence()
    cinematic = selected()
    if cinematic is None:
        add_cinematic()
        return
    location, rotation = view()
    new = point(sequence, location, rotation)
    kept = [p for p in cinematic["points"] if abs(p["tick"] - new["tick"]) >= SAME_POINT]
    replaced = len(kept) < len(cinematic["points"])
    cinematic["points"] = sorted(kept + [new], key=lambda p: p["tick"])
    actor = changed(sequence, cinematic)
    pilot(actor)
    number = cinematic["points"].index(new) + 1
    say("Point %d %s at %s." % (number, "replaced" if replaced else "added", clock(sequence, playhead())))


def remove_point():
    sequence = current_sequence()
    cinematic = selected()
    if cinematic is None:
        say("No cinematic: make one with New cinematic.")
        return
    tick = start_tick(sequence) + playhead()
    kept = [p for p in cinematic["points"] if abs(p["tick"] - tick) >= SAME_POINT]
    if len(kept) == len(cinematic["points"]):
        say("No point at the playhead: Earlier point and Later point go to one.")
        return
    cinematic["points"] = kept
    changed(sequence, cinematic)
    say("Point at %s removed; %d left." % (clock(sequence, playhead()), len(kept)))


def go_to_point(step):
    sequence = current_sequence()
    cinematic = selected()
    frames = frames_of(sequence, cinematic) if cinematic else []
    now = playhead()
    if step < 0:
        targets = [f for f in frames if f < now - SAME_POINT]
        target = targets[-1] if targets else None
    else:
        targets = [f for f in frames if f > now + SAME_POINT]
        target = targets[0] if targets else None
    if target is None:
        say("No point %s the playhead." % ("before" if step < 0 else "after"))
        return
    sequencer.set_current_time(int(round(target)))
    say("At point %d of %d (%s)." % (frames.index(target) + 1, len(frames), clock(sequence, target)))


def drop_cinematic():
    sequence = current_sequence()
    cinematic = selected()
    if cinematic is None:
        say("No cinematic to remove.")
        return
    count = len(cinematic["points"])
    answer = unreal.EditorDialog.show_message(
        "Remove %s?" % cinematic["name"],
        "Its camera, its %d point%s and its cut go." % (count, "" if count == 1 else "s"),
        unreal.AppMsgType.YES_NO, unreal.AppReturnType.NO)
    if answer != unreal.AppReturnType.YES:
        return
    level = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
    actor = cinematic_actors().get(cinematic["name"])
    if actor is not None and level.get_pilot_level_actor() == actor:
        level.eject_pilot_level_actor()
    _state["record"]["cinematics"].remove(cinematic)
    remove_bindings(sequence, cinematic["name"])
    if actor is not None:
        actors.destroy_actor(actor)
    update_cuts(sequence, _state["record"])
    save_record(sequence, _state["record"])
    sequencer.refresh_current_level_sequence()
    load_fields(next((c["name"] for c in _state["record"]["cinematics"]), ""))
    say("%s removed." % cinematic["name"])


def preview():
    sequence = current_sequence()
    cinematic = selected()
    frames = frames_of(sequence, cinematic) if cinematic else []
    unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).eject_pilot_level_actor()
    sequencer.set_lock_camera_cut_to_viewport(True)
    if frames:
        sequencer.set_current_time(max(int(frames[0]) - PREVIEW_LEAD, 0))
    sequencer.play()
    say("Playing through the cuts: Space stops; Fly the camera to change it.")


def fly():
    current_sequence()
    cinematic = selected()
    actor = cinematic_actors().get(cinematic["name"]) if cinematic else None
    if actor is None:
        say("No cinematic camera: make one with New cinematic.")
        return
    pilot(actor)
    say("Flying %s: hold the right mouse button, W A S D, Q and E; the wheel while flying sets the speed."
        % cinematic["name"])


def where(frames, now):
    """Where the playhead is among the points, in a few words."""
    at = [i for i, f in enumerate(frames) if abs(f - now) < SAME_POINT]
    if at:
        return "Playhead on point %d" % (at[0] + 1)
    if not frames:
        return "No points"
    if now < frames[0]:
        return "Playhead before point 1"
    if now > frames[-1]:
        return "Playhead after point %d" % len(frames)
    after = next(i for i, f in enumerate(frames) if f > now)
    return "Playhead between points %d and %d" % (after, after + 1)


def show():
    """The Points field: three short lines (the panel's rows below would move if it changed height): the
    cinematic's points, its cut, where the playhead is."""
    sequence = current_sequence()
    p = panel()
    if sequence is None:
        lines = ["Open a round first", "(the Studio's Open in Unreal)", ""]
    else:
        cinematic = selected()
        if cinematic is None:
            lines = ["No cinematic yet: put the view", "and the playhead where it starts,", "then New cinematic"]
        else:
            frames = frames_of(sequence, cinematic)
            cut = cut_range(sequence, cinematic)
            if len(frames) > 1:
                first = "%d points, %s to %s" % (len(frames), clock(sequence, frames[0]), clock(sequence, frames[-1]))
            else:
                first = "1 point, at %s" % clock(sequence, frames[0]) if frames else "No points"
            lines = [first, "Cut in for %.1f s" % ((cut[1] - 1 - cut[0]) * tick_interval(sequence)) if cut
                     else "Add a second point to cut to it", where(frames, playhead())]
    text = "\n".join(lines)
    if p.get_editor_property("points") != text:
        p.set_editor_property("points", text)


def apply_fields(sequence, now):
    """What changed in the panel since the last tick: another cinematic picked (its settings shown), a player
    setting (the cinematic keyed again), the depth of field (the camera's aperture: it is not keyed, so a slider
    dragged stays smooth), the field of view (the flown camera zoomed)."""
    last, values = _state["last"], now
    if values["cinematic"] != last["cinematic"]:
        load_fields(values["cinematic"] if any(c["name"] == values["cinematic"]
                                               for c in _state["record"]["cinematics"]) else last["cinematic"])
        return
    cinematic = selected()
    if cinematic is not None and any(values[k] != last[k] for k in ("keep_in_frame", "follow", "focus")):
        cinematic.update(keep_in_frame=player_field(values["keep_in_frame"]), follow=player_field(values["follow"]),
                         focus=values["focus"])
        changed(sequence, cinematic)
        if cinematic["focus"] and not (cinematic["keep_in_frame"] or cinematic["follow"]):
            say("Focus on them: pick a player to keep in frame or to follow.")
    if cinematic is not None and values["depth_of_field"] != last["depth_of_field"]:
        cinematic["depth_of_field"] = values["depth_of_field"]
        actor = cinematic_actors().get(cinematic["name"])
        if actor is not None:
            actor.get_cine_camera_component().set_editor_property("current_aperture",
                                                                  fstop(values["depth_of_field"]))
        save_record(sequence, _state["record"])
    if values["field_of_view"] != last["field_of_view"]:
        actor = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).get_pilot_level_actor()
        if actor is not None and CINEMATIC_TAG in actor.tags:
            actor.get_cine_camera_component().set_editor_property("current_focal_length",
                                                                  focal_length(values["field_of_view"]))
    _state["last"] = fields()


def _tick(_delta):
    if _state["panel"] is None:
        return
    try:
        sequence = current_sequence()
        if sequence is None:
            show()
            return
        values = fields()
        if values != _state["last"]:
            apply_fields(sequence, values)
        frame = playhead()
        if frame != _state["frame"]:
            _state["frame"] = frame
            cinematic = selected()
            if cinematic and cinematic["points"] and not sequencer.is_playing():
                # The field of view the camera has there, for the next point.
                tick0 = start_tick(sequence)
                points = sorted(cinematic["points"], key=lambda p: p["tick"])
                fov = spline([p["tick"] - tick0 for p in points], [[p["fov"]] for p in points])(frame)[0]
                panel().set_editor_property("field_of_view", round(fov, 2))
                _state["last"] = fields()
            show()
    except Exception as error:
        # Said once, not at every tick.
        if str(error) != _state.get("error"):
            _state["error"] = str(error)
            say("That failed: %s" % error)


def register():
    """OFAST > Cameras in the editor's menu bar (the OFAST menu made by ofast_look.register) and the tick callback
    that watches the panel; once a session."""
    if _state["handle"] is not None:
        return
    import ofast_look

    ofast_look.register()
    menus = unreal.ToolMenus.get()
    menu = menus.find_menu("LevelEditor.MainMenu.OFAST")
    if menu:
        entry = unreal.ToolMenuEntry(name="OFASTCameras", type=unreal.MultiBlockType.MENU_ENTRY)
        entry.set_label("Cameras…")
        entry.set_tool_tip("Your own cinematic shots of the round: fly the view, add points, preview.")
        entry.set_string_command(unreal.ToolMenuStringCommandType.PYTHON, "",
                                 "import ofast_cameras; ofast_cameras.open_panel()")
        menu.add_menu_entry("Cameras", entry)
        menus.refresh_all_widgets()
    _state["handle"] = unreal.register_slate_post_tick_callback(_tick)
    log("cameras: panel ready (OFAST > Cameras)")


def open_panel():
    """Opens the Cameras panel for the round open in Sequencer."""
    register()
    current_sequence()
    show()
    unreal.get_editor_subsystem(unreal.AssetEditorSubsystem).open_editor_for_assets([panel()])
    return panel()
