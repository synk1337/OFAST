//! Locating a Counter-Strike 2 installation on disk.
//!
//! OFAST never ships Valve content: every asset is read from the user's own CS2 install. This module finds that
//! install so the rest of the crate (and the `game-data` tests) can open `game/csgo/pak01_dir.vpk`.
//!
//! Lookup order:
//! 1. The `OFAST_CS2_DIR` environment variable, if set. It must point at the CS2 root folder (the one containing
//!    `game/`).
//! 2. Each Steam library listed in Steam's `steamapps/libraryfolders.vdf` (KeyValues 1, [`crate::kv1`]): CS2 is
//!    `steamapps/common/Counter-Strike Global Offensive` in one of them (it kept the CS:GO folder name). Steam's own
//!    folder is the one the registry names (`HKCU\Software\Valve\Steam`, `SteamPath`), else
//!    `C:\Program Files (x86)\Steam`.
//! 3. Steam's default library on Windows: `C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global
//!    Offensive`.
//!
//! It also reads the install's version (`game/csgo/steam.inf`) and tells whether OFAST was checked on that build
//! ([`CHECKED_VERSIONS`]): the Studio warns when it was not.

use std::path::{Path, PathBuf};

/// Environment variable that overrides CS2 install detection.
pub const CS2_DIR_ENV: &str = "OFAST_CS2_DIR";

/// Default CS2 location in Steam's main library on Windows.
const DEFAULT_WINDOWS_PATH: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive";

/// Steam's default folder on Windows.
const DEFAULT_STEAM_DIR: &str = r"C:\Program Files (x86)\Steam";

/// CS2's folder inside a Steam library.
const CS2_IN_LIBRARY: &str = r"steamapps\common\Counter-Strike Global Offensive";

/// Steam's own folder: the registry's `SteamPath`, else the default. Asked once per run.
fn steam_dir() -> PathBuf {
    static STEAM_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    STEAM_DIR
        .get_or_init(|| {
            steam_dir_from_registry().unwrap_or_else(|| PathBuf::from(DEFAULT_STEAM_DIR))
        })
        .clone()
}

/// The registry's `HKCU\Software\Valve\Steam` → `SteamPath`, asked through Windows' own `reg` tool (so no registry
/// crate is needed), without a console window: an app without one (the Studio) would flash one. `reg` writes in the
/// console's code page: a path outside ASCII may not read back, and the default folder is tried instead.
#[cfg(windows)]
fn steam_dir_from_registry() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`, from Windows' process creation flags.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let output = std::process::Command::new(Path::new(&system).join(r"System32\reg.exe"))
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    // A line like `    SteamPath    REG_SZ    c:/program files (x86)/steam`.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let (_, value) = line.split_once("REG_SZ")?;
            Some(PathBuf::from(value.trim()))
        })
}

/// No registry outside Windows (CS2's demos and archives are read on Windows).
#[cfg(not(windows))]
fn steam_dir_from_registry() -> Option<PathBuf> {
    None
}

/// The library folders of Steam's `libraryfolders.vdf` text: each numbered block's `path`.
pub fn library_folders(vdf: &str) -> Vec<PathBuf> {
    let root = crate::kv1::parse(vdf);
    root.get("libraryfolders")
        .map(|folders| {
            folders
                .pairs()
                .iter()
                .filter_map(|(_, folder)| folder.text("path"))
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// A Counter-Strike 2 installation that OFAST can read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cs2Install {
    root: PathBuf,
}

impl Cs2Install {
    /// Uses `root` as the CS2 install if it looks like one (contains `game/csgo/pak01_dir.vpk`).
    pub fn from_root(root: impl Into<PathBuf>) -> Option<Self> {
        let install = Self { root: root.into() };
        install.main_vpk().is_file().then_some(install)
    }

    /// Finds CS2 using the lookup order described in the [module docs](self).
    pub fn locate() -> Option<Self> {
        if let Some(dir) = std::env::var_os(CS2_DIR_ENV) {
            // An explicit override that is wrong should not silently fall back to another install.
            return Self::from_root(dir);
        }
        let vdf = steam_dir().join(r"steamapps\libraryfolders.vdf");
        let libraries = std::fs::read_to_string(vdf)
            .map(|text| library_folders(&text))
            .unwrap_or_default();
        libraries
            .iter()
            .find_map(|library| Self::from_root(library.join(CS2_IN_LIBRARY)))
            .or_else(|| Self::from_root(DEFAULT_WINDOWS_PATH))
    }

    /// The CS2 root folder (contains `game/`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `game/csgo` folder, where CS2's own content lives.
    pub fn csgo_dir(&self) -> PathBuf {
        self.root.join("game").join("csgo")
    }

    /// The main content archive, `game/csgo/pak01_dir.vpk`.
    pub fn main_vpk(&self) -> PathBuf {
        self.csgo_dir().join("pak01_dir.vpk")
    }

    /// The folder where CS2 saves downloaded match demos (`game/csgo/replays`).
    pub fn replays_dir(&self) -> PathBuf {
        self.csgo_dir().join("replays")
    }

    /// The installed CS2 build, from `game/csgo/steam.inf`; `None` when the file is missing or unreadable.
    pub fn version(&self) -> Option<Cs2Version> {
        Cs2Version::parse(&std::fs::read_to_string(self.csgo_dir().join("steam.inf")).ok()?)
    }
}

/// The CS2 builds OFAST was checked on, newest first: `PatchVersion` and `ClientVersion` of `game/csgo/steam.inf`.
///
/// A CS2 patch can change what OFAST reads (demo messages, animation graphs, shaders) without warning. After each
/// one, a round is exported and imported again and its checks compared (docs/development.md, "After a CS2 update");
/// the new build is added here once nothing is broken, or with the fix when something was.
pub const CHECKED_VERSIONS: &[(&str, u32)] = &[("1.41.9.3", 2_000_938)];

/// A CS2 build, as `game/csgo/steam.inf` names it (plain `key=value` lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cs2Version {
    /// The version players see (`PatchVersion`, "1.41.9.3").
    pub patch: String,
    /// The build number (`ClientVersion`), which every update changes.
    pub build: u32,
}

impl Cs2Version {
    /// Reads `steam.inf`'s text; `None` without a `PatchVersion` or a numeric `ClientVersion`.
    pub fn parse(steam_inf: &str) -> Option<Self> {
        let value = |key: &str| {
            steam_inf
                .lines()
                .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
                .map(str::trim)
        };
        Some(Self {
            patch: value("PatchVersion")?.to_owned(),
            build: value("ClientVersion")?.parse().ok()?,
        })
    }

    /// Whether OFAST was checked on this build ([`CHECKED_VERSIONS`]).
    pub fn is_checked(&self) -> bool {
        CHECKED_VERSIONS
            .iter()
            .any(|&(_, build)| build == self.build)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_root_rejects_a_folder_without_cs2_files() {
        let empty = std::env::temp_dir().join("ofast-not-a-cs2-install");
        assert_eq!(Cs2Install::from_root(empty), None);
    }

    #[test]
    fn version_reads_steam_inf() {
        let inf = "ClientVersion=2000938\r\nServerVersion=2000938\r\nPatchVersion=1.41.9.3\r\nProductName=cs2\r\n";
        let version = Cs2Version::parse(inf).expect("a version");
        assert_eq!(version.patch, "1.41.9.3");
        assert_eq!(version.build, 2_000_938);
        assert!(version.is_checked());
        let newer =
            Cs2Version::parse("ClientVersion=2000939\nPatchVersion=1.41.9.4\n").expect("a version");
        assert!(!newer.is_checked());
        assert_eq!(
            Cs2Version::parse("ClientVersion=x\nPatchVersion=1.41.9.3\n"),
            None
        );
        assert_eq!(Cs2Version::parse("ClientVersion=2000938\n"), None);
    }

    #[test]
    fn library_folders_lists_each_library_path() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\" { \"path\" \"C:\\\\Program Files (x86)\\\\Steam\" }\n\t\"1\" { \"path\" \"D:\\\\Games\" \"apps\" { \"730\" \"1\" } }\n}\n";
        assert_eq!(
            library_folders(vdf),
            [
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\Games")
            ]
        );
    }
}
