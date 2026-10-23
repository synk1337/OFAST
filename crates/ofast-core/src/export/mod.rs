//! Exporters: turn CS2 content into standard files other tools (Unreal, Blender) can open.
//!
//! - [`gltf`]: models (with materials and textures) as glTF 2.0, and the shared glTF writer.
//! - [`map`]: whole maps (world, props, lights) as glTF 2.0 plus an OFAST manifest for Unreal.
//! - [`sky`]: a map's 2D sky cube map as an equirectangular HDR image.
//! - `environment` (internal): CS2's `csgo_environment(_blend)` materials, baked for glTF.
//! - `tint` (internal): the model tints of CS2's prop shaders, masked by `g_tTintMask`.
//! - `light_cookie` (internal): barn lights' cookies as a radial profile and a matching spot cone.
//! - [`replay`]: one round of a demo (players' movement, kills, milestones) for Unreal's Level Sequence.
//! - [`players`]: the players of a round as animated CS2 agents, with their weapons in hand and holstered (one glTF
//!   per agent look).
//! - [`loadout`]: where each weapon a player carries sits on the agent, frame by frame.
//! - [`deaths`]: each death as CS2's ragdoll of the agent, simulated and written onto its bones.
//! - [`viewmodel`]: the players' first-person arms and weapons (one glTF per agent and glove skin).

pub mod deaths;
mod environment;
pub mod gltf;
mod light_cookie;
pub mod loadout;
pub mod map;
pub mod players;
pub mod replay;
pub mod sky;
mod tint;
pub mod viewmodel;

use crate::model::ModelError;
use crate::resource::ResourceError;
use crate::texture::TextureError;
use crate::vpk::VpkError;

/// Choices a map export leaves to the caller. The default suits OFAST's Unreal import (`import_map.py`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    /// Bake the model tints (`rendercolor`, scene-object tints) that only part of a surface takes into the colour
    /// textures, one texture per tint, as glTF readers other than OFAST's Unreal import need (Blender, viewers):
    /// those of `csgo_environment(_blend)` materials and those of prop shaders with a tint mask (`F_TINT_MASK`, when
    /// the mask uses the colour's UV set). Off by default: the colour is written once without the tint and the tint
    /// travels in `extras.ofast.environment` / `extras.ofast.tint_mask` for Unreal's materials to apply — de_inferno
    /// then writes about half the colour bakes, de_dust2 saves 261 tinted copies of 44 prop textures. See
    /// `docs/formats/material.md` ("Model tint", "Model tint of prop shaders and their tint masks").
    pub bake_model_tints: bool,
}

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
    /// A demo could not be read.
    #[error(transparent)]
    Demo(#[from] crate::demo::players::TrackError),
    /// An animation (graph, blend, pose) could not be computed.
    #[error(transparent)]
    Anim(#[from] crate::anim::AnimError),
    /// An animation clip could not be decoded.
    #[error("clip {path}: {source}")]
    Clip {
        /// The clip's path.
        path: String,
        /// What went wrong.
        source: crate::anim::AnimError,
    },
    /// A character (agent with weapons and gloves) could not be put together.
    #[error("character: {0}")]
    Character(String),
}
