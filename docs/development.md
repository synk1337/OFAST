# Development guide

## Prerequisites (Windows)

| Tool | Why | Check |
|---|---|---|
| Rust (stable, MSVC) | everything in `crates/` and `apps/` | `cargo --version` |
| Visual Studio Build Tools (C++ workload) | Rust linker, Unreal plugin | `vswhere -all` |
| Node.js | Tauri front-end (`apps/studio`, from M3) | `node --version` |
| Unreal Engine 5.8 | `unreal/OFAST` plugin (from M2) | Epic Games Launcher |
| Counter-Strike 2 | local test data and ground truth | Steam |

## Everyday commands

```bash
cargo fmt --all                     # format
cargo clippy --all-targets          # lint
cargo test                          # unit tests on synthetic fixtures (what CI runs)
cargo test --features game-data     # extra tests that read YOUR local CS2 install
cargo doc --no-deps --open          # browse the API docs
```

`game-data` tests find CS2 automatically through Steam's default path. Override with the `OFAST_CS2_DIR` environment
variable (the folder that contains `game\csgo\pak01_dir.vpk`). Add `--release` when running them: they read whole
VPK archives and 200 MB demos.

| Variable | Used by | Meaning |
|---|---|---|
| `OFAST_CS2_DIR` | all `game-data` tests | CS2 root folder, if not in Steam's default library |
| `OFAST_TEST_DEMO` | `tests/cs2_demo.rs` | demo to test with; default: the newest `.dem` in `game\csgo\replays` (download a match in CS2's Watch tab to get one) |
| `OFAST_ORACLE_PYTHON` | `tests/cs2_demo.rs` | Python with demoparser2 installed (see below); default: `python` on the PATH; skipped if neither has it |

## Test data rules

- **Never commit Valve content** — no `.vpk`, `_c` resources, demos or exports. `.gitignore` blocks the common ones.
- Local-only files (reference demos, oracle outputs, screenshots) go in `test-data/` at the repo root, which is ignored.
- CI tests use **synthetic fixtures**: small files our own tests build in memory, byte by byte, following the
  documented format. This also proves the docs in `docs/formats/` are complete.

## Oracles

Each reader is checked against a reference tool on the same input. Differences must be explained (in the format doc)
or fixed.

| Reader | Oracle |
|---|---|
| VPK | ValvePak / Source 2 Viewer CLI file listing |
| KV3 / resources | Source 2 Viewer CLI decompiled text |
| Demos | demoparser2 per-tick output, through `tools/oracle/demoparser2_oracle.py` |

Setting up the demo oracle once (in any folder outside the repository — it is a test tool, not a dependency;
on Linux/macOS the interpreter is `oracle-venv/bin/python`):

```bash
python -m venv oracle-venv
oracle-venv/Scripts/python -m pip install demoparser2 pandas
```

Then set `OFAST_ORACLE_PYTHON` to `oracle-venv/Scripts/python` (full path) and run
`cargo test --release --features game-data --test cs2_demo -- --nocapture`. The test prints how many rows matched
and the largest differences.

Developer tools for poking at demos: `cargo run --release --example demo_inspect -- <demo> [rounds | fields <Class>…]`
and `cargo run --release --example demo_round -- <demo> <round> > round.csv`.

Developer tools for game files: `cargo run --release --example resource_dump -- find <text>` lists archive paths,
`cargo run --release --example resource_dump -- <path>_c` dumps a compiled file's blocks (KV3 as text), and
`cargo run --release --example kv3_inspect -- <path>_c` prints raw KV3 headers.

## Exporting and checking in Unreal

```bash
cargo run --release -p ofast-cli -- export-model weapons/models/ak47/weapon_rif_ak47.vmdl_c --out test-data/exports/ak47
powershell -File tools/unreal/run_shot.ps1 -Gltf test-data/exports/ak47/weapon_rif_ak47.gltf -Shot test-data/shots/ak47.png -Name ak47
```

`run_shot.ps1` opens the sandbox project `unreal/Sandbox/OFASTSandbox.uproject` in UE 5.8, imports the glTF through
Interchange, builds a small level (sky, sun, floor, camera) and writes a 1920×1080 screenshot, then closes the editor
(`tools/unreal/import_and_shot.py`). The first run on a machine compiles shaders and is slow; later runs take under a
minute. Imported content in `unreal/Sandbox/Content/` is ignored by git (it comes from Valve files).

## Rules we work by

1. **Comment and document a lot.** Every crate has `missing_docs` on. Every module opens with a doc comment that
   explains the format or system and where the knowledge came from. New formats get a page in `docs/formats/`.
2. **CI/CD.** Every push runs fmt, clippy (warnings are errors), tests and docs. Tagged versions publish releases.
3. **Screenshots check results.** Anything visual is verified with a screenshot: Unreal `HighResShot`, CS2
   `screenshot`, the Studio UI, and overlay/diff images from `tools/compare`.
4. **CS2 in `-insecure` for testing.** Offline and local demo playback only, never online. Used for ground truth
   (`getpos`, `cl_showpos`, `demo_gototick`) and, later, live dumps through ofast-capture.
5. **Revise at dead ends.** Spikes are time-boxed. A dead end gets a decision-log entry with the alternatives, and big
   pivots are agreed before they happen.
6. **Use agents.** Reference sources are researched in parallel before a reader is written; independent crates can be
   built in parallel; code is reviewed before merging.

## Decision log

Architecture choices and dead ends are recorded in [docs/decisions/](decisions/). Copy
[decisions/000-template.md](decisions/000-template.md) to start a new one.
