# 023 — Renders: Movie Render Queue presets, a players layer and depth

- **Date:** 2026-10-27
- **Status:** accepted (first pass)

## Context

M9: a round built in Unreal (decision 021) rendered to frames an editor can cut and composite: the picture, a layer
with the players alone (text and effects behind them, colour on them only), depth (fog, depth of field), in one go and
without knowing Unreal. Unreal 5.8 renders sequences with Movie Render Queue, configured two ways: the classic
configuration (`MoviePipelinePrimaryConfig`, a flat list of settings: output, render passes, anti-aliasing, console
variables) and Movie Render Graph (`MovieGraphConfig`, nodes and branches, render layers made of collections and
modifiers). In 5.8 the queue's own code calls the classic configuration "legacy"; the graph is where Epic's work goes.

## Decision

- **The classic configuration, built by script** (`tools/unreal/render_round.py`). Three presets, each saved as an
  asset (`/Game/OFAST/Render/OFAST_<preset>`) that Unreal's Movie Render Queue window offers too:
  - `preview`: 1280 × 720, one sample a frame, PNG.
  - `final`: 1920 × 1080, 8 temporal samples (motion blur from the camera's shutter, 180° by default, and
    anti-aliasing), 32 warm-up frames (Lumen and the smokes settle), PNG and EXR, the passes below.
  - `4k`: as `final` at 3840 × 2160.
- **60 frames a second** whatever the sequence's 64 (the demo's tick rate): what editors cut at.
- **A players layer by stencil**: each player's body, visor and viewmodel is put in the editor layer "Players";
  the deferred pass renders that layer alone (the rest cut out, with alpha) and the rest of the world as a second
  layer (`DefaultLayer`, the players' shapes cut out). The full frame is rendered too.
- **Depth** from Movie Render Queue's own `MovieRenderQueue_WorldDepth` post-process material: in the EXR's
  `FinalImageWorldDepth.R` channel, centimetres from the camera (its G and B channels hold something else), as
  16-bit halves (1 cm steps to 20 m, 4 cm at 80 m): with 32-bit floats an EXR was 37 MB at 1080p, the pass's four
  channels written whole.
- **Files**: a folder per render (`<sequence>_<preset>[_<start>-<end>]`, demo ticks); in it a folder per pass
  (`FinalImage`, `FinalImagePlayers`, `FinalImageDefaultLayer`, `FinalImageWorldDepth`, PNG), named
  `<sequence>.<frame>.png` (5 digits, counted from the sequence's start at 60 frames a second), and every pass as
  layers of one EXR a frame beside them (16-bit half, PIZ: lossless, a fifth smaller than ZIP on these frames).
- **The same view as Sequencer**: Movie Render Queue renders through the player controller's view target, the cut's
  camera (`ViewInitOptions.ViewActor`, `MoviePipelineImagePassBase.cpp`), so each player's first-person camera hides
  that player's body as it does in the editor (decision 021).
- **The Studio's "Render"** runs the script on a round already sent (a renderer this time: not `-nullrhi`): a preset,
  the round around its kills (4 s before the first, 2 s after the last) or whole, into `Videos\OFAST`.

## Alternatives considered

- **Movie Render Graph** — Epic's newer system, and per-layer settings come with it. Its render layers need a graph
  built node by node (inputs, branches, collections, modifiers) where the classic configuration takes one list of
  layer names; for a first pass the classic one does all OFAST needs and was checked frame by frame. To move to when
  the classic configuration's days look numbered (the polish pass).
- **The editor's high-resolution screenshots, frame by frame** (as the checks so far): no motion blur, no layers, and
  every frame steps the sequence by script.
- **Mattes by custom stencil values and a post-process material**: one more material to keep; the stencil layers
  of the deferred pass give the same cut-out with alpha.
- **Depth in a PNG**: the material's centimetres clip to white in 8 bits; a normalised depth material is a polish
  item.

## Consequences

- Checked on de_dust2 round 17 (2026-10-27): YEKINDAR's first-person cut renders without his body in the view, his
  arms and gun in the players layer with clean alpha; the fixed camera on MAJ3R's kill puts both players in the
  players layer; depth reads 1.75 m on MAJ3R to 13 m down the tunnel (as 16-bit halves too). `final` takes about
  2 s a frame on an RTX 5070 (67 frames in 2 min 15 s, after a minute of loading) and about 20 MB a frame on disk at
  1080p (the EXR 16 MB with ZIP, the PNGs 6 MB): a 20-second highlight, 1200 frames, about 25 GB. `preview` ran at
  1.4 to 7 frames a second (slowest in the smokes): molodoy's 24-second AWP triple kill, 1430 frames, in 4 min 45 s
  and 1.7 GB, started from the Studio. The graphics card's 12 GB were nearly full in the smokes.
- Translucent effects near the players (muzzle smoke, blood) show in the players layer as well as in the world.
- The EXR holds the tone-mapped picture (sRGB, 0 – 1), not the scene's linear light; a linear version for grading
  is for later.
- The motion blur is baked in: no preset for velocity edits (sharp frames for time remapping) yet.
- The smokes' console variables (`smoke_volumes.SMOKE_CVARS`: Lumen's light on them) are set in the configuration,
  so a render made from Unreal's own Movie Render Queue window with an OFAST preset has them too.
- `render_round.py` loads the level and tags the players each time; a level saved after that keeps the "Players" layer
  for Unreal's own Movie Render Queue window.
