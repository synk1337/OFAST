//! A search path over several VPK archives — how the game itself finds files.
//!
//! CS2 content is split across archives: the shared `pak01_dir.vpk` holds most models, materials and textures,
//! while each map ships its own single-file VPK (`game/csgo/maps/de_mirage.vpk`) with the compiled world, its
//! baked geometry models, lightmaps and entities. A map file refers to shared content by path, so readers need to
//! look in the map archive first and then in `pak01`, exactly like the engine's search path.
//!
//! Paths are archive-relative (`materials/de_mirage/doors/door_b.vmat_c`); backslashes are accepted.

use crate::game::Cs2Install;
use crate::vpk::{Vpk, VpkError};

/// An ordered list of archives searched front to back.
#[derive(Debug, Default)]
pub struct Content {
    archives: Vec<Vpk>,
}

impl Content {
    /// An empty search path.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an archive at the end of the search path (searched after the ones already added).
    pub fn push(&mut self, vpk: Vpk) {
        self.archives.push(vpk);
    }

    /// The shared game content only (`pak01_dir.vpk`).
    pub fn game(install: &Cs2Install) -> Result<Self, VpkError> {
        let mut content = Self::new();
        content.push(Vpk::open(install.main_vpk())?);
        Ok(content)
    }

    /// A map's own archive (`maps/<name>.vpk`) searched first, then the shared game content.
    pub fn for_map(install: &Cs2Install, map: &str) -> Result<Self, VpkError> {
        let mut content = Self::new();
        content.push(Vpk::open(
            install.csgo_dir().join("maps").join(format!("{map}.vpk")),
        )?);
        content.push(Vpk::open(install.main_vpk())?);
        Ok(content)
    }

    /// The archives in search order.
    pub fn archives(&self) -> &[Vpk] {
        &self.archives
    }

    /// `true` when some archive contains `path`.
    pub fn exists(&self, path: &str) -> bool {
        self.archives.iter().any(|vpk| vpk.find(path).is_some())
    }

    /// Reads `path` from the first archive that has it (CRC-checked).
    pub fn read(&self, path: &str) -> Result<Vec<u8>, VpkError> {
        for vpk in &self.archives {
            if let Some(entry) = vpk.find(path) {
                return vpk.read(entry);
            }
        }
        Err(VpkError::NotFound(path.to_owned()))
    }

    /// Reads a compiled resource given the path stored inside other resources, which omits the `_c` suffix and may
    /// use backslashes (e.g. `materials/foo.vmat` → `materials/foo.vmat_c`).
    pub fn read_resource(&self, reference: &str) -> Result<Vec<u8>, VpkError> {
        let path = reference.replace('\\', "/");
        if path.ends_with("_c") {
            self.read(&path)
        } else {
            self.read(&format!("{path}_c"))
        }
    }
}
