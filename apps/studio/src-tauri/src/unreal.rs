//! "Send to Unreal" and "Open in Unreal": a round of a demo into OFAST's Unreal project (`unreal/Sandbox`), the same
//! steps as the command line (`docs/development.md`, "A demo round in Unreal"):
//!
//! 1. export the round with its players and its cameras (`ofast export-demo --players`, with the shots the user
//!    picked: [`CameraPlan`]),
//! 2. the first time a map is used, export it (`ofast export-map`) and import it (`tools/unreal/import_map.py`),
//! 3. import the round (`tools/unreal/import_demo.py`): a Level Sequence in the map's level.
//!
//! "Update cameras" changes the cameras of a round already sent, without importing it again: its cameras exported
//! (`export::cameras`), then `tools/unreal/import_cameras.py` (decision 021). "Render" renders a round already sent
//! with Movie Render Queue (`tools/unreal/render_round.py`, decision 023) into the user's Videos folder.
//!
//! The Unreal steps run `UnrealEditor-Cmd` without a window; its `OFAST:` log lines come to the UI as progress
//! ([`PROGRESS_EVENT`]), and a step counts as done only once its script logged its last line. Imports draw nothing,
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
use ofast_core::export::map::export_installed_map;
use ofast_core::export::players::export_players;
use ofast_core::export::replay::write_replay;
use ofast_core::game::Cs2Install;
use serde::Serialize;
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{PROJECT, UNREAL_EDITOR, blocking, repository, unreal_dir};

/// The event the UI listens to for a sending's progress ([`Progress`]).
pub const PROGRESS_EVENT: &str = "unreal-progress";

/// The editor without a window, for imports.
const EDITOR_CMD: &str = r"Engine\Binaries\Win64\UnrealEditor-Cmd.exe";

/// Unreal's log, for the user when an import fails.
const LOG: &str = r"unreal\Sandbox\Saved\Logs\OFASTSandbox.log";

/// The environment variables OFAST's Unreal scripts read: all are set for each run (empty when unused), so none
/// leaks from the Studio's own environment.
const SCRIPT_VARIABLES: [&str; 17] = [
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
];

/// Where a sending is.
#[derive(Debug, Clone, Copy, Serialize)]
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
}

/// What a sending made: where to look in Unreal.
#[derive(Debug, Clone, Serialize)]
pub struct Sent {
    /// The map's level, `/Game/OFAST/Maps/<map>`.
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
    app: AppHandle,
    sending: tauri::State<'_, Sending>,
) -> Result<Sent, String> {
    let _only = sending.0.try_lock().map_err(|_| BUSY.to_owned())?;
    blocking(move || send(&app, &path, start_tick, &plan)).await?
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
/// round without them), into a folder of its own in `Videos\OFAST`. Returns that folder. About two seconds a frame
/// for `final` on a recent graphics card, after a minute or two of Unreal starting.
#[tauri::command]
pub async fn render_round(
    path: String,
    round: u32,
    preset: String,
    ticks: Option<[u32; 2]>,
    app: AppHandle,
    sending: tauri::State<'_, Sending>,
) -> Result<String, String> {
    let _only = sending.0.try_lock().map_err(|_| BUSY.to_owned())?;
    blocking(move || render(&app, &path, round, &preset, ticks)).await?
}

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

fn render(
    app: &AppHandle,
    path: &str,
    round: u32,
    preset: &str,
    ticks: Option<[u32; 2]>,
) -> Result<String, String> {
    let report = |message: String, warning: bool| emit(app, Step::Render, message, warning);
    if !PRESETS.contains(&preset) {
        return Err(format!("There is no render preset \"{preset}\"."));
    }
    let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
    let repo = repository().ok_or("OFAST's Unreal project was not found.")?;
    unreal_closed()?;
    let prefix = format!("{}_r{round}", asset_name(stem(path)));
    if !repo
        .join(REPLAYS)
        .join(format!("{prefix}.uasset"))
        .is_file()
    {
        return Err("This round is not in Unreal yet: send it first.".into());
    }
    if running("cs2.exe") {
        report(
            "CS2 is open: rendering needs the graphics card's memory, close CS2 if Unreal fails."
                .into(),
            true,
        );
    }
    let out = renders_dir(app)?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let sequence = format!("/Game/OFAST/Replays/{prefix}");
    let range = ticks
        .map(|[start, end]| format!("{start},{end}"))
        .unwrap_or_default();
    report(
        "Starting Unreal (it loads the map first: a minute or two)…".into(),
        false,
    );
    let mut folder = None;
    run_script(
        &engine,
        &repo,
        "render_round.py",
        // Movie Render Queue renders in Play In Editor, which needs a renderer.
        true,
        &[
            ("OFAST_SEQUENCE", sequence.as_ref()),
            ("OFAST_PRESET", preset.as_ref()),
            ("OFAST_TICKS", range.as_ref()),
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
            report(line, false);
        },
    )?;
    folder.ok_or_else(|| "Unreal did not say where it put the frames.".into())
}

/// The rounds of the demo at `path` whose sequence is in OFAST's Unreal project (round numbers): sent before, their
/// cameras can change.
#[tauri::command]
pub async fn sent_rounds(path: String) -> Result<Vec<u32>, String> {
    blocking(move || {
        let Some(repo) = repository() else {
            return Vec::new();
        };
        let prefix = format!("{}_r", asset_name(stem(&path)));
        let mut rounds: Vec<u32> = std::fs::read_dir(repo.join(REPLAYS))
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

/// Where the rounds' sequences are, in the repository.
const REPLAYS: &str = r"unreal\Sandbox\Content\OFAST\Replays";

/// A demo path's file name without its extension.
fn stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("demo")
}

/// Tells the UI where a sending is (nobody may hear it: the UI may have gone to another page).
fn emit(app: &AppHandle, step: Step, message: String, warning: bool) {
    let _ = app.emit(
        PROGRESS_EVENT,
        Progress {
            step,
            message,
            warning,
        },
    );
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
    let report = |message: String, warning: bool| emit(app, Step::Cameras, message, warning);
    let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
    let repo = repository().ok_or("OFAST's Unreal project was not found.")?;
    unreal_closed()?;
    let install = Cs2Install::locate().ok_or("CS2 was not found.")?;
    report("Reading the round…".into(), false);
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = asset_name(stem(path));
    let track = track_round_at(&data, start_tick).map_err(|e| format!("{path}: {e}"))?;
    drop(data);
    let prefix = format!("{name}_r{}", track.round);
    if !repo
        .join(REPLAYS)
        .join(format!("{prefix}.uasset"))
        .is_file()
    {
        return Err("This round is not in Unreal yet: send it first.".into());
    }
    report("Making the cameras…".into(), false);
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
        report(warning, true);
    }
    report("Updating the cameras in Unreal…".into(), false);
    unreal_closed()?;
    let result = run_script(
        &engine,
        &repo,
        "import_cameras.py",
        false,
        &[("OFAST_CAMERAS", made.path.as_os_str())],
        "cameras updated ",
        |line| report(line, false),
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

fn send(app: &AppHandle, path: &str, start_tick: u32, plan: &CameraPlan) -> Result<Sent, String> {
    let report = |step: Step, message: String| emit(app, step, message, false);
    let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
    let repo = repository().ok_or("OFAST's Unreal project was not found.")?;
    unreal_closed()?;
    let install = Cs2Install::locate().ok_or("CS2 was not found.")?;
    let exports = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("exports");
    if running("cs2.exe") {
        emit(
            app,
            Step::Round,
            "CS2 is open: Unreal needs a lot of memory, close CS2 if the import fails.".into(),
            true,
        );
    }

    report(Step::Round, "Reading the round…".into());
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = asset_name(stem(path));
    let track = track_round_at(&data, start_tick).map_err(|e| format!("{path}: {e}"))?;
    let demos = exports.join("demos");
    let replay = write_replay(&track, &name, &demos).map_err(|e| e.to_string())?;
    report(
        Step::Round,
        format!(
            "Animating {} players with CS2's agents and weapons…",
            replay.players
        ),
    );
    let content = round_content(&install, &track.map_name)?;
    export_players(&content, &track, &name, &demos).map_err(|e| e.to_string())?;
    drop(data);
    report(Step::Round, "Making the cameras…".into());
    let cameras =
        export_cameras(Some(&content), &track, plan, &name, &demos).map_err(|e| e.to_string())?;
    for warning in cameras.warnings {
        emit(app, Step::Round, warning, true);
    }

    let map = track.map_name.as_str();
    let level = format!("/Game/OFAST/Maps/{map}");
    // A map import that did not finish keeps its marker: the next sending imports the map again.
    let importing = repo.join(r"unreal\Sandbox\Saved\OFAST\importing").join(map);
    let imported = !importing.exists()
        && repo
            .join(r"unreal\Sandbox\Content\OFAST\Maps")
            .join(format!("{map}.umap"))
            .is_file()
        && repo
            .join(r"unreal\Sandbox\Content\OFAST")
            .join(map)
            .join(map)
            .join(format!("SceneImport_{map}.uasset"))
            .is_file();
    if !imported {
        report(
            Step::MapExport,
            format!("Exporting {map} from CS2's files (the first time only: a few minutes)…"),
        );
        let out = exports.join("maps").join(map);
        export_installed_map(&install, map, &out, ExportOptions::default())
            .map_err(|e| format!("{map} could not be exported from CS2's files: {e}"))?;
        report(
            Step::MapImport,
            format!(
                "Importing {map} into Unreal (the first time only: Unreal compiles its shaders, it can take long)…"
            ),
        );
        unreal_closed()?;
        if let Some(dir) = importing.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&importing, "").map_err(|e| e.to_string())?;
        let gltf = out.join(format!("{map}.gltf"));
        run_script(
            &engine,
            &repo,
            "import_map.py",
            false,
            &[
                ("OFAST_GLTF", gltf.as_os_str()),
                ("OFAST_NAME", map.as_ref()),
            ],
            "imported ",
            |line| report(Step::MapImport, line),
        )?;
        let _ = std::fs::remove_file(&importing);
        // Imported: Unreal keeps what it needs, the export can go.
        let _ = std::fs::remove_dir_all(&out);
    }

    report(
        Step::RoundImport,
        "Building the round's sequence in Unreal…".into(),
    );
    unreal_closed()?;
    run_script(
        &engine,
        &repo,
        "import_demo.py",
        false,
        &[("OFAST_REPLAY", replay.path.as_os_str())],
        "built ",
        |line| report(Step::RoundImport, line),
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
/// `variables` set (the others empty), handing each `OFAST:` log line to `line`. Done once the script logged a line
/// starting with `last`; an error when it says `FAILED` or stops before. Without `draws`, the editor has no renderer.
fn run_script(
    engine: &Path,
    repo: &Path,
    script: &str,
    draws: bool,
    variables: &[(&str, &std::ffi::OsStr)],
    last: &str,
    mut line: impl FnMut(String),
) -> Result<(), String> {
    let mut command = Command::new(engine.join(EDITOR_CMD));
    command
        .arg(repo.join(PROJECT))
        .arg(format!(
            "-ExecutePythonScript={}",
            repo.join(r"tools\unreal").join(script).display()
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
    if !draws {
        // Nothing is drawn (no screenshots): no renderer, so the GPU's memory cannot run out.
        command.arg("-nullrhi");
    }
    for name in SCRIPT_VARIABLES {
        command.env(name, "");
    }
    for (name, value) in variables {
        command.env(name, value);
    }
    no_window(&mut command);
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
    let log = repo.join(LOG);
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

/// Opens Unreal (with its window) on the project, at a round's level and sequence ([`Sent`]). The script runs as a
/// console command (`py import open_sequence`, `tools/unreal` on Unreal's Python path), not through
/// `-ExecutePythonScript`, whose notification stays while the editor is open and quits it without a save prompt when
/// cancelled (`tools/unreal/open_sequence.py`).
#[tauri::command]
pub async fn open_in_unreal(level: String, sequence: String) -> Result<(), String> {
    blocking(move || -> Result<(), String> {
        let engine = unreal_dir().ok_or("Unreal Engine 5.8 was not found.")?;
        let repo = repository().ok_or("OFAST's Unreal project was not found.")?;
        if running("UnrealEditor.exe") {
            return Err(
                "Unreal is open already: open the sequence there (Content Browser, OFAST › Replays).".into(),
            );
        }
        let mut python_path = std::ffi::OsString::from(repo.join(r"tools\unreal"));
        if let Some(theirs) = std::env::var_os("UE_PYTHONPATH") {
            python_path.push(";");
            python_path.push(theirs);
        }
        let mut command = Command::new(engine.join(UNREAL_EDITOR));
        command
            .arg(repo.join(PROJECT))
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

/// A program of Windows' own.
fn system32(program: &str) -> PathBuf {
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&system).join("System32").join(program)
}

/// Starts a console program without a console window (the Studio has none to share).
fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`, from Windows' process creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
