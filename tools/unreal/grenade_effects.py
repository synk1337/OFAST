"""The grenades' effects in Unreal other than smokes (smoke_volumes.py): molotov and incendiary fires, HE blasts,
flashbang bursts and the flashed white of a blinded player's view (docs/decisions/017-grenades-and-effects.md;
docs/formats/replay.md, "In Unreal"). Used by import_demo.py and probe_smoke.py.

CS2 draws these with particle systems (particles/inferno_fx/molotov_groundfire.vpcf, explosions_fx/explosion_hegrenade,
explosion_flashbang). Niagara systems cannot be written from editor Python, so OFAST rebuilds their main layers as
"particle systems in a material", with CS2's own sprite sheets (the export's flipbooks, export::effects):

- Each effect (a fire, a blast, a burst) is one actor with one instanced static mesh per layer: a fire's flames
  (FIRE_LAYERS, a slot set per flame), the flames up the walls it meets (WALL_LAYERS, per wall point the export
  found), the wisps it leaves when it goes out (SMOKE_LAYERS, WALL_SMOKE_LAYERS); a blast's or a flashbang's layers
  (BLAST_LAYERS, FLASH_LAYERS). An instance is a sprite slot; its per-instance data hold where it is (Unreal cm), the
  ticks it may spawn particles in, a phase, a seed and the floor it dies at.
- The materials (SPRITE_MATERIALS: additive for flames and flares, translucent for smoke; unlit) read the demo tick
  from the parameter collection the smokes use (smoke_volumes.REPLAY_COLLECTION) and, in the vertex and the pixel
  shader alike, the same few lines (SPRITE_STATE): the slot respawns a particle every LifeMax seconds while its span
  lasts; a hash of the slot's seed and the respawn count draws the particle's lifetime, size, place, roll and sheet
  sequence, as CS2's initializers draw them from their ranges. A fire's slot spans its flame's burning ticks; a
  blast's or burst's spans one tick, so it spawns once. The vertex shader turns the quad to face the camera at the
  particle's place and size (from ScaleStart to ScaleEnd of its radius, rising Rise units over its life); the pixel
  shader plays the sequence's frames over the particle's life (CS2's ANIMATION_TYPE_MANUAL_FRAMES driven by the
  normalised age), tinted, fading in and out, and softened where it meets the world.
- Some particles fly (an HE blast's sparks, debris, ground smoke): the same lines throw them along their heading and
  up, slow them by CS2's drag and pull them down by gravity; sparks are drawn as streaks along their flight; sparks
  and debris die at the ground under the blast (the agents manifest's `blasts`), as CS2's die on touching the world.
- Every part of it is a function of the demo tick, so scrubbing the sequence shows the same flames every time.
- A point light per effect, keyed: a fire's flickers with its flames, a blast's and a flashbang's pulse and die out.
- A blast near the ground leaves scorch splats, a fire soot under its flames: decal actors with a material
  (SCORCH_MATERIAL) faded by the tick.
- A blinded player's camera gets a post-process material (FLASHED_MATERIAL) whitening its view from the player's
  blindings (the replay's `blindings`) and the demo tick.
"""

import math
import random

import unreal

import smoke_volumes

SPRITE_MATERIALS = {False: "/Game/OFAST/Replays/M_SpriteAdd", True: "/Game/OFAST/Replays/M_SpriteBlend"}
FLASHED_MATERIAL = "/Game/OFAST/Replays/M_Flashed"
# The sprites' quad: a copy of the engine's 1 m plane whose bounds reach SPRITE_BOUNDS_UP (local cm) above it and
# SPRITE_BOUNDS_DOWN below. The vertex shader moves the quad's corners to where the sprite is drawn, but the engine
# culls an instance by its mesh's bounds: the flat plane's have no height, so flames standing above a floor that a
# low wall hides would vanish with it. Instances are scaled 6 x: bounds 3 m to each side, 6 m up, 1.5 m down.
ENGINE_PLANE = "/Engine/BasicShapes/Plane"
SPRITE_MESH = "/Game/OFAST/Replays/Effects/SM_Sprite"
SPRITE_INSTANCE_SCALE = 6.0
SPRITE_BOUNDS_UP = 100.0
SPRITE_BOUNDS_DOWN = 25.0
# The quad of a layer drawn in its component's space (`local`: a first-person muzzle flash on the viewmodel): its
# instances stay at the component's origin, unscaled (the component's space is then theirs: see SPRITE_STATE), so its
# bounds reach SPRITE_VIEW_BOUNDS (cm) every way, past the barrel of any gun held in front of the camera.
SPRITE_VIEW_MESH = "/Game/OFAST/Replays/Effects/SM_SpriteView"
SPRITE_VIEW_BOUNDS = 300.0
FOLDER = "/Game/OFAST/Replays/Effects"
UNITS_TO_CM = 2.54
# The layers: the effect texture of that role in the agents manifest, sprite slots (per flame for a fire, per blast or
# burst otherwise), CS2's ranges (its particle systems, read 2026-10-24): lifetime (s), radius (units), size from birth
# to death (x radius), alpha, roll (+- degrees), how far from the centre a particle is born (units), how far it rises
# over its life (units); and for Unreal: a brightness factor (CS2's overbright, tuned against CS2's picture), a tint
# (CS2's grey fireball and smoke sheets are coloured by its shaders), translucent (alpha-blended) or additive.
# Optional (LAYER_DEFAULTS): `name` (the layer's own, when two share a texture), `size` (sprite half-size per unit of
# radius), `lift` (raised this much of its radius off its centre), how it flies — `speed` out along a random heading
# and `up` (units/s), `gravity` (units/s^2), CS2's `drag` — `stretch` (a streak along its flight as long as `stretch`
# seconds of it, `max_length` units at most), `sequence` (first sheet sequence, how many), `floor` (it dies at the
# ground under a blast, as CS2's particles die on touching the world), `hot` (r, g, b, share: born that colour,
# cooling to the tint over that share of its life) and `end_fade` (seconds: its particles die out that fast after
# their slot's last tick, whatever their age).
# A flame's particles go out within this of its last burning tick: at a fire's end CS2's flames are gone (the
# system stops), where a particle living out its 1.2 s showed flames half a second after (de_nuke, 2026-10-24).
FLAME_END_FADE = 0.15
# CS2's colour gradients (a renderer's 1D colour lookup, read from the particle files 2026-10-30): (position, sRGB
# colour 0 - 255) stops, which a layer's `gradient` names. CS2 colours its fires with them, the sheets' own colours
# only half showing (`gradient_blend` 0.5): low values deep red, most of the range orange to peach, the brightest
# white; the edge's flames dip through blue and purple at their faint ends (the magenta fringe a CS2 fire has).
GRADIENTS = {
    # molotov_groundfire_main_center, _main_fancy, molotov_child_flame01a.
    "fire": [(0.0, (0, 0, 0)), (0.0646, (128, 10, 10)), (0.1705, (167, 55, 46)), (0.2584, (210, 92, 29)),
             (0.8398, (210, 146, 97)), (0.9457, (255, 235, 201)), (0.9535, (255, 241, 218)), (1.0, (255, 255, 255))],
    # molotov_groundfire_snapshot_child_base: the flames lying on the ground, paler sooner.
    "fire_surface": [(0.0, (0, 0, 0)), (0.06, (128, 10, 10)), (0.12, (167, 55, 46)), (0.2, (210, 127, 86)),
                     (0.45, (210, 146, 97)), (0.95, (255, 235, 201)), (0.953, (255, 241, 218)),
                     (1.0, (255, 255, 255))],
    # molotov_groundfire_outline, _climbingoutline.
    "fire_edge": [(0.04, (0, 0, 0)), (0.4, (21, 21, 199)), (0.5, (108, 10, 128)), (0.64, (167, 55, 46)),
                  (0.7, (210, 92, 29)), (0.8, (210, 146, 97)), (0.93, (255, 235, 201)), (0.95, (255, 241, 218)),
                  (1.0, (255, 255, 255))],
    # molotov_explosion_child_flash: a fireball's sheet from near-black smoke through orange to white.
    "burst_ball": [(0.0, (28, 27, 27)), (0.28, (50, 39, 33)), (0.54, (81, 74, 63)), (0.58, (116, 42, 42)),
                   (0.68, (238, 119, 52)), (0.78, (229, 105, 38)), (0.83, (249, 135, 69)), (0.9, (243, 185, 154)),
                   (1.0, (255, 255, 255))],
    # molotov_explosion_child_sprays: burning splashes, red to orange to white.
    "burst_spray": [(0.07, (0, 0, 0)), (0.26, (255, 0, 0)), (0.69, (255, 140, 0)), (1.0, (255, 255, 255))],
    # incen_explosion_child_sprays: the incendiary's, orange to pale yellow.
    "incendiary_spray": [(0.0, (0, 0, 0)), (0.4, (255, 140, 0)), (0.75, (222, 199, 109)), (1.0, (255, 255, 189))],
}
GRADIENT_STOPS = max(len(stops) for stops in GRADIENTS.values())
# A fire as CS2 draws it (particles/inferno_fx/molotov_groundfire, read 2026-10-30; round 17's molotov at the top of
# mid, played at a quarter speed and every 8 ticks, 2026-10-31): a dense carpet of low flames over the ground it
# burns, cream and peach where they overlap, orange-brown where thin, a white-hot heart where it lit for its first
# seconds, and along its edge facing the camera flames trailing inward whose faint ends are blue and purple (the
# magenta fringe). Its flames' colours are CS2's gradients (GRADIENTS), not the sheets'.
# The carpet (FIRE_LAYERS), a slot set per flame: CS2 fills the ground between its flames with points of its own
# (molotov_groundfire_filltest: the client's filler points, up to 500, which the demo does not hold), here the
# flames' spread, with more slots, the larger radii and twice the brightness of one point's: at CS2's numbers on its 16
# flames, round 17's molotov was a few faint flames where CS2 shows a carpet (sprites larger still stood too tall).
FIRE_LAYERS = [
    # molotov_groundfire_main_center: 0.7 a second per point, living 1 - 2 s scaled down away from the fire's heart
    # (0.75 overall), radius 10 - 55 grown from 0.6 to 1.2 times, alpha 0.39 - 0.59, rolled -60 - 90, rising
    # (gravity +35, at most 40 units/s), in over 0.1 - 0.15 s and out over its last half second; fire_small_sim_a
    # half in the fire gradient, overbright 4.
    {"name": "fire_fill", "role": "fire_fill", "slots": 10, "life": (0.6, 1.2), "radius": (35.0, 55.0),
     "scale": (0.6, 1.2), "alpha": (0.39, 0.59), "roll": 75.0, "roll_base": 15.0, "spread": 45.0, "rise": 0.0,
     "bright": 8.0, "tint": (1.0, 0.9, 0.8), "gravity": -35.0, "fade_in": 0.12, "fade_out": 0.5, "gradient": "fire",
     "gradient_blend": 0.5, "end_fade": FLAME_END_FADE},
    # molotov_groundfire_main_fancy: taller flames among them (fire_small_sim_b, grown from half, rising faster:
    # gravity +75), overbright 2.
    {"name": "fire_fancy", "role": "fire", "slots": 3, "life": (0.6, 1.2), "radius": (30.0, 50.0),
     "scale": (0.5, 1.0), "alpha": (0.5, 0.7), "roll": 75.0, "roll_base": 15.0, "spread": 45.0, "rise": 0.0,
     "bright": 4.0, "gravity": -75.0, "drag": 0.05, "sequence": (0, 2), "gradient": "fire",
     "gradient_blend": 0.5, "end_fade": FLAME_END_FADE},
    # molotov_groundfire_snapshot_child_base: flames lying flat on the ground (fire_onsurface, aligned to the ground's
    # normal, radius x 0.85, self-illuminated 3 x, overbright 2).
    {"name": "fire_surface", "role": "fire_surface", "slots": 4, "life": (1.0, 2.0), "radius": (45.0, 65.0),
     "scale": (0.85, 0.85), "alpha": (0.5, 0.8), "roll": 180.0, "spread": 40.0, "rise": 0.0, "bright": 8.0,
     "size": 1.0, "lift": 0.0, "along": (1.0, 1.0), "flat": True, "fade_in": 0.2, "fade_out": 0.4,
     "gradient": "fire_surface", "end_fade": FLAME_END_FADE},
]
# The heart (CENTRE_LAYERS), slot sets where the fire lit (its burst's place, else its first flame): molotov_fire01's
# flames, born in a sphere round the fire's origin that grows from 8 units across to 50 between 3 and 7 s (5 to 50
# between 2 and 8 s for its gas): for its first seconds a white-hot column (round 17's molotov, 1 - 3 s in), then
# spread among the rest. Here a tight set until CENTRE_TIGHT_S, a wide one from CENTRE_WIDE_S.
CENTRE_TIGHT_S = 4.0
CENTRE_WIDE_S = 3.0
# molotov_child_flame01a: 10 a second, fire_small_sim_b half in the fire gradient, pale (244 237 208 - 235 172 95),
# overbright 6: where they overlap, white (round 17's: a column about 60 units across and 100 high, blown white; at
# overbright 6 and CS2's radius as a quarter of the sprite's size, Unreal's was a small glow).
HEART = {"role": "fire", "slots": 14, "life": (0.65, 1.2), "radius": (30.0, 50.0), "scale": (1.5, 0.75),
         "alpha": (0.47, 0.59), "roll": 24.0, "rise": 40.0, "bright": 12.0, "size": 1.0, "tint": (0.9, 0.7, 0.45),
         "gradient": "fire", "gradient_blend": 0.5, "end_fade": FLAME_END_FADE}
# molotov_child_flame03a: 10 a second slowing to 2, tongues of burning gas among them (in their sheet's colours, pale:
# 244 237 208 - 231 215 202, overbright 3).
HEART_GAS = {"role": "gas", "slots": 6, "life": (0.75, 1.35), "radius": (24.0, 55.0), "scale": (0.8, 1.25),
             "alpha": (0.78, 0.98), "roll": 45.0, "rise": 25.0, "bright": 3.0, "size": 0.8, "tint": (0.88, 0.8, 0.7),
             "end_fade": FLAME_END_FADE}
CENTRE_LAYERS = [dict(HEART, name="fire_heart", spread=8.0), dict(HEART_GAS, name="gas_heart", spread=8.0)]
WIDE_CENTRE_LAYERS = [dict(HEART, name="fire_heart_wide", spread=50.0),
                      dict(HEART_GAS, name="gas_heart_wide", spread=50.0)]
# The edge (EDGE_LAYERS), a slot set per point of the fire's outline: CS2's client draws the flames' outline (its own
# points, not in the demo); here the hull of the flames pushed EDGE_OFFSET out, a point every EDGE_SPACING units, at
# the nearest flame's height and burning ticks, its axis pointing inward.
EDGE_OFFSET = 20.0
EDGE_SPACING = 20.0
EDGE_LAYERS = [
    # molotov_groundfire_outline: 2 a second per point, thrown inward at 75 - 150 units/s (at most 50) and rising
    # (gravity +20), 0.5 - 1.5 s, radius 20 - 30 grown from half, in over the first 5 - 10 % and out over the rest;
    # trails from where it was born (fire_small_sim_b_desat through the edge gradient, alpha x 2, overbright 3); only
    # on the side the camera sees. Looked up lower as they fade (`gradient_fade`): looked up by the sheet alone they
    # burned red-orange with a few purple specks, where CS2's near edge is speckled magenta all along.
    {"name": "fire_edge", "role": "fire_edge", "slots": 3, "life": (0.5, 1.5), "radius": (20.0, 30.0),
     "scale": (0.5, 1.0), "alpha": (1.5, 2.0), "roll": 25.0, "spread": 5.0, "rise": 0.0, "bright": 4.0, "size": 1.0,
     "lift": 0.3, "up": (40.0, 50.0), "gravity": -20.0, "from_birth": True, "fade_in": 0.08, "fade_out": 0.85,
     "sequence": (0, 3), "gradient": "fire_edge", "gradient_fade": 0.4, "near_side": True,
     "end_fade": FLAME_END_FADE},
]
# particles/inferno_fx/molotov_groundfire_climbingoutline: flames shooting up a wall where a fire meets it (CS2's client
# finds those places by tracing from the fire's outline; the export traces from its flames: the agents manifest's
# `fires`' `walls`), from WALL_OFFSET off the wall and WALL_RAISE up, while the flame next to it burns: up at 40 - 120
# units/s (pushed up by a gravity of +200, at most 100 units/s: 40 - 100 here), 0.45 - 1 s, radius 5 - 15 grown from
# half to 1.5 times, CS2's trails from where they were born drawn as streaks along their flight, in the edge's
# colours (overbright 2).
WALL_LAYERS = [
    {"name": "fire_wall", "role": "fire_edge", "slots": 2, "life": (0.45, 1.0), "radius": (5.0, 15.0),
     "scale": (0.5, 1.5), "alpha": (0.8, 1.0), "roll": 0.0, "spread": 2.0, "rise": 0.0, "bright": 2.0, "size": 1.0,
     "lift": 0.0, "up": (40.0, 100.0), "drag": 0.01, "stretch": 0.35, "max_length": 100.0, "sequence": (0, 3),
     "gradient": "fire_edge", "gradient_fade": 0.4, "end_fade": FLAME_END_FADE},
]
# The burst a fire lights with (the replay's fire `burst`: CS2's server dispatches it, thrown along `direction`, the
# layers' axis), as CS2 draws a molotov's (particles/inferno_fx/molotov_explosion; round 17's molotov at the top of
# mid played at a quarter speed, 2026-10-30): +5 a ball of fire 40 units across; +7 - +11 it swells, flames licking
# up; +13 - +22 a yellow-orange fireball 130 units across, thrown its way, going dark at the edges; +24 - +38 deep red
# and black with embers; +40 - +66 black smoke rising off it and thinning; gone by +70 (1.1 s), the fire's heart
# burning under it from +45.
MOLOTOV_BURST_LAYERS = [
    # molotov_explosion_child_fireball1: 5 at once and 10 a second for 0.15 s, radius 3 - 5 grown 20 times, thrown
    # 200 - 500 units/s its way (and +-250 across, up to 450 up; stopped by the world, here by drag), 0.75 - 1.5 s,
    # fire_gas_batch_a_v2 tinted 244 237 208, overbright 2. Bigger here and rising (gravity -400: CS2's up to 450 up
    # in its own frame): at CS2's radius the fireball was 100 units across and low, where CS2's reaches 150 across and
    # the top of a wall.
    {"name": "burst_fire", "role": "gas", "slots": 6, "life": (0.5, 0.7), "radius": (90.0, 140.0),
     "scale": (0.15, 1.0), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 5.0, "rise": 0.0, "bright": 1.6,
     "tint": (1.0, 0.75, 0.45), "size": 1.0, "lift": 0.0, "up": (200.0, 500.0), "speed": (0.0, 250.0),
     "gravity": -400.0, "drag": 0.1, "fade_in": 0.05, "fade_out": 0.4, "sort": 1},
    # molotov_explosion_child_fireball2: 6 tongues (fire_gas_batch_b_top_v2, radius 2 - 4 grown 21 times, 50 - 250
    # units/s its way, rising), 0.5 - 1.5 s, overbright 2.
    {"name": "burst_tongues", "role": "burst_tongue", "slots": 6, "life": (0.5, 0.9), "radius": (42.0, 84.0),
     "scale": (0.05, 1.0), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 5.0, "rise": 0.0, "bright": 2.0,
     "size": 1.0, "lift": 0.0, "up": (50.0, 250.0), "speed": (0.0, 100.0), "gravity": -15.0, "drag": 0.05,
     "fade_in": 0.05, "fade_out": 0.35, "sort": 1},
    # molotov_explosion_child_flash: 4 fireballs (explosion_fireball_01, radius 20 - 40 grown 5 times, rising:
    # gravity +150) through a gradient from near-black smoke to orange and white, alpha-blended: the dark red and black
    # round the fire, then the smoke rising off it. Under the fire (sort 0) where they overlap. Its sheet is orange for
    # its first frames, dark red by the 5th of 32 and black by the 8th: over a life of 1 - 1.2 s it plays its first 40 %
    # (dark red from about +20, black from +40, as CS2's), thrown its way with the fire (at -50 - 250 units/s along it,
    # the smoke went black behind the fire from +11). Unlit, its colours need 6 times their value to show in the
    # exposure of a lit map (at 1 its orange read as dark brown); grown from a tenth (CS2's ball is 40 units at +5).
    {"name": "burst_smoke", "role": "burst_ball", "slots": 4, "life": (1.0, 1.2), "radius": (100.0, 170.0),
     "scale": (0.1, 1.0), "alpha": (0.9, 1.0), "roll": 45.0, "spread": 5.0, "rise": 0.0, "bright": 6.0,
     "tint": (0.9, 0.8, 0.7), "translucent": True, "size": 1.0, "lift": 0.0, "up": (100.0, 300.0),
     "speed": (0.0, 100.0), "gravity": -150.0, "drag": 0.1, "fade_in": 0.1, "fade_out": 0.4,
     "gradient": "burst_ball", "frame_span": 0.4},
    # molotov_explosion_child_sprays: 3 burning splashes thrown back from its way (200 - 500 units/s, +-250 across,
    # up to 450 up) and falling (gravity 550), 1 s, trails up to 350 long (blood_spray_multi through red - orange -
    # white, overbright 2).
    {"name": "burst_sprays", "role": "burst_spray", "slots": 3, "life": (0.8, 1.0), "radius": (3.0, 6.0),
     "scale": (1.0, 5.0), "alpha": (1.0, 1.0), "roll": 0.0, "spread": 5.0, "rise": 0.0, "bright": 2.0, "size": 1.0,
     "lift": 0.0, "up": (-500.0, -200.0), "speed": (0.0, 250.0), "gravity": 550.0, "drag": 0.1, "stretch": 0.1,
     "max_length": 150.0, "gradient": "burst_spray", "floor": True},
    # explosion_molotov_ground_debris: 8 flecks thrown 300 - 500 units/s its way (+-300 across, 150 up), 0.5 - 1 s.
    {"name": "burst_debris", "role": "debris", "slots": 8, "life": (0.5, 1.0), "radius": (1.0, 2.0),
     "scale": (1.0, 0.3), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 5.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.5, 0.5, 0.5), "translucent": True, "size": 1.0, "lift": 0.0, "up": (300.0, 500.0),
     "speed": (0.0, 300.0), "gravity": 400.0, "drag": 0.075, "sequence": (15, 25), "floor": True},
]
# An incendiary's (particles/inferno_fx/incendiary_explosion; round 17's at long doors, 2026-10-30): a flash of
# white sparks for about a third of a second, a small orange fireball, then its flames.
INCENDIARY_BURST_LAYERS = [
    # explosion_incen_ground_splash07a: 4 bursts of sparks (sparks_burst, radius 48 - 56 x 1.3, grown from half to 1.5
    # times, overbright 8), falling (gravity 400); 0.8 - 1 s in the file, its sheet spent in about 0.3.
    {"name": "burst_sparks", "role": "burst_sparks", "slots": 4, "life": (0.3, 0.4), "radius": (60.0, 75.0),
     "scale": (0.5, 1.5), "alpha": (0.45, 0.52), "roll": 180.0, "spread": 10.0, "rise": 0.0, "bright": 8.0,
     "size": 1.0, "lift": 0.3, "up": (10.0, 50.0), "speed": (0.0, 30.0), "gravity": 400.0, "drag": 0.05},
    dict(MOLOTOV_BURST_LAYERS[1], name="incendiary_tongues"),
    # incen_explosion_child_sprays: 3 burning splashes (fire_gas_batch_a_v2 through orange - pale yellow, radius x 3,
    # overbright 2), thrown back from its way and falling (gravity 450), 0.75 - 1.5 s.
    {"name": "incendiary_sprays", "role": "gas", "slots": 3, "life": (0.75, 1.5), "radius": (9.0, 18.0),
     "scale": (1.0, 5.0), "alpha": (1.0, 1.0), "roll": 45.0, "spread": 5.0, "rise": 0.0, "bright": 2.0, "size": 1.0,
     "lift": 0.0, "up": (-500.0, 0.0), "speed": (0.0, 250.0), "gravity": 450.0, "drag": 0.12,
     "gradient": "incendiary_spray", "floor": True},
    dict(MOLOTOV_BURST_LAYERS[4], name="incendiary_debris"),
]
WALL_OFFSET = 5.0
WALL_RAISE = 7.5
# particles/inferno_fx/molotov_groundfire_remnant's end caps: when a fire goes out, near-black wisps over where it
# burned for FIRE_SMOKE_SECONDS (_endcap_groundsmoke: 2 - 4 s each, radius 10 - 20 grown to 3 times, alpha
# 0.08 - 0.35 x 4, colour 117 x 39 of 255, rising 0 - 5 units/s; wispy_steam_set) and up the walls it met
# (_endcap_wall: 2.6 s, radius 5 - 15, alpha 0.34 - 0.69 x 4, rising 5 - 10). A burning fire has no smoke in CS2.
FIRE_SMOKE_SECONDS = 5.0
# CS2 emits 30 per second over a fire (about 90 alive): 6 slots per flame here, each born first at an even spread over
# its first lifetime (a 16-flame fire: 96 slots, 24 per second from the fire's end on).
SMOKE_LAYERS = [
    {"name": "fire_smoke", "role": "fire_smoke", "slots": 6, "life": (2.0, 4.0), "radius": (10.0, 20.0),
     "scale": (0.5, 3.0), "alpha": (0.3, 1.0), "roll": 180.0, "spread": 10.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.07, 0.07, 0.07), "translucent": True, "size": 1.0, "lift": 0.3, "up": (0.0, 5.0), "gravity": -5.0},
]
WALL_SMOKE_LAYERS = [
    {"name": "fire_wall_smoke", "role": "fire_smoke", "slots": 2, "life": (2.4, 2.8), "radius": (5.0, 15.0),
     "scale": (0.5, 3.0), "alpha": (0.6, 1.0), "roll": 180.0, "spread": 2.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.07, 0.07, 0.07), "translucent": True, "size": 1.0, "lift": 0.0, "up": (5.0, 10.0)},
]
# An HE blast as CS2 draws it (round 17's blast at the mid doors, tick 151907, played at a quarter speed and grabbed
# as it played, 2026-10-31): +3 - +11 a white-peach flash about 150 units across (a lens streak at first), the walls
# lit orange, rays of sparks to about +25; +13 - +30 a puffy orange fireball with a yellow-white core, about 150 units
# across; +28 - +45 dark red inside black smoke; +47 - +75 brown-black smoke thinning; a faint haze to about +110
# (1.7 s), the scorch left. The same stages as three HEs filmed tick by tick on 2026-10-30, but each `demo_step_tick`
# moved CS2's particles about 3 ticks further than the demo (tools/cs2/tick_shots.py): a stepped "+N" was +4.2 N, and
# lifetimes fitted to those frames were 4.2 times too short. They are the stepped fits times 4.2 now, near the files'
# (1 - 2 s for the fireball).
# The fire's colour: the top of CS2's heat gradient, 255 77 23, which is sRGB: linear (1, 0.075, 0.01), a deep red
# that turns orange and yellow where the brightness clips it, as CS2's core does. Taken as linear, the sRGB values
# drew a yellow-orange fireball beside CS2's red-orange one (mid, its fireball).
FIRE_ORANGE = (1.0, 0.075, 0.01)
BLAST_LAYERS = [
    # particles/explosions_fx/explosion_hegrenade: the white flash it goes off with (radius 2 - 3 grown 10 to 15
    # times, its renderers' radius scale and bloom making it far bigger): lasting about 17 ticks, 80 - 100 units round,
    # shrinking.
    {"name": "blast_flare", "role": "flare", "slots": 1, "life": (0.27, 0.27), "radius": (80.0, 100.0),
     "scale": (1.2, 0.6), "alpha": (1.0, 1.0), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 8.0,
     "tint": (1.0, 0.8, 0.6), "size": 1.0, "fade_in": 0.0, "fade_out": 0.5, "sort": 1},
    # particles/explosions_fx/explosion_hegrenade_smoke_core: the fireball, 3 puffs (born 0, 0.05 and 0.1 s in: all
    # at once here) of radius 90 - 180, rising at 90 - 180 units/s (and 60 any way, drag 0.1, a pull up of 50), its
    # frames fitted to the life; born at 0.8 of its size, as CS2's grows from a third with a bias of 0.81 (whole within
    # a few ticks: grown linearly from a third, Unreal's was a third of CS2's size). Its burning core is its heat sheet,
    # a heat map CS2 colours from black to orange (2 2 2 to 255 77 23) at an overbright of 10, additive, drawn over the
    # smoke (`sort`): bright in each sequence's first frames, black by its middle; over a life of 0.85 - 1.25 s it burns
    # orange to about +27 and dark red to +40, as CS2's (the grey flame sheet tinted orange, before 2026-10-30, drew a
    # white glow where CS2 shows an orange fireball: molodoy at tick 151909, KSCERATO at 151918). CS2's radius as the
    # sprite's half-size...
    {"name": "blast_heat", "role": "blast_heat", "slots": 3, "life": (0.85, 1.25), "radius": (90.0, 180.0),
     "scale": (0.8, 1.0), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 30.0, "rise": 0.0, "bright": 6.0,
     "tint": FIRE_ORANGE, "size": 1.0, "lift": 0.0, "up": (90.0, 180.0), "speed": (0.0, 60.0), "drag": 0.1,
     "gravity": -50.0, "fade_in": 0.0, "fade_out": 0.5, "sort": 1, "link": "blast_core"},
    # explosion_hegrenade_smoke_blasts: 3 jets of fire thrown out any way at 200 - 400 units/s (and 50 - 200 up),
    # radius 30 - 60, drag 0.025, drawn as trails of explosion_blast_01_heat in the same orange at an overbright of 5;
    # a streak as long as a fifth of a second of its flight here, out by +40 with the fireball. Brighter than CS2's
    # overbright: at 3 the jets did not show against de_dust2's sunlit walls.
    {"name": "blast_jets", "role": "blast_jet", "slots": 3, "life": (0.6, 1.25), "radius": (30.0, 60.0),
     "scale": (0.75, 1.0), "alpha": (1.0, 1.0), "roll": 0.0, "spread": 10.0, "rise": 0.0, "bright": 8.0,
     "tint": FIRE_ORANGE, "size": 1.0, "lift": 0.0, "up": (50.0, 200.0), "speed": (200.0, 400.0),
     "drag": 0.025, "stretch": 0.2, "max_length": 600.0, "fade_in": 0.0, "fade_out": 0.25, "sort": 1},
    # ... and the same puffs' smoke (its first renderer: explosion_fireball_01_smoke in dark grey, 64 of 255, alpha 1,
    # lit), under the fire: the heat added over dark smoke makes CS2's dark red-orange fireball, then the black smoke
    # thinning to nothing by about +90. Peach and half transparent at first, as before 2026-10-30, the added orange
    # washed out against sunlit walls.
    {"role": "blast_smoke", "slots": 3, "life": (1.5, 2.1), "radius": (90.0, 180.0), "scale": (0.8, 1.0),
     "alpha": (1.0, 1.0), "roll": 180.0, "spread": 30.0, "rise": 0.0, "bright": 1.0, "tint": (0.15, 0.14, 0.13),
     "translucent": True, "lift": 0.0, "up": (90.0, 180.0), "speed": (0.0, 60.0), "drag": 0.1, "gravity": -50.0,
     "fade_in": 0.0, "fade_out": 0.6, "link": "blast_core"},
    # explosion_hegrenade_smoke_ground: puffs rolling out along the ground in a ring (born 20 out, radius 75 – 125
    # grown from a third, out at 25 – 200 units/s and slowed by drag 0.1; colour 64 of 255), burning orange at first
    # as the fireball does (the same heat sheet, at an overbright of 5: CS2's orange puffs bursting out 4 – 17 ticks
    # after the blast, on A long), then a low haze thinning with the rest, gone by about +105 (+118 indoors).
    {"name": "blast_ground", "role": "blast_smoke", "slots": 8, "life": (1.25, 1.9), "radius": (75.0, 125.0),
     "scale": (0.8, 1.0), "alpha": (0.6, 0.7), "roll": 180.0, "spread": 20.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.18, 0.17, 0.16), "translucent": True, "speed": (25.0, 200.0), "drag": 0.1, "lift": 0.3,
     "fade_in": 0.0, "fade_out": 0.6, "link": "blast_ground"},
    {"name": "blast_ground_heat", "role": "blast_heat", "slots": 8, "life": (0.85, 1.25), "radius": (75.0, 125.0),
     "scale": (0.8, 1.0), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 20.0, "rise": 0.0, "bright": 4.0,
     "tint": FIRE_ORANGE, "size": 1.0, "speed": (25.0, 200.0), "drag": 0.1, "lift": 0.3, "fade_in": 0.0,
     "fade_out": 0.5, "sort": 1, "link": "blast_ground"},
    # explosion_hegrenade_smoketrails: 7 tongues of fire flung out at up to 1400 units/s (±1000 each way) and 250 - 1000
    # up, falling (gravity 250, drag 0.05), each a ball of fire (explosion_fireball_01_flame, overbright 40) trailing a
    # rope of dark smoke (smoke_trail_01_loop): the arcs CS2 throws out of its blasts (the lower B tunnel, +4 - +42).
    # Here the head in the fireball's heat sheet and the rope a streak of its smoke from where it was thrown (linked:
    # the same particles), both gone with the rest or at the ground.
    {"name": "blast_trails", "role": "blast_smoke", "slots": 7, "life": (1.5, 2.1), "radius": (4.0, 8.0),
     "scale": (1.0, 0.6), "alpha": (0.8, 0.9), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 1.0,
     "tint": (0.1, 0.1, 0.1), "translucent": True, "size": 1.0, "lift": 0.0, "speed": (300.0, 1400.0),
     "up": (250.0, 1000.0), "gravity": 250.0, "drag": 0.05, "from_birth": True, "fade_in": 0.0, "fade_out": 0.5,
     "floor": True, "link": "blast_trails"},
    {"name": "blast_trail_fire", "role": "blast_heat", "slots": 7, "life": (1.5, 2.1), "radius": (8.0, 14.0),
     "scale": (1.0, 0.333), "alpha": (1.0, 1.0), "roll": 180.0, "spread": 0.0, "rise": 0.0, "bright": 8.0,
     "tint": FIRE_ORANGE, "size": 1.0, "lift": 0.0, "speed": (300.0, 1400.0), "up": (250.0, 1000.0),
     "gravity": 250.0, "drag": 0.05, "fade_in": 0.0, "fade_out": 0.5, "sort": 1, "link": "blast_trails",
     "floor": True},
    # explosion_hegrenade_sparktrails: 3 - 5 carriers flung out at up to 1400 units/s and 250 - 1000 up (gravity 500),
    # each shedding streaks of sparks (sparks.vtex sequence 4, 252 / 209 / 187, self-lit) for half a second. Drawn as
    # single streaks along such flights, each as long as 0.06 s of it: the lines CS2's picture shows. Orange and dim
    # (CS2's are soft orange 45 ticks after a blast; at 4 x their colour they were hard white rays).
    {"name": "sparks", "role": "sparks", "slots": 24, "life": (0.3, 0.7), "radius": (1.5, 3.0), "scale": (1.0, 0.3),
     "alpha": (1.0, 1.0), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 1.2, "tint": (1.0, 0.62, 0.36),
     "size": 1.0, "lift": 0.0, "speed": (300.0, 1400.0), "up": (250.0, 1000.0), "gravity": 500.0, "drag": 0.025,
     "stretch": 0.06, "max_length": 100.0, "sequence": (4, 1), "floor": True},
    # explosion_hegrenade_sparktrails_sparks_dots: the glow the sparks start with (radius 10 - 40 at 0.25 - 0.5 of it,
    # 0.06 - 0.25 s).
    {"name": "spark_glow", "role": "spark_glow", "slots": 6, "life": (0.0625, 0.25), "radius": (10.0, 40.0),
     "scale": (1.0, 0.2), "alpha": (1.0, 1.0), "roll": 0.0, "spread": 10.0, "rise": 0.0, "bright": 1.5,
     "tint": (0.99, 0.8, 0.74), "size": 0.375, "lift": 0.0, "speed": (50.0, 100.0)},
    # explosion_hegrenade_debris_small: flecks (fleks.vtex sequences 15 - 39) thrown out at up to 250 units/s (plus up
    # to 300 more from where they are born: 350 here, together) and 62 - 500 up, falling hard (gravity 200 and a pull
    # of 400 - 600) until they touch the world; grey, alpha-blended. Radius
    # 0.25 - 2.5 in CS2; 1 - 3 here (a sprite that small is a speck at any distance).
    {"name": "debris", "role": "debris", "slots": 24, "life": (3.0, 4.0), "radius": (1.0, 3.0), "scale": (1.0, 0.5),
     "alpha": (1.0, 1.0), "roll": 180.0, "spread": 10.0, "rise": 0.0, "bright": 1.0, "tint": (0.55, 0.55, 0.55),
     "translucent": True, "size": 1.0, "lift": 0.0, "speed": (0.0, 350.0), "up": (62.5, 500.0), "gravity": 700.0,
     "drag": 0.025, "sequence": (15, 25), "floor": True},
]
FLASH_LAYERS = [
    # particles/explosions_fx/explosion_flashbang: one bright flare.
    {"role": "flare", "slots": 1, "life": (0.15, 0.25), "radius": (120.0, 160.0), "scale": (1.0, 1.6),
     "alpha": (1.0, 1.0), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 8.0},
]
# Depth fade of the sprites against the world, cm.
SOFT_EDGE_CM = 30.0
# A sprite's half-size is SPRITE_SIZE x the particle's radius: CS2's radii read as half-sizes drew flames twice as
# tall as CS2's (a molotov on de_nuke at the same tick, 2026-10-24).
SPRITE_SIZE = 0.5
# What a layer without these keys has: no flight, no streak, every sheet sequence, no ground; fading in over its first
# tenth and out over its last third; born at its centre; no least width; placed in the world. `up` is along the
# instance's axis (CUSTOM_DATA; straight up for fires and blasts), `speed` across it; `along` (units) where along the
# axis a particle is born; `fade_in`, `fade_out` shares of its life (0: none); `min_width` the least half-width of a
# streak with a reach (a tracer) as a share of its distance from the camera; `local`: its slots' centres and axes are in
# the space of the component it is drawn by (a first-person muzzle flash, on the viewmodel moving with the camera), not
# the world's (such a layer has no floor: see SPRITE_STATE); `roll_base` (degrees) what its roll (`roll`: +- degrees)
# is drawn around; `min_distance` (units) how far the camera must be from its centre for it to show (0: always; a
# blood's flash, CS2's `blood_impact_friendly_flash`, shows only from afar); `age` the share of its life a particle is
# born at (min, max: CS2's C_INIT_AgeNoise; a blood splash is already grown on its first frame); `from_birth`: drawn as
# a streak from where the particle was born to where it is (CS2's blood trails), its tail sagging `tail_drop` units in
# a second; `sort` its translucency sort priority (higher draws over lower where they overlap: a blast's fire over
# its smoke, as CS2 draws the same particle's renderers in order); `link` a name the layers drawing the same
# particles share (CS2's renderers of one particle: a fireball's smoke and its fire), so their particles are born with
# the same seeds; with the same flight, life and size they fly as one. `up` may be negative: down the axis.
# `gradient` names its colours in GRADIENTS (CS2's colour lookup; None: the sheet's own), mixed in by
# `gradient_blend`, looked up lower as the particle fades by `gradient_fade` (0 - 1: the share the lookup has lost
# when it has faded out); `flat`: lying across its axis instead of facing the camera (no soft edge: it lies on the
# ground); `near_side`: shown only where its axis, pointing into its fire, does not face the way the camera looks;
# `frame_span`: the share of its sheet's frames a life plays (a fireball whose sheet turns to smoke within a quarter of
# its frames, played over the first 40 %).
LAYER_DEFAULTS = {"size": SPRITE_SIZE, "lift": 0.6, "speed": (0.0, 0.0), "up": (0.0, 0.0), "gravity": 0.0,
                  "drag": 0.0, "stretch": 0.0, "max_length": 0.0, "sequence": None, "floor": False,
                  "tint": (1.0, 1.0, 1.0), "hot": None, "translucent": False, "end_fade": 0.0, "fade_in": 0.1,
                  "fade_out": 0.33, "along": (0.0, 0.0), "min_width": 0.0, "local": False, "roll_base": 0.0,
                  "min_distance": 0.0, "age": (0.0, 0.0), "from_birth": False, "tail_drop": 0.0, "sort": 0,
                  "link": None, "gradient": None, "gradient_blend": 1.0, "gradient_fade": 0.0, "flat": False,
                  "near_side": False, "frame_span": 1.0}
# CS2's particle drag takes a share of the velocity per simulation step; OFAST reads it as per 1/30 s (an assumption:
# the step is not in the files) and turns it into a rate per second, -30 ln(1 - drag): 0.025 -> 0.76, 0.1 -> 3.2.
DRAG_STEPS_PER_SECOND = 30.0
# Below the ground: what a layer that does not die there gets as its floor (cm).
NO_FLOOR_CM = -1.0e7
# How far below a blast its debris may fall when the agents manifest knows no ground there (units).
FALLBACK_DROP = 128.0
# Per-instance data: centre x, y, z (cm; a `local` layer's in its component's space, as its axis), first and last tick
# particles may be born, phase (0 - 1 of LifeMax: where in its first lifetime a slot starts, so a flame's slots do not
# all respawn together; 0 for a one-shot, born on its tick), seed (0 - 100: small, so the shaders' sin() hashes keep
# their precision), the floor (cm) a particle dies at,
# the axis its particles fly along (Unreal axes; up for a fire's or a blast's, a surface's normal for a bullet's
# impact, the barrel for a muzzle flash, the bullet's way for a tracer) and its reach (units along the axis it dies
# past: a tracer's way to what its bullet met; 0: none).
CUSTOM_DATA = ["CenterX", "CenterY", "CenterZ", "FirstTick", "LastTick", "Phase", "Seed", "FloorZ", "AxisX", "AxisY",
               "AxisZ", "Reach"]
# What a per-instance value an instance does not set reads (one written before the value existed).
CUSTOM_DEFAULTS = {"FloorZ": NO_FLOOR_CM, "AxisZ": 1.0}
# A fire's light: colour, intensity (candela) with LIGHT_FLAMES flames burning (the most a fire had in round 5 on
# de_nuke), reach (units), keyed every this many ticks.
LIGHT_COLOR = (1.0, 0.55, 0.22)
LIGHT_INTENSITY = 60.0
LIGHT_FLAMES = 16
LIGHT_RADIUS = 450.0
FLICKER_TICKS = 4
# A blast's and a flashbang's light: colour, peak intensity (candela; unitless with a `falloff`, spawn_light), reach
# (units), and how it dies out: (ticks after the detonation, share of the peak). CS2's blast light lives about a third
# of a second, its flash's less than two ticks: 2 – 4 ticks after two flashbangs of Aurora vs FURIA's round 17
# (molodoy's view at 154318, 376 units away; MAJ3R's at 153229, 409 units) CS2 lights nothing, where 40 – 60 % of
# this pulse turned a wall white (2026-10-30).
# (1500 cd over 600 units washed a whole room out white 2 ticks after a blast 272 units away.) The blast's is CS2's
# explosion_hegrenade_light: orange (255, 108, 34), 0.225 s, whole for its first quarter and fading out over the rest
# (C_OP_FadeOutSimple 0.75); its intensity (3) and radius (160) have no Unreal units, so the peak is fitted to CS2's
# picture: 2 – 6 ticks after the blast behind de_dust2's long doors (molodoy, 151909 and 151913) CS2 turns the wall and
# ground in front of them red-orange, where 200 cd of a paler orange over twice as long showed nothing there. CS2's
# particle lights fade smoothly to their radius: an inverse-square light strong enough to turn that wall orange (600
# cd) burned the door beside the blast yellow-white (KSCERATO, 151912), where CS2 shows it soft orange; so a falloff
# light (exponent 2), reaching 260 units (CS2's 160 and some: at 400 it lit the wall beside KSCERATO). Its colour in
# linear terms, as FIRE_ORANGE; at 12 that red lit the whole doorway and the ground round it deep red (mid doors, +0 -
# +10), where CS2 tints the walls next to a blast orange (the lower B tunnel, +1 - +5).
BLAST_LIGHT = {"color": (1.0, 0.15, 0.016), "peak": 8.0, "radius": 260.0, "falloff": 2.0,
               "decay": [(0, 1.0), (4, 1.0), (14, 0.0)]}
FLASH_LIGHT = {"color": (1.0, 1.0, 1.0), "peak": 600.0, "radius": 600.0,
               "decay": [(0, 1.0), (1, 0.15), (2, 0.0)]}
# Blindings per player the flashed material holds (two per vector parameter).
FLASHED_SLOTS = 12
# How white a blinding makes the view: FLASH_WHITE x saturate((seconds left - FLASH_END) / FLASH_FADE), in a warm
# white. Fitted to CS2 in round 5 on de_nuke (2026-10-24): 0.2 s into a 0.75 s blinding the view is not visibly
# white; 6 ticks into a 3.8 s one (FalleN, tick 34359) CS2's walls and floor read (181 – 190, 173 – 182, 167 – 175)
# where 0.8 gave (219 – 223, 211 – 215, 194 – 198), and the blend weight solved from each channel is 0.58 – 0.66;
# 0.6 rendered (182 – 191, 174 – 182, 158 – 167), and the colour's blue raised from 0.9 to 0.95 brings blue up 8.
FLASH_WHITE = 0.6
FLASH_END = 0.5
FLASH_FADE = 2.5
FLASH_COLOR = (1.0, 0.98, 0.95)

assets = unreal.EditorAssetLibrary
mel = unreal.MaterialEditingLibrary
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)


def log(message):
    unreal.log("OFAST: " + message)


# The particle a slot shows at the demo tick: its age, normalised age, nine hashes (0 - 1) of the slot's seed and the
# respawn count, where its flight has taken it (cm) and whether it is alive. Shared by the vertex and pixel shaders (a
# Custom node cannot define functions). Half a tick of slack: a one-shot's particle is born exactly on its first and
# last tick. The flight, in the frame of the instance's axis (straight up unless it says otherwise): born Along units
# up the axis, out along a random heading across it at Speed and along it at Up (units/s), slowed by Drag (a rate per
# second: v' = -Drag v) and pulled down by Gravity; without drag the plain parabola (the drag formulas lose their
# precision as Drag goes to 0). A particle below FloorZ has hit the ground and is gone; one with a Reach is gone past
# it (drawn as a streak, once its tail is: `head` and `tail` are its streak's ends along the axis, units). A Local
# layer's centre and axis are in its component's space: the vertex shader is given them in the world's (LocalOrigin,
# LocalAxis: the material's transform nodes), the pixel shader as they are (it needs no place but for the floor test,
# and a local layer has no floor).
SPRITE_STATE = r"""
float t = (Tick - FirstTick) / TicksPerSecond;
float cycle = floor((t + Phase * LifeMax) / LifeMax);
float age = t + Phase * LifeMax - cycle * LifeMax;
float born = Tick - age * TicksPerSecond;
float2 key = float2(Seed, cycle);
float h1 = frac(sin(dot(key, float2(12.9898, 78.233))) * 43758.5453);
float h2 = frac(sin(dot(key, float2(39.3468, 11.135))) * 24634.6345);
float h3 = frac(sin(dot(key, float2(73.156, 52.235))) * 63458.2342);
float h4 = frac(sin(dot(key, float2(27.619, 57.583))) * 31337.1234);
float h5 = frac(sin(dot(key, float2(91.211, 13.917))) * 12345.6789);
float h6 = frac(sin(dot(key, float2(19.773, 83.517))) * 54321.9876);
float h7 = frac(sin(dot(key, float2(45.164, 94.673))) * 21345.4321);
float h8 = frac(sin(dot(key, float2(68.721, 31.297))) * 45231.7654);
float h9 = frac(sin(dot(key, float2(53.871, 26.413))) * 37192.8355);
float h10 = frac(sin(dot(key, float2(84.317, 47.129))) * 28461.3579);
float life = lerp(LifeMin, LifeMax, h1);
// Born already AgeMin - AgeMax of its life old (CS2's C_INIT_AgeNoise): grown, faded, further through its frames and
// along its flight from the start, dying that much sooner (`flown`: the time its flight has taken). CS2's blood streaks
// reach 25 - 35 units from the wound one tick after a hit (MAJ3R's view of his hits on yuurih, ticks 153347 and
// 153354), where their speeds alone take them 2 - 5.
float flown = age + lerp(AgeMin, AgeMax, h10) * life;
float norm = flown / life;
float heading = h3 * 6.2831853;
float3 origin = Local > 0.5 ? LocalOrigin : float3(CenterX, CenterY, CenterZ);
float3 axis = Local > 0.5 ? LocalAxis : float3(AxisX, AxisY, AxisZ);
axis = length(axis) > 0.001 ? normalize(axis) : float3(0.0, 0.0, 1.0);
float3 side = abs(axis.z) < 0.999 ? normalize(cross(axis, float3(0.0, 0.0, 1.0))) : float3(1.0, 0.0, 0.0);
float3 outward = side * cos(heading) + cross(axis, side) * sin(heading);
float speed = lerp(SpeedMin, SpeedMax, h7);
float upward = lerp(UpMin, UpMax, h8);
bool drags = Drag > 0.01;
float slow = drags ? (1.0 - exp(-Drag * flown)) / Drag : flown;
float fall = drags ? (flown - slow) / Drag : 0.5 * flown * flown;
float along = lerp(AlongMin, AlongMax, h9);
float axial = along + upward * slow;
float3 place = origin
    + (outward * (sqrt(h4) * Spread + speed * slow) + axis * axial - float3(0.0, 0.0, 1.0) * Gravity * fall) * 2.54;
float head = Reach > 0.0 ? min(axial, Reach) : axial;
// A streak with a reach (a tracer) grows to its length over its first Stretch seconds, as CS2's trails do: one tick
// after a shot CS2 shows only the part of the tracer near its head, not a line back to the muzzle.
float trail = min(abs(upward) * Stretch, MaxLength) * (Reach > 0.0 ? saturate(age / max(Stretch, 0.001)) : 1.0);
float tail = max(axial - trail, along);
bool reached = Reach > 0.0 && (Stretch > 0.0 ? tail >= Reach : axial > Reach);
// A layer with EndFade (s) dies out that fast after the slot's last tick, whatever its age: a fire's flames go out
// with the fire, CS2's system being stopped then; without, a particle lives out its life.
float ending = (LastTick + EndFade * TicksPerSecond - Tick) / max(EndFade * TicksPerSecond, 0.001);
float end_fade = EndFade > 0.0 ? saturate(ending) : 1.0;
bool alive = Tick >= FirstTick && born >= FirstTick - 0.5 && born <= LastTick + 0.5 && norm <= 1.0
    && place.z >= FloorZ && end_fade > 0.0 && !reached;
"""
# Vertex shader: the world position offset putting the quad's corner where the particle's camera-facing sprite has it.
SPRITE_WPO = SPRITE_STATE + r"""
float2 corner = float2(UV.x * 2.0 - 1.0, 1.0 - UV.y * 2.0);
float roll = radians(RollBase + (h5 * 2.0 - 1.0) * Roll);
corner = float2(corner.x * cos(roll) - corner.y * sin(roll), corner.x * sin(roll) + corner.y * cos(roll));
// Shown only with the camera at least MinDistance units from its centre (0: always).
bool too_near = MinDistance > 0.0 && length(CameraPos - origin) < MinDistance * 2.54;
float radius = alive && !too_near ? lerp(RadiusMin, RadiusMax, h2) * lerp(ScaleStart, ScaleEnd, norm) * 2.54 : 0.0;
// Lifted by Lift of its size (most of it for flames: the frames' flames sit low in their cells, so the sprite stands
// on the ground rather than half in it), rising as it ages.
float3 centre = place + float3(0.0, 0.0, 1.0) * (Rise * norm * 2.54 + radius * Lift);
float3 forward = CameraPos - centre;
forward = length(forward) > 0.001 ? normalize(forward) : float3(1.0, 0.0, 0.0);
float3 right = cross(forward, float3(0.0, 0.0, 1.0));
right = length(right) > 0.001 ? normalize(right) : float3(0.0, 1.0, 0.0);
float3 up = cross(right, forward);
if (Flat > 0.5)
{
    // Lying across its axis, rolled (CS2's PARTICLE_ORIENTATION_ALIGN_TO_PARTICLE_NORMAL: flames flat on the
    // ground under a fire), whatever the camera.
    return centre + (side * corner.x + cross(axis, side) * corner.y) * radius - WorldPos;
}
if (FromBirth > 0.5)
{
    // A streak from where the particle was born to where it is (CS2's blood trails, whose tail is the birth point,
    // sagging TailDrop units in a second, about as t^2.2), at least its radius long either way, as wide as its radius,
    // facing the camera; the sheet's frame stands along it, its top at the particle.
    float3 tail_at = origin + (outward * sqrt(h4) * Spread + axis * along) * 2.54
        - float3(0.0, 0.0, 1.0) * TailDrop * pow(max(flown, 0.0), 2.2) * 2.54;
    float3 span = centre - tail_at;
    float span_length = length(span);
    float3 span_dir = span_length > 0.001 ? span / span_length : up;
    float3 middle = (centre + tail_at) * 0.5;
    float3 seen_from = normalize(CameraPos - middle);
    float3 across = cross(seen_from, span_dir);
    across = length(across) > 0.001 ? normalize(across) : right;
    float half_length = alive ? max(span_length * 0.5, radius) : 0.0;
    return middle + span_dir * corner.y * half_length + across * corner.x * radius - WorldPos;
}
if (Stretch > 0.0 && Reach > 0.0)
{
    // A streak with a reach (a tracer): from its tail to its head up the axis, as wide as its radius and at least
    // MinWidth of its distance from the camera (CS2 keeps its tracers a pixel or so wide however far they are).
    float3 middle = origin + axis * (head + tail) * 0.5 * 2.54;
    float3 seen_from = normalize(CameraPos - middle);
    float3 across = cross(seen_from, axis);
    across = length(across) > 0.001 ? normalize(across) : right;
    float width = alive ? max(radius, MinWidth * length(CameraPos - middle)) : 0.0;
    float half_length = alive ? (head - tail) * 0.5 * 2.54 : 0.0;
    return middle + axis * corner.y * half_length + across * corner.x * width - WorldPos;
}
if (Stretch > 0.0)
{
    // A streak along its flight as the camera sees it: as long as Stretch seconds of the flight (MaxLength units at
    // most), as wide as its radius; the sheet's frame stands along it.
    float decay = drags ? exp(-Drag * flown) : 1.0;
    float sinking = drags ? Gravity / Drag * (1.0 - decay) : Gravity * flown;
    float3 velocity = ((outward * speed + axis * upward) * decay - float3(0.0, 0.0, 1.0) * sinking) * 2.54;
    float3 sideways = velocity - dot(velocity, forward) * forward;
    float seen = length(sideways);
    float3 along = seen > 0.001 ? sideways / seen : up;
    float3 across = normalize(cross(forward, along));
    float half_length = alive ? max(min(length(velocity) * Stretch, MaxLength * 2.54) * 0.5, radius) : 0.0;
    return centre + along * corner.y * half_length + across * corner.x * radius - WorldPos;
}
return centre + (right * corner.x + up * corner.y) * radius - WorldPos;
"""
# Pixel shader: the sequence's frame at the particle's age, faded in over the FadeIn share of its life and out over the
# FadeOut share (a tenth and a third unless the layer says otherwise); (colour x tint x brightness, opacity) in a
# float4.
SPRITE_PIXEL = SPRITE_STATE + r"""
if (!alive) return float4(0.0, 0.0, 0.0, 0.0);
float sequence = SequenceFirst + min(floor(h6 * Sequences), Sequences - 1.0);
float frame = min(floor(norm * Frames), Frames - 1.0);
float cell = sequence * Stride + frame;
float row = floor((cell + 0.5) / Columns);
float column = cell - row * Columns;
float4 texel = Tex.Sample(TexSampler, (float2(column, row) + UV) / float2(Columns, Rows));
float fade_in = FadeIn > 0.0 ? saturate(norm / FadeIn) : 1.0;
float fade_out = FadeOut > 0.0 ? saturate((1.0 - norm) / FadeOut) : 1.0;
float alpha = lerp(AlphaMin, AlphaMax, h2) * fade_in * fade_out * texel.a * end_fade;
if (NearSide > 0.5)
{
    // Only on the side of its fire the camera sees (its axis points into the fire): CS2 kills the outline's flames
    // whose outward normal faces the way it looks (molotov_groundfire_outlineset: none from a dot of 0.5 on).
    float2 look = normalize((origin - CameraPos).xy + 0.001);
    float2 outward = -normalize(axis.xy + 0.001);
    alpha *= saturate(1.0 - 2.0 * dot(outward, look));
}
float3 rgb = texel.rgb;
if (Gradient > 0.5)
{
    // CS2's 1D colour lookup (m_bReplaceTextureWithGradient): the texel's brightness, as the sheet stores it (sRGB),
    // picks a colour along the layer's gradient (GRADIENTS, sRGB 0 - 255), mixed into the texel's own by
    // GradientBlend (CS2's m_flTextureBlend).
    float v = pow(saturate(dot(texel.rgb, float3(0.299, 0.587, 0.114))), 1.0 / 2.2);
    // With GradientFade, the particle's fading takes the lookup down the gradient too: a fading edge flame turns
    // red, purple and blue before it goes (CS2's fire edge, its magenta fringe).
    v *= lerp(1.0, fade_in * fade_out * end_fade, GradientFade);
    float4 stops[GRADIENT_STOPS];
GRADIENT_TABLE
    float3 shade = stops[0].yzw;
    for (int i = 1; i < GRADIENT_STOPS; i++)
    {
        if (v > stops[i - 1].x)
        {
            float span = max(stops[i].x - stops[i - 1].x, 0.0001);
            shade = lerp(stops[i - 1].yzw, stops[i].yzw, saturate((v - stops[i - 1].x) / span));
        }
    }
    rgb = lerp(rgb, pow(shade / 255.0, 2.2), GradientBlend);
}
// Born Hot (rgb), cooling to Tint over the share of its life in Hot.a (a blast's smoke is lit by its own fire at
// first); Hot.a = 0: Tint all along.
float3 colour = lerp(Hot.rgb, Tint, saturate(norm / max(Hot.a, 0.0001)));
return float4(rgb * colour * Bright, alpha);
"""
# Scalar parameters of the Custom nodes (each a material parameter), besides the per-instance data and the tick.
SPRITE_SCALARS = ["TicksPerSecond", "LifeMin", "LifeMax", "RadiusMin", "RadiusMax", "ScaleStart", "ScaleEnd", "Roll",
                  "Spread", "Rise", "AlphaMin", "AlphaMax", "Bright", "Sequences", "SequenceFirst", "Frames", "Stride",
                  "Columns", "Rows", "Lift", "SpeedMin", "SpeedMax", "UpMin", "UpMax", "Gravity", "Drag", "Stretch",
                  "MaxLength", "EndFade", "FadeIn", "FadeOut", "AlongMin", "AlongMax", "MinWidth", "Local", "RollBase",
                  "MinDistance", "AgeMin", "AgeMax", "FromBirth", "TailDrop", "Gradient", "GradientBlend",
                  "GradientFade", "Flat", "NearSide"]
# A scalar's value in the material itself, where not 1 (what an instance that does not set it reads): a layer is placed
# in the world, its roll centred on 0, shown from any distance, born new, in its sheet's colours and facing the
# camera from any side, unless its instance says otherwise.
SCALAR_DEFAULTS = {"Local": 0.0, "RollBase": 0.0, "MinDistance": 0.0, "AgeMin": 0.0, "AgeMax": 0.0, "FromBirth": 0.0,
                   "TailDrop": 0.0, "Gradient": 0.0, "GradientFade": 0.0, "Flat": 0.0, "NearSide": 0.0}


def gradient_table():
    """The HLSL filling SPRITE_PIXEL's `stops` (float4: position, sRGB colour) with the gradient `Gradient` picks
    (1-based, in GRADIENTS' order), each padded to GRADIENT_STOPS with its last stop."""
    lines = []
    for index, stops in enumerate(GRADIENTS.values(), 1):
        stops = stops + [stops[-1]] * (GRADIENT_STOPS - len(stops))
        body = " ".join("stops[%d] = float4(%g, %g, %g, %g);" % (k, p, r, g, b)
                        for k, (p, (r, g, b)) in enumerate(stops))
        lines.append("    %sif (Gradient < %d.5) { %s }" % ("else " if index > 1 else "", index, body))
    return "\n".join(lines)


SPRITE_PIXEL = SPRITE_PIXEL.replace("GRADIENT_TABLE", gradient_table()).replace("GRADIENT_STOPS",
                                                                                str(GRADIENT_STOPS))


def sprite_material(translucent):
    """SPRITE_MATERIALS[translucent], built on first use and rebuilt in place on every run (the layers' instances keep
    it): unlit, two-sided, for instanced static meshes; additive (emissive = colour x opacity) or translucent
    (emissive = colour, opacity)."""
    material = smoke_volumes.empty_material(SPRITE_MATERIALS[translucent])
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT if translucent
                                 else unreal.BlendMode.BLEND_ADDITIVE)
    material.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    material.set_editor_property("two_sided", True)
    material.set_editor_property("used_with_instanced_static_meshes", True)
    shared = ["Tick", "LocalOrigin", "LocalAxis"] + CUSTOM_DATA + SPRITE_SCALARS
    output = unreal.CustomMaterialOutputType
    wpo = smoke_volumes.custom_node(material, SPRITE_WPO, output.CMOT_FLOAT3, ["UV", "WorldPos", "CameraPos"] + shared,
                                    -400, 400)
    pixel = smoke_volumes.custom_node(material, SPRITE_PIXEL, output.CMOT_FLOAT4,
                                      ["UV", "Tex", "Tint", "Hot", "CameraPos"] + shared, -400, -200)

    def connect(node, input_name, *targets):
        for target in targets:
            if not mel.connect_material_expressions(node, "", target, input_name):
                raise RuntimeError("sprite material: could not connect " + input_name)

    connect(mel.create_material_expression(material, unreal.MaterialExpressionTextureCoordinate, -1200, -600), "UV",
            wpo, pixel)
    connect(mel.create_material_expression(material, unreal.MaterialExpressionWorldPosition, -1200, 500), "WorldPos",
            wpo)
    connect(mel.create_material_expression(material, unreal.MaterialExpressionCameraPositionWS, -1200, 600),
            "CameraPos", wpo, pixel)
    texture = mel.create_material_expression(material, unreal.MaterialExpressionTextureObjectParameter, -1200, -500)
    texture.set_editor_property("parameter_name", "Tex")
    texture.set_editor_property("texture", assets.load_asset("/Engine/EngineResources/Black"))
    connect(texture, "Tex", pixel)
    tint = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -1200, -700)
    tint.set_editor_property("parameter_name", "Tint")
    tint.set_editor_property("default_value", unreal.LinearColor(1.0, 1.0, 1.0, 1.0))
    connect(tint, "Tint", pixel)
    hot = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -1200, -800)
    hot.set_editor_property("parameter_name", "Hot")
    hot.set_editor_property("default_value", unreal.LinearColor(1.0, 1.0, 1.0, 0.0))
    # All four channels (the default output is RGB only): the alpha is the cooling share.
    if not mel.connect_material_expressions(hot, "RGBA", pixel, "Hot"):
        raise RuntimeError("sprite material: could not connect Hot")
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -1200, -400)
    tick.set_editor_property("collection", smoke_volumes.replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    connect(tick, "Tick", wpo, pixel)
    data = {}
    for index, name in enumerate(CUSTOM_DATA):
        node = mel.create_material_expression(material, unreal.MaterialExpressionPerInstanceCustomData, -1200,
                                              -300 + 60 * index)
        node.set_editor_property("data_index", index)
        # A level imported before a value existed has none: no floor (not one at z = 0), straight up.
        if name in CUSTOM_DEFAULTS:
            node.set_editor_property("const_default_value", CUSTOM_DEFAULTS[name])
        connect(node, name, wpo, pixel)
        data[name] = node
    scalars = {}
    for index, name in enumerate(SPRITE_SCALARS):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900,
                                              -300 + 60 * index)
        node.set_editor_property("parameter_name", name)
        node.set_editor_property("default_value", SCALAR_DEFAULTS.get(name, 1.0))
        connect(node, name, wpo, pixel)
        scalars[name] = node

    # A Local layer's centre and axis in the world's space for the vertex shader (its instances sit unscaled at their
    # component's origin, so the instance's local space is the component's), as they are for the pixel shader
    # (SPRITE_STATE).
    def vector(names, y):
        xy = mel.create_material_expression(material, unreal.MaterialExpressionAppendVector, -1050, y)
        mel.connect_material_expressions(data[names[0]], "", xy, "A")
        mel.connect_material_expressions(data[names[1]], "", xy, "B")
        xyz = mel.create_material_expression(material, unreal.MaterialExpressionAppendVector, -950, y)
        mel.connect_material_expressions(xy, "", xyz, "A")
        mel.connect_material_expressions(data[names[2]], "", xyz, "B")
        return xyz

    centre = vector(["CenterX", "CenterY", "CenterZ"], 900)
    axis = vector(["AxisX", "AxisY", "AxisZ"], 1000)
    placed = mel.create_material_expression(material, unreal.MaterialExpressionTransformPosition, -800, 900)
    placed.set_editor_property("transform_source_type",
                               unreal.MaterialPositionTransformSource.TRANSFORMPOSSOURCE_LOCAL)
    placed.set_editor_property("transform_type", unreal.MaterialPositionTransformSource.TRANSFORMPOSSOURCE_WORLD)
    turned = mel.create_material_expression(material, unreal.MaterialExpressionTransform, -800, 1000)
    turned.set_editor_property("transform_source_type", unreal.MaterialVectorCoordTransformSource.TRANSFORMSOURCE_LOCAL)
    turned.set_editor_property("transform_type", unreal.MaterialVectorCoordTransform.TRANSFORM_WORLD)
    if not (mel.connect_material_expressions(centre, "", placed, "")
            and mel.connect_material_expressions(axis, "", turned, "")):
        raise RuntimeError("sprite material: could not connect the local transforms")
    connect(placed, "LocalOrigin", wpo)
    connect(turned, "LocalAxis", wpo)
    connect(centre, "LocalOrigin", pixel)
    connect(axis, "LocalAxis", pixel)
    # Soft where the sprite meets the ground or a wall (CS2's particles fade by depth the same way).
    depth_fade = mel.create_material_expression(material, unreal.MaterialExpressionDepthFade, -150, 100)
    depth_fade.set_editor_property("fade_distance_default", SOFT_EDGE_CM)
    rgb = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -150, -300)
    rgb.set_editor_property("r", True)
    rgb.set_editor_property("g", True)
    rgb.set_editor_property("b", True)
    rgb.set_editor_property("a", False)
    opacity = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -150, -150)
    for channel in ("r", "g", "b"):
        opacity.set_editor_property(channel, False)
    opacity.set_editor_property("a", True)
    mel.connect_material_expressions(pixel, "", rgb, "")
    mel.connect_material_expressions(pixel, "", opacity, "")
    # A Flat layer lies on the ground: no soft edge (it would fade it out entirely).
    unfaded = mel.create_material_expression(material, unreal.MaterialExpressionMax, -50, 100)
    mel.connect_material_expressions(depth_fade, "", unfaded, "A")
    mel.connect_material_expressions(scalars["Flat"], "", unfaded, "B")
    soft = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, 0, -150)
    mel.connect_material_expressions(opacity, "", soft, "A")
    mel.connect_material_expressions(unfaded, "", soft, "B")
    prop = unreal.MaterialProperty
    if translucent:
        ok = (mel.connect_material_property(rgb, "", prop.MP_EMISSIVE_COLOR)
              and mel.connect_material_property(soft, "", prop.MP_OPACITY))
    else:
        emissive = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, 150, -250)
        mel.connect_material_expressions(rgb, "", emissive, "A")
        mel.connect_material_expressions(soft, "", emissive, "B")
        ok = mel.connect_material_property(emissive, "", prop.MP_EMISSIVE_COLOR)
    if not (ok and mel.connect_material_property(wpo, "", prop.MP_WORLD_POSITION_OFFSET)):
        raise RuntimeError("sprite material: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def sprite_mesh(local=False):
    """SPRITE_MESH (or SPRITE_VIEW_MESH for `local` layers): the engine plane copied once, its bounds extended (see
    SPRITE_BOUNDS_UP, SPRITE_VIEW_BOUNDS)."""
    path = SPRITE_VIEW_MESH if local else SPRITE_MESH
    if assets.does_asset_exist(path):
        mesh = assets.load_asset(path)
    else:
        mesh = assets.duplicate_asset(ENGINE_PLANE, path)
    if local:
        reach = unreal.Vector(SPRITE_VIEW_BOUNDS, SPRITE_VIEW_BOUNDS, SPRITE_VIEW_BOUNDS)
        mesh.set_editor_property("positive_bounds_extension", reach)
        mesh.set_editor_property("negative_bounds_extension", reach)
    else:
        mesh.set_editor_property("positive_bounds_extension", unreal.Vector(0.0, 0.0, SPRITE_BOUNDS_UP))
        mesh.set_editor_property("negative_bounds_extension", unreal.Vector(0.0, 0.0, SPRITE_BOUNDS_DOWN))
    assets.save_loaded_asset(mesh)
    bounds = mesh.get_bounds()
    log("sprite mesh %s bounds: extent %s" % (path.rsplit("/", 1)[-1], bounds.box_extent))
    return mesh


def option(layer, key):
    """A layer's value for `key`, or LAYER_DEFAULTS'."""
    return layer.get(key, LAYER_DEFAULTS[key])


def layer_name(layer):
    """A layer's own name (its material instance's, its components'): `name`, or its texture's role."""
    return layer.get("name", layer["role"])


def instance_scale(layer):
    """The scale of a layer's instances: SPRITE_INSTANCE_SCALE, or more for one whose particles fly further from their
    centre than its bounds reach (SPRITE_MESH's ±50 cm sideways and up to SPRITE_BOUNDS_UP, times the scale): out as
    far as the fastest flies in the longest life (or stops, slowed by drag), plus where it was born and its size. A
    flight along an instance's own axis (a surface's normal, a barrel) may go sideways too: up to `up` and `along`
    count both ways."""
    drag = option(layer, "drag")
    rate = -DRAG_STEPS_PER_SECOND * math.log(1.0 - drag) if drag > 0.0 else 0.0

    def flight(speed):
        return min(speed * layer["life"][1], speed / rate) if rate > 0.0 else speed * layer["life"][1]

    size = max(layer["radius"]) * option(layer, "size") * max(layer["scale"])
    axial = flight(max(abs(u) for u in option(layer, "up"))) + option(layer, "along")[1]
    out = (layer["spread"] + flight(option(layer, "speed")[1]) + axial + size) * UNITS_TO_CM / 50.0
    lifted = size * (1.0 + option(layer, "lift"))
    up = (axial + layer["rise"] + lifted) * UNITS_TO_CM / SPRITE_BOUNDS_UP
    return max(SPRITE_INSTANCE_SCALE, out, up)


def layer_instance(layer, effect, texture, material, tick_rate):
    """The material instance of one layer (shared by every round: CS2's sheets are the same), updated in place when
    it exists (the level's effects keep it): its texture, flipbook layout, CS2's ranges, flight, tint and
    brightness."""
    instance = smoke_volumes.material_instance(FOLDER, "MI_Sprite_" + layer_name(layer), material)
    sequences = effect["sequences"]
    first, count = option(layer, "sequence") or (0, len(sequences))
    first = min(first, len(sequences) - 1)
    count = max(min(count, len(sequences) - first), 1)
    size = option(layer, "size")
    drag = option(layer, "drag")
    values = {
        "TicksPerSecond": tick_rate,
        "LifeMin": layer["life"][0], "LifeMax": layer["life"][1],
        "RadiusMin": layer["radius"][0] * size, "RadiusMax": layer["radius"][1] * size,
        "ScaleStart": layer["scale"][0], "ScaleEnd": layer["scale"][1],
        "Roll": layer["roll"], "Spread": layer["spread"], "Rise": layer["rise"],
        "AlphaMin": layer["alpha"][0], "AlphaMax": layer["alpha"][1], "Bright": layer["bright"],
        # Every sequence plays as many frames as the shortest of the layer's has (they differ by one or two).
        "Sequences": count, "SequenceFirst": first,
        "Frames": max(1, round(option(layer, "frame_span")
                               * min(s["frames"] for s in sequences[first:first + count]))),
        "Stride": effect["stride"], "Columns": effect["columns"], "Rows": effect["rows"],
        "Lift": option(layer, "lift"),
        "SpeedMin": option(layer, "speed")[0], "SpeedMax": option(layer, "speed")[1],
        "UpMin": option(layer, "up")[0], "UpMax": option(layer, "up")[1],
        "Gravity": option(layer, "gravity"),
        "Drag": -DRAG_STEPS_PER_SECOND * math.log(1.0 - drag) if drag > 0.0 else 0.0,
        "Stretch": option(layer, "stretch"), "MaxLength": option(layer, "max_length"),
        "EndFade": option(layer, "end_fade"),
        "FadeIn": option(layer, "fade_in"), "FadeOut": option(layer, "fade_out"),
        "AlongMin": option(layer, "along")[0], "AlongMax": option(layer, "along")[1],
        "MinWidth": option(layer, "min_width"),
        "Local": 1.0 if option(layer, "local") else 0.0,
        "RollBase": option(layer, "roll_base"),
        "MinDistance": option(layer, "min_distance"),
        "AgeMin": option(layer, "age")[0], "AgeMax": option(layer, "age")[1],
        "FromBirth": 1.0 if option(layer, "from_birth") else 0.0, "TailDrop": option(layer, "tail_drop"),
        "Gradient": list(GRADIENTS).index(option(layer, "gradient")) + 1 if option(layer, "gradient") else 0.0,
        "GradientBlend": option(layer, "gradient_blend"),
        "GradientFade": option(layer, "gradient_fade"),
        "Flat": 1.0 if option(layer, "flat") else 0.0, "NearSide": 1.0 if option(layer, "near_side") else 0.0,
    }
    for param, value in values.items():
        mel.set_material_instance_scalar_parameter_value(instance, param, float(value))
    r, g, b = option(layer, "tint")
    mel.set_material_instance_vector_parameter_value(instance, "Tint", unreal.LinearColor(r=r, g=g, b=b, a=1.0))
    hr, hg, hb, cooling = option(layer, "hot") or (r, g, b, 0.0)
    mel.set_material_instance_vector_parameter_value(instance, "Hot", unreal.LinearColor(r=hr, g=hg, b=hb, a=cooling))
    mel.set_material_instance_texture_parameter_value(instance, "Tex", texture)
    assets.save_loaded_asset(instance)
    return instance


def grenade_layers():
    """Every layer of the grenades' effects."""
    return (FIRE_LAYERS + CENTRE_LAYERS + WIDE_CENTRE_LAYERS + EDGE_LAYERS + WALL_LAYERS + SMOKE_LAYERS
            + WALL_SMOKE_LAYERS + MOLOTOV_BURST_LAYERS + INCENDIARY_BURST_LAYERS + BLAST_LAYERS + FLASH_LAYERS)


def update_layers(replay, replay_path, effects, layers=None, rebuild=True):
    """Rebuilds the sprite materials (unless `rebuild` is false: built already in this run) and updates the instance of
    every layer of `layers` (the grenades' unless given) from the layer tables and the manifest's `effects` (the
    level's effects keep them: probe_smoke.py's quick look at a change). Returns layer name -> instance for the layers
    whose texture the export wrote."""
    by_role = {e["role"]: e for e in effects}
    materials = {translucent: (sprite_material(translucent) if rebuild
                               else assets.load_asset(SPRITE_MATERIALS[translucent]))
                 for translucent in (False, True)}
    tick_rate = tick_rate_of(replay)
    textures = {}
    out = {}
    for layer in grenade_layers() if layers is None else layers:
        role = layer["role"]
        effect = by_role.get(role)
        if effect is None:
            continue
        if "stride" not in effect:
            log("effect %s: the agents manifest predates regular flipbooks (no stride): export the round again" % role)
            continue
        if role not in textures:
            textures[role] = smoke_volumes.import_png(replay_path, effect["file"], FOLDER)
            assets.save_loaded_asset(textures[role])
        out[layer_name(layer)] = layer_instance(layer, effect, textures[role],
                                                materials[option(layer, "translucent")], tick_rate)
    return out


def add_component(actor, cls, name):
    """Adds a component of class `cls` to a level actor (as an editor would), returns it."""
    subsystem = unreal.get_engine_subsystem(unreal.SubobjectDataSubsystem)
    root = subsystem.k2_gather_subobject_data_for_instance(actor)[0]
    params = unreal.AddNewSubobjectParams(parent_handle=root, new_class=cls, blueprint_context=None)
    handle, failure = subsystem.add_new_subobject(params)
    if not failure.is_empty():
        raise RuntimeError("could not add %s to %s: %s" % (cls.__name__, actor.get_actor_label(), failure))
    subsystem.rename_subobject(handle, unreal.Text(name))
    data = unreal.SubobjectDataBlueprintFunctionLibrary.get_data(handle)
    return unreal.SubobjectDataBlueprintFunctionLibrary.get_object(data)


def add_sprites(actor, layer, instance, mesh, slots, rng, floor_cm=NO_FLOOR_CM):
    """One instanced mesh of `layer` on `actor`, returned: an instance per slot of `slots`, times the layer's slots;
    its particles die below `floor_cm` (Unreal z). A slot is (centre in Source units, first tick, last tick), or with
    (axis, reach) after them: the direction its particles fly along (Source axes, unit) and how far along it they
    may go (units; 0: no limit): an instance with a reach covers it with its bounds (centred halfway along). A `local`
    layer's centres and axes are in the space of `actor`'s root component (Source axes there: x forward, y left, z up)
    and its instances stay at its origin, unscaled (`mesh` is then SPRITE_VIEW_MESH, whose bounds hold them)."""
    ism = add_component(actor, unreal.InstancedStaticMeshComponent, "sprites_" + layer_name(layer))
    ism.set_static_mesh(mesh)
    ism.set_material(0, instance)
    ism.set_editor_property("cast_shadow", False)
    # Drawn after the layers of a lower `sort` wherever they overlap (Unreal sorts translucent components by
    # distance, which can put a fireball's smoke over its fire).
    ism.set_editor_property("translucency_sort_priority", int(option(layer, "sort")))
    ism.set_num_custom_data_floats(len(CUSTOM_DATA))
    layer_scale = instance_scale(layer)
    local = option(layer, "local")
    for slot in slots:
        (sx, sy, sz), first, last = slot[:3]
        (ax, ay, az), reach = slot[3:5] if len(slot) >= 5 else ((0.0, 0.0, 1.0), 0.0)
        position = unreal.Vector(sx * UNITS_TO_CM, -sy * UNITS_TO_CM, sz * UNITS_TO_CM)
        # Scaled up so the instance's bounds hold the sprites the shader draws around it (instance_scale); one with
        # a reach is put halfway along it, its bounds reaching both ends (SPRITE_MESH: ±50 cm across).
        bounds_at, scale = position, layer_scale
        if local:
            bounds_at, scale = unreal.Vector(0.0, 0.0, 0.0), 1.0
        elif reach > 0.0:
            # A particle with a reach never goes past it: its path decides the bounds, not the layer's flight (a
            # tracer layer's 20 500 units/s over its 1 s life would give every tracer bounds half a kilometre across).
            half = reach * 0.5
            bounds_at = unreal.Vector((sx + ax * half) * UNITS_TO_CM, -(sy + ay * half) * UNITS_TO_CM,
                                      (sz + az * half) * UNITS_TO_CM)
            scale = max(SPRITE_INSTANCE_SCALE, half * UNITS_TO_CM / 50.0 * 1.1)
        for n in range(layer["slots"]):
            index = ism.add_instance(unreal.Transform(bounds_at, unreal.Rotator(0.0, 0.0, 0.0),
                                                      unreal.Vector(scale, scale, scale)), True)
            # A one-shot's particle is born on its tick: a phase would put its birth up to a lifetime before or
            # after it, outside the half tick the shader allows, and it would never show.
            phase = rng.random() if last > first else 0.0
            seed = rng.uniform(0.0, 100.0)
            if option(layer, "link"):
                # The same seed in every layer of the link for this slot and particle: the same particles.
                seed = random.Random("%s %d %d %d %s %d" % (option(layer, "link"), round(sx * 8), round(sy * 8),
                                                            round(sz * 8), first, n)).uniform(0.0, 100.0)
            values = [position.x, position.y, position.z, first, last, phase, seed, floor_cm, ax, -ay, az, reach]
            for k, value in enumerate(values):
                ism.set_custom_data_value(index, k, float(value), k == len(values) - 1)
    return ism


def spawn_light(spawn, label, position, color, radius, shadows=True, falloff=None):
    """A point light at `position` (Source units), keyed later; physical (inverse-square, candela) unless `falloff`
    is given: then Unreal's unitless light, fading to nothing at `radius` as (1 - (d / radius)^2)^falloff."""
    light = spawn(unreal.PointLight, label, (position[0] * UNITS_TO_CM, -position[1] * UNITS_TO_CM,
                                             position[2] * UNITS_TO_CM))
    component = light.get_editor_property("light_component")
    # Named: unreal.Color's positional order is FColor's memory order (b, g, r, a), which made a light blue.
    r, g, b = color
    component.set_light_color(unreal.LinearColor(r=r, g=g, b=b, a=1.0))
    component.set_editor_property("attenuation_radius", radius * UNITS_TO_CM)
    component.set_editor_property("intensity", 0.0)
    component.set_editor_property("cast_shadows", shadows)
    if falloff is not None:
        component.set_editor_property("use_inverse_squared_falloff", False)
        component.set_editor_property("light_falloff_exponent", float(falloff))
    return light


def intensity_channel(sequence, light, end_frame):
    """The float channel keying the light component's intensity."""
    sequence.add_possessable(light)
    binding = sequence.add_possessable(light.get_editor_property("light_component"))
    track = binding.add_track(unreal.MovieSceneFloatTrack)
    track.set_property_name_and_path("Intensity", "Intensity")
    section = track.add_section()
    section.set_range(0, end_frame)
    return section.get_all_channels()[0]


def add_key(channel, frame, value):
    channel.add_key(unreal.FrameNumber(frame), value, 0.0, unreal.MovieSceneTimeUnit.DISPLAY_RATE,
                    unreal.MovieSceneKeyInterpolation.LINEAR)


def burning_flames(fire, tick):
    return sum(1 for f in fire["flames"] if f["first_tick"] <= tick <= f["last_tick"])


def light_intensity(fire, tick, tick_rate, rng=None):
    """A fire's light at a tick: LIGHT_INTENSITY x its burning flames' share of LIGHT_FLAMES (or of all its flames, a
    bigger fire's: one light at its middle, which more flames must not overexpose) x a flicker (two sines, and a
    little noise from `rng` when given)."""
    seconds = (tick - fire["start_tick"]) / tick_rate
    flicker = 0.75 + 0.15 * math.sin(seconds * 23.0) + 0.1 * math.sin(seconds * 37.0 + 1.3)
    if rng is not None:
        flicker += 0.1 * (rng.random() - 0.5)
    return LIGHT_INTENSITY * burning_flames(fire, tick) / max(LIGHT_FLAMES, len(fire["flames"])) * flicker


def light_label(fire):
    return "fire light #%d at %d" % (fire["entity"], fire["start_tick"])


def pulse_intensity(pulse, ticks_after):
    """A blast's or flashbang's light `ticks_after` its detonation (the decay keys, interpolated)."""
    points = pulse["decay"]
    if ticks_after < 0 or ticks_after > points[-1][0]:
        return 0.0
    for (t0, v0), (t1, v1) in zip(points, points[1:]):
        if t0 <= ticks_after <= t1:
            return pulse["peak"] * (v0 + (v1 - v0) * (ticks_after - t0) / max(t1 - t0, 1))
    return pulse["peak"] * points[0][1]


def tick_rate_of(replay):
    return round(1.0 / replay["tick_interval"])


# What goes off with an HE grenade and a flashbang: the sprite layers, the light pulse, the actor's name.
BURSTS = {"he": (BLAST_LAYERS, BLAST_LIGHT, "blast"), "flashbang": (FLASH_LAYERS, FLASH_LIGHT, "flash")}


def bursts_of(replay):
    """The replay's HE grenades and flashbangs that went off (a `detonation`)."""
    return [g for g in replay.get("grenades", []) if g["kind"] in BURSTS and g.get("detonation")]


def burst_label(grenade):
    return "%s #%d at %d" % (BURSTS[grenade["kind"]][2], grenade["entity"], grenade["detonation"]["tick"])


def burst_light_label(grenade):
    return burst_label(grenade) + " light"


def wall_slots(fire, walls):
    """The sprite slots of a fire's wall flames: per wall point the agents manifest found for it (`walls`), its place
    off the wall (WALL_OFFSET out, WALL_RAISE up from the floor) and the ticks any of the flames it was found from
    burns (matched by position: the replay and the manifest both have them to 0.01 unit; a flame that never burned is
    not in the replay)."""
    out = []
    for wall in walls:
        spans = []
        for position in wall["flames"]:
            flame = min(fire["flames"], key=lambda f: math.dist(f["position"], position))
            if math.dist(flame["position"], position) <= 0.5:
                spans.append((flame["first_tick"], flame["last_tick"]))
        if not spans:
            continue
        place = [p + n * WALL_OFFSET for p, n in zip(wall["position"], wall["normal"])]
        place[2] += WALL_RAISE
        out.append((place, min(s[0] for s in spans), max(s[1] for s in spans)))
    return out


def hull(points):
    """The convex hull of (x, y) points, counter-clockwise (Andrew's monotone chain); fewer than 3 points when they
    all lie on a line."""
    points = sorted(set(points))
    if len(points) < 3:
        return points

    def turn(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lower, upper = [], []
    for p in points:
        while len(lower) >= 2 and turn(lower[-2], lower[-1], p) <= 0:
            lower.pop()
        lower.append(p)
    for p in reversed(points):
        while len(upper) >= 2 and turn(upper[-2], upper[-1], p) <= 0:
            upper.pop()
        upper.append(p)
    return lower[:-1] + upper[:-1]


def edge_slots(fire):
    """The sprite slots of a fire's edge flames (EDGE_LAYERS): a point every EDGE_SPACING units along the hull of its
    flames pushed EDGE_OFFSET out (round their middle, for a fire whose flames lie on a line), at the height and
    burning ticks of the flame nearest it, its axis pointing inward (Source axes, level)."""
    flames = fire["flames"]
    corners = hull([(f["position"][0], f["position"][1]) for f in flames])
    cx = sum(f["position"][0] for f in flames) / len(flames)
    cy = sum(f["position"][1] for f in flames) / len(flames)
    points = []
    if len(corners) < 3:
        reach = max([math.hypot(x - cx, y - cy) for x, y in corners] + [0.0]) + EDGE_OFFSET
        count = max(6, round(2.0 * math.pi * reach / EDGE_SPACING))
        for k in range(count):
            a = 2.0 * math.pi * k / count
            points.append(((cx + reach * math.cos(a), cy + reach * math.sin(a)), (math.cos(a), math.sin(a))))
    else:
        for (ax, ay), (bx, by) in zip(corners, corners[1:] + corners[:1]):
            length = math.hypot(bx - ax, by - ay)
            if length < 0.01:
                continue
            # Counter-clockwise: the outside is on the right of the edge.
            nx, ny = (by - ay) / length, -(bx - ax) / length
            steps = max(1, round(length / EDGE_SPACING))
            for k in range(steps):
                s = k / steps
                points.append(((ax + (bx - ax) * s + nx * EDGE_OFFSET, ay + (by - ay) * s + ny * EDGE_OFFSET),
                               (nx, ny)))
    slots = []
    for (px, py), (nx, ny) in points:
        flame = min(flames, key=lambda f: math.hypot(f["position"][0] - px, f["position"][1] - py))
        slots.append(((px, py, flame["position"][2]), flame["first_tick"], flame["last_tick"], (-nx, -ny, 0.0), 0.0))
    return slots


def place_fires(sequence, replay, end_frame, instances, mesh, walls, spawn):
    """One actor per fire of the replay (`fires`), with a sprite slot per flame and layer of FIRE_LAYERS (quads:
    `mesh`), where it lit for CENTRE_LAYERS, per point of its edge for EDGE_LAYERS (edge_slots), per wall point and
    layer of WALL_LAYERS (`walls`: (entity, start tick) -> the agents manifest's wall points), its burst (the fire's
    `burst`: MOLOTOV_BURST_LAYERS or INCENDIARY_BURST_LAYERS thrown along its direction, once), its smoke when it goes
    out (SMOKE_LAYERS over its flames, WALL_SMOKE_LAYERS up its walls) and its flickering light. Returns the number of
    fires placed."""
    fires = [f for f in replay.get("fires", []) if f["flames"]]
    if not fires:
        return 0
    tick0 = replay["start_tick"]
    tick_rate = tick_rate_of(replay)
    (flame_layers, centre_layers, wide_layers, edge_layers, wall_layers, smoke_layers, wall_smoke_layers,
     molotov_layers, incendiary_layers) = (
        [layer for layer in layers if layer_name(layer) in instances]
        for layers in (FIRE_LAYERS, CENTRE_LAYERS, WIDE_CENTRE_LAYERS, EDGE_LAYERS, WALL_LAYERS, SMOKE_LAYERS,
                       WALL_SMOKE_LAYERS, MOLOTOV_BURST_LAYERS, INCENDIARY_BURST_LAYERS))
    placed_walls = bursts = 0
    for fire in fires:
        flames = fire["flames"]
        x, y, z = [sum(f["position"][k] for f in flames) / len(flames) for k in range(3)]
        actor = spawn(unreal.Actor, "fire #%d at %d" % (fire["entity"], fire["start_tick"]),
                      (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
        rng = random.Random(fire["entity"] * 104729 + fire["start_tick"])
        on_walls = wall_slots(fire, walls.get((fire["entity"], fire["start_tick"]), []))
        placed_walls += len(on_walls)
        out, smoke_end = fire["end_tick"], fire["end_tick"] + round(FIRE_SMOKE_SECONDS * tick_rate)
        burst = fire.get("burst")
        lit_at = burst["position"] if burst else flames[0]["position"]
        start, burning = fire["start_tick"], max(f["last_tick"] for f in flames)
        tight_end = min(burning, start + round(CENTRE_TIGHT_S * tick_rate))
        wide_start = start + round(CENTRE_WIDE_S * tick_rate)
        for layers, slots in (
                (flame_layers, [(f["position"], f["first_tick"], f["last_tick"]) for f in flames]),
                (centre_layers, [(lit_at, start, tight_end)]),
                (wide_layers, [(lit_at, wide_start, burning)] if wide_start < burning else []),
                (edge_layers, edge_slots(fire)),
                (wall_layers, on_walls),
                (smoke_layers, [(f["position"], out, smoke_end) for f in flames]),
                (wall_smoke_layers, [(place, out, smoke_end) for place, _, _ in on_walls])):
            for layer in layers:
                if slots:
                    add_sprites(actor, layer, instances[layer_name(layer)], mesh, slots, rng)
        if burst:
            # Its splashes and flecks die at the ground (the fire's lowest flame).
            floor_cm = (min(f["position"][2] for f in flames) - 1.0) * UNITS_TO_CM
            slot = (burst["position"], burst["tick"], burst["tick"], burst["direction"], 0.0)
            for layer in molotov_layers if burst["kind"] == "molotov" else incendiary_layers:
                add_sprites(actor, layer, instances[layer_name(layer)], mesh, [slot], rng,
                            floor_cm if option(layer, "floor") else NO_FLOOR_CM)
            bursts += 1
        light = spawn_light(spawn, light_label(fire), (x, y, z + 30.0), LIGHT_COLOR, LIGHT_RADIUS)
        channel = intensity_channel(sequence, light, end_frame)
        flicker = random.Random(fire["entity"] * 7919 + fire["start_tick"])
        start, end = fire["start_tick"], fire["end_tick"]
        add_key(channel, max(start - tick0 - 1, 0), 0.0)
        for tick in range(start, end + 1, FLICKER_TICKS):
            add_key(channel, tick - tick0, light_intensity(fire, tick, tick_rate, flicker))
        add_key(channel, min(end - tick0 + FLICKER_TICKS, end_frame), 0.0)
    log("fires: %d (%d with their burst), %d sprite layers, %d wall points" % (
        len(fires), bursts, len(flame_layers + centre_layers + wide_layers + edge_layers + wall_layers + smoke_layers
                                + wall_smoke_layers), placed_walls))
    return len(fires)


def place_bursts(sequence, replay, end_frame, instances, mesh, grounds, spawn):
    """One actor per HE grenade's and flashbang's detonation (bursts_of): its one-shot sprites (BLAST_LAYERS,
    FLASH_LAYERS on quads `mesh`: every slot spawns once, on the detonation's tick; a layer with `floor` dies at the
    ground under it, from `grounds`: (entity, tick) -> the agents manifest's blast) and its light pulse (BLAST_LIGHT,
    FLASH_LIGHT). Returns the number placed."""
    tick0 = replay["start_tick"]
    placed = 0
    for grenade in bursts_of(replay):
        layers, pulse, _ = BURSTS[grenade["kind"]]
        tick = grenade["detonation"]["tick"]
        x, y, z = grenade["detonation"]["position"]
        label = burst_label(grenade)
        actor = spawn(unreal.Actor, label, (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
        rng = random.Random(grenade["entity"] * 15485863 + tick)
        ground = (grounds.get((grenade["entity"], tick)) or {}).get("ground")
        floor_cm = (ground["z"] if ground else z - FALLBACK_DROP) * UNITS_TO_CM
        for layer in layers:
            if layer_name(layer) in instances:
                add_sprites(actor, layer, instances[layer_name(layer)], mesh, [((x, y, z), tick, tick)], rng,
                            floor_cm if option(layer, "floor") else NO_FLOOR_CM)
        # With shadows, unlike CS2's particle lights (molodoy's view at tick 151910 shows a blast's orange on the wall
        # in front of the door it went off behind): without them the light came through walls, which looked wrong
        # (the user's choice, 2026-10-30).
        light = spawn_light(spawn, burst_light_label(grenade), (x, y, z + 20.0), pulse["color"], pulse["radius"],
                            falloff=pulse.get("falloff"))
        channel = intensity_channel(sequence, light, end_frame)
        if tick - tick0 > 0:
            add_key(channel, tick - tick0 - 1, 0.0)
        for after, share in pulse["decay"]:
            if tick - tick0 + after <= end_frame:
                add_key(channel, tick - tick0 + after, pulse["peak"] * share)
        placed += 1
    if placed:
        log("blasts and flashes: %d" % placed)
    return placed


SCORCH_MATERIAL = "/Game/OFAST/Replays/M_Scorch"
# particles/explosions_fx/explosion_hegrenade_decal: up to 3 splats (fluidsplat_a_noisy_trans, made near black by
# scortch_projected_cheap.vmat: colour 0.04 x 32 - 96 of 255), 10 - 20 units from the blast across the ground, radius
# 30 - 40, alpha 0.9 - 1, rolled at random, projected onto what lies within 30 units of the blast's height (a blast
# higher above the ground leaves none); 20 - 30 s, in over 0.125 - 0.15 s, out over the second half of its life.
# In Unreal the decal's box sits on the ground found under the blast, `depth` units deep each way: a box at the blast's
# height 30 deep would also blacken the legs of players walking over it and a wall beside it.
SCORCH = {"count": 3, "offset": (10.0, 20.0), "radius": (30.0, 40.0), "alpha": (0.9, 1.0), "reach": 30.0,
          "depth": 8.0, "life": (20.0, 30.0), "fade_in": (0.125, 0.15), "fade_out": 0.5, "color": 0.01}
# The soot of a fire: CS2 projects scorch under it while it burns (molotov_groundfire_filltest's `projected` child,
# from its decal points) and, when it goes out, a splat per point it burned on (molotov_groundfire_remnant:
# particle_smokegrenade_trans through scortch_mod_projected_cheap.vmat, radius 20 - 65, 8 - 20 s): its floor is dark
# half a second after a fire ends. Here a splat per flame, radius 30 - 50 (the point's own radius is not in the
# demo; the flames are 25 - 50 apart), from when its flame lit, in over a second, until 8 - 20 s after the fire's end. Alpha
# 0.9 - 1 (the soft splat's own alpha is at most 0.7): at 0.6 - 0.8 the soot was hard to see where CS2's floor is
# plainly dark.
FIRE_SCORCH = {"radius": (30.0, 50.0), "alpha": (0.9, 1.0), "depth": 8.0, "after": (8.0, 20.0),
               "fade_in": (0.8, 1.2)}
# Flames lit within this many ticks of each other share their soot's material instance.
SOOT_TICKS = 16
# A splat's opacity at the demo tick (times the texture's alpha).
SCORCH_HLSL = r"""
float age = (Tick - FirstTick) / TicksPerSecond;
if (Tick < FirstTick || age > Life) return 0.0;
return Alpha * saturate(age / FadeIn) * saturate((Life - age) / (FadeOut * Life));
"""
SCORCH_SCALARS = ["TicksPerSecond", "FirstTick", "Life", "Alpha", "FadeIn", "FadeOut"]


def scorch_material():
    """SCORCH_MATERIAL, rebuilt in place: a translucent deferred decal, near black, its opacity the splat texture's
    alpha (`Tex`) x SCORCH_HLSL."""
    material = smoke_volumes.empty_material(SCORCH_MATERIAL)
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_DEFERRED_DECAL)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
    fade = smoke_volumes.custom_node(material, SCORCH_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT1,
                                     ["Tick"] + SCORCH_SCALARS, -400, 0)
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -900, -200)
    tick.set_editor_property("collection", smoke_volumes.replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    mel.connect_material_expressions(tick, "", fade, "Tick")
    for index, name in enumerate(SCORCH_SCALARS):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900, -100 + 60 * index)
        node.set_editor_property("parameter_name", name)
        node.set_editor_property("default_value", 1.0)
        mel.connect_material_expressions(node, "", fade, name)
    texture = mel.create_material_expression(material, unreal.MaterialExpressionTextureSampleParameter2D, -700, -400)
    texture.set_editor_property("parameter_name", "Tex")
    texture.set_editor_property("texture", assets.load_asset("/Engine/EngineResources/DefaultTexture"))
    opacity = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -150, -200)
    mel.connect_material_expressions(texture, "A", opacity, "A")
    mel.connect_material_expressions(fade, "", opacity, "B")
    color = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -400, -400)
    color.set_editor_property("constant", unreal.LinearColor(*[SCORCH["color"]] * 3, 1.0))
    rough = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -400, -300)
    rough.set_editor_property("r", 1.0)
    prop = unreal.MaterialProperty
    if not (mel.connect_material_property(color, "", prop.MP_BASE_COLOR)
            and mel.connect_material_property(rough, "", prop.MP_ROUGHNESS)
            and mel.connect_material_property(opacity, "", prop.MP_OPACITY)):
        raise RuntimeError("scorch material: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def splat_instance(folder, name, material, texture, values):
    """An instance of SCORCH_MATERIAL (`name`, in `folder`) holding `texture` and `values` (SCORCH_SCALARS)."""
    instance = smoke_volumes.material_instance(folder, name, material)
    for param, value in values.items():
        mel.set_material_instance_scalar_parameter_value(instance, param, float(value))
    mel.set_material_instance_texture_parameter_value(instance, "Tex", texture)
    assets.save_loaded_asset(instance)
    return instance


def place_splat(spawn, label, at, normal, roll, radius, depth, instance):
    """One splat of soot as a decal actor at `at` (Source units) projected into the surface of `normal`, rolled `roll`
    degrees, `radius` and `depth` units each way, drawn by `instance` (splat_instance). A decal projects along its X
    axis: into the surface, against its normal (Unreal's y is CS2's -y)."""
    nx, ny, nz = normal
    facing = unreal.MathLibrary.make_rot_from_x(unreal.Vector(-nx, ny, -nz))
    actor = spawn(unreal.DecalActor, label, (at[0] * UNITS_TO_CM, -at[1] * UNITS_TO_CM, at[2] * UNITS_TO_CM))
    actor.set_actor_rotation(unreal.Rotator(roll=roll, pitch=facing.pitch, yaw=facing.yaw), False)
    component = actor.get_editor_property("decal")
    component.set_editor_property("decal_size", unreal.Vector(depth * UNITS_TO_CM, radius * UNITS_TO_CM,
                                                              radius * UNITS_TO_CM))
    component.set_decal_material(instance)


def splat_texture(replay_path, effects, role):
    """The effect texture of `role` from the agents manifest's `effects`, imported, or None when it has none."""
    effect = next((e for e in effects if e["role"] == role), None)
    if effect is None:
        return None
    texture = smoke_volumes.import_png(replay_path, effect["file"], FOLDER)
    assets.save_loaded_asset(texture)
    return texture


def place_scorches(replay, replay_path, effects, blasts, spawn):
    """The scorch an HE blast leaves (SCORCH), a decal actor per splat (place_splat), for the agents manifest's
    `blasts` whose ground lies within SCORCH's reach below them: on that ground. Returns the number of splats."""
    reached = [b for b in blasts if b.get("ground") and b["position"][2] - b["ground"]["z"] <= SCORCH["reach"]]
    texture = splat_texture(replay_path, effects, "scorch") if reached else None
    if texture is None:
        return 0
    material = scorch_material()
    folder = smoke_volumes.round_folder(FOLDER, replay)
    placed = 0
    for blast in reached:
        x, y, _ = blast["position"]
        entity, tick = blast["entity"], blast["tick"]
        rng = random.Random(entity * 7727 + tick)
        for k in range(SCORCH["count"]):
            angle = rng.uniform(0.0, 2.0 * math.pi)
            away = rng.uniform(*SCORCH["offset"])
            at = (x + math.cos(angle) * away, y + math.sin(angle) * away, blast["ground"]["z"])
            # Drawn in this order since the scorches were checked in Unreal (2026-10-24): roll, radius, then the rest.
            roll, radius = rng.uniform(0.0, 360.0), rng.uniform(*SCORCH["radius"])
            values = {"TicksPerSecond": tick_rate_of(replay), "FirstTick": tick, "Life": rng.uniform(*SCORCH["life"]),
                      "Alpha": rng.uniform(*SCORCH["alpha"]), "FadeIn": rng.uniform(*SCORCH["fade_in"]),
                      "FadeOut": SCORCH["fade_out"]}
            instance = splat_instance(folder, "MI_Scorch_%d_%d_%d" % (entity, tick, k), material, texture, values)
            place_splat(spawn, "scorch #%d at %d (%d)" % (entity, tick, k), at, blast["ground"]["normal"], roll,
                        radius, SCORCH["depth"], instance)
            placed += 1
    log("scorches: %d splats under %d blasts" % (placed, len(reached)))
    return placed


def place_fire_scorches(replay, replay_path, effects, spawn):
    """The soot a fire leaves where it burned (FIRE_SCORCH): a decal actor per flame (place_splat), on the surface
    it burns on, darkening from when the flame lit. The splats of flames that lit within SOOT_TICKS of each other
    share an instance (an instance per splat made a round's import minutes slower; one per fire darkened the floor
    before the fire spread there, up to 2 s after it started). Returns the number of splats."""
    fires = [f for f in replay.get("fires", []) if f["flames"]]
    texture = splat_texture(replay_path, effects, "fire_scorch") if fires else None
    if texture is None:
        return 0
    material = scorch_material()
    folder = smoke_volumes.round_folder(FOLDER, replay)
    placed = 0
    tick_rate = tick_rate_of(replay)
    for fire in fires:
        entity, start = fire["entity"], fire["start_tick"]
        rng = random.Random(entity * 6151 + start)
        by_start = {}
        for k, flame in enumerate(fire["flames"]):
            lit = start + (flame["first_tick"] - start) // SOOT_TICKS * SOOT_TICKS
            if lit not in by_start:
                # From then to 8 - 20 s after the fire's end, fading out over the second half of that time after.
                after = rng.uniform(*FIRE_SCORCH["after"])
                life = (fire["end_tick"] - lit) / tick_rate + after
                values = {"TicksPerSecond": tick_rate, "FirstTick": lit, "Life": life,
                          "Alpha": rng.uniform(*FIRE_SCORCH["alpha"]), "FadeIn": rng.uniform(*FIRE_SCORCH["fade_in"]),
                          "FadeOut": 0.5 * after / life}
                by_start[lit] = splat_instance(folder, "MI_Soot_%d_%d_%d" % (entity, start, lit - start), material,
                                               texture, values)
            place_splat(spawn, "soot #%d at %d (%d)" % (entity, start, k), flame["position"], flame["normal"],
                        rng.uniform(0.0, 360.0), rng.uniform(*FIRE_SCORCH["radius"]), FIRE_SCORCH["depth"],
                        by_start[lit])
            placed += 1
    log("fire soot: %d splats under %d fires" % (placed, len(fires)))
    return placed


# The flashed white over a camera's view: for each blinding (start tick, duration in seconds; two per Flash vector,
# FLASHED_VECTORS of them), FlashWhite x saturate((seconds left - FlashEnd) / FlashFade); the whitest blinding wins.
FLASHED_VECTORS = FLASHED_SLOTS // 2
FLASHED_HLSL = r"""
float4 flashes[%(n)d] = { %(names)s };
float white = 0.0;
for (int i = 0; i < %(n)d; i++)
{""" % {"n": FLASHED_VECTORS, "names": ", ".join("Flash%d" % k for k in range(FLASHED_VECTORS))} + r"""
    for (int k = 0; k < 2; k++)
    {
        float start = k == 0 ? flashes[i].x : flashes[i].z;
        float duration = k == 0 ? flashes[i].y : flashes[i].w;
        float remaining = duration - (Tick - start) / TicksPerSecond;
        if (duration > 0.0 && Tick >= start && remaining > 0.0)
            white = max(white, FlashWhite * saturate((remaining - FlashEnd) / FlashFade));
    }
}
return lerp(Scene, FlashColor, white);
"""


def flashed_material():
    """FLASHED_MATERIAL, rebuilt in place: a post-process material after tonemapping whitening the view as FLASHED_HLSL
    says, from its instance's Flash vectors and the demo tick."""
    material = smoke_volumes.empty_material(FLASHED_MATERIAL)
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_POST_PROCESS)
    material.set_editor_property("blendable_location", unreal.BlendableLocation.BL_SCENE_COLOR_AFTER_TONEMAPPING)
    names = (["Scene", "Tick", "TicksPerSecond", "FlashWhite", "FlashEnd", "FlashFade", "FlashColor"]
             + ["Flash%d" % k for k in range(FLASHED_VECTORS)])
    custom = smoke_volumes.custom_node(material, FLASHED_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT3, names,
                                       -400, 0)
    scene = mel.create_material_expression(material, unreal.MaterialExpressionSceneTexture, -900, -300)
    scene.set_editor_property("scene_texture_id", unreal.SceneTextureId.PPI_POST_PROCESS_INPUT0)
    rgb = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -700, -300)
    for channel in ("r", "g", "b"):
        rgb.set_editor_property(channel, True)
    rgb.set_editor_property("a", False)
    mel.connect_material_expressions(scene, "Color", rgb, "")
    mel.connect_material_expressions(rgb, "", custom, "Scene")
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -900, -200)
    tick.set_editor_property("collection", smoke_volumes.replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    mel.connect_material_expressions(tick, "", custom, "Tick")
    scalars = (("TicksPerSecond", 64.0), ("FlashWhite", FLASH_WHITE), ("FlashEnd", FLASH_END),
               ("FlashFade", FLASH_FADE))
    for index, (param, default) in enumerate(scalars):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900, -100 + 60 * index)
        node.set_editor_property("parameter_name", param)
        node.set_editor_property("default_value", default)
        mel.connect_material_expressions(node, "", custom, param)
    color = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -900, 150)
    color.set_editor_property("constant", unreal.LinearColor(*FLASH_COLOR, 1.0))
    mel.connect_material_expressions(color, "", custom, "FlashColor")
    for k in range(FLASHED_VECTORS):
        node = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -900, 250 + 60 * k)
        node.set_editor_property("parameter_name", "Flash%d" % k)
        node.set_editor_property("default_value", unreal.LinearColor(0.0, 0.0, 0.0, 0.0))
        # All four channels: the default output is RGB only (the shader then saw n x 3 values, not n x 4).
        if not mel.connect_material_expressions(node, "RGBA", custom, "Flash%d" % k):
            raise RuntimeError("flashed material: could not connect Flash%d" % k)
    if not mel.connect_material_property(custom, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR):
        raise RuntimeError("flashed material: could not connect its output")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def flash_cameras(replay, cameras):
    """Gives each blinded player's camera (`cameras`: controller -> camera actor) an instance of FLASHED_MATERIAL with
    the player's blindings (the replay's players' `blindings`, the last FLASHED_SLOTS of them), in the round's own
    folder (another round's import must not change this round's views). Returns the number of cameras."""
    blinded = [p for p in replay["players"] if p.get("blindings") and p["controller"] in cameras]
    if not blinded:
        return 0
    material = flashed_material()
    folder = smoke_volumes.round_folder(FOLDER, replay)
    for player in blinded:
        instance = smoke_volumes.material_instance(folder, "MI_Flashed_%d" % player["controller"], material)
        mel.set_material_instance_scalar_parameter_value(instance, "TicksPerSecond", float(tick_rate_of(replay)))
        blindings = player["blindings"][-FLASHED_SLOTS:]
        values = [(b["tick"], b["duration"]) for b in blindings] + [(0.0, 0.0)] * (FLASHED_SLOTS - len(blindings))
        for k in range(FLASHED_VECTORS):
            (s0, d0), (s1, d1) = values[2 * k], values[2 * k + 1]
            mel.set_material_instance_vector_parameter_value(
                instance, "Flash%d" % k, unreal.LinearColor(r=float(s0), g=float(d0), b=float(s1), a=float(d1)))
        assets.save_loaded_asset(instance)
        component = cameras[player["controller"]].get_editor_property("camera_component")
        settings = component.get_editor_property("post_process_settings")
        blendable = unreal.WeightedBlendable(weight=1.0, object=instance)
        settings.set_editor_property("weighted_blendables", unreal.WeightedBlendables(array=[blendable]))
        component.set_editor_property("post_process_settings", settings)
        component.set_editor_property("post_process_blend_weight", 1.0)
    log("flashed views: %d players" % len(blinded))
    return len(blinded)


def place_effects(sequence, replay, replay_path, end_frame, agents, cameras, spawn):
    """Everything above for a round: the sprite layers (from the agents manifest `agents`' `effects`; without one: no
    sprites), the fires, the blasts and flashes (their debris on the ground of its `blasts`), the scorches, the
    blinded players' views."""
    effects = agents.get("effects", [])
    blasts = agents.get("blasts", [])
    instances = update_layers(replay, replay_path, effects) if effects else {}
    if (replay.get("fires") or bursts_of(replay)) and not instances:
        log("fires and blasts: no sprite textures in the agents manifest (export with --players): lights only")
    mesh = sprite_mesh()
    walls = {(f["entity"], f["start_tick"]): f["walls"] for f in agents.get("fires", [])}
    place_fires(sequence, replay, end_frame, instances, mesh, walls, spawn)
    place_bursts(sequence, replay, end_frame, instances, mesh, {(b["entity"], b["tick"]): b for b in blasts}, spawn)
    place_scorches(replay, replay_path, effects, blasts, spawn)
    place_fire_scorches(replay, replay_path, effects, spawn)
    flash_cameras(replay, cameras)
