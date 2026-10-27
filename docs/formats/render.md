# Renders (OFAST's own)

What `tools/unreal/render_round.py` writes (the Studio's "Render", M9, [decision 023](../decisions/023-render-presets.md)):
a round's Level Sequence rendered by Unreal's Movie Render Queue, as image sequences an editor imports.

## Folders

One folder per render, in the folder it was given (the Studio: `Videos\OFAST`):

```
<demo>_r<round>_<preset>[_<start tick>-<end tick>]/
    FinalImage/<demo>_r<round>.<frame>.png               the picture
    FinalImagePlayers/<demo>_r<round>.<frame>.png        final, 4k: the players alone, the rest transparent
    FinalImageDefaultLayer/<demo>_r<round>.<frame>.png   final, 4k: the world, the players' shapes transparent
    FinalImageWorldDepth/<demo>_r<round>.<frame>.png     final, 4k: depth, clipped to white (use the EXR)
    <demo>_r<round>.<frame>.exr                          final, 4k: all of the above, one layer each
```

- `<start tick>-<end tick>`: the demo ticks asked for (the end left out); no suffix for the whole round.
- `<frame>`: 5 digits, the frame's number at 60 frames a second from the sequence's start (the round's start), so
  two renders of one round line up by number.
- 60 frames a second, whatever the demo's 64 ticks a second.

## Passes

| Pass | What | Where |
|---|---|---|
| `FinalImage` | the picture as Sequencer shows it, through the cut's camera | PNG (8-bit RGBA, opaque); EXR channels `R`, `G`, `B`, `A` |
| `FinalImagePlayers` | every player's body, visor and viewmodel (editor layer "Players"), alpha around them; translucent effects near them (muzzle smoke, blood) come along | PNG; EXR `FinalImagePlayers.R` … `.A` |
| `FinalImageDefaultLayer` | everything else, the players' shapes cut out (alpha 0) | PNG; EXR `FinalImageDefaultLayer.R` … `.A` |
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
| `final` | 1920 × 1080 | 8 (motion blur: 180° shutter) | all passes, PNG and EXR |
| `4k` | 3840 × 2160 | 8 | as `final` |

`final` and `4k` start each cut with 32 warm-up frames that are not written (Lumen's lighting and the smokes settle).
