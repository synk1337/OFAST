"""Gun shots in Unreal (M6): muzzle flashes, tracers, impacts on the world and on players, bullet holes
(docs/formats/replay.md, "In Unreal"; the agents manifest's `shots` and `decals`, written by export::shots). Used by
import_demo.py.

CS2 draws these with particle systems: a gun's muzzle flash comes from its shoot clip's particle event
(particles/unified_weapon_fx/uweapon_muzflsh_*.vpcf on the world model's `muzzle_flash` attachment), its tracer from
scripts/weapons.vdata (particles/weapons/cs_weapon_fx/weapon_tracers_*.vpcf, on every shot whose m_nTracerFrequency
is not 0: export::shots' `tracer`), an
impact from the surface met (scripts/surfaceproperties_impact_effects.txt: particles/impact_fx/impact_<surface>.vpcf,
and a bullet hole from its decal group), a hit on a player is blood (particles/blood_impact/). OFAST rebuilds their
main layers with grenade_effects' "particle system in a material" (CS2's own sprite sheets, driven by the demo tick):

- Instanced meshes of sprite layers, a slot per particle burst. Per shooter an actor with their muzzle flashes (at
  the muzzle, flying along the barrel) and tracers (from the muzzle to where the bullet stopped, leaving when the
  shot was fired, as fast as CS2's), hidden while the camera looks through their eyes; one actor for the rest
  ("shots"): per place a bullet met the world its surface family's impact (flying out along the surface's normal:
  both sides of each wall it went through, and where it stopped) and per player it met blood (back toward the
  shooter). A silenced shot has no flame and no light, only its smoke.
- In the shooter's own view CS2 draws the first-person system instead (particles/unified_weapon_fx/*_fps.vpcf): the
  same layers smaller, on the viewmodel's barrel. Here they are instanced meshes on the shooter's viewmodel actor
  (shown and hidden with it, moved and mirrored with it), drawn with Unreal's first-person rendering as the viewmodel
  is. Their tracers there leave from where the viewmodel's barrel shows (export::shots' `view_tracer`), on an actor
  of their own shown with the viewmodel.
- A point light per shooter, moved to each shot's muzzle and pulsed for a few ticks (CS2's muzzle flashes light the
  world briefly: their `fakelight` child), hidden with the body; and one on the viewmodel for their own view (the
  first-person system has its own).
- A decal actor per bullet hole: one of its decal group's textures, where the bullet met the world (both sides of a
  wall it went through), facing the surface; it shows
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

# Sprite layers (grenade_effects' layer tables: CS2's ranges, read 2026-10-25). Muzzle flashes, along the barrel.
# particles/unified_weapon_fx/uweapon_muzflsh_ak47 plays one flame child per shot (C_OP_ChooseRandomChildrenInGroup,
# group 1: _primaryflash, _primaryflash_alt listed twice, or an empty one with a beam; here the first two, one in three
# and two in three, picked by the shot: FLAME_CHOICE), its smoke and its light always.
# _primaryflash: 8 sprites of fire_gas_batch_b_top (one of its 4 sequences), 0.05 s, born 3 - 20 units along the
# barrel and flying on at 100 - 150 units/s, colour (198, 131, 80) to (216, 216, 216) of 255 at 4 x overbright, alpha
# 0.24 - 0.59, radius 1 - 1.5 x a curve of 2.7 - 10 along the flash x 0.5. In full at once (a 0.05 s flash is three
# ticks: faded in over a tenth of it, its first tick would be dark).
MUZZLE_FLAME = {"name": "muzzle_flash", "role": "muzzle_flash", "slots": 8, "life": (0.05, 0.05),
                "radius": (2.7, 15.0), "scale": (1.0, 1.0), "alpha": (0.24, 0.59), "roll": 180.0, "spread": 0.5,
                "rise": 0.0, "bright": 4.0, "tint": (0.82, 0.67, 0.55), "size": 0.5, "lift": 0.0, "along": (3.0, 20.0),
                "up": (100.0, 150.0), "fade_in": 0.0, "fade_out": 0.3}
# _primaryflash_alt: wispy tongues of flame, 6 sprites of wispy_steam_burst_b (one of 4 sequences), 0.05 - 0.095 s,
# rolled -60 to -120 degrees, thrown out along the barrel at 500 units/s (drag 0.2), alpha 0.71 - 0.78, faded out over
# its last 0.015 s; the same colours and overbright, times its renderer's colour scale (205, 133, 63) of 255: the grey
# wisps burn orange, their bright middles yellow-white (without it they were white smoke beside CS2's orange flames).
# Fitted to CS2's first-person flash (YEKINDAR's and molodoy's AK one tick after a shot, de_nuke round 5, ticks 37950
# and 40840: its wispy tongues show CS2 drew this flame in both): its radius, 3.2 - 9.7 (0.35 of it in first person),
# where its initializers read as 0.75 - 1 x 0.35 - 0.8 (by particle) of the default radius 5, which drew the tongues
# at 0.4 of CS2's size; and twice
# CS2's overbright: the sheet is white with a faint alpha (median 0.15), and at 4 its tongues were a beige haze where
# CS2's are bright flames (its renderer adds the texture's own colour too, m_flAddSelfAmount 1, which the sprite
# material does not model).
MUZZLE_ALT = {"name": "muzzle_alt", "role": "muzzle_alt", "slots": 6, "life": (0.05, 0.095), "radius": (3.2, 9.7),
              "scale": (1.0, 1.0), "alpha": (0.71, 0.78), "roll": 30.0, "roll_base": -90.0, "spread": 0.5,
              "rise": 0.0, "bright": 8.0, "tint": (0.66, 0.35, 0.14), "size": 1.0, "lift": 0.0, "along": (0.0, 1.0),
              "up": (500.0, 500.0), "drag": 0.2, "fade_in": 0.0, "fade_out": 0.2}
# _gen_smoke: 2 puffs of smokeloop_i_0_sc_hardedge, 0.2 - 0.5 s, alpha 0.15 - 0.3.
MUZZLE_SMOKE = {"name": "muzzle_smoke", "role": "muzzle_smoke", "slots": 2, "life": (0.2, 0.5), "radius": (4.0, 8.0),
                "scale": (0.6, 1.6), "alpha": (0.15, 0.3), "roll": 180.0, "spread": 1.0, "rise": 2.0, "bright": 1.0,
                "tint": (0.6, 0.6, 0.6), "translucent": True, "size": 1.0, "lift": 0.0, "along": (2.0, 8.0),
                "up": (10.0, 30.0)}
MUZZLE_LAYERS = [MUZZLE_FLAME, MUZZLE_ALT, MUZZLE_SMOKE]
# The share of shots drawing _primaryflash rather than _primaryflash_alt (1 of the 3 flames the chooser has).
FLAME_CHOICE = 1.0 / 3.0
# First person: each weapon's `_fps` system (m_nViewModelEffect) has its third-person system as its child and, in its
# "game" configuration, puts control point 5 at (scale, 1, 1), which the children's C_INIT_GlobalScale reads (read
# 2026-10-25: uweapon_muzflsh_ak47_fps 0.35, _riffle_fps 0.5 — the M4A4's). The layers above are the AK's, so other
# weapons take the AK's scale too (their own systems are not rebuilt yet). Every length of a layer is scaled (its
# sizes, where its particles are born, how far they fly): C_INIT_GlobalScale's switches are left at their defaults in
# the files, which do not say what they are; with the sizes left whole, the main flame filled a fifth of the screen in
# first person.
VIEW_SCALES = {"weapon_ak47": 0.35, "weapon_m4a1": 0.5}
VIEW_SCALE_DEFAULT = 0.35


def view_layer(layer, scale):
    """A muzzle layer as first person draws it (VIEW_SCALES): on the viewmodel (`local`), every length times
    `scale`."""
    option = grenade_effects.option
    out = dict(layer, name="%s_fp%d" % (grenade_effects.layer_name(layer), round(scale * 100)), local=True)
    out["radius"] = tuple(r * scale for r in layer["radius"])
    for key in ("along", "up", "speed"):
        out[key] = tuple(v * scale for v in option(layer, key))
    out["spread"] = layer["spread"] * scale
    out["rise"] = layer["rise"] * scale
    return out


def view_scale(shot):
    return VIEW_SCALES.get(shot.get("weapon") or "", VIEW_SCALE_DEFAULT)


VIEW_LAYERS = [view_layer(layer, scale) for scale in sorted(set(VIEW_SCALES.values()) | {VIEW_SCALE_DEFAULT})
               for layer in MUZZLE_LAYERS]
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
    layers = MUZZLE_LAYERS + VIEW_LAYERS + TRACER_LAYERS + BLOOD_LAYERS
    for family_layers in FAMILY_LAYERS.values():
        layers += family_layers
    return layers


def muzzle_layers(shot, alt=True):
    """The muzzle layers a shot draws: its flame (MUZZLE_FLAME or MUZZLE_ALT, picked by the shot as CS2's chooser
    picks one: FLAME_CHOICE; MUZZLE_FLAME always without `alt`; none when silenced) and its smoke."""
    if shot.get("silenced"):
        return [MUZZLE_SMOKE]
    pick = random.Random(shot["tick"] * 7919 + shot["controller"] * 104729).random()
    return [MUZZLE_FLAME if pick < FLAME_CHOICE or not alt else MUZZLE_ALT, MUZZLE_SMOKE]


def family_of(bullet):
    return IMPACT_FAMILIES.get(bullet.get("impact") or "", "concrete")


def bullet_hits(bullet):
    """What a bullet met, in order: export::shots' `through` (each wall's way in and out, each player it went
    through), then where it stopped (the bullet itself): each with `on` ("world", "player" or "air") and `end`, on the
    world its `normal`, `impact` and `decal`. An export from before penetration has at most one wall on the way to a
    player (`wall`)."""
    if "through" in bullet:
        return list(bullet["through"]) + [bullet]
    wall = bullet.get("wall")
    return ([dict(wall, on="world")] if wall else []) + [bullet]


def unit(v):
    length = math.sqrt(sum(c * c for c in v))
    return [c / length for c in v] if length > 1e-6 else [0.0, 0.0, 1.0]


def sprite_slots(shots, alt=True, own_views=None):
    """The sprite slots (grenade_effects.add_sprites) of every shot: (shooter controller -> layer name -> slots of
    their muzzle flashes (muzzle_layers: `alt` whether MUZZLE_ALT can be drawn), not the silenced shots' flame, and
    of their tracers from the third-person barrel; the same for their first-person flashes, in the viewmodel rig's
    space; the same for their tracers as their own view shows them, from the viewmodel's barrel (export::shots'
    `view_tracer`, in the world; only for the shooters in `own_views`, those with a viewmodel shown in their view,
    when given); victim controller -> layer name -> slots of the blood of hits on that player (export::shots'
    `victim`: their own view shows none); layer name -> slots of the impacts by surface family, the blood of hits
    whose victim is not known, and the tracers of shots that have no own-view tracer, seen in every view)."""
    muzzles = {}
    views = {}
    own = {}
    blood = {}
    slots = {}

    def add(layers, slot, into):
        for layer in layers:
            into.setdefault(grenade_effects.layer_name(layer), []).append(slot)

    for shot in shots:
        tick = shot["tick"]
        layers = muzzle_layers(shot, alt)
        muzzle, barrel = shot.get("muzzle"), shot.get("muzzle_direction")
        if muzzle and barrel:
            add(layers, (muzzle, tick, tick, barrel, 0.0), muzzles.setdefault(shot["controller"], {}))
        view, view_barrel = shot.get("view_muzzle"), shot.get("view_muzzle_direction")
        if view and view_barrel:
            scale = view_scale(shot)
            add([view_layer(layer, scale) for layer in layers], (view, tick, tick, view_barrel, 0.0),
                views.setdefault(shot["controller"], {}))
        start = muzzle or shot["eye"]
        # A tracer leaves when the shot was fired (export::shots' `time`: CS2 fires between ticks, at the start of the
        # tick interval in a tournament demo), not on its message's tick: one tick after a shot CS2's is further
        # along than one born on the tick.
        fired = shot.get("time", tick)
        for bullet in shot["bullets"]:
            end = bullet["end"]
            if shot.get("tracer"):
                kind = shot["tracer"] if shot["tracer"] in TRACERS else TRACER_DEFAULT
                # From the barrel the body holds (hidden with it in the shooter's own view, where it would leave just
                # under the eyes), and in that view from where the viewmodel's barrel shows (CS2's first-person
                # tracer leaves the viewmodel: YEKINDAR's view, de_nuke round 5, ticks 37683 - 37702). A shot without
                # that (no viewmodel then, an older export, or none imported for the shooter) keeps its tracer in
                # every view.
                view_tracer = shot.get("view_tracer")
                if own_views is not None and shot["controller"] not in own_views:
                    view_tracer = None
                world = muzzles if view_tracer else {shot["controller"]: slots}
                for begin, into in ((start, world), (view_tracer, own)):
                    if not begin:
                        continue
                    way = [e - s for e, s in zip(end, begin)]
                    reach = math.sqrt(sum(c * c for c in way))
                    if reach > 1.0:
                        add([tracer_layer(kind)], (begin, fired, fired, unit(way), reach),
                            into.setdefault(shot["controller"], {}))
            # An impact on each side of every wall it went through (flying out of the far side along the bullet: the
            # way out's normal faces along it) and where it stopped; blood on every player it met.
            for at in bullet_hits(bullet):
                if at["on"] == "world" and at.get("normal"):
                    add(FAMILY_LAYERS[family_of(at)], (at["end"], tick, tick, at["normal"], 0.0), slots)
                elif at["on"] == "player":
                    # Back toward the shooter: CS2's mist flies along its effect's own axis, and the shooter sees it
                    # in front of the player hit (molodoy's view at tick 40840 of de_nuke round 5; along the bullet it
                    # hid behind the body). On the player hit's own actor when the export names them (`victim`).
                    hit = at["end"]
                    victim = at.get("victim")
                    into = slots if victim is None else blood.setdefault(victim, {})
                    add(BLOOD_LAYERS, (hit, tick, tick, unit([s - e for e, s in zip(hit, shot["eye"])]), 0.0), into)
    return muzzles, views, own, blood, slots


def light_frames(shots, tick0, end_frame, key="muzzle"):
    """Per shooter, the muzzle light's keys, one per frame from the frame before a shot to the end of its pulse:
    [(frame, position or None, intensity)], for the shots with a muzzle (`key`: "muzzle", in the world, or
    "view_muzzle", in the viewmodel rig's space) that are not silenced. A frame's intensity is the brightest of the
    pulses (MUZZLE_LIGHT) of the shooter's shots then, so shots closer together than a pulse (a burst, a Negev) do not
    cut each other short; the position is keyed on each shot's own frame."""
    by_shooter = {}
    for shot in shots:
        if shot.get(key) and not shot.get("silenced"):
            by_shooter.setdefault(shot["controller"], []).append((shot["tick"] - tick0, shot[key]))
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


def key_light(place, light, keys, end_frame):
    """Keys a muzzle light from light_frames' `keys`: its location (constant keys at each shot's muzzle, Source axes
    to Unreal's) on the binding `place`, its intensity on the binding `light` (of its light component)."""
    constant = unreal.MovieSceneKeyInterpolation.CONSTANT
    intensity = light.add_track(unreal.MovieSceneFloatTrack)
    intensity.set_property_name_and_path("Intensity", "Intensity")
    intensity_section = intensity.add_section()
    intensity_section.set_range(0, end_frame)
    channel = intensity_section.get_all_channels()[0]
    track = place.add_track(unreal.MovieScene3DTransformTrack)
    section = track.add_section()
    section.set_range(0, end_frame)
    location = {str(c.channel_name): c for c in section.get_all_channels()}
    for axis in ("X", "Y", "Z"):
        location["Scale." + axis].set_default(1.0)
    previous = None
    for frame, position, value in sorted(keys, key=lambda k: k[0]):
        if frame < 0:
            continue
        if position is not None:
            x, y, z = position
            for axis, cm in zip(("X", "Y", "Z"), (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM)):
                location["Location." + axis].add_key(unreal.FrameNumber(frame), cm, 0.0,
                                                     unreal.MovieSceneTimeUnit.DISPLAY_RATE, constant)
        if (frame, value) != previous:
            grenade_effects.add_key(channel, frame, value)
            previous = (frame, value)


def place_lights(sequence, replay, shots, end_frame, spawn, light, viewmodels):
    """A muzzle light per shooter (light_frames): movable (a location track on a light that is not is refused in a
    game world, which Movie Render Queue renders), its location keyed (constant) at each shot's muzzle, its intensity
    pulsed. In the shooter's own view their first-person flash has its own light, as CS2's `_fps` system has (its
    third-person child's `fakelight`): a light component on their viewmodel actor (`viewmodels`), keyed at each shot's
    `view_muzzle` there. A light cannot be hidden in one camera only: `light(owner, binding, controller, first_person,
    component=False)` keys which of the two shows by the camera cuts (the viewmodel's inside the cuts through the
    shooter's eyes, the other outside them) and tags it for later camera changes (replay_cameras). CS2's lights the
    door edge in front of YEKINDAR's AK one tick after his shot (de_nuke round 5, tick 37950); the third-person light,
    low and to the right of his eyes, lit the floor and door frame beside him orange instead. Returns the number of
    lights."""
    tick0 = replay["start_tick"]
    names = {p["controller"]: p["name"] for p in replay["players"]}
    lights = light_frames(shots, tick0, end_frame)
    for controller, keys in lights.items():
        first = next((p for _, p, _ in keys if p), None)
        if first is None:
            continue
        light_actor = grenade_effects.spawn_light(spawn, "muzzle light %s" % names.get(controller, controller),
                                                  first, MUZZLE_LIGHT["color"], MUZZLE_LIGHT["radius"])
        component = light_actor.get_editor_property("light_component")
        component.set_mobility(unreal.ComponentMobility.MOVABLE)
        component.set_editor_property("cast_shadows", False)
        # The actor's binding (its location), then its light component's (its intensity), as intensity_channel does.
        binding = sequence.add_possessable(light_actor)
        key_light(binding, sequence.add_possessable(component), keys, end_frame)
        light(light_actor, binding, controller, False)
    view_lights = light_frames(shots, tick0, end_frame, "view_muzzle")
    for controller, keys in view_lights.items():
        if controller not in viewmodels:
            continue
        component = grenade_effects.add_component(viewmodels[controller], unreal.PointLightComponent, "muzzle light")
        r, g, b = MUZZLE_LIGHT["color"]
        component.set_light_color(unreal.LinearColor(r=r, g=g, b=b, a=1.0))
        component.set_editor_property("attenuation_radius", MUZZLE_LIGHT["radius"] * UNITS_TO_CM)
        component.set_editor_property("intensity", 0.0)
        component.set_mobility(unreal.ComponentMobility.MOVABLE)
        component.set_editor_property("cast_shadows", False)
        binding = sequence.add_possessable(component)
        key_light(binding, binding, keys, end_frame)
        light(component, binding, controller, True, component=True)
    return len(lights) + sum(1 for c in view_lights if c in viewmodels)


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


def hole_depth(at):
    """How far a hole's decal reaches each way from the surface: HOLE_DEPTH, or less in a wall thinner than twice
    that (export::shots' `thickness`), so a hole on one face does not show on the other too."""
    thickness = at.get("thickness")
    return min(HOLE_DEPTH, thickness * 0.5) if thickness else HOLE_DEPTH


def place_hole(spawn, label, tick, end, normal, options, rng, depth=HOLE_DEPTH):
    """One bullet hole: a decal actor at `end` facing the surface of `normal` (Source axes), a random option of its
    group (`options`: hole_instances' list), size and roll, reaching `depth` units each way (hole_depth); its decal
    colour's red is `tick`, from which it shows."""
    instance, width, height, _ = rng.choices(options, weights=[o[3] for o in options])[0]
    size = rng.uniform(*HOLE_SIZE)
    nx, ny, nz = normal
    facing = unreal.MathLibrary.make_rot_from_x(unreal.Vector(-nx, ny, -nz))
    x, y, z = end
    actor = spawn(unreal.DecalActor, label, (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
    actor.set_actor_rotation(unreal.Rotator(roll=rng.uniform(0.0, 360.0), pitch=facing.pitch, yaw=facing.yaw), False)
    component = actor.get_editor_property("decal")
    component.set_editor_property("decal_size", unreal.Vector(
        depth * UNITS_TO_CM, width * 0.5 * size * UNITS_TO_CM, height * 0.5 * size * UNITS_TO_CM))
    component.set_decal_material(instance)
    component.set_decal_color(unreal.LinearColor(r=float(tick), g=0.0, b=0.0, a=1.0))
    # Never faded out by size: Unreal drops a decal under 1 % of the screen by default, and CS2 still shows a hole 11
    # units across from 449 units away (beside de_nuke's A sign) that Unreal's default left out.
    component.set_fade_screen_size(0.0)


def place_holes(shots, earlier, instances, spawn):
    """A bullet hole (place_hole) per place a bullet of `shots` met the world with a decal group (`instances`:
    hole_instances: both sides of each wall it went through, and where it stopped), showing from its shot's tick, and
    per hole of `earlier` (the agents manifest's `earlier_holes`: left by the shots before the round, which CS2
    keeps), showing from the round's start. Returns the number placed."""
    placed = 0
    for shot in shots:
        for k, bullet in enumerate(shot["bullets"]):
            for n, at in enumerate(bullet_hits(bullet)):
                options = instances.get(at.get("decal") or "")
                if at["on"] != "world" or not options or not at.get("normal"):
                    continue
                rng = random.Random(shot["tick"] * 7919 + shot["controller"] * 104729 + k * 31 + n)
                place_hole(spawn, "bullet hole %d %d.%d.%d" % (shot["controller"], shot["tick"], k, n), shot["tick"],
                           at["end"], at["normal"], options, rng, hole_depth(at))
                placed += 1
    for k, hole in enumerate(earlier):
        options = instances.get(hole.get("decal") or "")
        if not options or not hole.get("normal"):
            continue
        rng = random.Random(hole["tick"] * 7919 + k)
        place_hole(spawn, "earlier bullet hole %d.%d" % (hole["tick"], k), hole["tick"], hole["end"], hole["normal"],
                   options, rng, hole_depth(hole))
        placed += 1
    return placed


def place_shots(sequence, replay, replay_path, end_frame, agents, spawn, hide, viewmodels=None, show_first_person=None,
                light=None):
    """Everything above for a round, from the agents manifest `agents` (its `shots`, `earlier_holes`, `effects` and
    `decals`; without them: nothing): an actor per shooter for their muzzle flashes and tracers, hidden with their
    body (`hide(actor, binding, controller)`: shown while the body is, and not in their own camera), their
    first-person flashes on their viewmodel actor (`viewmodels`: controller -> actor, from
    import_demo.spawn_viewmodel), an actor per shooter with a viewmodel for their tracers as their own view shows them
    (`show_first_person(actor, binding, controller)`: shown as their viewmodel is, in their own camera only), their
    muzzle lights (place_lights, with `light`), an actor per player hit for the blood of the hits on them (hidden with
    their body), one actor ("shots") for the impacts, the blood of hits on no known player and the tracers seen in
    every view, and the bullet holes. Returns the number of shots."""
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
    meshes = {local: grenade_effects.sprite_mesh(local) for local in (False, True)}
    rng = random.Random(replay["round"] * 7349 + len(shots))
    layers = {grenade_effects.layer_name(layer): layer for layer in shot_layers()}
    names = {p["controller"]: p["name"] for p in replay["players"]}
    # An export from before the alternate flame's sheet (effects without `muzzle_alt`) draws the main flame always.
    alt = grenade_effects.layer_name(MUZZLE_ALT) in instances
    if not alt:
        log("effects: no muzzle_alt sheet (export the round again): every shot draws the main flame")
    # Own-view tracers for the shooters whose viewmodel their own view shows.
    own_views = set((viewmodels or {}).keys()) if show_first_person is not None else set()
    muzzles, views, own, blood, shared = sprite_slots(shots, alt, own_views)
    drawn = 0

    def draw(actor, by_layer):
        count = 0
        for name, slots in sorted(by_layer.items()):
            if name in instances:
                local = grenade_effects.option(layers[name], "local")
                ism = grenade_effects.add_sprites(actor, layers[name], instances[name], meshes[local], slots, rng)
                if local:
                    # Drawn as the viewmodel is: at its field of view, in front of the world.
                    ism.set_editor_property("first_person_primitive_type",
                                            unreal.FirstPersonPrimitiveType.FIRST_PERSON)
                count += len(slots)
        return count

    for controller, by_layer in sorted(muzzles.items()):
        actor = spawn(unreal.Actor, "muzzle flashes and tracers %s" % names.get(controller, controller),
                      (0.0, 0.0, 0.0))
        drawn += draw(actor, by_layer)
        hide(actor, sequence.add_possessable(actor), controller)
    viewmodels = viewmodels or {}
    first_person = 0
    for controller, by_layer in sorted(views.items()):
        if controller in viewmodels:
            first_person += draw(viewmodels[controller], by_layer)
    # Their tracers as their own view shows them: in the world, from the viewmodel's barrel, shown with the viewmodel.
    # (sprite_slots put them only for the shooters in own_views: a viewmodel to show them with.)
    for controller, by_layer in sorted(own.items()):
        actor = spawn(unreal.Actor, "own tracers %s" % names.get(controller, controller), (0.0, 0.0, 0.0))
        first_person += draw(actor, by_layer)
        show_first_person(actor, sequence.add_possessable(actor), controller)
    # The blood of the hits on each player: hidden with their body, as CS2's own view of a player hit shows none.
    for controller, by_layer in sorted(blood.items()):
        actor = spawn(unreal.Actor, "blood on %s" % names.get(controller, controller), (0.0, 0.0, 0.0))
        drawn += draw(actor, by_layer)
        hide(actor, sequence.add_possessable(actor), controller)
    drawn += first_person + draw(spawn(unreal.Actor, "shots", (0.0, 0.0, 0.0)), shared)
    lights = place_lights(sequence, replay, shots, end_frame, spawn, light,
                          viewmodels if show_first_person else {}) if light else 0
    holes = place_holes(shots, agents.get("earlier_holes", []), hole_instances(replay_path, agents.get("decals", {})),
                        spawn)
    log("shots: %d (%d sprite slots, %d of them first person; %d muzzle lights, %d bullet holes)"
        % (len(shots), drawn, first_person, lights, holes))
    return len(shots)
