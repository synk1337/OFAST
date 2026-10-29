# OFAST — One For All Source Toolkit

**A free, open-source, beginner-friendly fragmovie toolkit for Counter-Strike 2.**
Pick a demo, pick a moment, and get the round rebuilt in Unreal Engine 5.8: the map, the players, their weapons and
skins, the grenades, the shots and the cameras — without hand-fixing materials, bones, scales or FOVs.

**Version 1.0** — [download OFAST Studio](https://github.com/synk1337/OFAST/releases/latest) (Windows) ·
[user guide](docs/user-guide/README.md) · [roadmap](docs/roadmap.md)

## What it does

**OFAST Studio**, the app you start from:

- lists the demos you downloaded in CS2 (or any folder), and reads a whole match in a few seconds: every round with
  its winner and score, the kill feed, and a finder for the moments worth filming (3Ks and more, aces, clutches,
  collaterals, no-scopes, wallbangs, kills through smoke, in the air, while blind, with a knife);
- replays a round in 2D on the map's radar, players, deaths, grenades and smokes included;
- picks a camera per kill: the killer's or the victim's eyes (as played, or smoothed), behind or around the killer, a
  fixed camera, from above, or a camera path made in HLAE;
- sends the round to Unreal in one click, then opens it there or renders it (preview, final with compositing passes,
  4K) into `Videos\OFAST`;
- checks your setup: CS2, its version against the ones OFAST was checked on, Unreal Engine 5.8, OFAST's Unreal
  project.

**The round in Unreal**, read from your own CS2 files and the demo:

- **Maps**: any official map, read from its own files (no per-map scripts): its geometry, CS2's materials
  (environment, blends, tint masks, decals, overlays), its sun, sky and lights, CS2's tone curve; calibrated against
  CS2 screenshots on de_dust2, de_mirage, de_inferno, de_nuke, de_anubis and de_vertigo.
- **Players**: their agents and gloves, animated by CS2's own animation graphs and the pose the server sent, deaths as
  CS2's ragdolls; each player's first-person view with their viewmodel, field of view, offsets, hand and recoil.
- **Skins**: paint kits in all nine of CS2's paint styles (Dopplers, Fades, case hardening, Asiimovs, gunsmith…) with
  their wear and pattern seed, ported from CS2's own paint shader; glove skins; pearlescence; stickers (holo, foil,
  gold, paper, scraped, freely placed); charms that swing; name tags; StatTrak counters that count up.
- **Grenades and shots**: smokes shaped by the demo's own smoke data (with holes blown by HEs), molotovs climbing
  walls, HE blasts, flashes; every bullet rebuilt from the demo with its tracer, muzzle flash, impact, blood, bullet
  hole and case, through walls by CS2's penetration rule.
- **Cameras**: a Level Sequence at the demo's 64 ticks a second with the cuts you chose; camera paths go back to HLAE.

**For CS2 itself** (`ofast-tools` zip): `ofast-attach` and its DLL play OFAST's cameras in a CS2 started with
`-insecure` and record the frames; `ofast` is the command line behind everything (export a map, a model, a round).

## Getting started

You need Windows 10 or 11, Counter-Strike 2 (Steam), [Unreal Engine 5.8](https://www.unrealengine.com/) (free, from
the Epic Games Launcher) and a graphics card that runs Unreal Engine 5 comfortably.

1. Download OFAST Studio's setup program (`…_x64-setup.exe`) from the
   [latest release](https://github.com/synk1337/OFAST/releases/latest) and run it (it installs for your user only; the
   setup program is not signed yet, so Windows may warn about an unknown publisher: **More info → Run anyway**).
2. In CS2, download a match from the Watch tab.
3. Start OFAST Studio, open the demo, pick a round and a moment, choose the cameras, **Send to Unreal**. The first
   time a map is used, Unreal imports it and compiles its shaders: this takes a while, once per map.
4. **Open in Unreal** to work on it in Sequencer, or **Render** it.

The [user guide](docs/user-guide/README.md) walks through every screen, for people who have never used Unreal.

## Good to know

- **CS2 updates** can change what OFAST reads. The Studio shows your CS2 version in green when OFAST was checked on
  it, in orange when CS2 updated since; OFAST is checked against each CS2 update and released again when needed.
- **Memory**: importing a round takes Unreal 15–20 GB of RAM, and a big round most of a 12 GB graphics card.
- **Not drawn yet**: glitter and iridescent skins' sparkle, a few rarer map shaders (water, some liquids). See the
  [roadmap](docs/roadmap.md) for what comes next.

## Building from source

```bash
cargo test --workspace                 # the core library and the command line (Rust only)
cd apps/studio && npm install && npm run app   # the Studio (Node.js 22+, Rust)
```

Tests that read the real game sit behind the `game-data` feature and need CS2 installed. Everything else — the
layout of the code, the checks we run, the rules we work by — is in [docs/development.md](docs/development.md).

## How it is built

```
crates/ofast-core   Rust library: reads CS2's files (VPK, KV3, textures, models, maps, animations, shaders) and
                    demos, and exports everything as glTF and JSON for Unreal.
crates/ofast-cli    The `ofast` command line over the core.
apps/studio         OFAST Studio: Rust + Tauri desktop app with a React UI.
apps/capture        The CS2 capture DLL and `ofast-attach` (only in -insecure mode).
tools/unreal        Unreal Python scripts that build the scene in OFAST's Unreal project (unreal/Sandbox).
```

OFAST is written from scratch. Existing community tools (HLAE, Source 2 Viewer / ValveResourceFormat, demoparser2…)
are studied as references and used as *test oracles* — our output is compared against theirs — but their code is not
bundled. See [CREDITS.md](CREDITS.md) and [docs/decisions/001-rust-core-remake-not-reuse.md](docs/decisions/001-rust-core-remake-not-reuse.md).

## Valve content and fair use

OFAST **never ships Valve assets**. Everything is extracted locally from *your own* CS2 installation: never share
what it exports. Videos made with it fall under Valve's [video policy](https://store.steampowered.com/video_policy).
OFAST is non-commercial and is not affiliated with or endorsed by Valve.

The capture DLL refuses to run unless CS2 was started with `-insecure`, which blocks all VAC-secured servers.
**Never** try to use it online.

## Documentation

- [docs/user-guide](docs/user-guide/README.md) — using OFAST, step by step
- [docs/architecture.md](docs/architecture.md) — how the pieces fit together
- [docs/roadmap.md](docs/roadmap.md) — milestones M0–M10 and what comes next
- [docs/development.md](docs/development.md) — building, testing, rules we work by
- [docs/formats/](docs/formats/) — what we know about each CS2 file format
- [docs/research/](docs/research/) — how CS2 does things (skins, animation, capture)
- [docs/decisions/](docs/decisions/) — why things are the way they are (and dead ends)
- [docs/releases/](docs/releases/) — release notes

## License

[MIT](LICENSE).
