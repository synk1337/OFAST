//! Locating a Counter-Strike 2 installation on disk.
//!
//! OFAST never ships Valve content: every asset is read from the user's own CS2 install. This module finds that
//! install so the rest of the crate (and the `game-data` tests) can open `game/csgo/pak01_dir.vpk`.
//!
//! Lookup order:
//! 1. The `OFAST_CS2_DIR` environment variable, if set. It must point at the CS2 root folder (the one containing
//!    `game/`).
//! 2. Steam's default library on Windows: `C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global
//!    Offensive` (CS2 kept the CS:GO folder name).
//!
//! Reading Steam's `libraryfolders.vdf` for installs on other drives comes later, once the KeyValues (v1) text
//! reader exists.

use std::path::{Path, PathBuf};

/// Environment variable that overrides CS2 install detection.
pub const CS2_DIR_ENV: &str = "OFAST_CS2_DIR";

/// Default CS2 location in Steam's main library on Windows.
const DEFAULT_WINDOWS_PATH: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive";

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
        Self::from_root(DEFAULT_WINDOWS_PATH)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_root_rejects_a_folder_without_cs2_files() {
        let empty = std::env::temp_dir().join("ofast-not-a-cs2-install");
        assert_eq!(Cs2Install::from_root(empty), None);
    }
}
