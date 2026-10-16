//! Exporters: turn CS2 content into standard files other tools (Unreal, Blender) can open.
//!
//! - [`gltf`]: models (with materials and textures) as glTF 2.0, and the shared glTF writer.
//! - [`map`]: whole maps (world, props, lights) as glTF 2.0 plus an OFAST manifest for Unreal.
//! - [`sky`]: a map's 2D sky cube map as an equirectangular HDR image.
//! - `environment` (internal): CS2's `csgo_environment(_blend)` materials, baked for glTF.

mod environment;
pub mod gltf;
pub mod map;
pub mod sky;

use crate::model::ModelError;
use crate::resource::ResourceError;
use crate::texture::TextureError;
use crate::vpk::VpkError;

/// Everything that can go wrong while exporting.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// Writing output files failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Reading from the game archives failed.
    #[error(transparent)]
    Vpk(#[from] VpkError),
    /// A resource could not be parsed.
    #[error(transparent)]
    Resource(#[from] ResourceError),
    /// A model could not be decoded.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// A texture could not be decoded.
    #[error(transparent)]
    Texture(#[from] TextureError),
    /// A map could not be read.
    #[error(transparent)]
    Map(#[from] crate::map::MapError),
    /// PNG encoding failed.
    #[error("PNG encoding failed: {0}")]
    Png(String),
}
