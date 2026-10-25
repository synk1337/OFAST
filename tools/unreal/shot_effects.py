"""Gun shots in Unreal (M6): muzzle flashes, tracers, impacts on the world and on players, bullet holes
(docs/formats/replay.md, "In Unreal"; the agents manifest's `shots` and `decals`, written by export::shots). Used by
import_demo.py.

CS2 draws these with particle systems: a gun's muzzle flash comes from its shoot clip's particle event
(particles/unified_weapon_fx/uweapon_muzflsh_*.vpcf on the world model's `muzzle_flash` attachment), its tracer from
scripts/weapons.vdata (particles/weapons/cs_weapon_fx/weapon_tracers_*.vpcf, one shot in m_nTracerFrequency), an
impact from the surface met (scripts/surfaceproperties_impact_effects.txt: particles/impact_fx/impact_<surface>.vpcf,
and a bullet hole from its decal group), a hit on a player is blood (particles/blood_impact/). OFAST rebuilds their
main layers with grenade_effects' "particle system in a material" (CS2's own sprite sheets, driven by the demo tick):

- One actor for a round's shots ("shots"), one instanced mesh per layer: a slot per shot for its muzzle flash (at its
  muzzle, flying along its barrel), per tracer (from the muzzle to where its bullet stopped, as fast as CS2's), per
  bullet that met the world (its surface family's impact, flying out along the surface's normal) or a player (blood,
  back toward the shooter; and the impact of a wall it went through first). A silenced shot has no flame and no light,
  only its smoke.
- A point light per shooter, moved to each shot's muzzle and pulsed for a few ticks (CS2's muzzle flashes light the
  world briefly: their `fakelight` child).
- A decal actor per bullet hole: one of its decal group's textures, at the bullet's end, facing the surface; it shows
  from its shot's tick on. The tick rides in the decal's colour (DecalColor's red), read by the material: one material
  instance per texture, not per hole. The holes the shots before the round left (the agents manifest's
  `earlier_holes`) show from the start: CS2's clients keep them from round to round.
"""

import math
import random

import unreal

import grenade_effects
import smoke_volumes

UNITS_TO_CM = grenade_effects.UNITS_TO_CM
FOLDER = grenade_effects.FOLDER
BULLET_HOLE_MATERIAL = "/Game/OFAST/Replays/M_BulletHole"

# Sprite layers (grenade_effects' layer tables: CS2's ranges, read 2026-10-25). Muzzle flashes, along the barrel:
# particles/unified_weapon_fx/uweapon_muzflsh_ak47_primaryflash: 8 sprites of fire_gas_batch_b_top (one of its 4
# sequences), 0.05 s, born 3 - 20 units along the barrel and flying on at 100 - 150 units/s, colour (198, 131, 80) to
# (216, 216, 216) of 255 at 4 x overbright, alpha 0.24 - 0.59, radius 1 - 1.5 x a curve of 2.7 - 10 along the flash x
# 0.5. In full at once (a 0.05 s flash is three ticks: faded in over a tenth of it, its first tick would be dark).
# ..._gen_smoke: 2 puffs of smokeloop_i_0_sc_hardedge, 0.2 - 0.5 s, alpha 0.15 - 0.3.
MUZZLE_LAYERS = [
    {"name": "muzzle_flash", "role": "muzzle_flash", "slots": 8, "life": (0.05, 0.05), "radius": (2.7, 15.0),
     "scale": (1.0, 1.0), "alpha": (0.24, 0.59), "roll": 180.0, "spread": 0.5, "rise": 0.0, "bright": 4.0,
     "tint": (0.82, 0.67, 0.55), "size": 0.5, "lift": 0.0, "along": (3.0, 20.0), "up": (100.0, 150.0),
     "fade_in": 0.0, "fade_out": 0.3},
    {"name": "muzzle_smoke", "role": "muzzle_smoke", "slots": 2, "life": (0.2, 0.5), "radius": (4.0, 8.0),
     "scale": (0.6, 1.6), "alpha": (0.15, 0.3), "roll": 180.0, "spread": 1.0, "rise": 2.0, "bright": 1.0,
     "tint": (0.6, 0.6, 0.6), "translucent": True, "size": 1.0, "lift": 0.0, "along": (2.0, 8.0),
     "up": (10.0, 30.0)},
]
# Tracers (particles/weapons/cs_weapon_fx/weapon_tracers_*): one particle flying from the muzzle to where the bullet
# stopped (C_INIT_MoveBetweenPoints) at the tracer's speed (units/s), drawn as a trail of sparks.vtex's streak
# (sequence 4) as long as 0.085 - 0.1 s of its flight, at most `max_length` units, its colour (255, 68, 21) to
# (255, 165, 0) to white, additive, alpha 0.63 - 0.69, about a pixel wide at the least (0.00075 - 0.002 of the
# screen: MinWidth here, as a share of its distance). The streak here runs from its tail to its head and stops at the
# bullet's end, its tail catching up.
TRACERS = {"weapon_tracers_assrifle": (20500.0, 1200.0), "weapon_tracers_rifle": (30000.0, 900.0),
           "weapon_tracers_pistol": (18000.0, 1200.0), "weapon_tracers_shot": (24250.0, 1200.0),
           "weapon_tracers_smg": (20000.0, 1200.0)}
TRACER_DEFAULT = "weapon_tracers_assrifle"


def tracer_layer(kind):
    """The sprite layer of a tracer kind (TRACERS)."""
    speed, max_length = TRACERS[kind]
    return {"name": kind, "role": "sparks", "slots": 1, "life": (1.0, 1.0), "radius": (1.25, 1.9),
            "scale": (1.0, 1.0), "alpha": (0.63, 0.69), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 3.0,
            "tint": (1.0, 0.62, 0.3), "size": 1.0, "lift": 0.0, "up": (speed, speed), "stretch": 0.0925,
            "max_length": max_length, "sequence": (4, 1), "fade_in": 0.0, "fade_out": 0.0, "min_width": 0.001}


TRACER_LAYERS = [tracer_layer(kind) for kind in TRACERS]


# Impacts, flying out along the surface's normal. particles/impact_fx/impact_concrete: 1 - 4 puffs of smokeloop_i_0_sc
# (0.75 - 1.5 s, alpha 0.16 - 0.63), a wispy_steam_set puff (radius 4 - 12, growing to at most 30), 8 flecks thrown
# out (models/particle/flecks3.vmdl, radius 0.75 - 1.5, speed +-100 across and 50 - 350 out, gravity 500: drawn as
# fleks.vtex sprites here) and 2 sparks. Tinted per surface family (CS2 has one system per surface, alike in shape).
def dust_layers(family, tint):
    """The layers of a dusty impact (concrete, plaster, tile, dirt, wood) in `tint`."""
    r, g, b = tint
    return [
        {"name": "impact_dust_" + family, "role": "impact_smoke", "slots": 2, "life": (0.75, 1.5),
         "radius": (6.0, 12.0), "scale": (0.5, 1.6), "alpha": (0.16, 0.63), "roll": 180.0, "spread": 2.0, "rise": 0.0,
         "bright": 1.0, "tint": (r, g, b), "translucent": True, "size": 1.0, "lift": 0.0, "up": (15.0, 60.0),
         "speed": (0.0, 15.0), "gravity": -5.0, "fade_in": 0.05},
        {"name": "impact_puff_" + family, "role": "fire_smoke", "slots": 1, "life": (0.4, 0.8), "radius": (4.0, 12.0),
         "scale": (1.0, 2.5), "alpha": (0.4, 0.6), "roll": 180.0, "spread": 0.0, "rise": 0.0, "bright": 1.0,
         "tint": (r, g, b), "translucent": True, "size": 1.0, "lift": 0.0, "up": (20.0, 40.0), "fade_in": 0.05},
        {"name": "impact_flecks_" + family, "role": "debris", "slots": 8, "life": (0.6, 1.2), "radius": (0.75, 1.5),
         "scale": (1.0, 1.0), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 1.0, "rise": 0.0, "bright": 1.0,
         "tint": (r * 0.8, g * 0.8, b * 0.8), "translucent": True, "size": 1.0, "lift": 0.0, "speed": (0.0, 140.0),
         "up": (50.0, 350.0), "gravity": 500.0, "sequence": (15, 25), "fade_in": 0.0},
    ]


# particles/impact_fx/impact_metal: 8 - 32 spark trails (sparks.vtex, 0.025 - 0.2 s), a yellowflare glow (0.1 s) and
# a puff of slate smoke.
METAL_LAYERS = [
    {"name": "impact_sparks_metal", "role": "sparks", "slots": 16, "life": (0.025, 0.2), "radius": (0.5, 1.0),
     "scale": (1.0, 0.3), "alpha": (1.0, 1.0), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 2.0,
     "tint": (1.0, 0.75, 0.45), "size": 1.0, "lift": 0.0, "speed": (100.0, 500.0), "up": (100.0, 450.0),
     "gravity": 400.0, "stretch": 0.05, "max_length": 25.0, "sequence": (4, 1), "fade_in": 0.0},
    {"name": "impact_glow_metal", "role": "impact_glow", "slots": 1, "life": (0.1, 0.1), "radius": (6.0, 10.0),
     "scale": (1.0, 0.5), "alpha": (1.0, 1.0), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 2.5,
     "tint": (1.0, 0.8, 0.5), "size": 1.0, "lift": 0.0, "along": (1.0, 1.0), "fade_in": 0.0},
    {"name": "impact_smoke_metal", "role": "impact_smoke", "slots": 1, "life": (0.5, 1.0), "radius": (4.0, 8.0),
     "scale": (0.5, 1.5), "alpha": (0.2, 0.4), "roll": 180.0, "spread": 0.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.42, 0.44, 0.47), "translucent": True, "size": 1.0, "lift": 0.0, "up": (10.0, 40.0)},
]
# Each impact system of export::shots' `impact` (particles/impact_fx/<name>.vpcf) -> the family drawn; any other is
# concrete's.
IMPACT_FAMILIES = {
    "impact_concrete": "concrete", "impact_tile": "concrete", "impact_plaster": "plaster", "impact_dirt": "dirt",
    "impact_grass": "dirt", "impact_wood": "wood", "impact_glass": "plaster", "impact_metal": "metal",
    "impact_metal_grate": "metal", "impact_metal_vent": "metal",
}
FAMILY_LAYERS = {
    "concrete": dust_layers("concrete", (0.62, 0.6, 0.57)),
    "plaster": dust_layers("plaster", (0.75, 0.74, 0.7)),
    "dirt": dust_layers("dirt", (0.45, 0.37, 0.28)),
    "wood": dust_layers("wood", (0.5, 0.38, 0.25)),
    "metal": METAL_LAYERS,
}
# particles/blood_impact/blood_impact_medium's mist (blood_impact_mist1): 8 blood_core sprites, 0.5 s, radius 12 - 16
# grown from a quarter, out along its axis at 60 - 132 units/s (back toward the shooter: sprite_slots) and 4 - 9 any
# way, drag 0.2, gravity 215, dark red
# (95 - 136 of 255), alpha 0.67 - 0.86, faded in over its first eighth and out from a fifth on.
# The radius is the sprite's half-size here, grown from 0.6 (blood_core is a sparse white splash, a tenth opaque on
# average: at half that size and from a quarter of it, the mist on a player 8 ticks after a hit was a faint speck where
# CS2 shows a dark red splatter over his chest; molodoy's view at tick 40840 of de_nuke round 5).
BLOOD_LAYERS = [
    {"name": "blood_mist", "role": "blood", "slots": 8, "life": (0.5, 0.5), "radius": (12.0, 16.0),
     "scale": (0.6, 1.2), "alpha": (0.67, 0.86), "roll": 180.0, "spread": 1.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.5, 0.02, 0.02), "translucent": True, "size": 1.0, "lift": 0.0, "up": (60.0, 132.0),
     "speed": (4.0, 9.0), "drag": 0.2, "gravity": 215.0, "fade_in": 0.125, "fade_out": 0.8},
]
# A muzzle flash's light (uweapon_muzflsh_*_fakelight: colour (226, 125, 37), radius about 185 units, 0.05 s): peak
# intensity (candela) and how it dies out, (ticks after the shot, share of the peak). In CS2 it only tints what is
# next to the barrel (the door edge beside YEKINDAR's AK one tick after his shot, de_nuke round 5, tick 37950; around
# molodoy in a chase camera one tick after his, 40840, nothing shows): 40 cd lit the floor and silos round molodoy
# plainly orange.
MUZZLE_LIGHT = {"color": (0.89, 0.49, 0.15), "peak": 8.0, "radius": 185.0,
                "decay": [(0, 1.0), (1, 0.4), (3, 0.0)]}
# Bullet holes: how far the decal's box reaches each way from the surface (units: CS2's DecalDepth is 12 through), and
# the random size factor over DecalWorldWidth / Height (CS2's DecalSizeVariance 0.5 and DecalBoxGrowth 2 are not
# understood): CS2's holes beside de_nuke's A sign, 449 units away, measure about 11 units across where concrete's
# width is 5.8.
HOLE_DEPTH = 4.0
HOLE_SIZE = (1.5, 2.1)

assets = unreal.EditorAssetLibrary
mel = unreal.MaterialEditingLibrary


def log(message):
    unreal.log("OFAST: " + message)


def shot_layers():
    """Every sprite layer of the shots."""
    layers = MUZZLE_LAYERS + TRACER_LAYERS + BLOOD_LAYERS
    for family_layers in FAMILY_LAYERS.values():
        layers += family_layers
    return layers


def family_of(bullet):
    return IMPACT_FAMILIES.get(bullet.get("impact") or "", "concrete")


def unit(v):
    length = math.sqrt(sum(c * c for c in v))
    return [c / length for c in v] if length > 1e-6 else [0.0, 0.0, 1.0]


def sprite_slots(shots):
    """The sprite slots (grenade_effects.add_sprites) of every shot: (shooter controller -> layer name -> slots of
    their muzzle flashes, not the silenced shots' flame; layer name -> slots of the tracers, impacts by surface family
    and blood)."""
    muzzles = {}
    slots = {}

    def add(layers, slot, into):
        for layer in layers:
            into.setdefault(grenade_effects.layer_name(layer), []).append(slot)

    for shot in shots:
        tick = shot["tick"]
        muzzle, barrel = shot.get("muzzle"), shot.get("muzzle_direction")
        if muzzle and barrel:
            layers = MUZZLE_LAYERS[1:] if shot.get("silenced") else MUZZLE_LAYERS
            add(layers, (muzzle, tick, tick, barrel, 0.0), muzzles.setdefault(shot["controller"], {}))
        start = muzzle or shot["eye"]
        for bullet in shot["bullets"]:
            end = bullet["end"]
            if shot.get("tracer"):
                kind = shot["tracer"] if shot["tracer"] in TRACERS else TRACER_DEFAULT
                way = [e - s for e, s in zip(end, start)]
                reach = math.sqrt(sum(c * c for c in way))
                if reach > 1.0:
                    add([tracer_layer(kind)], (start, tick, tick, unit(way), reach), slots)
            if bullet["on"] == "world" and bullet.get("normal"):
                add(FAMILY_LAYERS[family_of(bullet)], (end, tick, tick, bullet["normal"], 0.0), slots)
            elif bullet["on"] == "player":
                # Back toward the shooter: CS2's mist flies along its effect's own axis, and the shooter sees it in
                # front of the player hit (molodoy's view at tick 40840 of de_nuke round 5; along the bullet it hid
                # behind the body).
                add(BLOOD_LAYERS, (end, tick, tick, unit([s - e for e, s in zip(end, shot["eye"])]), 0.0), slots)
                # A wall it went through first: its impact there too.
                wall = bullet.get("wall")
                if wall and wall.get("normal"):
                    add(FAMILY_LAYERS[family_of(wall)], (wall["end"], tick, tick, wall["normal"], 0.0), slots)
    return muzzles, slots


def light_frames(shots, tick0, end_frame):
    """Per shooter, the muzzle light's keys, one per frame from the frame before a shot to the end of its pulse:
    [(frame, position or None, intensity)], for the shots with a muzzle that are not silenced. A frame's intensity is
    the brightest of the pulses (MUZZLE_LIGHT) of the shooter's shots then, so shots closer together than a pulse
    (a burst, a Negev) do not cut each other short; the position is keyed on each shot's own frame."""
    by_shooter = {}
    for shot in shots:
        if shot.get("muzzle") and not shot.get("silenced"):
            by_shooter.setdefault(shot["controller"], []).append((shot["tick"] - tick0, shot["muzzle"]))
    last = MUZZLE_LIGHT["decay"][-1][0]
    out = {}
    for controller, own in by_shooter.items():
        frames = sorted({f for at, _ in own for f in range(at - 1, at + last + 1) if 0 <= f <= end_frame})
        keys = []
        for frame in frames:
            intensity = max(grenade_effects.pulse_intensity(MUZZLE_LIGHT, frame - at) for at, _ in own
                            if at - 1 <= frame <= at + last)
            position = next((muzzle for at, muzzle in own if at == frame), None)
            keys.append((frame, position, intensity))
        out[controller] = keys
    return out


def place_lights(sequence, replay, shots, end_frame, spawn, hide):
    """A muzzle light per shooter (light_frames): movable (a location track on a light that is not is refused in a
    game world, which Movie Render Queue renders), its location keyed (constant) at each shot's muzzle, its intensity
    pulsed, hidden with the shooter's body (`hide`: see place_shots). Returns the number of lights."""
    tick0 = replay["start_tick"]
    names = {p["controller"]: p["name"] for p in replay["players"]}
    constant = unreal.MovieSceneKeyInterpolation.CONSTANT
    lights = light_frames(shots, tick0, end_frame)
    for controller, keys in lights.items():
        first = next((p for _, p, _ in keys if p), None)
        if first is None:
            continue
        light = grenade_effects.spawn_light(spawn, "muzzle light %s" % names.get(controller, controller), first,
                                            MUZZLE_LIGHT["color"], MUZZLE_LIGHT["radius"])
        component = light.get_editor_property("light_component")
        component.set_mobility(unreal.ComponentMobility.MOVABLE)
        component.set_editor_property("cast_shadows", False)
        # The actor's binding (its location), then its light component's (its intensity), as intensity_channel does.
        binding = sequence.add_possessable(light)
        intensity = sequence.add_possessable(component).add_track(unreal.MovieSceneFloatTrack)
        intensity.set_property_name_and_path("Intensity", "Intensity")
        intensity_section = intensity.add_section()
        intensity_section.set_range(0, end_frame)
        channel = intensity_section.get_all_channels()[0]
        track = binding.add_track(unreal.MovieScene3DTransformTrack)
        section = track.add_section()
        section.set_range(0, end_frame)
        location = {str(c.channel_name): c for c in section.get_all_channels()}
        for axis in ("X", "Y", "Z"):
            location["Scale." + axis].set_default(1.0)
        previous = None
        for frame, position, intensity in sorted(keys, key=lambda k: k[0]):
            if frame < 0:
                continue
            if position is not None:
                x, y, z = position
                for axis, value in zip(("X", "Y", "Z"), (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM)):
                    location["Location." + axis].add_key(unreal.FrameNumber(frame), value, 0.0,
                                                         unreal.MovieSceneTimeUnit.DISPLAY_RATE, constant)
            if (frame, intensity) != previous:
                grenade_effects.add_key(channel, frame, intensity)
                previous = (frame, intensity)
        hide(binding, controller)
    return len(lights)


BULLET_HOLE_HLSL = r"""
// The hole shows from its shot's tick on (the tick rides in the decal's colour).
return Tick >= FirstTick - 0.5 ? 1.0 : 0.0;
"""


def bullet_hole_material():
    """BULLET_HOLE_MATERIAL, rebuilt in place: a translucent deferred decal, the texture's colour and alpha (`Tex`), shown
    from the tick in its decal colour's red (BULLET_HOLE_HLSL)."""
    material = smoke_volumes.empty_material(BULLET_HOLE_MATERIAL)
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_DEFERRED_DECAL)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
    show = smoke_volumes.custom_node(material, BULLET_HOLE_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT1,
                                     ["Tick", "FirstTick"], -400, 0)
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -900, -100)
    tick.set_editor_property("collection", smoke_volumes.replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    mel.connect_material_expressions(tick, "", show, "Tick")
    decal_color = mel.create_material_expression(material, unreal.MaterialExpressionDecalColor, -900, 0)
    first_tick = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -700, 0)
    first_tick.set_editor_property("r", True)
    for channel in ("g", "b", "a"):
        first_tick.set_editor_property(channel, False)
    mel.connect_material_expressions(decal_color, "", first_tick, "")
    mel.connect_material_expressions(first_tick, "", show, "FirstTick")
    texture = mel.create_material_expression(material, unreal.MaterialExpressionTextureSampleParameter2D, -700, -400)
    texture.set_editor_property("parameter_name", "Tex")
    texture.set_editor_property("texture", assets.load_asset("/Engine/EngineResources/DefaultTexture"))
    # The hole's depth darkens it: CS2 draws the depth with parallax and occlusion, and its holes read black where the
    # colour texture is grey concrete dust. The height map is imported as sRGB, so it samples as about height^2.2;
    # squared, about height^4.4: black in the hole and dark round it, the surface untouched (with height^2.2 only the
    # hole's middle was dark: half as wide as CS2's beside de_nuke's A sign).
    depth = mel.create_material_expression(material, unreal.MaterialExpressionTextureSampleParameter2D, -900, -650)
    depth.set_editor_property("parameter_name", "Height")
    depth.set_editor_property("texture", assets.load_asset("/Engine/EngineResources/WhiteSquareTexture"))
    deeper = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -650, -650)
    mel.connect_material_expressions(depth, "R", deeper, "A")
    mel.connect_material_expressions(depth, "R", deeper, "B")
    colour = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -400, -500)
    mel.connect_material_expressions(texture, "RGB", colour, "A")
    mel.connect_material_expressions(deeper, "", colour, "B")
    opacity = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -150, -200)
    mel.connect_material_expressions(texture, "A", opacity, "A")
    mel.connect_material_expressions(show, "", opacity, "B")
    rough = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -400, -300)
    rough.set_editor_property("r", 0.9)
    prop = unreal.MaterialProperty
    if not (mel.connect_material_property(colour, "", prop.MP_BASE_COLOR)
            and mel.connect_material_property(rough, "", prop.MP_ROUGHNESS)
            and mel.connect_material_property(opacity, "", prop.MP_OPACITY)):
        raise RuntimeError("bullet hole material: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def hole_instances(replay_path, decals):
    """Decal group -> [(instance, width, height, probability)]: the bullet holes' textures (the agents manifest's
    `decals`) imported, an instance of BULLET_HOLE_MATERIAL each (shared by every round: CS2's holes are the same)."""
    if not decals:
        return {}
    material = bullet_hole_material()
    out = {}
    for group, options in decals.items():
        for option in options:
            texture = smoke_volumes.import_png(replay_path, option["file"], FOLDER)
            assets.save_loaded_asset(texture)
            name = "MI_BulletHole_" + option["file"].rsplit(".", 1)[0]
            instance = smoke_volumes.material_instance(FOLDER, name, material)
            mel.set_material_instance_texture_parameter_value(instance, "Tex", texture)
            if option.get("height_file"):
                height = smoke_volumes.import_png(replay_path, option["height_file"], FOLDER)
                assets.save_loaded_asset(height)
                mel.set_material_instance_texture_parameter_value(instance, "Height", height)
            assets.save_loaded_asset(instance)
            out.setdefault(group, []).append((instance, option["width"], option["height"],
                                              option.get("probability", 1.0)))
    return out


def place_hole(spawn, label, tick, end, normal, options, rng):
    """One bullet hole: a decal actor at `end` facing the surface of `normal` (Source axes), a random option of its
    group (`options`: hole_instances' list), size and roll; its decal colour's red is `tick`, from which it shows."""
    instance, width, height, _ = rng.choices(options, weights=[o[3] for o in options])[0]
    size = rng.uniform(*HOLE_SIZE)
    nx, ny, nz = normal
    facing = unreal.MathLibrary.make_rot_from_x(unreal.Vector(-nx, ny, -nz))
    x, y, z = end
    actor = spawn(unreal.DecalActor, label, (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
    actor.set_actor_rotation(unreal.Rotator(roll=rng.uniform(0.0, 360.0), pitch=facing.pitch, yaw=facing.yaw), False)
    component = actor.get_editor_property("decal")
    component.set_editor_property("decal_size", unreal.Vector(
        HOLE_DEPTH * UNITS_TO_CM, width * 0.5 * size * UNITS_TO_CM, height * 0.5 * size * UNITS_TO_CM))
    component.set_decal_material(instance)
    component.set_decal_color(unreal.LinearColor(r=float(tick), g=0.0, b=0.0, a=1.0))
    # Never faded out by size: Unreal drops a decal under 1 % of the screen by default, and CS2 still shows a hole 11
    # units across from 449 units away (beside de_nuke's A sign) that Unreal's default left out.
    component.set_fade_screen_size(0.0)


def place_holes(shots, earlier, instances, spawn):
    """A bullet hole (place_hole) per bullet of `shots` that met the world with a decal group (`instances`:
    hole_instances), showing from its shot's tick, and per hole of `earlier` (the agents manifest's `earlier_holes`:
    left by the shots before the round, which CS2 keeps), showing from the round's start. Returns the number
    placed."""
    placed = 0
    for shot in shots:
        for k, bullet in enumerate(shot["bullets"]):
            # Where it met the world: its end, or the wall it went through on its way to a player.
            at = bullet if bullet["on"] == "world" else bullet.get("wall") or {}
            options = instances.get(at.get("decal") or "")
            if not options or not at.get("normal"):
                continue
            rng = random.Random(shot["tick"] * 7919 + shot["controller"] * 104729 + k)
            place_hole(spawn, "bullet hole %d %d.%d" % (shot["controller"], shot["tick"], k), shot["tick"],
                       at["end"], at["normal"], options, rng)
            placed += 1
    for k, hole in enumerate(earlier):
        options = instances.get(hole.get("decal") or "")
        if not options or not hole.get("normal"):
            continue
        rng = random.Random(hole["tick"] * 7919 + k)
        place_hole(spawn, "earlier bullet hole %d.%d" % (hole["tick"], k), hole["tick"], hole["end"], hole["normal"],
                   options, rng)
        placed += 1
    return placed


def place_shots(sequence, replay, replay_path, end_frame, agents, spawn, hide):
    """Everything above for a round, from the agents manifest `agents` (its `shots`, `earlier_holes`, `effects` and
    `decals`; without them: nothing): an actor per shooter for their muzzle flashes and their muzzle light, hidden
    with their body (`hide(binding, controller)` keys the body's visibility on a sequence binding: not while the camera
    looks through their own eyes, where CS2 draws the first-person flash instead), one ("shots") for the rest, and the
    bullet holes. Returns the number of shots."""
    shots = agents.get("shots", [])
    effects = agents.get("effects", [])
    if not shots:
        holes = place_holes([], agents.get("earlier_holes", []),
                            hole_instances(replay_path, agents.get("decals", {})), spawn)
        if holes:
            log("shots: none; %d earlier bullet holes" % holes)
        return 0
    rebuild = not assets.does_asset_exist(grenade_effects.SPRITE_MATERIALS[False])
    instances = grenade_effects.update_layers(replay, replay_path, effects, shot_layers(), rebuild) if effects else {}
    mesh = grenade_effects.sprite_mesh()
    rng = random.Random(replay["round"] * 7349 + len(shots))
    layers = {grenade_effects.layer_name(layer): layer for layer in shot_layers()}
    names = {p["controller"]: p["name"] for p in replay["players"]}
    muzzles, shared = sprite_slots(shots)
    drawn = 0

    def draw(actor, by_layer):
        count = 0
        for name, slots in sorted(by_layer.items()):
            if name in instances:
                grenade_effects.add_sprites(actor, layers[name], instances[name], mesh, slots, rng)
                count += len(slots)
        return count

    for controller, by_layer in sorted(muzzles.items()):
        actor = spawn(unreal.Actor, "muzzle flashes %s" % names.get(controller, controller), (0.0, 0.0, 0.0))
        drawn += draw(actor, by_layer)
        hide(sequence.add_possessable(actor), controller)
    drawn += draw(spawn(unreal.Actor, "shots", (0.0, 0.0, 0.0)), shared)
    lights = place_lights(sequence, replay, shots, end_frame, spawn, hide)
    holes = place_holes(shots, agents.get("earlier_holes", []), hole_instances(replay_path, agents.get("decals", {})),
                        spawn)
    log("shots: %d (%d sprite slots, %d muzzle lights, %d bullet holes)" % (len(shots), drawn, lights, holes))
    return len(shots)
