"""Scopes and sights through the players' eyes (import_demo.py): what CS2 draws over the view while a player looks
through a sniper's scope or the AUG's and SG 553's sight (the cameras file's `scopes`, export::cameras::scope_spans;
docs/formats/cameras.md).

The zoom itself is the POV camera's field of view (the cameras file's eyes, keyed by ofast_viewmodel.key), and the
sight's pose the viewmodel's (anim::viewmodel, weapon_ironsight_amount). What lies over the view is drawn here, as
CS2 draws it after the world (checked on a local game's AWP, AUG and SG 553 at 2560 x 1440, 2026-11-05):

- A sniper's scope (the AWP, SSG 08, SCAR-20 and G3SG1; CS2 hides the gun meanwhile): materials/effects/
  legacy_sniper_scope.vmat, a shader on the whole screen with no textures (legacy_sniper_scope.vfx, decompiled): a
  lens of radius 0.425 x the screen's height, black outside it with an edge 0.03 high, darkened toward its rim
  (smoothstep(0.55, 0.2, r) of the picture kept) and crossed by two one-pixel black lines through the middle, the width
  and height of the screen (cl_crosshair_sniper_width 1). Its slight barrel warp and colour fringe at the rim are not
  drawn here.
- The AUG's and SG 553's sight (CS:GO's iron sight code): the gun brought up to the eye, the picture outside the
  sight's lens blurred (at a quarter of the resolution in CS2: about 8 pixels of a 1440-high picture, the gun's own
  edges too), the lens a little darker (scope_filter.vmat) and a small green dot in its middle (scope.vmat, the
  crosshair's colour: CS2's default green). The lens is the sight model's (a stencil in CS2); here a circle of
  SIGHT_LENS_RADIUS x the screen's height, where the sight's opening shows once ofast_viewmodel has brought it to the
  eye (SIGHT_BACK_IN), its rear glass see-through (see_through_lenses).

Each is a card in front of the player's POV camera (SCOPE_DISTANCE_CM: just past the near clip plane, in front of the
first-person gun) with a translucent material drawn from the screen's own pixels: shown only while the player looks
through it (keyed visibility, from their spans) and only through their eyes (the camera owns it: OnlyOwnerSee, as
their viewmodel). The Look panel's **Scope overlay** switch (ofast_look.py) sets the replay collection's
`ScopeOverlay` (1 on, 0 off), which both materials multiply their effect by.
"""

import unreal

import smoke_volumes

assets = unreal.EditorAssetLibrary
mel = unreal.MaterialEditingLibrary

SNIPER_MATERIAL = "/Game/OFAST/Replays/M_ScopeSniper"
SIGHT_MATERIAL = "/Game/OFAST/Replays/M_ScopeSight"
PLANE = "/Engine/BasicShapes/Plane.Plane"
# The card's distance in front of the camera (cm): past the project's near clip plane (2 cm, DefaultEngine.ini) and in
# front of the first-person gun, which FIRST_PERSON_SCALE brings toward the camera; its size (the engine plane's
# 100 cm times this): 40 cm across at 2.5 cm away covers fields of view up to 165 degrees.
SCOPE_DISTANCE_CM = 2.5
SCOPE_CARD_SCALE = 0.4

# legacy_sniper_scope.vfx at no inaccuracy: the lens's radius and its edge, in screen heights, the vignette's range.
SNIPER_HLSL = r"""
float2 px = (UV - 0.5) * Size;
float r = length(px) / Size.y;
float outside = smoothstep(0.425 - 0.03, 0.425, r);
float kept = smoothstep(0.55, 0.2, r);
float lines = max(saturate(1.5 - abs(px.x)), saturate(1.5 - abs(px.y)));
return saturate(max(outside, max(lines, 1.0 - kept))) * Strength;
"""

# The sight's lens: its radius in screen heights (CS2's AUG at viewmodel_fov 68: 0.257), how dark it is (the share of
# the picture scope_filter takes away), the dot's radius (screen heights: 8 pixels across at 1440) and colour, and the
# blur's two rings of taps (screen heights).
SIGHT_LENS_RADIUS = 0.257
SIGHT_FILTER = 0.3
SIGHT_DOT_RADIUS = 0.0028
SIGHT_DOT_COLOR = (0.15, 1.0, 0.15)
SIGHT_DOT_BRIGHTNESS = 6.0
SIGHT_BLUR = (0.004, 0.008)
SIGHT_TAPS = 12

SIGHT_HLSL = r"""
float2 px = (UV - 0.5) * Size;
float r = length(px) / Size.y;
float edge = 1.5 / Size.y;
float outside = smoothstep(LensRadius - edge, LensRadius + edge, r);
float3 blurred = (Center * 2.0 + T0 + T1 + T2 + T3 + T4 + T5 + T6 + T7 + T8 + T9 + T10 + T11) / 14.0;
float dot = saturate(1.0 - (r - DotRadius) * Size.y);
float3 color = blurred * outside + DotColor * DotBrightness * dot;
float alpha = max(outside, max(dot, (1.0 - outside) * Filter));
return float4(color / max(alpha, 1e-4), alpha * Strength);
"""


def _connect(source, output, target, input_name, what):
    if not mel.connect_material_expressions(source, output, target, input_name):
        raise RuntimeError("%s: could not connect %s" % (what, input_name))


def _overlay_material(path):
    """An empty translucent unlit material over everything (no depth test, after the depth of field)."""
    material = smoke_volumes.empty_material(path)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
    material.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    material.set_editor_property("two_sided", True)
    material.set_editor_property("disable_depth_test", True)
    material.set_editor_property("translucency_pass", unreal.MaterialTranslucencyPass.MTP_AFTER_DOF)
    return material


def _screen_inputs(material, custom, what):
    """The screen's UV, its size in pixels and the Look panel's switch into `custom`'s UV, Size and Strength."""
    screen = mel.create_material_expression(material, unreal.MaterialExpressionScreenPosition, -900, -200)
    _connect(screen, "ViewportUV", custom, "UV", what)
    size = mel.create_material_expression(material, unreal.MaterialExpressionViewSize, -900, -100)
    _connect(size, "", custom, "Size", what)
    strength = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -900, 0)
    strength.set_editor_property("collection", smoke_volumes.replay_collection())
    strength.set_editor_property("parameter_name", "ScopeOverlay")
    _connect(strength, "", custom, "Strength", what)


def sniper_material():
    """SNIPER_MATERIAL, rebuilt in place: black (emissive 0) as opaque as SNIPER_HLSL says."""
    material = _overlay_material(SNIPER_MATERIAL)
    custom = smoke_volumes.custom_node(material, SNIPER_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT1,
                                       ["UV", "Size", "Strength"], -400, 0)
    _screen_inputs(material, custom, "sniper scope")
    black = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -400, -200)
    black.set_editor_property("constant", unreal.LinearColor(0.0, 0.0, 0.0, 1.0))
    if not (mel.connect_material_property(black, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
            and mel.connect_material_property(custom, "", unreal.MaterialProperty.MP_OPACITY)):
        raise RuntimeError("sniper scope: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def sight_material():
    """SIGHT_MATERIAL, rebuilt in place: SIGHT_HLSL over the picture behind the card (SceneColor taps around each
    pixel, SIGHT_BLUR)."""
    import math

    material = _overlay_material(SIGHT_MATERIAL)
    names = ["UV", "Size", "Strength", "Center"] + ["T%d" % k for k in range(SIGHT_TAPS)]
    names += ["LensRadius", "Filter", "DotRadius", "DotColor", "DotBrightness"]
    custom = smoke_volumes.custom_node(material, SIGHT_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT4, names,
                                       -400, 0)
    _screen_inputs(material, custom, "sight")
    # The taps: SceneColor at offsets of the screen's fraction, ring radius (screen heights) x (1 / aspect, 1).
    size = mel.create_material_expression(material, unreal.MaterialExpressionViewSize, -1700, 300)
    height = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -1550, 300)
    height.set_editor_property("r", False)
    height.set_editor_property("g", True)
    height.set_editor_property("b", False)
    height.set_editor_property("a", False)
    _connect(size, "", height, "", "sight")
    # Pixels per screen height over pixels per axis: (height / width, 1).
    aspect = mel.create_material_expression(material, unreal.MaterialExpressionDivide, -1400, 300)
    _connect(height, "", aspect, "A", "sight")
    _connect(size, "", aspect, "B", "sight")
    center = mel.create_material_expression(material, unreal.MaterialExpressionSceneColor, -900, 150)
    _connect(center, "", custom, "Center", "sight")
    per_ring = SIGHT_TAPS // len(SIGHT_BLUR)
    for k in range(SIGHT_TAPS):
        ring = k // per_ring
        angle = 2.0 * math.pi * (k % per_ring + 0.5 * ring) / per_ring
        radius = SIGHT_BLUR[ring]
        offset = mel.create_material_expression(material, unreal.MaterialExpressionConstant2Vector, -1400,
                                                400 + 80 * k)
        offset.set_editor_property("r", radius * math.cos(angle))
        offset.set_editor_property("g", radius * math.sin(angle))
        scaled = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -1150, 400 + 80 * k)
        _connect(offset, "", scaled, "A", "sight")
        _connect(aspect, "", scaled, "B", "sight")
        tap = mel.create_material_expression(material, unreal.MaterialExpressionSceneColor, -900, 400 + 80 * k)
        # Its input read as an offset from the pixel, in fractions of the screen (its first input, whose name Unreal
        # gives by the mode).
        tap.set_editor_property("input_mode", unreal.MaterialSceneAttributeInputMode.OFFSET_FRACTION)
        _connect(scaled, "", tap, "", "sight")
        _connect(tap, "", custom, "T%d" % k, "sight")
    for index, (param, value) in enumerate((("LensRadius", SIGHT_LENS_RADIUS), ("Filter", SIGHT_FILTER),
                                            ("DotRadius", SIGHT_DOT_RADIUS),
                                            ("DotBrightness", SIGHT_DOT_BRIGHTNESS))):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900,
                                              -600 + 60 * index)
        node.set_editor_property("parameter_name", param)
        node.set_editor_property("default_value", value)
        _connect(node, "", custom, param, "sight")
    dot = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -900, -700)
    dot.set_editor_property("constant", unreal.LinearColor(*SIGHT_DOT_COLOR, 1.0))
    _connect(dot, "", custom, "DotColor", "sight")
    rgb = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -200, -100)
    alpha = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -200, 100)
    for node, channels in ((rgb, "rgb"), (alpha, "a")):
        for channel in "rgba":
            node.set_editor_property(channel, channel in channels)
        _connect(custom, "", node, "", "sight")
    if not (mel.connect_material_property(rgb, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
            and mel.connect_material_property(alpha, "", unreal.MaterialProperty.MP_OPACITY)):
        raise RuntimeError("sight: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


MATERIALS = {"scope": sniper_material, "sight": sight_material}
LABELS = {"scope": "Scope", "sight": "Sight"}
# The cards placed: both since the sight is brought to the eye (ofast_viewmodel's SIGHT_BACK_IN), its lens where the
# sight card's circle is.
PLACED = ("scope", "sight")
# The sight's card shows once the sight is up (ofast_viewmodel.SIGHT_UP_FRAMES): from the run's first frame its blur
# showed round the middle while the gun was still coming up.
SIGHT_DELAY_FRAMES = 6


def changes(spans, kind, end_frame):
    """(frame, visible) for a card of `kind` from a player's spans [[first tick, last tick, kind], ...] (frames); a
    sight's from SIGHT_DELAY_FRAMES into its run."""
    out = [(0, False)]
    delay = SIGHT_DELAY_FRAMES if kind == "sight" else 0
    for first, last, k in spans:
        if k == kind and first + delay <= last:
            out += [(first + delay, True), (min(last + 1, end_frame), False)]
    return out


def place_scopes(sequence, scopes, tick0, end_frame, cameras, names, spawn_tagged, key_visibility, own_view_only):
    """Each scoping player's cards on their POV camera (`cameras`: controller -> camera actor; `names`: controller ->
    name), keyed visible while they look through them (`scopes`: the cameras file's, controller (text) -> spans in
    ticks). `own_view_only(actor, controller)` makes it their view's only. Returns the number of cards."""
    built = {}
    placed = 0
    for controller_text, spans in (scopes or {}).items():
        controller = int(controller_text)
        camera = cameras.get(controller)
        if camera is None or not spans:
            continue
        frames = [[first - tick0, last - tick0, kind] for first, last, kind in spans]
        for kind in sorted({span[2] for span in frames}):
            if kind not in PLACED:
                continue
            if kind not in built:
                built[kind] = MATERIALS[kind]()
            card = spawn_tagged(unreal.load_asset(PLANE), "%s %s" % (LABELS[kind], names.get(controller, controller)))
            component = card.static_mesh_component
            component.set_mobility(unreal.ComponentMobility.MOVABLE)
            component.set_material(0, built[kind])
            component.set_editor_property("cast_shadow", False)
            component.set_editor_property("translucency_sort_priority", 100)
            keep = unreal.AttachmentRule.KEEP_RELATIVE
            card.attach_to_actor(camera, "", keep, keep, keep, False)
            # The plane faces +Z: turned to face along the camera's view (+X), a little in front of it.
            card.set_actor_relative_location(unreal.Vector(SCOPE_DISTANCE_CM, 0.0, 0.0), False, False)
            card.set_actor_relative_rotation(unreal.Rotator(roll=0.0, pitch=90.0, yaw=0.0), False, False)
            card.set_actor_relative_scale3d(unreal.Vector(SCOPE_CARD_SCALE, SCOPE_CARD_SCALE, 1.0))
            key_visibility(sequence.add_possessable(card), changes(frames, kind, end_frame), end_frame)
            own_view_only(card, controller)
            placed += 1
    return placed


# The first-person guns' sight glass: CS2's `shared_scope_lens` (the AUG's and SG 553's sights), which CS2's first
# person draws as a window the view shows through, a little darker (karrigan's AUG against CS2's view, 2026-11-05):
# the glTF's glass, opaque and shiny, hid the view through the sight brought to the eye. Black at LENS_OPACITY.
LENS_SLOTS = ("shared_scope_lens",)
LENS_MATERIAL = "/Game/OFAST/Replays/M_ViewScopeLens"
LENS_OPACITY = 0.3


def lens_material():
    """LENS_MATERIAL, rebuilt in place: translucent unlit black at LENS_OPACITY."""
    material = smoke_volumes.empty_material(LENS_MATERIAL)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
    material.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    material.set_editor_property("two_sided", True)
    black = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -400, -200)
    black.set_editor_property("constant", unreal.LinearColor(0.0, 0.0, 0.0, 1.0))
    opacity = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -400, 0)
    opacity.set_editor_property("r", LENS_OPACITY)
    if not (mel.connect_material_property(black, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
            and mel.connect_material_property(opacity, "", unreal.MaterialProperty.MP_OPACITY)):
        raise RuntimeError("scope lens: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def see_through_lenses(mesh, lens=None):
    """Gives a first-person skeletal mesh's sight glass slots (LENS_SLOTS) the see-through lens (lens_material).
    Returns the number of slots changed (the mesh is saved when any is)."""
    materials = list(mesh.get_editor_property("materials"))
    changed = 0
    for slot in materials:
        name = str(slot.get_editor_property("material_slot_name")).lower()
        if any(s in name for s in LENS_SLOTS):
            lens = lens or lens_material()
            slot.set_editor_property("material_interface", lens)
            changed += 1
    if changed:
        mesh.set_editor_property("materials", materials)
        assets.save_loaded_asset(mesh)
    return changed


def set_overlay(on):
    """The Look panel's Scope overlay: the replay collection's `ScopeOverlay` default (1 drawn, 0 not); saved when it
    changes."""
    collection = smoke_volumes.replay_collection()
    parameters = list(collection.get_editor_property("scalar_parameters"))
    value = 1.0 if on else 0.0
    changed = False
    for parameter in parameters:
        if str(parameter.get_editor_property("parameter_name")) == "ScopeOverlay" \
                and parameter.get_editor_property("default_value") != value:
            parameter.set_editor_property("default_value", value)
            changed = True
    if changed:
        collection.set_editor_property("scalar_parameters", parameters)
        assets.save_loaded_asset(collection)
    return changed


def scoped_frames(spans, tick0):
    """The frames on which a player's sniper scope hides their gun (CS2's m_bHideViewModelWhenZoomed): within a
    `scope` span."""
    out = set()
    for first, last, kind in spans or []:
        if kind == "scope":
            out.update(range(first - tick0, last - tick0 + 1))
    return out
