//! The commands the UI calls (`invoke` in `apps/studio/src/api.ts`). Each returns plain data the UI shows, or an
//! error message for the user. Commands that read files run on a worker thread, so a slow folder never
//! freezes the window.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::players::{RoundTrack, track_round_at};
use ofast_core::demo::summary::{DemoSummary, summarize};
use ofast_core::export::gltf::write_png;
use ofast_core::game::{CHECKED_VERSIONS, Cs2Install, Cs2Version};
use ofast_core::map::overview::{Overview, overview_path, radar_image_path};
use ofast_core::resource::Resource;
use ofast_core::texture::Texture;
use serde::Serialize;
use tauri::Manager;
use tauri::async_runtime::Mutex as AsyncMutex;

/// Environment variable naming Unreal Engine 5.8's folder, for an install the Epic Games Launcher does not list.
const UNREAL_ENV: &str = "OFAST_UE_DIR";

/// Where Unreal Engine 5.8 installs by default (the Epic Games Launcher's folder), as `tools/unreal/run_ue.ps1`.
const UNREAL_DEFAULT: &str = r"C:\Program Files\Epic Games\UE_5.8";

/// The Epic Games Launcher's list of what it installed (JSON: `InstallationList`, each with `AppName` and
/// `InstallLocation`; the engine is `UE_5.8`).
const EPIC_INSTALLS: &str = r"C:\ProgramData\Epic\UnrealEngineLauncher\LauncherInstalled.dat";

/// The editor inside an engine folder.
pub(crate) const UNREAL_EDITOR: &str = r"Engine\Binaries\Win64\UnrealEditor.exe";

/// OFAST's Unreal project, inside the repository.
const PROJECT: &str = "unreal/Sandbox/OFASTSandbox.uproject";

/// The project file's name, wherever the project is.
const PROJECT_FILE: &str = "OFASTSandbox.uproject";

/// Next to an installed Studio's executable (the installer's resources, `tauri.conf.json`): OFAST's Unreal scripts,
/// and the project's template (its project file and settings, `Config`).
const SCRIPTS: &str = r"tools\unreal";
const TEMPLATE: &str = "project-template";

/// Environment variable naming the folder an installed Studio keeps its Unreal project in, instead of
/// `%LOCALAPPDATA%\OFAST\Unreal` (apart from the Studio's cache, `%LOCALAPPDATA%\com.ofast.studio`, which is safe to
/// delete: the project holds the maps and rounds sent).
const PROJECT_DIR_ENV: &str = "OFAST_PROJECT_DIR";

/// The file in an installed Studio's project folder that says which Studio version set it up.
const PROJECT_VERSION: &str = ".ofast-version";

/// The first bytes of a demo read for its header: the header is the file's first frame, a few hundred bytes.
const HEADER_BYTES: u64 = 64 * 1024;

/// Runs `work` on Tauri's pool of blocking threads and waits for it without holding the window's thread.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())
}

/// What the Studio needs on this computer, each `None` when not found.
#[derive(Debug, Serialize)]
pub struct Environment {
    /// The CS2 install's root folder.
    pub cs2: Option<String>,
    /// The CS2 install's version against those OFAST was checked on (`None` without an install).
    pub cs2_version: Option<VersionCheck>,
    /// CS2's replays folder, where the game saves downloaded demos.
    pub replays: Option<String>,
    /// Unreal Engine 5.8's folder.
    pub unreal: Option<String>,
    /// OFAST's Unreal project (`.uproject`), where "Send to Unreal" imports rounds.
    pub project: Option<String>,
}

/// The CS2 install's version against those OFAST was checked on ([`ofast_core::game::CHECKED_VERSIONS`]): a CS2
/// patch can change what OFAST reads without warning.
#[derive(Debug, Serialize)]
pub struct VersionCheck {
    /// The install's version (`PatchVersion`), `None` when `steam.inf` could not be read.
    pub installed: Option<String>,
    /// Whether OFAST was checked on the installed build.
    pub synced: bool,
    /// The newest version OFAST was checked on.
    pub checked: String,
}

/// Finds CS2, Unreal Engine 5.8 and OFAST's Unreal project.
#[tauri::command]
pub async fn environment() -> Result<Environment, String> {
    blocking(|| {
        let cs2 = Cs2Install::locate();
        let replays = cs2
            .as_ref()
            .map(Cs2Install::replays_dir)
            .filter(|p| p.is_dir());
        let cs2_version = cs2.as_ref().map(|c| {
            let version = c.version();
            VersionCheck {
                synced: version.as_ref().is_some_and(Cs2Version::is_checked),
                installed: version.map(|v| v.patch),
                checked: CHECKED_VERSIONS
                    .first()
                    .map_or_else(String::new, |&(patch, _)| patch.to_owned()),
            }
        });
        Environment {
            cs2: cs2.map(|c| display(c.root())),
            cs2_version,
            replays: replays.as_deref().map(display),
            unreal: unreal_dir().as_deref().map(display),
            project: unreal_side()
                .map(|side| side.project)
                .filter(|p| p.is_file())
                .as_deref()
                .map(display),
        }
    })
    .await
}

/// Unreal Engine 5.8's folder: [`UNREAL_ENV`] if set (and right), else where the Epic Games Launcher installed it,
/// else the default folder.
pub(crate) fn unreal_dir() -> Option<PathBuf> {
    let has_editor = |dir: &PathBuf| dir.join(UNREAL_EDITOR).is_file();
    if let Some(dir) = std::env::var_os(UNREAL_ENV) {
        // An explicit choice that is wrong should not silently fall back to another install.
        return Some(PathBuf::from(dir)).filter(has_editor);
    }
    let listed = std::fs::read_to_string(EPIC_INSTALLS)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|installs| {
            installs["InstallationList"]
                .as_array()?
                .iter()
                .find(|i| i["AppName"] == "UE_5.8")?["InstallLocation"]
                .as_str()
                .map(PathBuf::from)
        });
    listed
        .into_iter()
        .chain([PathBuf::from(UNREAL_DEFAULT)])
        .find(has_editor)
}

/// OFAST's side in Unreal: the project the rounds go into and the scripts that build them there.
#[derive(Debug, Clone)]
pub(crate) struct UnrealSide {
    /// The project file (`OFASTSandbox.uproject`).
    pub(crate) project: PathBuf,
    /// OFAST's Unreal scripts (`tools/unreal`).
    pub(crate) scripts: PathBuf,
}

impl UnrealSide {
    /// The project's folder, where Unreal keeps what it makes (`Content`, `Saved`).
    pub(crate) fn dir(&self) -> &Path {
        self.project.parent().unwrap_or(&self.project)
    }
}

/// Where OFAST's side in Unreal is:
/// - the Studio runs from OFAST's source tree (`npm run app`): the nearest folder above the executable that holds
///   `unreal/Sandbox/OFASTSandbox.uproject`, with `tools/unreal` beside it;
/// - an installed Studio: the scripts next to the executable, the project in the user's own writable folder
///   ([`PROJECT_DIR_ENV`], else `%LOCALAPPDATA%\OFAST\Unreal`), set up from the template beside the scripts
///   on first use and again when the Studio's version changes ([`set_up_project`]).
///
/// `None` when neither is there.
pub(crate) fn unreal_side() -> Option<UnrealSide> {
    let exe = std::env::current_exe().ok()?;
    if let Some(repo) = exe.ancestors().find(|dir| dir.join(PROJECT).is_file()) {
        return Some(UnrealSide {
            project: repo.join(PROJECT),
            scripts: repo.join(SCRIPTS),
        });
    }
    let installed = exe.parent()?;
    let scripts = installed.join(SCRIPTS);
    let template = installed.join(TEMPLATE);
    if !scripts.join("import_demo.py").is_file() || !template.join(PROJECT_FILE).is_file() {
        return None;
    }
    let dir = std::env::var_os(PROJECT_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("OFAST").join("Unreal"))
        })?;
    set_up_project(&template, &dir).ok()?;
    Some(UnrealSide {
        project: dir.join(PROJECT_FILE),
        scripts,
    })
}

/// Copies the template's project file and settings into `dir` when it has no project yet or an older Studio set it
/// up ([`PROJECT_VERSION`]); what Unreal made there (`Content`, `Saved`: the maps and rounds sent) stays.
fn set_up_project(template: &Path, dir: &Path) -> std::io::Result<()> {
    let marker = dir.join(PROJECT_VERSION);
    let version = env!("CARGO_PKG_VERSION");
    let current = std::fs::read_to_string(&marker).is_ok_and(|v| v.trim() == version);
    if current && dir.join(PROJECT_FILE).is_file() {
        return Ok(());
    }
    copy_dir(template, dir)?;
    std::fs::write(marker, version)
}

/// Copies the files of `from` into `to`, folders and all, over what is there.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// A path for the UI, without the `\\?\` prefix Windows gives canonical paths (`\\?\UNC\server\share` becomes
/// `\\server\share`).
fn display(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

/// One demo file, from its header.
#[derive(Debug, Serialize)]
pub struct DemoEntry {
    /// The file's path.
    pub path: String,
    /// The file's name without `.dem`.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
    /// When it was last written, seconds since 1970 (0 if unknown).
    pub modified: u64,
    /// The map, from the header (empty if the header does not read).
    pub map: String,
    /// The server's name, from the header (a tournament's name on its demos).
    pub server: String,
}

/// The demos (`.dem` files) in `folder`, newest first, each with its header's map and server. A file whose name is
/// not valid Unicode is left out: the UI could not hand its path back.
#[tauri::command]
pub async fn list_demos(folder: String) -> Result<Vec<DemoEntry>, String> {
    blocking(move || {
        let entries = std::fs::read_dir(&folder).map_err(|e| format!("{folder}: {e}"))?;
        let mut demos: Vec<DemoEntry> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dem")))
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
                let (map, server) = header(&path).unwrap_or_default();
                Some(DemoEntry {
                    name: path.file_stem()?.to_str()?.to_owned(),
                    path: path.to_str()?.to_owned(),
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs()),
                    map,
                    server,
                })
            })
            .collect();
        demos.sort_by_key(|d| std::cmp::Reverse(d.modified));
        Ok(demos)
    })
    .await?
}

/// A demo's map and server name, from its first [`HEADER_BYTES`] bytes.
fn header(path: &Path) -> Option<(String, String)> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEADER_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    let parser = DemoParser::new(&bytes).ok()?;
    let header = parser.header();
    Some((
        header.map_name().to_owned(),
        header.server_name().to_owned(),
    ))
}

/// A demo file's size and modification time (seconds since 1970): a summary holds while they are unchanged.
type Stamp = (u64, u64);

/// The stamp of the file at `path`.
fn stamp(path: &str) -> Result<Stamp, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), modified))
}

/// A demo's summary once read, with the file's stamp when it was.
type Saved = Option<(Stamp, Arc<DemoSummary>)>;

/// What [`summarize_demo`] read this session, by demo path. Each demo has its own lock, held while it is read: a
/// second opening (React runs effects twice in development; the user goes back and opens it again) waits for the
/// first instead of reading the file once more.
#[derive(Debug, Default)]
pub struct Summaries(Mutex<HashMap<String, Arc<AsyncMutex<Saved>>>>);

/// Bumped whenever `ofast_core::demo::summary` changes what it reads: summaries saved by an older Studio are read
/// again. (A saved summary that no longer fits the types is read again as well.)
const SUMMARY_FORMAT: u32 = 2;

/// Sums a demo up ([`ofast_core::demo::summary::summarize`]): rounds, scores, kill feed and highlights. Reading a
/// whole match takes seconds, so each summary is kept, in memory and in the app's cache folder, until the file
/// changes.
#[tauri::command]
pub async fn summarize_demo(
    path: String,
    app: tauri::AppHandle,
    summaries: tauri::State<'_, Summaries>,
) -> Result<Arc<DemoSummary>, String> {
    let stamp = stamp(&path)?;
    let slot = summaries
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .entry(path.clone())
        .or_default()
        .clone();
    let mut saved = slot.lock().await;
    if let Some((at, summary)) = saved.as_ref()
        && *at == stamp
    {
        return Ok(summary.clone());
    }
    let cache = app
        .path()
        .app_cache_dir()
        .ok()
        .map(|dir| dir.join("summaries").join(cache_name(&path, stamp)));
    let summary = blocking(move || -> Result<DemoSummary, String> {
        if let Some(cached) = cache
            .as_deref()
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            return Ok(cached);
        }
        let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let summary = summarize(&data).map_err(|e| format!("{path}: {e}"))?;
        // A cache that cannot be written only costs the next opening its seconds.
        if let Some(file) = cache
            && let Some(dir) = file.parent()
            && std::fs::create_dir_all(dir).is_ok()
            && let Ok(json) = serde_json::to_vec(&summary)
        {
            let _ = std::fs::write(&file, json);
        }
        Ok(summary)
    })
    .await??;
    let summary = Arc::new(summary);
    *saved = Some((stamp, summary.clone()));
    Ok(summary)
}

/// The cache file of a demo's summary: a hash of its path, size, time and [`SUMMARY_FORMAT`] (Rust's default hasher:
/// a Studio built with another Rust version may name the same summary differently, and reads the demo again).
fn cache_name(path: &str, (size, modified): Stamp) -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    (path, size, modified, SUMMARY_FORMAT).hash(&mut hasher);
    format!("{:016x}.json", hasher.finish())
}

/// Ticks between two frames of the 2D replay (16 a second at 64 ticks; the UI moves players smoothly between them).
const RADAR_STEP: u32 = 4;

/// Replays kept in memory: a round's is a megabyte or two, and reading one again takes seconds.
const RADARS_KEPT: usize = 8;

/// The rounds [`radar_round`] read this session, least recently asked first, each with its own lock held while it
/// is read (as [`Summaries`]); and the reading itself, one at a time: a demo read whole, its round tracked, takes
/// seconds and a lot of memory, and only the round asked last is still on screen.
#[derive(Debug, Default)]
pub struct Radars {
    kept: Mutex<Vec<RadarSlot>>,
    /// The number of the request asked last: a request still waiting to read when a newer one came is dropped.
    latest: AtomicU64,
    reading: AsyncMutex<()>,
}

/// One round of [`Radars`]: its demo and the file's stamp, its start tick, and the replay once read.
type RadarSlot = (String, Stamp, u32, Arc<AsyncMutex<Option<Arc<RadarRound>>>>);

/// A round for the 2D replay.
#[derive(Debug, Serialize)]
pub struct RadarRound {
    /// The map (its radar files are named after it).
    pub map: String,
    /// Where the radar image sits in the world; `None` when CS2 has no overview for the map.
    pub overview: Option<Overview>,
    /// The round's players; a frame's players point into it.
    pub players: Vec<RadarPlayerInfo>,
    /// The round every 4 ticks (16 frames a second), and its last tick.
    pub frames: Vec<RadarFrame>,
}

/// One player of a [`RadarRound`].
#[derive(Debug, Serialize)]
pub struct RadarPlayerInfo {
    /// Their name.
    pub name: String,
    /// Their Steam id as text, `"0"` for a bot.
    pub steam_id: String,
}

/// The round at one tick.
#[derive(Debug, Serialize)]
pub struct RadarFrame {
    /// Demo tick.
    pub tick: u32,
    /// The players with a pawn.
    pub players: Vec<RadarPlayer>,
    /// Grenades in flight: x, y, z (world inches).
    pub grenades: Vec<[f32; 3]>,
    /// Smoke clouds: x, y.
    pub smokes: Vec<[f32; 2]>,
    /// Burning flames: x, y.
    pub fires: Vec<[f32; 2]>,
}

/// A player at one tick.
#[derive(Debug, Serialize)]
pub struct RadarPlayer {
    /// Index into [`RadarRound::players`].
    pub slot: usize,
    /// 2 = T, 3 = CT.
    pub team: u32,
    /// Feet, world inches.
    pub x: f32,
    /// See [`x`](Self::x).
    pub y: f32,
    /// See [`x`](Self::x).
    pub z: f32,
    /// Where they look, degrees (0 = +x, counter-clockwise).
    pub yaw: f32,
    /// Alive.
    pub alive: bool,
}

/// The round of `path` that starts at `start_tick`, for the 2D replay (the demo is read up to the round's end).
#[tauri::command]
pub async fn radar_round(
    path: String,
    start_tick: u32,
    radars: tauri::State<'_, Radars>,
) -> Result<Arc<RadarRound>, String> {
    let request = radars.latest.fetch_add(1, Ordering::SeqCst) + 1;
    let stamp = stamp(&path)?;
    let slot = {
        let mut kept = radars.kept.lock().map_err(|e| e.to_string())?;
        let slot = match kept
            .iter()
            .position(|(p, s, t, _)| *p == path && *s == stamp && *t == start_tick)
        {
            Some(i) => kept.remove(i).3,
            None => Arc::new(AsyncMutex::new(None)),
        };
        // Most recently asked last; the least recent goes when there are too many.
        kept.push((path.clone(), stamp, start_tick, slot.clone()));
        if kept.len() > RADARS_KEPT {
            kept.remove(0);
        }
        slot
    };
    let mut saved = slot.lock().await;
    if let Some(radar) = saved.as_ref() {
        return Ok(radar.clone());
    }
    let _reading = radars.reading.lock().await;
    if radars.latest.load(Ordering::SeqCst) != request {
        return Err("another round was asked for since".into());
    }
    let radar = blocking(move || -> Result<RadarRound, String> {
        let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let track = track_round_at(&data, start_tick).map_err(|e| format!("{path}: {e}"))?;
        Ok(radar_of(&track))
    })
    .await??;
    let radar = Arc::new(radar);
    *saved = Some(radar.clone());
    Ok(radar)
}

/// The events that end a grenade while its entity stays (tournament demos keep a popped HE or flashbang for
/// seconds): a detonation, a smoke's expiry, a fire put out — as the 3D replay ends them.
const GRENADE_ENDS: [&str; 5] = [
    "hegrenade_detonate",
    "flashbang_detonate",
    "decoy_detonate",
    "smokegrenade_expired",
    "inferno_expire",
];

/// A tracked round as the replay draws it.
fn radar_of(track: &RoundTrack) -> RadarRound {
    let overview = Cs2Install::locate()
        .and_then(|install| Content::game(&install).ok())
        .and_then(|content| content.read(&overview_path(&track.map_name)).ok())
        .and_then(|text| Overview::parse(&String::from_utf8_lossy(&text)));
    // The ticks each grenade entity ended at; an entity index can serve a later grenade too.
    let mut ends: HashMap<u32, Vec<u32>> = HashMap::new();
    for event in &track.events {
        if GRENADE_ENDS.contains(&event.name.as_str())
            && let Some(entity) = event.int("entityid").and_then(|e| u32::try_from(e).ok())
        {
            ends.entry(entity).or_default().push(event.tick);
        }
    }
    // When each grenade (entity, serial) was first seen.
    let mut first_seen: HashMap<(u32, u32), u32> = HashMap::new();
    let mut ended = |entity: u32, serial: u32, tick: u32| {
        let first = *first_seen.entry((entity, serial)).or_insert(tick);
        ends.get(&entity)
            .is_some_and(|ticks| ticks.iter().any(|&end| (first..=tick).contains(&end)))
    };
    let mut players: Vec<RadarPlayerInfo> = Vec::new();
    let mut slots: HashMap<u32, usize> = HashMap::new();
    let mut frames = Vec::new();
    let mut next = track.start_tick;
    for (i, sample) in track.samples.iter().enumerate() {
        // A tick every RADAR_STEP (the next sampled one when a tick had no update), and the round's last.
        if sample.tick < next && i + 1 < track.samples.len() {
            continue;
        }
        next = sample.tick + RADAR_STEP;
        let mut frame = RadarFrame {
            tick: sample.tick,
            players: Vec::with_capacity(sample.players.len()),
            grenades: Vec::new(),
            smokes: Vec::new(),
            fires: Vec::new(),
        };
        for p in &sample.players {
            let slot = *slots.entry(p.controller).or_insert_with(|| {
                players.push(RadarPlayerInfo {
                    name: p.name.clone(),
                    steam_id: p.steam_id.to_string(),
                });
                players.len() - 1
            });
            let [x, y, z] = p.position;
            frame.players.push(RadarPlayer {
                slot,
                team: p.team,
                x,
                y,
                z,
                yaw: p.yaw,
                alive: p.alive,
            });
        }
        for g in &sample.grenades {
            if ended(g.entity, g.serial, sample.tick) {
                continue;
            }
            match g.smoke {
                Some(cloud) => frame.smokes.push([cloud.position[0], cloud.position[1]]),
                None => frame.grenades.push(g.position),
            }
        }
        for f in &sample.fires {
            if ended(f.entity, f.serial, sample.tick) {
                continue;
            }
            let burning = f.flames.iter().filter(|flame| flame.burning);
            frame
                .fires
                .extend(burning.map(|flame| [flame.position[0], flame.position[1]]));
        }
        frames.push(frame);
    }
    RadarRound {
        map: track.map_name.clone(),
        overview,
        players,
        frames,
    }
}

/// A map's radar image (one level of it: `default`, or `lower` on a map on two floors) as PNG bytes, decoded from
/// CS2's files and kept in the app's cache folder, under a name that changes when CS2's archive does (a map update
/// can redraw its radar).
#[tauri::command]
pub async fn radar_image(
    map: String,
    section: String,
    app: tauri::AppHandle,
) -> Result<tauri::ipc::Response, String> {
    // Map and section names come from the demo and CS2's overview: keep them to a file name's characters.
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if !plain(&map) || !plain(&section) {
        return Err(format!("no radar for {map} ({section})"));
    }
    let folder = app
        .path()
        .app_cache_dir()
        .map(|dir| dir.join("radar"))
        .unwrap_or_else(|_| std::env::temp_dir().join("ofast-studio-radar"));
    let bytes = blocking(move || -> Result<Vec<u8>, String> {
        let install = Cs2Install::locate().ok_or("CS2 was not found")?;
        let version = std::fs::metadata(install.main_vpk())
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let cached = folder.join(format!("{map}_{section}_{version}.png"));
        if let Ok(bytes) = std::fs::read(&cached) {
            return Ok(bytes);
        }
        let content = Content::game(&install).map_err(|e| e.to_string())?;
        let file = radar_image_path(&map, Some(&section));
        let data = content.read(&file).map_err(|e| format!("{file}: {e}"))?;
        let resource = Resource::parse(&data).map_err(|e| format!("{file}: {e}"))?;
        let texture = Texture::parse(&resource).map_err(|e| format!("{file}: {e}"))?;
        let image = texture
            .decode_rgba8(&data)
            .map_err(|e| format!("{file}: {e}"))?;
        // Written whole under a name of its own, then renamed: a cache file is never half written. The rename
        // failing (another reading got there first) is fine.
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let partial = folder.join(format!("{map}_{section}.{}.part", std::process::id()));
        write_png(&partial, &image).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&partial).map_err(|e| e.to_string())?;
        if std::fs::rename(&partial, &cached).is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        Ok(bytes)
    })
    .await??;
    Ok(tauri::ipc::Response::new(bytes))
}
