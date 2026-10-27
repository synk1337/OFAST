# 022 — ofast-capture: attached to an insecure CS2, CS2's own interfaces, checked offsets

- **Date:** 2026-10-27
- **Status:** accepted (M8, first pass)

## Context

M8 is OFAST's own remake of HLAE's CS2 capture: a DLL inside CS2 that films demos (OFAST's cameras played in the
game, frames sent to FFmpeg, later several passes and the players' bones). How HLAE does it is written up in
[research/hlae-cs2-capture.md](../research/hlae-cs2-capture.md): it starts `cs2.exe` suspended, injects early, hooks
`LoadLibrary` to catch each module as it loads, swaps entries of the client's interface, patches the code that sets
up the view, and hooks Direct3D 11. Its weak point is upkeep: hardcoded offsets, vtable slots and even registers broke
with CS2 updates in January and September 2026.

Two things are not negotiable for OFAST: it must never touch a CS2 that could join a VAC-secured server, and it must
fail safe — when CS2 changes, it refuses and says why rather than crashing the game or writing over the wrong memory.

## Decision

- **Only an insecure CS2.** CS2 is started the usual way, through Steam, with `-insecure` (which keeps it off every
  VAC-secured server). The launcher (`ofast-capture attach`) reads the running `cs2.exe`'s own command line and
  refuses without `-insecure`; the DLL checks `GetCommandLineW` again and does nothing at all without it. OFAST films
  demos: what changes the picture (cameras, capture) runs only during demo playback.
- **Attach to a running CS2**, rather than starting it suspended: the user starts CS2 as always, and the DLL is loaded
  with `CreateRemoteThread(LoadLibraryW)` once CS2 is at its menu. Nothing has to be caught while modules load, so
  there are no early `LoadLibrary` hooks.
- **CS2's own interfaces first**: `CreateInterface` of `client.dll` (`Source2Client002`) and `engine2.dll`
  (`Source2EngineToClient001`), whose slots give the demo state (`IsPlayingDemo`, `GetDemoFile` → `GetDemoTick`) and
  the client's frame (`FrameStageNotify`, swapped for OFAST's). The game's clock (`curtime`) is in the globals the
  engine hands the client through `SetGlobals`, which runs before OFAST is there: OFAST reads where that function
  stores the pointer instead (its `mov [rip+disp32], rdx`), so it needs no hook there.
- **Every slot and offset in one place, checked before use** (`cs2.rs`): a slot's function must lie in its module's
  code, the decoded globals pointer must point into the module's data, the clock must move. When a check fails the DLL
  logs what it found and does not hook. Its log, `%LOCALAPPDATA%\OFAST\capture.log`, says what it did.

## Alternatives considered

- **HLAE's suspended start and early hooks**: catches everything from the first instruction, but OFAST would then
  start CS2 itself, bypassing Steam's own launch, and carry hooks on `LoadLibrary` for every module.
- **Screen capture from outside** (OBS, Windows' graphics capture): no extra passes (depth, mattes), no frame-exact
  timing, and no camera of OFAST's.
- **Recording in Unreal only** (M9): the plan; the capture is for what Unreal does not have (CS2's exact picture, and
  its exact player animation for Unreal through bone capture).

- **Cameras through the view setup's demo check**: `CViewRender::SetUpView` asks the engine `IsPlayingDemo` with the
  view setup in `rbx` (HLAE's finding); those 16 bytes, found by a pattern built from OFAST's own slot number and
  matched exactly once, become a call to OFAST with the view setup, which answers the engine's question itself. The
  patch goes in from the game's thread between frames; the view setup's fields must look like a view on their first
  read, or are never changed.
- **Frames through `Present`**, hooked by its slot in `dxgi.dll`'s swap chain vtable (found with a throwaway swap
  chain; CS2's shares it), copied through a staging texture.

## Checked (2026-10-27)

All three on CS2's build of that day, de_dust2 round 17 of the Aurora vs FURIA demo (apps/capture/README.md):
every check passed; the game's clock minus the demo tick is the demo's server tick offset OFAST reads (134067);
OFAST's first-person camera of a player matches CS2's own view of him to a few pixels; a shot camera of OFAST's plays
in CS2 as it does in Unreal; 120 frames at 2560 × 1440 are written in 1.6 s.

## Not verified yet

- How long the slot numbers, the view setup's offsets and its code pattern hold: they are HLAE's findings on CS2's
  2026-09-23 build and still held on 2026-10-27; the checks are there to catch the next change.
