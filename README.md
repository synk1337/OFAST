# OFAST — One For All Source Toolkit

**A free, open-source, beginner-friendly fragmovie toolkit for Counter-Strike 2.**
Pick a demo, pick a moment, and get the scene rebuilt in Unreal Engine 5: the map, the players, the camera and the
effects — without hand-fixing materials, bones, scales or FOVs.

> **Status: early development (milestone M0 — foundations).** Nothing is usable yet. Follow along in
> [docs/roadmap.md](docs/roadmap.md).

## What OFAST will do

| Area | Goal |
|---|---|
| Demos | Browse a `.dem` by rounds and kills, pick a highlight in a few clicks. |
| Unreal scene | One click: map, materials, lights, players, weapons, POV camera, grenades and effects in a UE 5.8 Level Sequence. |
| Game capture | Everything HLAE does in CS2 (free cam, camera paths, multi-pass EXR/depth/matte recording, FFmpeg) with a friendly UI. |
| Camera tools | Camera paths, smoothing, cinematic presets; imports HLAE `.cam` files. |
| Skins | Weapon finishes with wear, pattern seed and stickers (stretch goal). |

## How it is built

```
ofast-core     Rust library: reads CS2 files (VPK, KV3, textures, models, maps) and demos; all the math.
ofast-studio   Rust + Tauri desktop app: the beginner-friendly front door.
ofast-capture  Rust DLL loaded into CS2 (only in -insecure mode) for recording and live data.
ofast-unreal   Unreal Engine 5.8 plugin: thin C++ shell over ofast-core that builds the scene.
```

OFAST is written from scratch. Existing community tools (HLAE, Source 2 Viewer / ValveResourceFormat, demoparser2…)
are studied as references and used as *test oracles* — our output is compared against theirs — but their code is not
bundled. See [CREDITS.md](CREDITS.md) and [docs/decisions/001-rust-core-remake-not-reuse.md](docs/decisions/001-rust-core-remake-not-reuse.md).

## Valve content and fair use

OFAST **never ships Valve assets**. Everything is extracted locally from *your own* CS2 installation. Videos made with
it fall under Valve's [video policy](https://store.steampowered.com/video_policy). OFAST is non-commercial and is not
affiliated with or endorsed by Valve.

The capture DLL refuses to run unless CS2 was started with `-insecure`, which blocks all VAC-secured servers.
**Never** try to use it online.

## Documentation

- [docs/architecture.md](docs/architecture.md) — how the pieces fit together
- [docs/roadmap.md](docs/roadmap.md) — milestones M0–M10
- [docs/development.md](docs/development.md) — building, testing, rules we work by
- [docs/formats/](docs/formats/) — what we know about each CS2 file format
- [docs/decisions/](docs/decisions/) — why things are the way they are (and dead ends)

## License

[MIT](LICENSE).
