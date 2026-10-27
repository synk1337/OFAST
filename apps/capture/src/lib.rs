//! # ofast-capture (M8)
//!
//! OFAST's remake of HLAE's CS2 capture: a DLL loaded into CS2 to film demos — OFAST's cameras played in the game,
//! frames sent to FFmpeg, later extra passes and the players' bones (docs/decisions/022-capture-attach.md,
//! docs/research/hlae-cs2-capture.md).
//!
//! It works only in a CS2 started with `-insecure`, which keeps it off every VAC-secured server: the launcher
//! (`attach`, the `ofast-attach` program) refuses any other, and the DLL checks again on its own ([`gate`]), refusing
//! to load at all without it.
//!
//! - [`gate`]: the `-insecure` check.
//! - [`x64`]: the bits of machine code read in CS2's functions.
//! - `cs2` (Windows): CS2's modules, interfaces, slots and offsets, each checked before use.
//! - `hook` (Windows): vtable slot swaps.
//! - `dll` (Windows): the DLL's entry point and its work inside CS2.
//! - `capture` (Windows): frames read from the swap chain and written as images; the command file.
//! - `view` (Windows): OFAST's cameras played in CS2 (the view setup patched).
//! - `log` (Windows): `%LOCALAPPDATA%\OFAST\capture.log`.
//! - `attach` (Windows): finding CS2 and loading the DLL into it.
//!
//! Everything but [`gate`] and [`x64`] is Windows-only: elsewhere the crate builds empty.

pub mod gate;
pub mod x64;

#[cfg(windows)]
pub mod attach;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod cs2;
#[cfg(windows)]
mod dll;
#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod view;
