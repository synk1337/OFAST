# Renders (OFAST's own)

What `tools/unreal/render_round.py` writes (the Studio's "Render", M9, [decision 023](../decisions/023-render-presets.md)):
a round's Level Sequence rendered by Unreal's Movie Render Queue, as image sequences or videos an editor imports.

## Folders

One folder per render, in the folder it was given (the Studio: `Videos\OFAST`). As images (`OFAST_OUTPUT` images,
the default):

```
<demo>_r<round>_<preset>[_<fps>fps][_sharp][_<start tick>-<end tick>]/
    FinalImage/<demo>_r<round>.<frame>.png               the picture
    FinalImagePlayers/<demo>_r<round>.<frame>.png        with the players layer: the players alone, the rest
                                                         transparent
    FinalImageDefaultLayer/<demo>_r<round>.<frame>.png   with the players layer: the world, the players' shapes
                                                         transparent
    FinalImageWorldDepth/<demo>_r<round>.<frame>.png     depth, clipped to white (use the EXR)
    <demo>_r<round>.<frame>.exr                          all of the above, one layer each
```

As videos (`OFAST_OUTPUT` prores, the Studio's **As: ProRes video**; [decision 028](../decisions/028-prores-video.md)):

```
<demo>_r<round>_<preset>[_<fps>fps][_sharp]_prores[_<start tick>-<end tick>]/
    <demo>_r<round>_FinalImage.mov                       the picture
    <demo>_r<round>_FinalImagePlayers.mov                with the players layer: the players alone, with alpha
    <demo>_r<round>_FinalImageDefaultLayer.mov           with the players layer: the world, the players cut out
```

Apple ProRes 4444 (`ap4h`), 4:4:4 at 12 bits with a 16-bit alpha channel (the file says 32-bit: the layers'
transparency, opaque in the picture's), a timecode track, no sound; every camera cut of the render one after the other
in each file, at the render's frame rate. No depth: the EXR images hold it. Not 4444 XQ: Unreal 5.8 labels XQ files
24-bit though their frames hold the alpha, and editors then drop it. About 1.3 MB a 1080p frame as 4444 XQ (de_nuke
round 17, 2026-11-08; 4444 a little less), against about 2 MB for the picture's PNG.

- `<fps>fps`: the frames a second asked for, when not 60.
- `sharp`: rendered without motion blur.
- `prores`: written as videos.
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

The preset is the picture's size (the user's ask, 2026-11-08: 720p, 1080p, 2K and 4K instead of preview, final and
4k); every size is rendered alike: 8 samples a frame (motion blur: a 180° shutter; sharp: 8 spatial), the picture and
its depth, PNG and EXR (or ProRes), the players layer's two passes when asked.

| Preset | Size |
|---|---|
| `720p` | 1280 × 720 |
| `1080p` | 1920 × 1080 |
| `2k` | 2560 × 1440 (1440p, as games and YouTube editors call 2K) |
| `4k` | 3840 × 2160 |

Each cut starts with 32 warm-up frames that are not written (Lumen's lighting and the smokes settle).

A round must still be in its level: the level holds one round at a time, and Movie Render Queue renders an older
round's sequence all the same, from the map's origin with nobody in it. `render_round.py` checks that the cameras the
sequence cuts to are in the level and fails otherwise ("this round is no longer in its level … Send the round
again").

## Graphics memory

The size is always the preset's; what Unreal shades may be smaller
([decision 027](../decisions/027-renders-fit-the-card.md)): when the graphics card's own memory would not hold the
render, `render_round.py` shades the frames at the largest of
100, 85, 75, 67, 60, 50, 40, 33 % of the size that fits (`r.ScreenPercentage`) and TSR scales them up from the frame's
samples. Its log says what it chose:

```
OFAST: graphics card: 11943 MB, the render needs about 10721 MB: shaded at 50 % (1920x1080), 8192 shadow pages
OFAST: warning: Shaded at 1920x1080 and scaled up to 3840x2160 by Unreal's TSR, to fit the graphics card's 11943 MB.
```

On a 12 GB card: 720p and 1080p whole, 2K at 85 % (2176 × 1224), 4K at 50 % (a firefight on de_nuke through a
player's eyes, 2026-11-12: 11.1, 11.6 and 11.5 GB of the card, 2.1, 2.4 and 2.5 s a frame; native 4K spilled 1.7 GB
and ran a 32 GB PC out of its own memory); the players layer (each of its layers about 1.1 GB more, mostly whatever
the size) does not fit: 1080p with it at 67 %, still spilling a little, and warned. On 16 GB every size whole, the
players layer too up to 1080p; on 24 GB everything. Unreal also holds 17 to 21 GB of the PC's own memory, 18 to 25
with the players layer or at native 4K: on a 32 GB PC with other programs open (a game), close them. `OFAST_SCREEN_PERCENTAGE` (10 to 100) forces a size instead. For the memory, the editor's
active viewport draws unlit during the render (its view mode given back before it quits), TSR's history is the
picture's size and the shadow page pool has 8192 pages for 1080p shaded, more for more pixels, up to 16384.

## Power

`OFAST_POWER` (the Studio's **Power**): `full` renders flat out; below it each of Unreal's frames owes a rest, a third
of its time at `balanced` (the Studio's default), all of it at `low`, slept off on the game thread half a second or
more at a time; the Studio also starts Unreal at below-normal CPU priority. A firefight of de_nuke round 17 in 1080p
on a 12 GB RTX 5070 (2026-11-12):

| Power | Seconds a frame | Graphics card busy | Its power |
|---|---|---|---|
| `full` | 2.1 | 70 % | 153 W |
| `balanced` | 2.7 | 57 % | 128 W |
| `low` | 3.9 | 40 % | 98 W |

The frames are the same at any power.
