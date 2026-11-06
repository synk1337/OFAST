# 027 — Renders fit the graphics card, and take only the power asked

- **Date:** 2026-11-06
- **Status:** accepted

## Context

The user's 4K render (2026-11-04, FaZe vs Falcons round 14 on de_anubis, sharp) slowed the whole PC "like crazy"
(RTX 5070, 12 GB; Ryzen 9 9900X; 32 GB). Unreal's log showed why: after the warm-up each frame took 1 to 4 minutes;
6 frames in 8 minutes, then it was stopped. The same render of de_dust2 round 17, watched (2026-11-05): the card's
12 GB full, 11 GB more spilled into the PC's memory, Unreal holding 32.5 GB, the PC at 64 of its 65 GB committed
before it was stopped. It was never the power the card draws: it was memory. Once the card's memory is full, Windows
pages its allocations over PCIe, every frame crawls and every other program that draws on the screen with it.

Unreal's own allocation lists (`rhi.DumpResourceMemory`, grouped by owner) during renders said where the memory went:

- **The editor's copy of the world.** Movie Render Queue renders in Play In Editor: the world it renders is a copy, and
  the editor's own world stays loaded beside it. Its viewport kept drawing it lit during the render, so it held a
  second shadow page pool, Lumen scene and distance fields: two 2 GB pools in every dump. Turning the viewport's
  realtime off did not stop it (Play In Editor redraws it once at its start, at the size the pool then has).
- **The shadow page pool**: OFAST's editor asks for 16384 pages (2 GB a pool; `DefaultEngine.ini`, for de_nuke's 174
  lights), Unreal's default 4096.
- **TSR's history**: Unreal's cinematic quality keeps it at twice the picture's size: 1 GB of colour alone at 4K.
- **The players layer**: each of its two layers is a full render of the frame with its own Lumen and TSR histories
  (about 1.1 GB each at 1080p). Final (1080p) spilled 3.4 GB with it.
- **The picture's own size**: the frame's work memory scales with the pixels shaded.

## Decision

- **The editor's active viewport drawn unlit** for the render (`render_round.unlit_editor`), its view mode given back
  before the editor quits (the editor saves it). Only the active one: the layout's others are orthographic and unlit
  already. de_nuke round 17 at 1080p: 6.3 GB of resources instead of 8.5, one shadow pool, and the render in 58 s
  instead of 96 (the editor's lit copy cost GPU time too).
- **TSR's history at the picture's size** (`r.TSR.History.ScreenPercentage 100`): the frame's 8 samples already smooth
  it.
- **Shadow pages by the shaded size**: 8192 for 1080p (de_nuke round 17 asked for 5659; 4096 overflowed by 1563),
  proportionally more for more pixels, at most the editor's 16384.
- **The players layer off unless asked** (the user, 2026-11-06): a **Players layer** switch in the Studio; without it
  Final and 4K render the picture and its depth once.
- **The picture shaded smaller when the card would not hold it** (`render_round.fit_card`): the card's memory from
  Unreal's log ("Adapter has … MB of dedicated video memory" of the chosen adapter), the render's need from a model
  fitted to de_nuke round 17 (the heaviest map) on the 12 GB card, the largest of 100, 85, 75, 67, 60, 50, 40, 33 % that
  leaves 0.5 GB free; TSR scales it up to the picture's size from the frame's 8 jittered samples. The user's choice
  ("not everybody will have the same amount of video memory"): native wherever the card holds it. A Studio warning says
  when a render is scaled up, or does not fit at all.
- **Power** (the user's question: "limit the power usage while rendering"): Full, Balanced (default) or Low. Movie
  Render Queue has no frame rate cap (its own time step); below Full each of Unreal's frames owes a rest as long as it
  took times (1 − share) / share (share 0.75 or 0.5), which the game thread sleeps off half a second or more at a
  time, and the Studio starts Unreal at below-normal CPU priority (the shader compilers inherit it), so the user's
  programs come first. Rests after every frame, a few tens of milliseconds, did nothing: they fell in the gaps the card
  already had while the samples were read back (11 % slower, the same 126 W). In chunks, on de_nuke in 4K at 67 %:
  Full 2.2 s a frame, the card busy 72 %, 154 W; Balanced 2.5 s, 64 %, 136 W; Low 3.7 s, 41 %, 106 W.

- **A warning when the disk will not hold the render**: the Studio estimates the frames' size (35 MB a 4K frame without
  the players layer, 100 MB with it, 12 MB a Final frame, 1.5 MB a preview one) and warns before Unreal
  starts. The user's 4K render was heading for about 166 GB on a disk with 42 GB free; a full disk stops the render
  and slows the PC too.

## Alternatives considered

- **Movie Render Queue's high-resolution tiles**: each tile a 1080p render, but TSR is off with tiles, screen-space
  effects (bloom, depth of field, Lumen's screen traces) seam at the tiles' edges, and each tile keeps its own
  histories.
- **Rendering in a separate process** (`MoviePipelineNewProcessExecutor`): no editor world at all, but every step
  `render_round` does in the editor (the Look panel's settings, the tone mapper's movie mode, the cinematics, the
  viewmodel keys, the players' layer) would have to be saved into the level first, or done again in a game process
  without the editor's scripting.
- **A players matte instead of the stencil layers** (one render and a custom-stencil post-process pass, the layers
  composited from it): a third of the memory and faster, but other files than today's; the user preferred the layer
  off unless asked.
- **The card's clocks or power limit** (`nvidia-smi -pl`): needs administrator rights, NVIDIA only, and changes the
  system, not the render.
- **Unreal's frame rate cap** (`t.MaxFPS`): ignored under Movie Render Queue's own time step.

## Consequences

- de_nuke round 17 on the 12 GB card (2026-11-06): 1080p 10.3 GB, no spill; 4K at 50 % (1920 × 1080 shaded) 10.7 GB,
  1.8 s a frame; at 67 % 11.7 GB, 2.2 s; native 4K 13.0 GB, 1.2 GB spilled but 3.4 s a frame (before: 1 to 4 minutes).
- The fit picks on a 12 GB card: Final 100 %, Final with the players layer 75 %, 4K 50 %; 16 GB: 4K 100 %, 4K with the
  layer 60 %; 24 GB: all native. Below about 9 GB nothing fits: the map, the players, the editor and the desktop take
  8.6 GB on de_nuke before a pixel is shaded; such renders spill (warned) but no longer tenfold.
- The model is fitted to one map and one card: lighter maps get a smaller size than they could hold. Each render logs
  its estimate beside the card's size (`graphics card: …`) to check it against.
- Shaded below 100 %, the picture is TSR's reconstruction: a little softer than native, with the frame's 8 samples a
  frame much closer than a game's.
- The editor's world still costs memory (its scene, Lumen's surface cache, textures it loaded): rendering in a separate
  process would save more and is the next step if cards of 8 GB matter.
