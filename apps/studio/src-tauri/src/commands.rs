//! The commands the UI calls (`invoke` in `apps/studio/src/api.ts`). Each returns plain data the UI shows, or an
//! error message for the user. Commands that read files run on a worker thread, so a slow folder never
//! freezes the window.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ofast_core::demo::DemoParser;
use ofast_core::game::Cs2Install;
use serde::Serialize;

/// Environment variable naming Unreal Engine 5.8's folder, for an install the Epic Games Launcher does not list.
const UNREAL_ENV: &str = "OFAST_UE_DIR";

/// Where Unreal Engine 5.8 installs by default (the Epic Games Launcher's folder), as `tools/unreal/run_ue.ps1`.
const UNREAL_DEFAULT: &str = r"C:\Program Files\Epic Games\UE_5.8";

/// The Epic Games Launcher's list of what it installed (JSON: `InstallationList`, each with `AppName` and
/// `InstallLocation`; the engine is `UE_5.8`).
const EPIC_INSTALLS: &str = r"C:\ProgramData\Epic\UnrealEngineLauncher\LauncherInstalled.dat";

/// The editor inside an engine folder.
const UNREAL_EDITOR: &str = r"Engine\Binaries\Win64\UnrealEditor.exe";

/// OFAST's Unreal project, inside the repository.
const PROJECT: &str = "unreal/Sandbox/OFASTSandbox.uproject";

/// The first bytes of a demo read for its header: the header is the file's first frame, a few hundred bytes.
const HEADER_BYTES: u64 = 64 * 1024;

/// Runs `work` on Tauri's pool of blocking threads and waits for it without holding the window's thread.
async fn blocking<T: Send + 'static>(
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
    /// CS2's replays folder, where the game saves downloaded demos.
    pub replays: Option<String>,
    /// Unreal Engine 5.8's folder.
    pub unreal: Option<String>,
    /// OFAST's Unreal project (`.uproject`), where "Send to Unreal" imports rounds.
    pub project: Option<String>,
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
        Environment {
            cs2: cs2.map(|c| display(c.root())),
            replays: replays.as_deref().map(display),
            unreal: unreal_dir().as_deref().map(display),
            project: repository()
                .map(|r| r.join(PROJECT))
                .filter(|p| p.is_file())
                .as_deref()
                .map(display),
        }
    })
    .await
}

/// Unreal Engine 5.8's folder: [`UNREAL_ENV`] if set (and right), else where the Epic Games Launcher installed it,
/// else the default folder.
fn unreal_dir() -> Option<PathBuf> {
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

/// The OFAST repository the Studio runs from (the Unreal project and its scripts live there): the nearest folder
/// above the executable that holds the project, else the source tree it was built from (a build copied elsewhere).
fn repository() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    if let Some(dir) = exe
        .as_deref()
        .and_then(|exe| exe.ancestors().find(|dir| dir.join(PROJECT).is_file()))
    {
        return Some(dir.to_path_buf());
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    source
        .join(PROJECT)
        .is_file()
        .then(|| source.canonicalize().ok())
        .flatten()
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
