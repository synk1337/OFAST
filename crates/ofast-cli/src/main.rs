//! `ofast` — command-line access to OFAST's CS2 readers and exporters.
//!
//! ```text
//! ofast find ak47                                  list files in CS2's archives whose path contains "ak47"
//! ofast export-model weapons/models/ak47/weapon_rif_ak47.vmdl_c --out exports/ak47
//! ofast vpk verify                                 check CS2's archive checksums
//! ```
//!
//! CS2 is found automatically (Steam's default library), or through `--cs2 <folder>` / `OFAST_CS2_DIR`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use ofast_core::export::gltf::export_model;
use ofast_core::game::Cs2Install;
use ofast_core::vpk::Vpk;

#[derive(Parser)]
#[command(
    name = "ofast",
    version,
    about = "OFAST — Counter-Strike 2 fragmovie toolkit"
)]
struct Cli {
    /// CS2 install folder (the one containing `game/`). Defaults to auto-detection.
    #[arg(long, global = true)]
    cs2: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List files in CS2's main archive whose path contains TEXT.
    Find {
        /// Text to look for (case-sensitive; CS2 paths are lowercase).
        text: String,
    },
    /// Export a model (with materials and textures) to glTF for Unreal or Blender.
    ExportModel {
        /// Path inside the archive, e.g. `weapons/models/ak47/weapon_rif_ak47.vmdl_c`.
        model: String,
        /// Output folder (created if needed).
        #[arg(long, short)]
        out: PathBuf,
    },
    /// Archive tools.
    Vpk {
        #[command(subcommand)]
        command: VpkCommand,
    },
}

#[derive(Subcommand)]
enum VpkCommand {
    /// Verify the directory file's MD5 checksums.
    Verify,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let install = match &cli.cs2 {
        Some(root) => Cs2Install::from_root(root),
        None => Cs2Install::locate(),
    }
    .ok_or("CS2 not found — pass --cs2 <folder> or set OFAST_CS2_DIR")?;
    let vpk = Vpk::open(install.main_vpk()).map_err(|e| e.to_string())?;

    match cli.command {
        Command::Find { text } => {
            for entry in vpk.entries().iter().filter(|e| e.path().contains(&text)) {
                println!("{:>12}  {}", entry.size(), entry.path());
            }
        }
        Command::ExportModel { model, out } => {
            let started = Instant::now();
            let report = export_model(&vpk, &model, &out).map_err(|e| e.to_string())?;
            println!("wrote {}", report.gltf.display());
            println!(
                "{} meshes, {} triangles, {} materials, {} textures in {:.1}s",
                report.meshes,
                report.triangles,
                report.materials,
                report.textures,
                started.elapsed().as_secs_f32()
            );
            for warning in &report.warnings {
                println!("warning: {warning}");
            }
        }
        Command::Vpk {
            command: VpkCommand::Verify,
        } => {
            let ok = vpk.verify_checksums().map_err(|e| e.to_string())?;
            println!(
                "{} files, checksums {}",
                vpk.len(),
                if ok { "OK" } else { "not present (v1 archive)" }
            );
        }
    }
    Ok(())
}
