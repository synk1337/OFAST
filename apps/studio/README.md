# ofast-studio

The beginner-friendly desktop app: Rust + [Tauri 2](https://tauri.app/) with a React web UI. All file reading
goes through `crates/ofast-core`, as the CLI does.

What it does so far:

- Finds CS2, Unreal Engine 5.8 and OFAST's Unreal project (`unreal/Sandbox`)
- Lists the demos in CS2's replays folder (or any folder), with each one's map and server from its header

Coming next: each demo's rounds with their kill feed and highlights (multi-kills, clutches, no-scopes, wallbangs…),
a 2D radar replay of a round, and "Send to Unreal" (export, map import, round import) with "Open in Unreal".

## Running it

Needs Node.js (22 or newer) and the Rust toolchain.

```bash
cd apps/studio
npm install
npm run tauri dev
```

`npm run tauri dev` starts the web UI's dev server (Vite, port 1420) and the app's window, and rebuilds either side
when its files change. `npm run tauri build` makes the installer (NSIS), with the web UI built and embedded.

The Rust side (`src-tauri`) is in the repository's Cargo workspace but not among its default members:
`cargo test --workspace` checks it. A plain `cargo build -p ofast-studio` makes a development build that loads its UI
from the dev server; only the npm commands above give a window that works on its own.

The Studio finds CS2 through Steam's library list and Unreal Engine 5.8 through the Epic Games Launcher's. For an
install they do not list, set `OFAST_CS2_DIR` or `OFAST_UE_DIR` to its folder.

## Layout

- `src/`: the web UI (`App.tsx`; `api.ts` types the commands)
- `src-tauri/`: the Rust side (`commands.rs`: what the UI can ask), its configuration (`tauri.conf.json`), its
  permissions (`capabilities/`) and icons (`icons/source.png`, the source image; the rest are made from it with
  `npx tauri icon`)
