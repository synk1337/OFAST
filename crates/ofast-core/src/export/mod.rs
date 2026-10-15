//! Exporters: turn CS2 content into standard files other tools (Unreal, Blender) can open.
//!
//! - [`gltf`]: models (with materials and textures) as glTF 2.0.

pub mod gltf;

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
    /// PNG encoding failed.
    #[error("PNG encoding failed: {0}")]
    Png(String),
}
