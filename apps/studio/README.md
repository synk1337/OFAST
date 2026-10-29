# ofast-studio

The beginner-friendly desktop app: Rust + [Tauri 2](https://tauri.app/) with a React web UI. All file reading
goes through `crates/ofast-core`, as the CLI does.

What it does so far:

- Finds CS2, Unreal Engine 5.8 and OFAST's Unreal project (`unreal/Sandbox`), and shows CS2's version: green when
  OFAST was checked on it, orange when CS2 updated since (`ofast_core::game::CHECKED_VERSIONS`)
- Lists the demos in CS2's replays folder (or any folder), with each one's map and server from its header
- Opens a demo on its page: every round with its winner, score and best moments, the kill feed of the round chosen,
  and a finder for the moments worth filming — highlights (3K and more, aces, won clutches, collaterals, no-scopes,
  wallbangs, kills through smoke, in the air, while blind, with a knife) or every kill, by player and weapon (AWP,
  pistols…). Reading a whole match takes a few seconds; the result is kept in the app's cache folder until the demo
  changes.
- Replays a round in 2D on the map's radar (CS2's own radar image and overview): players, deaths, grenades,
  smokes and fires, with play / pause, speed and a timeline; a kill clicked in the feed plays from a few seconds
  before it
- Picks each kill's camera (the killer's or the victim's eyes, behind, around or above the killer, a fixed camera),
  first-person smoothing and HLAE camera files for a round (`docs/formats/cameras.md`)
- Sends a round to OFAST's Unreal project ("Send to Unreal": the round exported with its players and cameras, the
  map exported and imported the first time, the round built as a Level Sequence in the map's level, with progress),
  changes the cameras of a round already there ("Update cameras"), then opens it ("Open in Unreal": the level, and
  the sequence in Sequencer)
- Renders a round already there ("Render": Movie Render Queue, a preset, around the kills or the whole round, into
  `Videos\OFAST`; `docs/formats/render.md`)

How to use it, for beginners: [docs/user-guide](../../docs/user-guide/README.md).

## Running it

Needs Node.js (22 or newer) and the Rust toolchain.

```bash
cd apps/studio
npm install
npm run app
```

`npm run app` starts the web UI's dev server (Vite, port 1420) and the app's window, and rebuilds either side when its
files change. Its Rust side is built optimized: reading a demo in a debug build is about 20 times slower (a whole
match: a minute instead of seconds). `npm run tauri dev` gives the debug build, quicker to rebuild while working on
the Rust side. `npm run tauri build` makes the installer (NSIS), with the web UI built and embedded.

The Rust side (`src-tauri`) is in the repository's Cargo workspace but not among its default members:
`cargo test --workspace` checks it. A plain `cargo build -p ofast-studio` makes a development build that loads its UI
from the dev server; only the npm commands above give a window that works on its own.

The Studio finds CS2 through Steam's library list and Unreal Engine 5.8 through the Epic Games Launcher's. For an
install they do not list, set `OFAST_CS2_DIR` or `OFAST_UE_DIR` to its folder.

## Layout

- `src/`: the web UI (`App.tsx`: setup and demo list; `DemoPage.tsx`: a demo's page; `RadarView.tsx`: the 2D
  replay; `SendBar.tsx`: "Send to Unreal" and "Render" while they run; `names.ts`: weapons and highlights as people
  say them; `api.ts` types the commands)
- `src-tauri/`: the Rust side (`commands.rs`: setup, demos, summaries, radar; `unreal.rs`: sending a round to
  Unreal, rendering it and opening it), its configuration (`tauri.conf.json`), its permissions (`capabilities/`) and icons
  (`icons/source.png`, the source image; the rest are made from it with `npx tauri icon`)
