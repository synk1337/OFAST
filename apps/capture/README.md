# ofast-capture (M8, first pass)

A Rust DLL loaded into CS2 to film demos: OFAST's cameras (a round's cameras file, `docs/formats/cameras.md`) played
in the game, and the frames written out. Design: [decision 022](../../docs/decisions/022-capture-attach.md); how HLAE
does the same: [research notes](../../docs/research/hlae-cs2-capture.md).

## Safety rules (enforced in code)

- Works only in a CS2 started with `-insecure`, which keeps it off every VAC-secured server: `ofast-attach` reads
  the running `cs2.exe`'s command line and refuses any other; the DLL checks its own process's again as it loads and
  refuses to load without it (Windows unloads it at once). Both split the command line by Windows' own rules.
- It films demos: the camera override and the capture act only while a demo plays.
- Every slot, offset and code pattern it relies on is checked before use (`src/cs2.rs`, `src/view.rs`): code where
  code is expected, globals in the module's writable data, the patch site's call on CS2's own engine interface,
  memory readable before it is read; when one does not look as expected it logs why and changes nothing.

## Use

```bash
cargo build -p ofast-capture --release          # target/release/ofast_capture.dll and ofast-attach.exe
"C:\Program Files (x86)\Steam\steam.exe" -applaunch 730 -insecure -console -netconport 2121
target/release/ofast-attach.exe                 # once CS2 is at its menu; refuses without -insecure
```

The DLL writes what it does to `%LOCALAPPDATA%\OFAST\capture.log`. It is driven by a command file,
`%LOCALAPPDATA%\OFAST\capture\command.txt` (read and deleted within half a second; UTF-8 or UTF-16 as Windows
PowerShell writes it; paths may be quoted; write it whole, or to another name and rename it), one command a line:

| Command | What |
|---|---|
| `camera <cameras file> <number>` | play that camera of the file (a shot camera, an HLAE camera) at the game's clock |
| `camera <cameras file> eyes <controller>` | play a player's first-person camera from the file |
| `camera off` | CS2's own view again |
| `record <frames> <folder>` | the next frames as `frame_00000.tga` … (32-bit, top-down), while a demo plays |
| `record <frames> from <tick> <folder>` | the same, from the first frame at or past that demo tick |

For frames at an even pace of demo time, set `host_framerate 60` in CS2 first (CS2 then runs a demo faster than real
time, so start recordings by tick, and pause the demo around a seek). With a camera that looks at the player CS2
spectates, use the free spectator mode (`spec_mode 4`): CS2 does not draw the spectated player's own body in first
person. `cl_drawhud 0` hides the HUD.

## What it does (first pass)

- **Attach** (`src/attach.rs`): `CreateRemoteThread(LoadLibraryW)` on a copy of the DLL
  (`%LOCALAPPDATA%\OFAST\capture\ofast_capture-<time>.dll`), so the built one can be built again. Once in, it stays
  until CS2 quits; attaching twice is refused.
- **Clock** (`src/cs2.rs`): CS2's interfaces from `CreateInterface`, the client's `FrameStageNotify` hooked by vtable
  slot, the game's clock read through the pointer `SetGlobals` stores (decoded from its code).
- **Frames** (`src/capture.rs`): `IDXGISwapChain::Present` hooked by slot in `dxgi.dll`'s vtable, the back buffer
  copied to a staging texture and written as TGA. 120 frames at 2560 × 1440 in 1.6 s.
- **Cameras** (`src/view.rs`): the 16 bytes where `CViewRender::SetUpView` asks `IsPlayingDemo` patched to call
  OFAST with the view setup, whose origin, angles and field of view it sets from the camera at the game's clock.

## Checked (2026-10-27, de_dust2 round 17 of Aurora vs FURIA)

- The game's clock minus the demo tick: 134067 + a fraction, the demo's own server tick offset, as OFAST reads it
  from the demo (within 2 ticks after a seek: CS2 draws a little behind its clock).
- `host_framerate 60`: every 32 frames, 34 demo ticks.
- OFAST's first-person camera of YEKINDAR against CS2's own view of him, same tick: the same picture to a few pixels
  (mean difference 7 – 10 / 255, the rest from interpolation between ticks).
- A fixed camera of the cameras file, played in CS2: YEKINDAR shooting MAJ3R through the upper tunnel's doorway, as
  the same camera shows the round in Unreal.

## Not yet

- FFmpeg: frames are images; piping them to FFmpeg waits for an FFmpeg install.
- Extra passes (depth, mattes), the Studio's buttons, unloading without quitting CS2, the players' bones.
