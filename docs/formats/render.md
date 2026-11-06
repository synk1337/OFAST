# Renders (OFAST's own)

What `tools/unreal/render_round.py` writes (the Studio's "Render", M9, [decision 023](../decisions/023-render-presets.md)):
a round's Level Sequence rendered by Unreal's Movie Render Queue, as image sequences an editor imports.

## Folders

One folder per render, in the folder it was given (the Studio: `Videos\OFAST`):

```
<demo>_r<round>_<preset>[_<fps>fps][_sharp][_<start tick>-<end tick>]/
    FinalImage/<demo>_r<round>.<frame>.png               the picture
    FinalImagePlayers/<demo>_r<round>.<frame>.png        final, 4k with the players layer: the players alone, the rest
                                                         transparent
    FinalImageDefaultLayer/<demo>_r<round>.<frame>.png   final, 4k with the players layer: the world, the players'
                                                         shapes transparent
    FinalImageWorldDepth/<demo>_r<round>.<frame>.png     final, 4k: depth, clipped to white (use the EXR)
    <demo>_r<round>.<frame>.exr                          final, 4k: all of the above, one layer each
```

- `<fps>fps`: the frames a second asked for, when not 60.
- `sharp`: rendered without motion blur.
- `<start tick>-<end tick>`: the demo ticks asked for (the end left out); no suffix for the whole round.
- `<frame>`: 5 digits (6 above 600 frames a second), the frame's number at the render's frame rate from the
  sequence's start (the round's start), so two renders of one round at one rate line up by number.
- 60 frames a second by default, or any rate from 10 to 1000 (`OFAST_FPS`, the Studio's fps box), whatever the demo's
  64 ticks a second. Above 64 every frame is still a picture of its own moment, not a copy or a blend: the keys
  (players, cameras, bones) are interpolated between ticks, the effects are played at the frame's fraction of a tick
  and the smokes' volumes blend between their baked frames (250 frames a second over 24 ticks: 95 frames, no two
  alike). Motion blur is a 180° shutter at any rate, so at 250 each frame is blurred over 1/500 s, as a real camera
  would.
- Sharp frames (`OFAST_MOTION_BLUR=0`, the Studio's **Motion blur** unticked), for velocity edits: the editor ramps
  the speed, then blends frames or adds the blur (RSMB, Twixtor), which a blur baked in at one speed would spoil. The
  preset's samples are spatial instead of temporal (as many, so the edges are as smooth, all at the frame's own
  moment) and Unreal's own motion blur is off (`r.MotionBlurQuality 0`).

## Passes

| Pass | What | Where |
|---|---|---|
| `FinalImage` | the picture as Sequencer shows it, through the cut's camera | PNG (8-bit RGBA, opaque); EXR channels `R`, `G`, `B`, `A` |
| `FinalImagePlayers` | with the players layer (`OFAST_PLAYERS_LAYER`, the Studio's **Players layer**): every player's body, visor and viewmodel (editor layer "Players"), alpha around them; translucent effects near them (muzzle smoke, blood) come along | PNG; EXR `FinalImagePlayers.R` … `.A` |
| `FinalImageDefaultLayer` | with the players layer: everything else, the players' shapes cut out (alpha 0) | PNG; EXR `FinalImageDefaultLayer.R` … `.A` |
| `FinalImageWorldDepth` | depth from the camera, centimetres (Unreal units: 1 CS2 unit = 2.54 cm) | EXR `FinalImageWorldDepth.R` (16-bit half: 1 cm steps to 20 m, 4 cm at 80 m); G and B hold other values; the PNG is clipped |

The colour passes are tone-mapped (sRGB, 0 – 1), in the PNGs as 8 bits and in the EXR as 16-bit halves. A
player's first-person cut hides that player's own body, as in Sequencer (decision 021).

## EXR

One file a frame, scanlines, PIZ compression (lossless), every pass a layer (channels `<pass>.R` …; the picture
unprefixed). Movie Render Queue adds its metadata as string attributes: `unreal/sequenceName`, `unreal/levelName`,
`unreal/frameRate`, the camera (`unreal/camera/curPos/x` …, `unreal/camera/FinalImage/fov`), the samples
(`unreal/aa/temporalSampleCount`), timings and the machine.

## Presets

| Preset | Size | Samples a frame | Files |
|---|---|---|---|
| `preview` | 1280 × 720 | 1 | `FinalImage` PNG |
| `final` | 1920 × 1080 | 8 (motion blur: 180° shutter; sharp: 8 spatial) | the picture and depth, PNG and EXR; the players layer's two passes when asked |
| `4k` | 3840 × 2160 | 8 | as `final` |

`final` and `4k` start each cut with 32 warm-up frames that are not written (Lumen's lighting and the smokes settle).

## Graphics memory

The size is always the preset's; what Unreal shades may be smaller ([decision 027](../decisions/027-renders-fit-the-card.md)):
when the graphics card's own memory would not hold the render, `render_round.py` shades the frames at the largest of
100, 85, 75, 67, 60, 50, 40, 33 % of the size that fits (`r.ScreenPercentage`) and TSR scales them up from the frame's
samples. Its log says what it chose:

```
OFAST: graphics card: 11943 MB, the render needs about 10721 MB: shaded at 50 % (1920x1080), 8192 shadow pages
OFAST: warning: Shaded at 1920x1080 and scaled up to 3840x2160 by Unreal's TSR, to fit the graphics card's 11943 MB.
```

On a 12 GB card: Final whole, Final with the players layer at 75 %, 4K at 50 %; on 16 GB 4K whole; on 24 GB
everything. `OFAST_SCREEN_PERCENTAGE` (10 to 100) forces a size instead. For the memory, the editor's active viewport
draws unlit during the render (its view mode given back before it quits), TSR's history is the picture's size and the
shadow page pool has 8192 pages for 1080p shaded, more for more pixels, up to 16384.

## Power

`OFAST_POWER` (the Studio's **Power**): `full` renders flat out; below it each of Unreal's frames owes a rest, a third
of its time at `balanced` (the Studio's default), all of it at `low`, slept off on the game thread half a second or
more at a time; the Studio also starts Unreal at below-normal CPU priority. de_nuke round 17 in 4K shaded at 67 % on
a 12 GB RTX 5070 (2026-11-06):

| Power | Seconds a frame | Graphics card busy | Its power |
|---|---|---|---|
| `full` | 2.2 | 72 % | 154 W |
| `balanced` | 2.5 | 64 % | 136 W |
| `low` | 3.7 | 41 % | 106 W |

The frames are the same at any power.
