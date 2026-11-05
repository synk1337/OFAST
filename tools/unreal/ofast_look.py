"""OFAST's Look panel: a few beginner settings for the look of a round, changed with sliders in Unreal and seen live in
the viewport (the user's choice, 2026-10-31: change the lighting, the depth of field and so on without knowing
Unreal). Docs: docs/user-guide/README.md, "The Look panel"; docs/formats/lighting.md, "The Look panel".

- The panel is an object of OfastLook, a class made here in Python, opened in an editor tab of its own: its fields
  are the panel's sliders, in plain words, in five groups (Sun, Atmosphere, Camera, Colour and film, Reset). Most are
  changes from the map's own look (the sun 10° higher, the sky light twice as bright); a few are amounts (depth of
  field, vignette, grain) that start at the look's own. Unreal cannot save an object of a class made in Python (its
  class is transient: saving one as an asset failed, with an error box each time), so the object lives in memory and
  its values are kept per level in a file, <project>/Saved/OFAST/Looks/<level>.json.
- The map's own look (what the import made: the sun's angle, strength and colour, the sky light, the haze, the
  exposure, the lens effects) is kept in that file too, read from the level the first time and again whenever
  import_map.py has built it anew (the level's OFAST_import_id), so every change is made from it, and "Reset
  everything" goes back to it.
- What the sliders drive: the level's sun (DirectionalLight: its angle, intensity, colour temperature, how much it
  lights the haze), sky light, haze (ExponentialHeightFog), post-process volume (OFAST_PostProcess: exposure, motion
  blur; in the realistic look Unreal's colour grading, vignette, grain, bloom and lens flare), the round's POV
  cameras' depth of field (a lens focused where the player looks, or a blur growing with distance: their focus
  tracks in the round's sequence switched off meanwhile), and in CS2's look the grading of OFAST's tone mapper
  (tonemapper.py: Unreal's own does nothing after a replacing tone mapper).
- Live: a Slate tick callback (register) watches the open look and applies it to the level when a value changes, and
  writes its file once the sliders rest. Rendering, shooting, importing a round or its cameras apply the level's look
  too (apply_level_look), so what Unreal renders is what the panel shows, even if the level was not saved.

Opened by the Studio's "Open in Unreal" (open_sequence.py) and from the editor's menu bar: OFAST > Look.
"""

import json
import math
import os

import unreal

import replay_cameras
import tonemapper

# The level's tag import_map.py stamps when it builds the level.
IMPORT_TAG = "OFAST_import_id"
POV_PREFIX = "POV "
POST_VOLUME = "OFAST_PostProcess"
# Depth of field 1 is f/1, 0.8 about f/1.9 (the realistic look's f/1.8), 0.5 f/4.7: f = DOF_RANGE ** (1 - amount).
DOF_RANGE = 22.0
# "Blur with distance" (the user's choice, 2026-11-04): the POV cameras' depth of field as a fragmovie's depth pass
# blurred in After Effects, sharp at the player and blurring more the further away, instead of a lens focused where
# the player looks. Unreal's depth blur: a blur of DEPTH_BLUR_WIDTH × the depth of field amount far away (a radius, in
# fractions of the picture's width), half that at the panel's Blur distance. Unreal adds it only to what lies behind a
# lens's focus (in front, it is the foreground, which the foreground limit keeps sharp for the gun): so the lens
# focuses 1 m away, at f/32 on a full-frame sensor, too small an opening to blur anything itself (0.01 % of the
# width far away), and the POV camera's own focus track (where the player looks, replay_cameras.key_focus) is
# switched off in the round's sequence meanwhile.
DEPTH_BLUR_WIDTH = 0.008
DEPTH_FOCUS_CM = 100.0
DEPTH_FSTOP = 32.0
DEPTH_SENSOR_MM = 36.0
BLUR_DISTANCE_M = 20.0
# Unreal's depth blur radius is "pixels of a 1920-wide picture", doubled when used (DiaphragmDOFUtils.cpp).
DEPTH_BLUR_PIXELS = 1920.0 / 2.0
# The POV cameras' own focus track (replay_cameras.key_focus).
FOCUS_PATH = "PostProcessSettings.DepthOfFieldFocalDistance"
# Their focus where they have no focus track (replay_cameras.key_focus without rows), cm.
NO_TRACK_FOCUS_CM = 1000.0
# The sun's colour: warmth 1 takes it 2500 K warmer (a lower temperature), −1 as much cooler; from 6500 K (white)
# when the look gives it no temperature of its own.
SUN_WARMTH_KELVIN = 2500.0
NEUTRAL_KELVIN = 6500.0
# Unreal's white balance in the realistic look: warmth 1 sets it 2500 K higher (a warmer picture).
WHITE_BALANCE_KELVIN = 2500.0
# Writes the look's file once its values have not changed for this many editor ticks.
SAVE_AFTER_TICKS = 30
# The sun's height above the horizon is kept within this, degrees.
SUN_ELEVATION = (1.0, 89.0)


def log(message):
    unreal.log("OFAST: " + message)


def slider(kind, category, label, low, high, tip, clamp=None):
    """A field of the panel: a slider from `low` to `high` (typed values clamped to `clamp`, default the same)."""
    clamp = clamp or (low, high)
    return unreal.uproperty(kind, meta={
        "Category": category, "DisplayName": label, "ToolTip": tip, "UIMin": str(low), "UIMax": str(high),
        "ClampMin": str(clamp[0]), "ClampMax": str(clamp[1])})


REALISTIC_ONLY = " (the realistic look only)"


# The Details panel lists a Python class's fields last first, so they are written here bottom up: Reset, Colour and
# film, Camera, Atmosphere, Sun.
@unreal.uclass()
class OfastLook(unreal.PrimaryDataAsset):
    reset = unreal.uproperty(bool, meta={
        "Category": "Reset", "DisplayName": "Reset everything",
        "ToolTip": "Tick to put every setting back to the map's own look."})
    lens_flare = slider(float, "Colour and film", "Lens flare", 0.0, 1.0,
                        "Streaks and rings round bright lights" + REALISTIC_ONLY + ".")
    bloom = slider(float, "Colour and film", "Bloom", 0.0, 2.0,
                   "A glow round bright things" + REALISTIC_ONLY + ".", (0.0, 8.0))
    film_grain = slider(float, "Colour and film", "Film grain", 0.0, 1.0, "Grain, as on film. 0: none.")
    vignette = slider(float, "Colour and film", "Vignette", 0.0, 1.0, "Darkens the corners. 0: none.")
    contrast = slider(float, "Colour and film", "Contrast", 0.5, 1.5,
                      "1 is the look's own; more makes darks darker and lights lighter.", (0.0, 3.0))
    saturation = slider(float, "Colour and film", "Saturation", 0.0, 2.0,
                        "1 is the look's own; 0 black and white, 2 twice as colourful.", (0.0, 4.0))
    warmth = slider(float, "Colour and film", "Warm / cool", -1.0, 1.0,
                    "The whole picture warmer (+, orange) or cooler (−, blue). 0 is the look's own.")
    scope_overlay = unreal.uproperty(bool, meta={
        "Category": "Camera", "DisplayName": "Scope overlay",
        "ToolTip": "Through the players' eyes, CS2's scope over the view while they look through a sniper's scope "
                   "(the black ring and lines), and the AUG's and SG 553's sight (the blur around it, the dot). Off: "
                   "only the zoom, to add your own scope in editing."})
    motion_blur = slider(float, "Camera", "Motion blur", 0.0, 1.0,
                         "How much moving things and fast turns smear. 0: none; 0.5 is Unreal's usual.")
    blur_distance = unreal.uproperty(float, meta={
        "Category": "Camera", "DisplayName": "Blur distance", "UIMin": "2.0", "UIMax": "60.0", "ClampMin": "0.5",
        "ClampMax": "500.0", "EditCondition": "depth_blur",
        "ToolTip": "With Blur with distance: how far away, in metres, the map is blurred half as much as the most "
                   "it gets (Depth of field). Nearer: more of the map blurred."})
    depth_blur = unreal.uproperty(bool, meta={
        "Category": "Camera", "DisplayName": "Blur with distance",
        "ToolTip": "Depth of field through the players' eyes as a depth pass blurred in After Effects: the gun and "
                   "what is near sharp, the map blurring more the further away it is, whatever the player looks at "
                   "(off: sharp where the player looks, blurred behind it). Depth of field sets how strong. Not "
                   "with focus keys (OFAST > Cameras, Depth of field), which focus as they say."})
    depth_of_field = slider(float, "Camera", "Depth of field", 0.0, 1.0,
                            "Blur away from what the player looks at, through their eyes. 0: everything sharp; "
                            "0.8 about f/1.9, a strong cinematic blur; 1 stronger still. With Blur with distance, "
                            "how blurred the far map gets (1: 0.8 % of the picture's width). A round with focus keys "
                            "(OFAST > Cameras, Depth of field) takes them instead.")
    brightness = slider(float, "Camera", "Brightness", -3.0, 3.0,
                        "Brighter (+) or darker (−) in camera stops: +1 is twice as bright. 0 is the look's own.",
                        (-8.0, 8.0))
    light_beams = slider(float, "Atmosphere", "Light beams", 0.0, 3.0,
                         "How strongly the sun's beams show in the haze. 1 is the look's own" + REALISTIC_ONLY + ".",
                         (0.0, 10.0))
    haze = slider(float, "Atmosphere", "Haze", 0.0, 3.0,
                  "How thick the air's haze is. 1 is the look's own; 0 clear air" + REALISTIC_ONLY + ".",
                  (0.0, 10.0))
    ambient = slider(float, "Sun", "Shadow brightness", 0.0, 3.0,
                     "The light from the sky, which lights the shade. 1 is the map's own.", (0.0, 10.0))
    sun_warmth = slider(float, "Sun", "Sun colour", -1.0, 1.0,
                        "The sunlight warmer (+, golden) or cooler (−, bluish). 0 is the map's own.")
    sun_strength = slider(float, "Sun", "Sun strength", 0.0, 3.0, "1 is the map's own sun; 0 turns it off.",
                          (0.0, 10.0))
    sun_direction = slider(float, "Sun", "Sun direction", -180.0, 180.0,
                           "Turns the sun round the map, degrees from where the map has it.")
    sun_height = slider(float, "Sun", "Sun height", -60.0, 60.0,
                        "Raises (+) or lowers (−) the sun, degrees from where the map has it. In the realistic look "
                        "the sky follows: low for a golden hour.")


# The panel's fields, in their order on screen.
FIELDS = ["sun_height", "sun_direction", "sun_strength", "sun_warmth", "ambient", "haze", "light_beams", "brightness",
          "depth_of_field", "depth_blur", "blur_distance", "motion_blur", "scope_overlay", "warmth", "saturation",
          "contrast", "vignette", "film_grain", "bloom", "lens_flare", "reset"]
# Their values for the map's own look; those not here start at what the level has (start_values).
NEUTRAL = {"sun_height": 0.0, "sun_direction": 0.0, "sun_strength": 1.0, "sun_warmth": 0.0, "ambient": 1.0,
           "haze": 1.0, "light_beams": 1.0, "brightness": 0.0, "depth_blur": False, "blur_distance": BLUR_DISTANCE_M,
           "scope_overlay": True, "warmth": 0.0, "saturation": 1.0, "contrast": 1.0, "reset": False}


def editor_world():
    return unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world()


def level_path(world=None):
    """The open level's asset path, /Game/OFAST/Maps/<level>."""
    return (world or editor_world()).get_path_name().split(".")[0]


def look_file(level):
    """The level's look file: <project>/Saved/OFAST/Looks/<level>.json."""
    return os.path.join(unreal.Paths.project_saved_dir(), "OFAST", "Looks", level.rsplit("/", 1)[-1] + ".json")


def realistic(level):
    return level.endswith("_realistic")


def level_actors():
    return unreal.get_editor_subsystem(unreal.EditorActorSubsystem).get_all_level_actors()


def first(cls):
    return next((a for a in level_actors() if isinstance(a, cls)), None)


def post_volume():
    return next((a for a in level_actors() if isinstance(a, unreal.PostProcessVolume)
                 and a.get_actor_label() == POST_VOLUME), None)


def fog_component():
    fog = first(unreal.ExponentialHeightFog)
    return fog.get_component_by_class(unreal.ExponentialHeightFogComponent) if fog else None


def tone_instance():
    """CS2's look's tone mapper instance (the post-process volume's blendable), moved onto tonemapper.py's version
    (with the grading) when the map was imported with an older one; None in the realistic look."""
    volume = post_volume()
    if volume is None:
        return None
    blendables = volume.settings.get_editor_property("weighted_blendables").get_editor_property("array")
    instance = next((b.get_editor_property("object") for b in blendables
                     if isinstance(b.get_editor_property("object"), unreal.MaterialInstanceConstant)), None)
    if instance is None:
        return None
    parent = instance.get_editor_property("parent")
    if parent is None or parent.get_path_name().split(".")[0] != tonemapper.TONEMAPPER:
        unreal.MaterialEditingLibrary.set_material_instance_parent(instance, tonemapper.ensure_tonemapper())
        # Its new parent does not mark it dirty: saved anyway.
        unreal.EditorAssetLibrary.save_loaded_asset(instance, only_if_is_dirty=False)
        log("look: %s moved onto %s" % (instance.get_name(), tonemapper.TONEMAPPER))
    return instance


def read_base():
    """The map's own look, as the open level has it (before any panel setting)."""
    base = {}
    sun = first(unreal.DirectionalLight)
    if sun:
        component = sun.light_component
        rotation = sun.get_actor_rotation()
        base.update(sun_pitch=rotation.pitch, sun_yaw=rotation.yaw, sun_roll=rotation.roll,
                    sun_intensity=component.get_editor_property("intensity"),
                    sun_use_temperature=component.get_editor_property("use_temperature"),
                    sun_temperature=component.get_editor_property("temperature"),
                    sun_scattering=component.get_editor_property("volumetric_scattering_intensity"))
    sky = first(unreal.SkyLight)
    if sky:
        base["sky_intensity"] = sky.light_component.get_editor_property("intensity")
    fog = fog_component()
    if fog:
        base["fog_density"] = fog.get_editor_property("fog_density")
    volume = post_volume()
    if volume:
        settings = volume.settings
        for name in ("auto_exposure_bias", "bloom_intensity", "film_grain_intensity", "vignette_intensity",
                     "lens_flare_intensity", "motion_blur_amount"):
            if settings.get_editor_property("override_" + name):
                base[name] = settings.get_editor_property(name)
    base["depth_of_field"] = pov_depth_of_field()
    return base


def pov_cameras():
    return [a for a in level_actors() if isinstance(a, unreal.CameraActor)
            and a.get_actor_label().startswith(POV_PREFIX)]


def pov_depth_of_field():
    """The POV cameras' depth of field as an amount (0: off), from the first one's lens."""
    for camera in pov_cameras():
        settings = camera.camera_component.get_editor_property("post_process_settings")
        if not settings.get_editor_property("override_depth_of_field_focal_distance"):
            return 0.0
        if settings.get_editor_property("override_depth_of_field_depth_blur_radius"):
            radius = settings.get_editor_property("depth_of_field_depth_blur_radius")
            return round(min(radius / DEPTH_BLUR_PIXELS / DEPTH_BLUR_WIDTH, 1.0), 2)
        fstop = max(settings.get_editor_property("depth_of_field_fstop"), 1.0)
        return round(1.0 - math.log(fstop) / math.log(DOF_RANGE), 2)
    return 0.0


def start_values(base):
    """The panel's values for the map's own look: neutral changes, and the look's own amounts."""
    values = dict(NEUTRAL)
    values.update(depth_of_field=base.get("depth_of_field", 0.0),
                  motion_blur=base.get("motion_blur_amount", 0.5),
                  vignette=base.get("vignette_intensity", 0.0),
                  film_grain=base.get("film_grain_intensity", 0.0),
                  bloom=base.get("bloom_intensity", 0.0),
                  lens_flare=base.get("lens_flare_intensity", 0.0))
    return values


# The looks of this editor session, by level: (panel object, its file's record {"values", "base", "import_id"}).
_looks = {}


def get_base(look):
    """The map's own look a panel object's settings are changes from."""
    return next((record["base"] for obj, record in _looks.values() if obj == look), {})


def write_record(level):
    obj, record = _looks[level]
    record["values"] = values_of(obj)
    path = look_file(level)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(record, f, indent=1)


def look_for(level, create=True):
    """The level's look panel object, its values from the level's look file (made from the level the first time,
    `create`; its map's own look read again when import_map.py has built the level anew since); None when the level has
    no look file and `create` is false."""
    stamp = unreal.EditorAssetLibrary.get_metadata_tag(unreal.load_asset(level), IMPORT_TAG) or ""
    if level in _looks:
        obj, record = _looks[level]
    else:
        path = look_file(level)
        record = None
        if os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                record = json.load(f)
        elif not create:
            return None
        obj = unreal.new_object(OfastLook, name="Look_" + level.rsplit("/", 1)[-1])
        if record is None:
            base = read_base()
            record = {"values": start_values(base), "base": base, "import_id": stamp}
            log("look: made %s from %s" % (path, level))
        for field, value in {**start_values(record["base"]), **record["values"]}.items():
            if field in FIELDS:
                obj.set_editor_property(field, value)
        _looks[level] = (obj, record)
        write_record(level)
    if record.get("import_id", "") != stamp:
        # The level was built again: its own look read again, the user's settings kept.
        record.update(base=read_base(), import_id=stamp)
        write_record(level)
        log("look: %s took the rebuilt level's own look" % level)
    return obj


def values_of(look):
    return {field: look.get_editor_property(field) for field in FIELDS}


def apply(look, level, sequence=None):
    """Sets the open level as `look` says, from its map's own look; the POV cameras' focus tracks in `sequence` (the
    round, by default the one open in Sequencer: pov_lens)."""
    base = get_base(look)
    v = values_of(look)
    real = realistic(level)

    sun = first(unreal.DirectionalLight)
    if sun and "sun_pitch" in base:
        elevation = min(max(-base["sun_pitch"] + v["sun_height"], SUN_ELEVATION[0]), SUN_ELEVATION[1])
        sun.set_actor_rotation(unreal.Rotator(roll=base["sun_roll"], pitch=-elevation,
                                              yaw=base["sun_yaw"] + v["sun_direction"]), False)
        component = sun.light_component
        component.set_editor_property("intensity", base["sun_intensity"] * v["sun_strength"])
        if v["sun_warmth"]:
            kelvin = base["sun_temperature"] if base["sun_use_temperature"] else NEUTRAL_KELVIN
            component.set_editor_property("use_temperature", True)
            component.set_editor_property("temperature",
                                          min(max(kelvin - SUN_WARMTH_KELVIN * v["sun_warmth"], 1700.0), 12000.0))
        else:
            component.set_editor_property("use_temperature", base["sun_use_temperature"])
            component.set_editor_property("temperature", base["sun_temperature"])
        component.set_editor_property("volumetric_scattering_intensity",
                                      base["sun_scattering"] * v["light_beams"])

    sky = first(unreal.SkyLight)
    if sky and "sky_intensity" in base:
        sky.light_component.set_editor_property("intensity", base["sky_intensity"] * v["ambient"])

    fog = fog_component()
    if fog and "fog_density" in base:
        fog.set_editor_property("fog_density", base["fog_density"] * v["haze"])

    volume = post_volume()
    if volume:
        settings = volume.settings

        def override(name, value):
            settings.set_editor_property("override_" + name, True)
            settings.set_editor_property(name, value)

        override("auto_exposure_bias", base.get("auto_exposure_bias", 0.0) + v["brightness"])
        override("motion_blur_amount", v["motion_blur"])
        if real:
            # Unreal's own grading and lens effects (its tone mapper is the one drawing the realistic look).
            override("white_temp", NEUTRAL_KELVIN + WHITE_BALANCE_KELVIN * v["warmth"])
            override("color_saturation", unreal.Vector4(v["saturation"], v["saturation"], v["saturation"], 1.0))
            override("color_contrast", unreal.Vector4(v["contrast"], v["contrast"], v["contrast"], 1.0))
            override("vignette_intensity", v["vignette"])
            override("film_grain_intensity", v["film_grain"])
            override("bloom_intensity", v["bloom"])
            override("lens_flare_intensity", v["lens_flare"])
        volume.set_editor_property("settings", settings)
    if not real:
        instance = tone_instance()
        if instance:
            mel = unreal.MaterialEditingLibrary
            for param, value in (("Warmth", v["warmth"]), ("Saturation", v["saturation"]),
                                 ("Contrast", v["contrast"]), ("Vignette", v["vignette"]),
                                 ("Grain", v["film_grain"])):
                mel.set_material_instance_scalar_parameter_value(instance, param, value)
            mel.update_material_instance(instance)

    pov_lens(v, sequence or unreal.LevelSequenceEditorBlueprintLibrary.get_current_level_sequence())
    import scopes

    scopes.set_overlay(v["scope_overlay"])


def own_focus_sections(sequence):
    """The POV cameras' own focus sections in `sequence` (row 0 of their focus track, replay_cameras.key_focus; the
    Cameras panel's focus keys are on rows of their own), by camera label."""
    import ofast_cameras

    out = {}
    if sequence is None:
        return out
    cameras = {c.get_actor_label(): c for c in pov_cameras()}
    for binding in sequence.get_bindings():
        camera = cameras.get(str(binding.get_display_name()))
        if camera is None:
            continue
        for child in binding.get_child_possessables():
            if str(child.get_display_name()) != camera.camera_component.get_name():
                continue
            for track in child.find_tracks_by_type(unreal.MovieSceneFloatTrack):
                if str(track.get_property_path()) == FOCUS_PATH:
                    out.setdefault(camera.get_actor_label(), []).extend(
                        s for s in track.get_sections() if s.get_row_index() < ofast_cameras.FOCUS_ROW)
    return out


def depth_blur_radius(amount):
    """Unreal's depth blur radius for a depth of field amount (DEPTH_BLUR_WIDTH far away at 1)."""
    return DEPTH_BLUR_WIDTH * amount * DEPTH_BLUR_PIXELS


def pov_lens(v, sequence):
    """The POV cameras' depth of field as the panel's values `v` say: a lens focused where the player looks (their own
    focus track), or with Blur with distance Unreal's depth blur (DEPTH_BLUR_WIDTH), their focus track switched off in
    `sequence` (the round; None: their tracks left as they are). The round with focus keys (the Cameras panel's depth
    of field) keeps its lens as they key it, without depth blur."""
    import ofast_cameras

    keyed = ofast_cameras.focus_keyed(sequence)
    on = v["depth_of_field"] > 0.0
    depth = on and v["depth_blur"] and not keyed
    sections = own_focus_sections(sequence)
    changed = 0
    for camera in pov_cameras():
        component = camera.camera_component
        settings = component.get_editor_property("post_process_settings")
        settings.set_editor_property("override_depth_of_field_depth_blur_amount", depth)
        settings.set_editor_property("override_depth_of_field_depth_blur_radius", depth)
        if keyed:
            component.set_editor_property("post_process_settings", settings)
            continue
        settings.set_editor_property("override_depth_of_field_focal_distance", on)
        settings.set_editor_property("override_depth_of_field_fstop", on)
        settings.set_editor_property("override_depth_of_field_sensor_width", on)
        if depth:
            settings.set_editor_property("depth_of_field_focal_distance", DEPTH_FOCUS_CM)
            settings.set_editor_property("depth_of_field_fstop", DEPTH_FSTOP)
            settings.set_editor_property("depth_of_field_sensor_width", DEPTH_SENSOR_MM)
            settings.set_editor_property("depth_of_field_depth_blur_amount", v["blur_distance"] / 1000.0)
            settings.set_editor_property("depth_of_field_depth_blur_radius", depth_blur_radius(v["depth_of_field"]))
        elif on:
            settings.set_editor_property("depth_of_field_fstop",
                                         max(DOF_RANGE ** (1.0 - v["depth_of_field"]), 1.0))
            # The lens the world blurs with (replay_cameras.FOCUS_SENSOR_MM), set again: a round sent before
            # 2026-11-03 has a full frame's, which blurred only the gun.
            settings.set_editor_property("depth_of_field_sensor_width", replay_cameras.FOCUS_SENSOR_MM)
            if sequence is not None and camera.get_actor_label() not in sections:
                # No focus track: the focus a round without one has, not Blur with distance's.
                settings.set_editor_property("depth_of_field_focal_distance", NO_TRACK_FOCUS_CM)
        component.set_editor_property("post_process_settings", settings)
        for section in sections.get(camera.get_actor_label(), []):
            if section.is_active() == depth:
                section.set_is_active(not depth)
                changed += 1
    if changed:
        log("look: %d POV focus sections of %s %s" % (changed, sequence.get_name(),
                                                       "off (blur with distance)" if depth else "on again"))
        if sequence == unreal.LevelSequenceEditorBlueprintLibrary.get_current_level_sequence():
            unreal.LevelSequenceEditorBlueprintLibrary.refresh_current_level_sequence()


def apply_level_look(create=False, sequence=None):
    """Applies the open level's look, if it has a look file (or makes one, `create`), the POV cameras' focus tracks in
    `sequence` (the round; by default the one open in Sequencer); returns its panel object or None."""
    level = level_path()
    if not level.startswith("/Game/OFAST/Maps/"):
        return None
    look = look_for(level, create)
    if look is not None:
        apply(look, level, sequence)
        log("look: applied %s" % look.get_name())
    return look


# What the tick callback watches: the look of the open level and its last values.
_watch = {"handle": None, "level": None, "look": None, "last": None, "quiet": 0}


def _tick(delta):
    level = level_path()
    if level != _watch["level"]:
        _watch.update(level=level, look=None, last=None)
        if level.startswith("/Game/OFAST/Maps/"):
            _watch["look"] = look_for(level, create=False)
    look = _watch["look"]
    if look is None:
        return
    values = values_of(look)
    if values == _watch["last"]:
        _watch["quiet"] += 1
        if _watch["quiet"] == SAVE_AFTER_TICKS:
            write_record(level)
        return
    if values["reset"]:
        for field, value in start_values(get_base(look)).items():
            look.set_editor_property(field, value)
        values = values_of(look)
    _watch["last"] = values
    _watch["quiet"] = 0
    apply(look, level)


def register():
    """The editor's OFAST menu (OFAST > Look) and the tick callback that applies the panel live; once a session."""
    if _watch["handle"] is not None:
        return
    menus = unreal.ToolMenus.get()
    main = menus.find_menu("LevelEditor.MainMenu")
    if main:
        main.add_sub_menu("OFAST", "", "OFAST", "OFAST", "OFAST's tools for the round in this level")
        menu = menus.register_menu("LevelEditor.MainMenu.OFAST", "", unreal.MultiBoxType.MENU, False)
        entry = unreal.ToolMenuEntry(name="OFASTLook", type=unreal.MultiBlockType.MENU_ENTRY)
        entry.set_label("Look…")
        entry.set_tool_tip("The look of this map: sun, haze, camera, colours. Sliders, seen live in the viewport.")
        entry.set_string_command(unreal.ToolMenuStringCommandType.PYTHON, "",
                                 "import ofast_look; ofast_look.open_panel()")
        menu.add_menu_entry("Look", entry)
        menus.refresh_all_widgets()
    _watch["handle"] = unreal.register_slate_post_tick_callback(_tick)
    log("look: panel ready (OFAST > Look)")


def open_panel():
    """Opens the open level's Look panel (made the first time), applied to the level."""
    register()
    look = apply_level_look(create=True)
    if look is None:
        log("look: open an OFAST map's level first")
        return None
    _watch.update(level=level_path(), look=look, last=values_of(look), quiet=SAVE_AFTER_TICKS)
    unreal.get_editor_subsystem(unreal.AssetEditorSubsystem).open_editor_for_assets([look])
    return look
