# 001 — Rust core, remake instead of reuse

- **Date:** 2026-10-15
- **Status:** accepted

## Context

Every building block for a CS2 → Unreal fragmovie pipeline already exists in some form:

- **HLAE** (C++, MIT) records CS2 footage and exports cameras, but has no 3D scene export for CS2.
- **Source 2 Viewer / ValveResourceFormat** (C#, MIT) decompiles CS2 maps, models and materials to glTF.
- **demoparser2** (Rust, MIT) reads CS2 demos.
- **UEditingTools** (GPL) and **afxlivelink** (no license) show partial Unreal pipelines.

Nobody has joined them into one beginner-friendly tool. The quickest route would be to glue these together. The
project owner prefers to learn from them and **rewrite everything cleanly**, in one consistent codebase.

## Decision

- OFAST is written from scratch. Existing tools are **references** (read to learn the formats) and **test oracles**
  (our output is compared to theirs), never runtime dependencies.
- **Rust** is used everywhere it can be: the core library, the desktop app (Tauri 2), the CLI and the CS2 capture DLL.
- The **Unreal plugin is C++** because Unreal requires it, but it stays a thin shell that calls the Rust core through a
  C interface (`cbindgen`).
- Code that is closely derived from an MIT project keeps the original notice (see `CREDITS.md`). Nothing is taken
  from GPL or unlicensed projects.

## Alternatives considered

- **Glue existing tools together** (C# desktop app on ValveResourceFormat + demofile-net, drive HLAE). Much faster
  to a first result, but inherits three codebases in three languages, their limits and their release cadence.
- **C++ everywhere.** Matches Unreal and HLAE, but binary parsers are where memory-safety bugs live, and Rust's
  tooling (cargo, tests, docs) keeps a hobby project healthier.
- **C# everywhere.** Good for parsing, but awkward for an injected game DLL and for embedding into Unreal.

## Consequences

- Much more work up front: the Source 2 asset reader alone is a large job (ValveResourceFormat is ~10 years of work).
  We only implement what the CS2 → Unreal path needs, in roadmap order.
- One language and one test suite for all format knowledge; CI catches regressions everywhere at once.
- We must keep up with CS2 updates ourselves (demo protobufs, game internals). The capture DLL reads offsets from
  CS2's schema system at runtime to reduce this.
