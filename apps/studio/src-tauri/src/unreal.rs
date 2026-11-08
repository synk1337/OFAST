//! "Send to Unreal" and "Open in Unreal": a round of a demo into OFAST's Unreal project (`unreal/Sandbox`), the same
//! steps as the command line (`docs/development.md`, "A demo round in Unreal"):
//!
//! 1. export the round with its players and its cameras (`ofast export-demo --players`, with the shots the user
//!    picked: [`CameraPlan`]),
//! 2. the first time a map is used in a look (CS2's, or realistic: `LOOKS`), export it (`ofast export-map`) and
//!    import it (`tools/unreal/import_map.py`, its style: the realistic look's level is `<map>_realistic`); again
//!    once when a release changed how maps are made (`MAP_VERSION`),
//! 3. import the round (`tools/unreal/import_demo.py`, in the same style): a Level Sequence in that level.
//!
//! "Update cameras" changes the cameras of a round already sent, without importing it again: its cameras exported
//! (`export::cameras`), then `tools/unreal/import_cameras.py` (decision 021). "Render" renders a round already sent
//! with Movie Render Queue (`tools/unreal/render_round.py`, decision 023) into the user's Videos folder.
//!
//! The Unreal steps run `UnrealEditor-Cmd` without a window; its `OFAST:` log lines come to the UI as progress
//! ([`PROGRESS_EVENT`]), and a step counts as done only once its script logged its last line. Each line carries where
//! the job's progress bar is: each step takes a share of it as long as it usually takes ([`Progress`]), and the map's
//! export and Unreal's scripts (their `progress` lines) move it within their step. Imports draw nothing,
//! so the editor runs without a renderer (`-nullrhi`): it takes no GPU memory; renders need one. Exports go to the
//! app's data folder (`exports/demos`, `exports/maps/<map>`) and are deleted once imported: they are made from the
//! user's CS2 files and are big (a map: gigabytes).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use ofast_core::content::Content;
use ofast_core::demo::players::track_round_at;
use ofast_core::export::ExportOptions;
use ofast_core::export::cameras::{CameraPlan, export_cameras};
use ofast_core::export::map::export_installed_map_reporting;
use ofast_core::export::players::export_players;
use ofast_core::export::replay::write_replay;
use ofast_core::game::Cs2Install;
use serde::Serialize;
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{UNREAL_EDITOR, UnrealSide, blocking, unreal_dir, unreal_side};

/// The event the UI listens to for a sending's progress ([`Progress`]).
pub const PROGRESS_EVENT: &str = "unreal-progress";

/// The editor without a window, for imports.
const EDITOR_CMD: &str = r"Engine\Binaries\Win64\UnrealEditor-Cmd.exe";

/// Unreal's log in the project's folder, for the user when an import fails.
const LOG: &str = r"Saved\Logs\OFASTSandbox.log";

/// The environment variables OFAST's Unreal scripts read: all are set for each run (empty when unused), so none
/// leaks from the Studio's own environment.
const SCRIPT_VARIABLES: [&str; 22] = [
    "OFAST_CAMERAS",
    "OFAST_GLTF",
    "OFAST_SHOT",
    "OFAST_NAME",
    "OFAST_VIEWS",
    "OFAST_STYLE",
    "OFAST_DISABLE",
    "OFAST_ONLY",
    "OFAST_PRESET",
    "OFAST_TRY",
    "OFAST_REPLAY",
    "OFAST_POV_SHOTS",
    "OFAST_CLOSE_SHOTS",
    "OFAST_VIEW_SHOTS",
    "OFAST_LEVEL",
    "OFAST_SEQUENCE",
    "OFAST_TICKS",
    "OFAST_FPS",
    "OFAST_MOTION_BLUR",
    "OFAST_PLAYERS_LAYER",
    "OFAST_POWER",
    "OFAST_OUTPUT",
];

/// Where a sending is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Exporting the round and its players.
    Round,
    /// Exporting the map (the first time it is used).
    MapExport,
    /// Importing the map into Unreal (the first time).
    MapImport,
    /// Importing the round into Unreal.
    RoundImport,
    /// Changing the cameras of a round already in Unreal.
    Cameras,
    /// Rendering a round already in Unreal.
    Render,
}

/// One line of a sending's progress.
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// The step it belongs to.
    pub step: Step,
    /// What happens, for the user.
    pub message: String,
    /// A warning the UI keeps showing (the later progress lines replace the others).
    pub warning: bool,
    /// How much of the job is done, 0 to 1, for the UI's progress bar (each step takes a share of it as long as it
    /// usually takes).
    pub done: f64,
    /// Where the bar will be once what runs now is done (at least `done`): the UI moves the bar slowly towards it
    /// while it waits for the next line.
    pub next: f64,
    /// About how long until the bar is at `next` (seconds, as long as the steps usually take): the UI's bar creeps
    /// over that time.
    pub wait_s: f64,
}

/// A job's progress bar: its steps in order, each with about how long it takes (seconds), which is its share of the
/// bar.
#[derive(Debug, Clone)]
struct Bar(Vec<(Step, f64)>);

impl Bar {
    /// About how long the whole job takes, seconds.
    fn seconds(&self) -> f64 {
        self.0.iter().map(|(_, seconds)| seconds).sum()
    }

    /// The bar's share done at `within` (0 to 1) of `step`; the whole bar for a step not in it.
    fn at(&self, step: Step, within: f64) -> f64 {
        let total = self.seconds();
        let mut before = 0.0;
        for (s, seconds) in &self.0 {
            if *s == step {
                return (before + seconds * within.clamp(0.0, 1.0)) / total;
            }
            before += seconds;
        }
        1.0
    }
}

/// About how long the steps take (seconds), for their shares of the bar, timed on the PC OFAST is made on (24
/// threads; Unreal's in its logs and a sending from the Studio, 2026-11-03). The round's export (de_dust2's round 1,
/// 10 players: 156 s): reading it (2 s), its players' agents and weapons (most of it), its cameras (a few seconds).
const ROUND_S: f64 = 150.0;
/// A map's export: de_dust2 17 to 18 s, de_inferno 29 s.
const MAP_EXPORT_S: f64 = 30.0;
/// A map's import into Unreal again (de_dust2: 70 to 100 s of `import_map.py`, after Unreal's start).
const MAP_IMPORT_AGAIN_S: f64 = UNREAL_START_S + 100.0;
/// A map's first import, a guess: Interchange imports its textures and Unreal compiles its shaders.
const MAP_IMPORT_FIRST_S: f64 = UNREAL_START_S + 600.0;
/// A round's import (de_dust2's round 1: 125 to 390 s of `import_demo.py`, most of it the players' files).
const ROUND_IMPORT_S: f64 = UNREAL_START_S + 250.0;
/// How long Unreal takes to start, before its script runs (10 to 55 s): the first part of an Unreal step.
const UNREAL_START_S: f64 = 40.0;
/// "Update cameras": the cameras made (a few seconds), then Unreal's start and `import_cameras.py`.
const CAMERAS_S: f64 = 90.0;
/// The share of "Update cameras" before Unreal starts.
const CAMERAS_MADE: f64 = 0.1;
/// A render, for the bar's creep (its frames make it longer or shorter: `render_round.py` says how far they are).
const RENDER_S: f64 = 600.0;

/// Unreal events closer than this on the bar (its share) are not sent: the map's export moves it once per texture.
const SMALLEST_MOVE: f64 = 0.005;

/// Tells the UI where a job is ([`PROGRESS_EVENT`]): what it does, and where its bar is ([`Bar`]). Shared with the
/// threads of the map's export.
struct Reporter<'a> {
    app: &'a AppHandle,
    place: Mutex<Place>,
}

/// Where a job is.
struct Place {
    bar: Bar,
    step: Step,
    message: String,
    /// The bar's share done, and where it goes once the part running is done ([`Progress`]).
    done: f64,
    next: f64,
    /// `done` as the last event sent it.
    sent: f64,
}

impl<'a> Reporter<'a> {
    fn new(app: &'a AppHandle, bar: Bar) -> Self {
        let step = bar.0.first().map_or(Step::Round, |(step, _)| *step);
        Reporter {
            app,
            place: Mutex::new(Place {
                bar,
                step,
                message: String::new(),
                done: 0.0,
                next: 0.0,
                sent: 0.0,
            }),
        }
    }

    fn place(&self) -> std::sync::MutexGuard<'_, Place> {
        self.place
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The job's steps once they are known (a sending's, once the round says which map it needs). The bar never goes
    /// back: a plan is set before any of its steps moved it.
    fn plan(&self, bar: Bar) {
        self.place().bar = bar;
    }

    /// A part of `step` begins, from `from` to `to` of it (0 to 1): `message` says what it does.
    fn part(&self, step: Step, from: f64, to: f64, message: impl Into<String>) {
        self.go(step, from, to, Some(message.into()));
    }

    /// The same, saying nothing new (Unreal's progress lines, the map export's textures): sent only once the bar
    /// moved by [`SMALLEST_MOVE`].
    fn stage(&self, step: Step, from: f64, to: f64) {
        self.go(step, from, to, None);
    }

    fn go(&self, step: Step, from: f64, to: f64, message: Option<String>) {
        let mut place = self.place();
        let done = place.bar.at(step, from).max(place.done);
        place.next = place.bar.at(step, to).max(done);
        place.done = done;
        place.step = step;
        let moved = done - place.sent >= SMALLEST_MOVE;
        if let Some(message) = message {
            place.message = message;
        } else if !moved {
            return;
        }
        self.send(&mut place, None);
    }

    /// A line of what runs (Unreal's): the bar stays where it is.
    fn line(&self, message: String) {
        let mut place = self.place();
        place.message = message;
        self.send(&mut place, None);
    }

    /// A warning the UI keeps showing.
    fn warn(&self, message: String) {
        let mut place = self.place();
        self.send(&mut place, Some(message));
    }

    /// Nobody may hear it: the UI may have gone to another page.
    fn send(&self, place: &mut Place, warning: Option<String>) {
        place.sent = place.done;
        let _ = self.app.emit(
            PROGRESS_EVENT,
            Progress {
                step: place.step,
                warning: warning.is_some(),
                message: warning.unwrap_or_else(|| place.message.clone()),
                done: place.done,
                next: place.next,
                wait_s: (place.next - place.done) * place.bar.seconds(),
            },
        );
    }
}

/// Where an Unreal script's run is on the bar: in `step`, from `from` (its share) to the step's end, Unreal's start
/// taking the first `start` of that.
#[derive(Debug, Clone, Copy)]
struct Run {
    step: Step,
    from: f64,
    start: f64,
}

impl Run {
    /// A run filling `step`, which takes about `seconds`.
    fn whole(step: Step, seconds: f64) -> Self {
        Run {
            step,
            from: 0.0,
            start: (UNREAL_START_S / seconds).min(1.0),
        }
    }

    /// The step's share at `script` (0 to 1) of the script's own work.
    fn at(&self, script: f64) -> f64 {
        let started = self.from + (1.0 - self.from) * self.start;
        started + (1.0 - started) * script.clamp(0.0, 1.0)
    }
}

/// What a sending made: where to look in Unreal.
#[derive(Debug, Clone, Serialize)]
pub struct Sent {
    /// The map's level, `/Game/OFAST/Maps/<map>` (or `<map>_realistic`; after "Update cameras" always the former:
    /// `open_sequence.py` opens the level the sequence names).
    pub level: String,
    /// The round's Level Sequence, `/Game/OFAST/Replays/<demo>_r<round>`.
    pub sequence: String,
}

/// One sending (or render) at a time: two editors on one project would trip over each other.
#[derive(Debug, Default)]
pub struct Sending(AsyncMutex<()>);

/// Why a sending or a render cannot start: another one runs.
const BUSY: &str = "Unreal is busy with another round (sent or rendered): wait for it to finish.";

/// The presets of `tools/unreal/render_round.py` (listed there, and in the UI's `names.ts`, too).
const PRESETS: [&str; 3] = ["preview", "final", "4k"];

/// The `UnrealEditor-Cmd` running for a sending, so that closing the Studio stops it ([`stop_sending`]) rather than
/// leaving a hidden editor importing on its own.
static EDITOR_RUNNING: Mutex<Option<u32>> = Mutex::new(None);

/// Sends the round of `path` that starts at `start_tick` to Unreal with the cameras of `plan` (see the
/// [module docs](self)). Takes minutes, the first time on a map much longer (the map's export and import, and Unreal
/// compiling its shaders).
#[tauri::command]
pub async fn send_to_unreal(
    path: String,
    start_tick: u32,
    plan: CameraPlan,
    look: Option<String>,
    app: AppHandle,
    sending: tauri::State<'_, Sending>,
) -> Result<Sent, String> {
    let _only = sending.0.try_lock().map_err(|_| BUSY.to_owned())?;
    let look = look.unwrap_or_else(|| STANDARD.to_owned());
    if !LOOKS.contains(&look.as_str()) {
        return Err(format!("unknown look {look:?}"));
    }
    blocking(move || send(&app, &path, start_tick, &plan, &look)).await?
}

/// The map's looks a round can be built in (`tools/unreal/import_map.py`'s styles): CS2's own, in the level named
/// after the map, or lit physically with an ambiance, in `<map>_realistic`.
const LOOKS: [&str; 2] = [STANDARD, "realistic"];
const STANDARD: &str = "standard";

/// The level of `map` in `look`.
fn level_name(map: &str, look: &str) -> String {
    if look == STANDARD {
        map.to_owned()
    } else {
        format!("{map}_{look}")
    }
}

/// How OFAST exports and imports maps. A release that changes what a map looks like in Unreal (its materials, lights,
/// shading) counts it up: a level imported by an older Studio is then imported again, once, the next time a round is
/// sent on it. "1": OFAST 1.0.7 (map lamps, barn lights, glass and steel); the Studios before it wrote no version.
/// "2": the map's translucent surfaces (glass, cards) drawn under the smokes (`import_map.py`, `TRANSLUCENCY_PASS`).
const MAP_VERSION: &str = "2";

/// Where each level's map version is kept once its import finished (a file named after the level, in the project).
const IMPORTED: &str = r"Saved\OFAST\imported";

/// The level's file `level_name`, in the project.
fn level_file(level_name: &str) -> PathBuf {
    Path::new(r"Content\OFAST\Maps").join(format!("{level_name}.umap"))
}

/// `map`'s import record (`import_map.py`'s `SceneImport_<map>`, shared by its looks), in the project.
fn import_record(map: &str) -> PathBuf {
    Path::new(r"Content\OFAST")
        .join(map)
        .join(map)
        .join(format!("SceneImport_{map}.uasset"))
}

/// Whether `map`'s level `level_name` is in the project at today's [`MAP_VERSION`], with the map's import record.
fn level_imported(project: &Path, map: &str, level_name: &str) -> bool {
    let version = std::fs::read_to_string(project.join(IMPORTED).join(level_name));
    version.is_ok_and(|v| v.trim() == MAP_VERSION)
        && project.join(level_file(level_name)).is_file()
        && project.join(import_record(map)).is_file()
}

/// Changes the cameras of the round of `path` that starts at `start_tick`, already sent, to those of `plan` (see the
/// [module docs](self)): about a minute, most of it Unreal starting.
#[tauri::command]
pub async fn update_cameras(
    path: String,
    start_tick: u32,
    plan: CameraPlan,
    app: AppHandle,
    sending: tauri::State<'_, Sending>,
) -> Result<Sent, String> {
    let _only = sending.0.try_lock().map_err(|_| BUSY.to_owned())?;
    blocking(move || cameras(&app, &path, start_tick, &plan)).await?
}

/// Renders round `round` of the demo at `path`, already sent, with Movie Render Queue (see the
/// [module docs](self)): `preset`'s frames and passes between the demo ticks `ticks` (the end left out; the whole
/// round without them), `fps` frames a second (60 without it; 250 or more for slow motion made in the editing
/// software), with motion blur unless `motion_blur` is false (sharp frames, for velocity edits), into a folder of its
/// own in `Videos\OFAST`, with the players layer if `players_layer`, as `output` says (`OUTPUTS`; images without
/// it), taking as much of the PC as `power` lets it ([`Power`]; all of it without it). `frames`, the frames the UI
/// counts, is checked against the disk's free space (`frame_bytes`). Returns that folder. About a second a frame for
/// `final` on a recent graphics card at full power, after a minute or two of Unreal starting.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri passes a command's arguments one by one.
pub async fn render_round(
    path: String,
    round: u32,
    preset: String,
    ticks: Option<[u32; 2]>,
    fps: Option<u32>,
    motion_blur: Option<bool>,
    players_layer: Option<bool>,
    power: Option<String>,
    output: Option<String>,
    frames: Option<u32>,
    app: AppHandle,
    sending: tauri::State<'_, Sending>,
) -> Result<String, String> {
    let _only = sending.0.try_lock().map_err(|_| BUSY.to_owned())?;
    let power = match power {
        None => Power::Full,
        Some(name) => Power::parse(&name).ok_or(format!("There is no power \"{name}\"."))?,
    };
    let output = output.unwrap_or_else(|| OUTPUTS[0].to_owned());
    if !OUTPUTS.contains(&output.as_str()) {
        return Err(format!("A render cannot be written as \"{output}\"."));
    }
    let job = RenderJob {
        preset,
        ticks,
        fps,
        motion_blur: motion_blur.unwrap_or(true),
        players_layer: players_layer.unwrap_or(false),
        power,
        output,
        frames,
    };
    blocking(move || render(&app, &path, round, &job)).await?
}

/// What a render writes (`tools/unreal/render_round.py`'s OUTPUTS, the UI's `names.ts`): image sequences (PNG, and
/// EXR for `final` and `4k`), or one Apple ProRes 4444 XQ video a pass.
const OUTPUTS: [&str; 2] = ["images", "prores"];

/// About how much disk a frame of `preset` takes (bytes) as `output`, with the players layer if `layer`: measured
/// 1.2 MB a preview frame (round 17 of Aurora vs FURIA on de_dust2, 1430 frames in 1.7 GB), 12 MB a Final frame and
/// 35 MB a 4K one without the layer, 100 MB a 4K one with it (2026-11-04 and 06), Final with it about a quarter of
/// that; as ProRes 4444 XQ 1.3 MB a Final frame (de_nuke round 17, 2026-11-08), four times that in 4K, the layer's two
/// videos about as much again (mostly transparent).
fn frame_bytes(preset: &str, output: &str, layer: bool) -> u64 {
    const MB: u64 = 1 << 20;
    let prores = output == "prores";
    let base = match (preset, prores) {
        ("preview", false) => 3 * MB / 2,
        ("preview", true) => MB / 2,
        ("4k", false) if layer => 100 * MB,
        ("4k", false) => 35 * MB,
        ("4k", true) => 6 * MB,
        (_, false) if layer => 25 * MB,
        (_, false) => 12 * MB,
        (_, true) => 3 * MB / 2,
    };
    if prores && layer { 2 * base } else { base }
}

/// The free space (bytes) on the disk of `dir` for this user, if Windows says.
#[cfg(windows)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path that outlives the call; `free` is a valid u64 and the other two
    // outputs may be null (GetDiskFreeSpaceExW's documentation).
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

/// What a render makes, and how ([`render_round`]'s arguments).
struct RenderJob {
    preset: String,
    ticks: Option<[u32; 2]>,
    fps: Option<u32>,
    motion_blur: bool,
    /// The players layer (`final` and `4k`): each frame rendered twice more, the players alone and the world without
    /// them.
    players_layer: bool,
    power: Power,
    /// What it writes: one of [`OUTPUTS`].
    output: String,
    /// How many frames the UI counts (for the disk's room).
    frames: Option<u32>,
}

/// How much of the PC a render may take (the UI's "Power", `tools/unreal/render_round.py`'s POWER). Movie Render
/// Queue renders flat out; below `Full` the script rests the graphics card between frames (it then draws less power,
/// and other programs get it) and Unreal runs at a CPU priority below the user's programs, the shader compilers it
/// starts too (they inherit it). de_nuke in 4K on an RTX 5070 (decision 027): `Full` 2.2 s a frame, 154 W;
/// `Balanced` 2.5 s, 136 W; `Low` 3.7 s, 106 W.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    /// All of it, as fast as the PC goes.
    Full,
    /// Rests now and then: about 15 % longer.
    Balanced,
    /// Rests as long as it works: about 1.7 times as long.
    Low,
}

impl Power {
    /// The names the UI and the script use.
    const NAMES: [(&str, Power); 3] = [
        ("full", Power::Full),
        ("balanced", Power::Balanced),
        ("low", Power::Low),
    ];

    fn parse(name: &str) -> Option<Power> {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| *p)
    }

    fn name(self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|(_, p)| *p == self)
            .map_or("full", |(n, _)| n)
    }

    /// Unreal's CPU priority, as a Windows process creation flag (0: normal, the default).
    fn priority_class(self) -> u32 {
        /// `BELOW_NORMAL_PRIORITY_CLASS`, from Windows' process creation flags.
        const BELOW_NORMAL: u32 = 0x0000_4000;
        match self {
            Power::Full => 0,
            Power::Balanced | Power::Low => BELOW_NORMAL,
        }
    }
}

/// The frame rates a render may ask for (`tools/unreal/render_round.py`'s FPS_RANGE).
pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 10..=1000;

/// Shows a render's folder (as [`render_round`] returned it) in Windows' Explorer; no other folder.
#[tauri::command]
pub async fn open_render_folder(folder: String, app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let root = renders_dir(&app)?;
        let folder = PathBuf::from(folder);
        // `file_name` is None for a path ending in "..", which would climb out of the renders.
        if folder.file_name().is_none()
            || folder.parent() != Some(root.as_path())
            || !folder.is_dir()
        {
            return Err(format!("{} is not a render's folder.", folder.display()));
        }
        let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        Command::new(Path::new(&system).join("explorer.exe"))
            .arg(&folder)
            .spawn()
            .map(drop)
            .map_err(|e| format!("Explorer did not start: {e}"))
    })
    .await?
}

/// Where renders go: `OFAST` in the user's Videos folder, a folder per render in it.
fn renders_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let videos = app.path().video_dir().map_err(|e| e.to_string())?;
    Ok(videos.join("OFAST"))
}

fn render(app: &AppHandle, path: &str, round: u32, job: &RenderJob) -> Result<String, String> {
    let report = Reporter::new(app, Bar(vec![(Step::Render, RENDER_S)]));
    let preset = job.preset.as_str();
    if !PRESETS.contains(&preset) {
        return Err(format!("There is no render preset \"{preset}\"."));
    }
    if let Some(fps) = job.fps.filter(|f| !FPS_RANGE.contains(f)) {
        return Err(format!(
            "{fps} frames a second: pick from {} to {}.",
            FPS_RANGE.start(),
            FPS_RANGE.end()
        ));
    }
    let fps = job.fps.map(|f| f.to_string()).unwrap_or_default();
    let blur = if job.motion_blur { "" } else { "0" };
    let layer = if job.players_layer { "1" } else { "" };
    let ticks = job.ticks;
    let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
    let side = unreal_side().ok_or("OFAST's Unreal project was not found.")?;
    unreal_closed()?;
    let prefix = format!("{}_r{round}", asset_name(stem(path)));
    if !side
        .dir()
        .join(REPLAYS)
        .join(format!("{prefix}.uasset"))
        .is_file()
    {
        return Err("This round is not in Unreal yet: send it first.".into());
    }
    if running("cs2.exe") {
        report.warn(
            "CS2 is open: rendering needs the graphics card's memory, close CS2 if Unreal fails."
                .into(),
        );
    }
    let out = renders_dir(app)?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // A disk that fills stops the render and slows the whole PC (a 1-minute 4K render takes about 130 GB).
    if let (Some(frames), Some(free)) = (job.frames, free_bytes(&out)) {
        let need = u64::from(frames) * frame_bytes(preset, &job.output, job.players_layer);
        if need > free {
            const GB: f64 = (1u64 << 30) as f64;
            report.warn(format!(
                "This render takes about {:.0} GB and the disk has {:.0} GB free: it will stop when the disk is \
                 full. Render less of the round, or free some space.",
                need as f64 / GB,
                free as f64 / GB
            ));
        }
    }
    let sequence = format!("/Game/OFAST/Replays/{prefix}");
    let range = ticks
        .map(|[start, end]| format!("{start},{end}"))
        .unwrap_or_default();
    let run = Run::whole(Step::Render, RENDER_S);
    report.part(
        Step::Render,
        0.0,
        run.at(0.0),
        "Starting Unreal (it loads the map first: a minute or two)…",
    );
    let mut folder = None;
    run_script(
        &engine,
        &side,
        "render_round.py",
        Editor::Drawing(job.power),
        &report,
        run,
        &[
            ("OFAST_SEQUENCE", sequence.as_ref()),
            ("OFAST_PRESET", preset.as_ref()),
            ("OFAST_TICKS", range.as_ref()),
            ("OFAST_FPS", fps.as_ref()),
            ("OFAST_MOTION_BLUR", blur.as_ref()),
            ("OFAST_PLAYERS_LAYER", layer.as_ref()),
            ("OFAST_POWER", job.power.name().as_ref()),
            ("OFAST_OUTPUT", job.output.as_ref()),
            ("OFAST_SHOT", out.as_os_str()),
        ],
        "rendered ",
        |line| {
            // "rendered <folder>: ok", the script's last line.
            if let Some(done) = line
                .strip_prefix("rendered ")
                .and_then(|l| l.strip_suffix(": ok"))
            {
                folder = Some(done.to_owned());
            }
            // "warning: …": the picture shaded smaller to fit the graphics card, or a render too big for it; kept on
            // screen.
            match line.strip_prefix("warning: ") {
                Some(warning) => report.warn(warning.to_owned()),
                None => report.line(line),
            }
        },
    )?;
    folder.ok_or_else(|| "Unreal did not say where it put the frames.".into())
}

/// The rounds of the demo at `path` whose sequence is in OFAST's Unreal project (round numbers): sent before, their
/// cameras can change.
#[tauri::command]
pub async fn sent_rounds(path: String) -> Result<Vec<u32>, String> {
    blocking(move || {
        let Some(side) = unreal_side() else {
            return Vec::new();
        };
        let prefix = format!("{}_r", asset_name(stem(&path)));
        let mut rounds: Vec<u32> = std::fs::read_dir(side.dir().join(REPLAYS))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|file| {
                let name = file.file_name().to_string_lossy().into_owned();
                name.strip_prefix(&prefix)?
                    .strip_suffix(".uasset")?
                    .parse()
                    .ok()
            })
            .collect();
        rounds.sort_unstable();
        rounds
    })
    .await
}

/// Where the rounds' sequences are, in the project's folder.
const REPLAYS: &str = r"Content\OFAST\Replays";

/// A demo path's file name without its extension.
fn stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("demo")
}

/// The CS2 content a round's export reads: the game's archives and the map's, for its collision (the dead fall on
/// it, shot cameras keep out of its walls); the game's alone if the map's is missing.
fn round_content(install: &Cs2Install, map: &str) -> Result<Content, String> {
    Content::for_map(install, map)
        .or_else(|_| Content::game(install))
        .map_err(|e| e.to_string())
}

fn cameras(
    app: &AppHandle,
    path: &str,
    start_tick: u32,
    plan: &CameraPlan,
) -> Result<Sent, String> {
    let report = Reporter::new(app, Bar(vec![(Step::Cameras, CAMERAS_S)]));
    let step = Step::Cameras;
    let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
    let side = unreal_side().ok_or("OFAST's Unreal project was not found.")?;
    unreal_closed()?;
    let install = Cs2Install::locate().ok_or("CS2 was not found.")?;
    report.part(step, 0.0, CAMERAS_MADE / 2.0, "Reading the round…");
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = asset_name(stem(path));
    let track = track_round_at(&data, start_tick)
        .map_err(|e| format!("{path}: {e}"))?
        .without_freeze();
    drop(data);
    let prefix = format!("{name}_r{}", track.round);
    if !side
        .dir()
        .join(REPLAYS)
        .join(format!("{prefix}.uasset"))
        .is_file()
    {
        return Err("This round is not in Unreal yet: send it first.".into());
    }
    report.part(
        step,
        CAMERAS_MADE / 2.0,
        CAMERAS_MADE,
        "Making the cameras…",
    );
    let content = round_content(&install, &track.map_name)?;
    let demos = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("exports")
        .join("demos");
    let made =
        export_cameras(Some(&content), &track, plan, &name, &demos).map_err(|e| e.to_string())?;
    for warning in made.warnings {
        report.warn(warning);
    }
    let run = Run {
        step,
        from: CAMERAS_MADE,
        start: UNREAL_START_S / (CAMERAS_S * (1.0 - CAMERAS_MADE)),
    };
    report.part(
        step,
        CAMERAS_MADE,
        run.at(0.0),
        "Updating the cameras in Unreal…",
    );
    unreal_closed()?;
    let result = run_script(
        &engine,
        &side,
        "import_cameras.py",
        Editor::Headless,
        &report,
        run,
        &[("OFAST_CAMERAS", made.path.as_os_str())],
        "cameras updated ",
        |line| report.line(line),
    );
    let _ = std::fs::remove_file(&made.path);
    result?;
    Ok(Sent {
        level: format!("/Game/OFAST/Maps/{}", track.map_name),
        sequence: format!("/Game/OFAST/Replays/{prefix}"),
    })
}

/// Stops the editor a sending runs, if any (the Studio is closing).
pub fn stop_sending() {
    let pid = EDITOR_RUNNING.lock().ok().and_then(|mut p| p.take());
    if let Some(pid) = pid {
        let mut command = Command::new(system32("taskkill.exe"));
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
        no_window(&mut command);
        let _ = command.output();
    }
}

fn send(
    app: &AppHandle,
    path: &str,
    start_tick: u32,
    plan: &CameraPlan,
    look: &str,
) -> Result<Sent, String> {
    // Without the map's steps until the round says which map it needs.
    let report = Reporter::new(
        app,
        Bar(vec![
            (Step::Round, ROUND_S),
            (Step::RoundImport, ROUND_IMPORT_S),
        ]),
    );
    let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
    let side = unreal_side().ok_or("OFAST's Unreal project was not found.")?;
    unreal_closed()?;
    let install = Cs2Install::locate().ok_or("CS2 was not found.")?;
    let exports = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("exports");
    if running("cs2.exe") {
        report.warn(
            "CS2 is open: Unreal needs a lot of memory, close CS2 if the import fails.".into(),
        );
    }

    // The round's parts (`ROUND_S`): reading it, its players (agents and weapons), its cameras.
    const READ: f64 = 0.02;
    const PLAYERS: f64 = 0.97;
    report.part(Step::Round, 0.0, READ, "Reading the round…");
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = asset_name(stem(path));
    let track = track_round_at(&data, start_tick)
        .map_err(|e| format!("{path}: {e}"))?
        .without_freeze();
    let map = track.map_name.as_str();
    // The map's level in the look picked: each look is imported once per map version (`MAP_VERSION`), the realistic
    // one into a level of its own (sharing the map's meshes and materials, which its import brings up to date).
    let level_name = level_name(map, look);
    let level = format!("/Game/OFAST/Maps/{level_name}");
    let project = side.dir();
    let imported = level_imported(project, map, &level_name);
    // A level already there that needs importing was imported by an older Studio, or its import did not finish.
    let again = project.join(level_file(&level_name)).is_file();
    let map_import_s = if again {
        MAP_IMPORT_AGAIN_S
    } else {
        MAP_IMPORT_FIRST_S
    };
    if !imported {
        report.plan(Bar(vec![
            (Step::Round, ROUND_S),
            (Step::MapExport, MAP_EXPORT_S),
            (Step::MapImport, map_import_s),
            (Step::RoundImport, ROUND_IMPORT_S),
        ]));
    }
    let demos = exports.join("demos");
    let replay = write_replay(&track, &name, &demos).map_err(|e| e.to_string())?;
    report.part(
        Step::Round,
        READ,
        PLAYERS,
        format!(
            "Animating {} players with CS2's agents and weapons…",
            replay.players
        ),
    );
    let content = round_content(&install, map)?;
    export_players(&content, &track, &name, &demos).map_err(|e| e.to_string())?;
    drop(data);
    report.part(Step::Round, PLAYERS, 1.0, "Making the cameras…");
    let cameras =
        export_cameras(Some(&content), &track, plan, &name, &demos).map_err(|e| e.to_string())?;
    for warning in cameras.warnings {
        report.warn(warning);
    }

    if !imported {
        report.part(
            Step::MapExport,
            0.0,
            1.0,
            if again {
                format!("Exporting {map} from CS2's files again…")
            } else {
                format!("Exporting {map} from CS2's files (the first time only)…")
            },
        );
        let out = exports.join("maps").join(map);
        export_installed_map_reporting(&install, map, &out, ExportOptions::default(), &|done| {
            report.stage(Step::MapExport, done, done)
        })
        .map_err(|e| format!("{map} could not be exported from CS2's files: {e}"))?;
        let run = Run::whole(Step::MapImport, map_import_s);
        report.part(
            Step::MapImport,
            0.0,
            run.at(0.0),
            if again {
                format!("Importing {map} into Unreal again (a few minutes)…")
            } else {
                format!(
                    "Importing {map} into Unreal (the first time only: Unreal compiles its shaders, it can take long)…"
                )
            },
        );
        unreal_closed()?;
        // Written once the import finished: an import that did not finish leaves none, and the next sending imports
        // the map again.
        let version = project.join(IMPORTED).join(&level_name);
        let _ = std::fs::remove_file(&version);
        if let Some(dir) = version.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let gltf = out.join(format!("{map}.gltf"));
        run_script(
            &engine,
            &side,
            "import_map.py",
            Editor::Headless,
            &report,
            run,
            &[
                ("OFAST_GLTF", gltf.as_os_str()),
                ("OFAST_NAME", map.as_ref()),
                ("OFAST_STYLE", look.as_ref()),
            ],
            "imported ",
            |line| report.line(line),
        )?;
        std::fs::write(&version, MAP_VERSION).map_err(|e| e.to_string())?;
        // Imported: Unreal keeps what it needs, the export can go.
        let _ = std::fs::remove_dir_all(&out);
    }

    let run = Run::whole(Step::RoundImport, ROUND_IMPORT_S);
    report.part(
        Step::RoundImport,
        0.0,
        run.at(0.0),
        "Building the round's sequence in Unreal…",
    );
    unreal_closed()?;
    run_script(
        &engine,
        &side,
        "import_demo.py",
        Editor::Headless,
        &report,
        run,
        &[
            ("OFAST_REPLAY", replay.path.as_os_str()),
            ("OFAST_STYLE", look.as_ref()),
        ],
        "built ",
        |line| report.line(line),
    )?;
    // Imported: the round's export files (replay, agents, weapons) can go.
    let prefix = format!("{name}_r{}", track.round);
    if let Ok(files) = std::fs::read_dir(&demos) {
        for file in files.filter_map(Result::ok) {
            if file.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
    Ok(Sent {
        level,
        sequence: format!("/Game/OFAST/Replays/{prefix}"),
    })
}

/// A demo's file name as a name Unreal takes for assets: letters, digits, `_` and `-` (`(1)`, spaces and dots are
/// refused), so the replay, the agents and the sequence can all be named after it.
fn asset_name(stem: &str) -> String {
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() { "demo".into() } else { name }
}

/// An error unless no Unreal editor runs: the import needs the project to itself.
fn unreal_closed() -> Result<(), String> {
    if running("UnrealEditor.exe") || running("UnrealEditor-Cmd.exe") {
        return Err("Unreal is open (perhaps on another project, or an import still running): close it first, \
                    the import needs OFAST's project to itself."
            .into());
    }
    Ok(())
}

/// Runs one of OFAST's Unreal scripts (`tools/unreal/<script>`) in `UnrealEditor-Cmd` on the project, with
/// `variables` set (the others empty), handing each `OFAST:` log line to `line` but its progress lines
/// (`progress <start> <end>`: where its next stage starts and ends, 0 to 1 of its work), which move the bar where
/// `run` says. Done once the script logged a line starting with `last`; an error when it says `FAILED` or stops
/// before. The editor runs as `editor` says.
#[allow(clippy::too_many_arguments)] // Each is the run's own.
fn run_script(
    engine: &Path,
    side: &UnrealSide,
    script: &str,
    editor: Editor,
    report: &Reporter,
    run: Run,
    variables: &[(&str, &std::ffi::OsStr)],
    last: &str,
    mut line: impl FnMut(String),
) -> Result<(), String> {
    let mut command = Command::new(engine.join(EDITOR_CMD));
    command
        .arg(unreal_arg(&side.project))
        .arg(format!(
            "-ExecutePythonScript={}",
            unreal_arg(&side.scripts.join(script))
        ))
        .args([
            "-unattended",
            "-nosplash",
            "-stdout",
            "-FullStdOutLogOutput",
            "-NoSound",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let priority = match editor {
        Editor::Headless => {
            // Nothing is drawn (no screenshots): no renderer, so the GPU's memory cannot run out.
            command.arg("-nullrhi");
            0
        }
        Editor::Drawing(power) => power.priority_class(),
    };
    for name in SCRIPT_VARIABLES {
        command.env(name, "");
    }
    for (name, value) in variables {
        command.env(name, value);
    }
    hidden(&mut command, priority);
    let mut editor = command
        .spawn()
        .map_err(|e| format!("Unreal did not start: {e}"))?;
    if let Ok(mut running) = EDITOR_RUNNING.lock() {
        *running = Some(editor.id());
    }
    let mut failure = None;
    let mut finished = false;
    let stdout = editor.stdout.take().expect("piped above");
    let mut reader = BufReader::new(stdout);
    let mut bytes = Vec::new();
    while reader.read_until(b'\n', &mut bytes).unwrap_or(0) > 0 {
        let text = String::from_utf8_lossy(&bytes);
        if let Some((_, message)) = text.split_once("OFAST: ") {
            let message = message.trim_end().to_owned();
            if let Some([start, end]) = progress_line(&message) {
                report.stage(run.step, run.at(start), run.at(end));
                bytes.clear();
                continue;
            }
            if let Some(why) = message.strip_prefix("FAILED: ") {
                failure = Some(why.to_owned());
            }
            finished |= message.starts_with(last);
            line(message);
        }
        bytes.clear();
    }
    let status = editor.wait().map_err(|e| e.to_string())?;
    if let Ok(mut running) = EDITOR_RUNNING.lock() {
        *running = None;
    }
    let log = side.dir().join(LOG);
    match failure {
        Some(why) => Err(format!(
            "Unreal could not finish ({script}): {why}. Unreal's log: {}",
            log.display()
        )),
        None if !finished => Err(format!(
            "Unreal stopped before finishing ({script}, {status}). Unreal's log: {}",
            log.display()
        )),
        None => Ok(()),
    }
}

/// The stage an Unreal script's progress line (`progress 0.350 0.720`) says it starts: its start and end.
fn progress_line(message: &str) -> Option<[f64; 2]> {
    let (start, end) = message.strip_prefix("progress ")?.split_once(' ')?;
    Some([start.parse().ok()?, end.parse().ok()?])
}

/// Opens Unreal (with its window) on the project, at a round's level and sequence ([`Sent`]). The script runs as a
/// console command (`py import open_sequence`, `tools/unreal` on Unreal's Python path), not through
/// `-ExecutePythonScript`, whose notification stays while the editor is open and quits it without a save prompt when
/// cancelled (`tools/unreal/open_sequence.py`).
#[tauri::command]
pub async fn open_in_unreal(level: String, sequence: String) -> Result<(), String> {
    blocking(move || -> Result<(), String> {
        let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
        let side = unreal_side().ok_or("OFAST's Unreal project was not found.")?;
        if running("UnrealEditor.exe") {
            return Err(
                "Unreal is open already: open the sequence there (Content Browser, OFAST › Replays).".into(),
            );
        }
        let mut python_path = std::ffi::OsString::from(&side.scripts);
        if let Some(theirs) = std::env::var_os("UE_PYTHONPATH") {
            python_path.push(";");
            python_path.push(theirs);
        }
        let mut command = Command::new(engine.join(UNREAL_EDITOR));
        command
            .arg(unreal_arg(&side.project))
            .arg("-ExecCmds=py import open_sequence");
        for name in SCRIPT_VARIABLES {
            command.env(name, "");
        }
        command
            .env("UE_PYTHONPATH", python_path)
            .env("OFAST_LEVEL", level)
            .env("OFAST_SEQUENCE", sequence);
        command
            .spawn()
            .map(drop)
            .map_err(|e| format!("Unreal did not start: {e}"))
    })
    .await?
}

/// Whether a program of this name runs (Windows' `tasklist`).
fn running(program: &str) -> bool {
    let mut command = Command::new(system32("tasklist.exe"));
    command.args(["/FI", &format!("IMAGENAME eq {program}"), "/NH"]);
    no_window(&mut command);
    command.output().is_ok_and(|o| {
        String::from_utf8_lossy(&o.stdout)
            .to_ascii_lowercase()
            .contains(&program.to_ascii_lowercase())
    })
}

/// `path` as an argument for Unreal's command line, with forward slashes (which Unreal takes on Windows). An
/// argument with a space is quoted, and Unreal reads backslash escapes inside quotes: an installed Studio's
/// `…\OFAST Studio\tools\unreal\import_map.py` reached it as `OFAST Studio<tab>ools` (1.0.0, every default install).
fn unreal_arg(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// A program of Windows' own.
fn system32(program: &str) -> PathBuf {
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&system).join("System32").join(program)
}

/// Starts a console program without a console window (the Studio has none to share).
fn no_window(command: &mut Command) {
    hidden(command, 0);
}

/// [`no_window`], at the priority class `priority` (a Windows process creation flag, [`Power::priority_class`]; 0
/// for normal).
fn hidden(command: &mut Command, priority: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`, from Windows' process creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW | priority);
    }
    #[cfg(not(windows))]
    let _ = (command, priority);
}

/// How the editor runs a script ([`run_script`]).
#[derive(Debug, Clone, Copy)]
enum Editor {
    /// Without a renderer (`-nullrhi`): imports, which draw nothing.
    Headless,
    /// With one (Movie Render Queue renders in Play In Editor), taking as much of the PC as the [`Power`] says.
    Drawing(Power),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreal_gets_paths_without_backslashes() {
        // A default install: a space (so the argument is quoted) and `\t` (Unreal's escape for a tab).
        assert_eq!(
            unreal_arg(Path::new(
                r"C:\Users\X\AppData\Local\OFAST Studio\tools\unreal\import_map.py"
            )),
            "C:/Users/X/AppData/Local/OFAST Studio/tools/unreal/import_map.py"
        );
    }

    #[test]
    fn demo_names_become_asset_names() {
        assert_eq!(
            asset_name("faze-vs-navi-m1-mirage (1)"),
            "faze-vs-navi-m1-mirage__1_"
        );
        assert_eq!(asset_name("my clutch.v2"), "my_clutch_v2");
        assert_eq!(asset_name("match730_0038"), "match730_0038");
        assert_eq!(asset_name(""), "demo");
    }

    #[test]
    fn steps_share_the_bar_as_long_as_they_take() {
        let bar = Bar(vec![
            (Step::Round, 30.0),
            (Step::MapExport, 30.0),
            (Step::RoundImport, 60.0),
        ]);
        assert_eq!(bar.at(Step::Round, 0.0), 0.0);
        assert_eq!(bar.at(Step::MapExport, 0.5), 0.375);
        assert_eq!(bar.at(Step::RoundImport, 1.0), 1.0);
        assert_eq!(
            bar.at(Step::RoundImport, 2.0),
            1.0,
            "a step's end is its end"
        );
        assert_eq!(bar.at(Step::Render, 0.0), 1.0, "not in the plan");
        assert_eq!(bar.seconds(), 120.0);
    }

    #[test]
    fn a_script_runs_after_unreal_started() {
        let run = Run::whole(Step::RoundImport, 4.0 * UNREAL_START_S);
        assert_eq!(run.at(0.0), 0.25);
        assert_eq!(run.at(1.0), 1.0);
        // After the cameras were made (the first half), Unreal's start takes 10% of the rest.
        let run = Run {
            step: Step::Cameras,
            from: 0.5,
            start: 0.1,
        };
        assert!((run.at(0.0) - 0.55).abs() < 1e-9);
        assert!((run.at(0.5) - 0.775).abs() < 1e-9);
    }

    #[test]
    fn progress_lines_are_read() {
        assert_eq!(progress_line("progress 0.350 0.720"), Some([0.35, 0.72]));
        assert_eq!(progress_line("progress 0.35"), None);
        assert_eq!(progress_line("imported de_dust2"), None);
    }

    #[test]
    fn render_powers_are_named_as_the_script_names_them() {
        for (name, power) in Power::NAMES {
            assert_eq!(Power::parse(name), Some(power));
            assert_eq!(power.name(), name);
        }
        assert_eq!(Power::parse("max"), None);
        // Full power keeps Unreal at the normal priority; below it, under the user's programs.
        assert_eq!(Power::Full.priority_class(), 0);
        assert_ne!(Power::Low.priority_class(), 0);
    }

    #[test]
    fn levels_of_an_older_map_version_are_imported_again() {
        let project =
            std::env::temp_dir().join(format!("ofast-map-version-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        for file in [import_record("de_nuke"), level_file("de_nuke")] {
            let file = project.join(file);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "").unwrap();
        }
        // Imported by a Studio before 1.0.7: no version.
        assert!(!level_imported(&project, "de_nuke", "de_nuke"));
        std::fs::create_dir_all(project.join(IMPORTED)).unwrap();
        std::fs::write(project.join(IMPORTED).join("de_nuke"), "0").unwrap();
        assert!(!level_imported(&project, "de_nuke", "de_nuke"));
        std::fs::write(project.join(IMPORTED).join("de_nuke"), MAP_VERSION).unwrap();
        assert!(level_imported(&project, "de_nuke", "de_nuke"));
        // Each look's level goes by its own version.
        assert!(!level_imported(&project, "de_nuke", "de_nuke_realistic"));
        let _ = std::fs::remove_dir_all(&project);
    }
}
