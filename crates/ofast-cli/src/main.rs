//! `ofast` — command-line access to OFAST's CS2 readers and exporters.
//!
//! ```text
//! ofast find ak47                                  list files in CS2's archives whose path contains "ak47"
//! ofast export-model weapons/models/ak47/weapon_rif_ak47.vmdl_c --out exports/ak47
//! ofast export-map de_mirage --out exports/de_mirage   (--bake-model-tints: tints in the textures, for Blender)
//! ofast demo-rounds match.dem                      a demo's rounds and their ticks
//! ofast export-demo match.dem --round 3 --out exports/demos   one round as a replay file for Unreal
//!                                       (--players: and the players as animated agents with their weapons)
//! ofast vpk verify                                 check CS2's archive checksums
//! ```
//!
//! CS2 is found automatically (Steam's default library), or through `--cs2 <folder>` / `OFAST_CS2_DIR`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use ofast_core::content::Content;
use ofast_core::demo::players::{list_rounds, track_round};
use ofast_core::export::ExportOptions;
use ofast_core::export::gltf::{ExportReport, export_model_with_clips};
use ofast_core::export::map::{Skybox, export_map};
use ofast_core::export::players::export_players;
use ofast_core::export::replay::write_replay;
use ofast_core::game::Cs2Install;
use ofast_core::map::{Map, sky_to_world};
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
        /// CS2 animation clips to add as glTF animations, retargeted onto the model's skeleton (repeatable), e.g.
        /// `animation/anims/world/rifle/_default_rifle/run_n_rifle.vnmclip`.
        #[arg(long = "clip")]
        clips: Vec<String>,
    },
    /// Export a whole map (world, props, lights) to glTF plus an OFAST manifest for Unreal.
    ExportMap {
        /// Map name, e.g. `de_mirage` (reads `game/csgo/maps/<name>.vpk`).
        map: String,
        /// Output folder (created if needed).
        #[arg(long, short)]
        out: PathBuf,
        /// Bake placement tints that recolour only part of a surface (environment materials, props with a tint
        /// mask) into the colour textures, one texture per tint, for glTF readers other than OFAST's Unreal import
        /// (Blender, viewers). Without it Unreal's materials apply them and the export is smaller (de_inferno: about
        /// half the colour textures).
        #[arg(long)]
        bake_model_tints: bool,
    },
    /// List a demo's rounds with their tick ranges.
    DemoRounds {
        /// The `.dem` file.
        demo: PathBuf,
    },
    /// Export one round of a demo (players' movement, kills, milestones) as a replay file for Unreal.
    ExportDemo {
        /// The `.dem` file.
        demo: PathBuf,
        /// Round number (1 = first round after warmup; see `demo-rounds`).
        #[arg(long, short)]
        round: u32,
        /// Output folder (created if needed).
        #[arg(long, short)]
        out: PathBuf,
        /// Also write the players as animated CS2 agents with their weapons in hand and holstered: one glTF per agent
        /// look (`<demo>_r<round>_<agent>_<mesh groups>.gltf`) and `<demo>_r<round>.agents.json` listing them, their
        /// animation baked from the demo: CS2's own poses from the demo's pose recipes where it can, OFAST's own
        /// elsewhere (the environment variable `OFAST_RECIPE=0` keeps OFAST's own only). Needs the CS2 install
        /// (agent and weapon models, animation clips).
        #[arg(long)]
        players: bool,
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

fn print_report(report: &ExportReport, started: Instant) {
    println!("wrote {}", report.gltf.display());
    println!(
        "{} meshes, {} placements, {} triangles, {} materials, {} textures, {} lights in {:.1}s",
        report.meshes,
        report.nodes,
        report.triangles,
        report.materials,
        report.textures,
        report.lights,
        started.elapsed().as_secs_f32()
    );
    if report.degenerate_triangles > 0 {
        println!(
            "removed {} zero-area triangles",
            report.degenerate_triangles
        );
    }
    if report.shared_textures > 0 {
        println!(
            "{} textures equal to another one share its file",
            report.shared_textures
        );
    }
    if report.removed_textures > 0 {
        println!(
            "removed {} texture files no glTF in the folder uses",
            report.removed_textures
        );
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let started = Instant::now();
    // Demos are plain files: no CS2 install needed.
    match &cli.command {
        Command::DemoRounds { demo } => {
            let data = std::fs::read(demo).map_err(|e| format!("{}: {e}", demo.display()))?;
            for r in list_rounds(&data).map_err(|e| e.to_string())? {
                println!(
                    "round {:2}: ticks {:6} ..= {:6}",
                    r.round, r.start_tick, r.end_tick
                );
            }
            return Ok(());
        }
        Command::ExportDemo {
            demo,
            round,
            out,
            players,
        } => {
            let data = std::fs::read(demo).map_err(|e| format!("{}: {e}", demo.display()))?;
            let name = demo.file_stem().and_then(|s| s.to_str()).unwrap_or("demo");
            let track = track_round(&data, *round).map_err(|e| e.to_string())?;
            let report = write_replay(&track, name, out).map_err(|e| e.to_string())?;
            println!("wrote {}", report.path.display());
            println!(
                "{} round {round}: {} players, {} ticks ({:.0} s), {} kills in {:.1}s",
                report.map,
                report.players,
                report.ticks,
                report.seconds,
                report.kills,
                started.elapsed().as_secs_f32()
            );
            for warning in &report.warnings {
                println!("warning: {warning}");
            }
            if *players {
                let install = match &cli.cs2 {
                    Some(root) => Cs2Install::from_root(root),
                    None => Cs2Install::locate(),
                }
                .ok_or("CS2 not found — pass --cs2 <folder> or set OFAST_CS2_DIR")?;
                // The map's archive too, for its collision (the dead fall on it); the game's alone if it is missing.
                let content = Content::for_map(&install, &track.map_name)
                    .or_else(|_| Content::game(&install))
                    .map_err(|e| e.to_string())?;
                let report =
                    export_players(&content, &track, name, out).map_err(|e| e.to_string())?;
                for file in &report.files {
                    println!("wrote {}", file.display());
                }
                println!("wrote {}", report.manifest.display());
                println!(
                    "{} players animated with {} weapon bones ({} deaths as ragdolls), {} frames each, {} dropped weapon models, {} first-person viewmodels, in {:.1}s",
                    report.players,
                    report.weapons,
                    report.deaths,
                    report.frames,
                    report.dropped,
                    report.viewmodels,
                    started.elapsed().as_secs_f32()
                );
                for fall in &report.falls {
                    println!("  death: {fall}");
                }
                for check in &report.checks {
                    println!("  check: {check}");
                }
                for warning in &report.warnings {
                    println!("warning: {warning}");
                }
            }
            return Ok(());
        }
        _ => {}
    }

    let install = match &cli.cs2 {
        Some(root) => Cs2Install::from_root(root),
        None => Cs2Install::locate(),
    }
    .ok_or("CS2 not found — pass --cs2 <folder> or set OFAST_CS2_DIR")?;

    match cli.command {
        Command::DemoRounds { .. } | Command::ExportDemo { .. } => unreachable!("handled above"),
        Command::Find { text } => {
            let content = Content::game(&install).map_err(|e| e.to_string())?;
            for vpk in content.archives() {
                for entry in vpk.entries().iter().filter(|e| e.path().contains(&text)) {
                    println!("{:>12}  {}", entry.size(), entry.path());
                }
            }
        }
        Command::ExportModel { model, out, clips } => {
            let content = Content::game(&install).map_err(|e| e.to_string())?;
            let report = export_model_with_clips(&content, &model, &clips, &out)
                .map_err(|e| e.to_string())?;
            print_report(&report, started);
        }
        Command::ExportMap {
            map,
            out,
            bake_model_tints,
        } => {
            let mut content = Content::for_map(&install, &map).map_err(|e| e.to_string())?;
            let loaded = Map::load(&content, &map).map_err(|e| e.to_string())?;

            // The 3D skybox is its own world in its own archive; searched right after the map's archive. A skybox
            // that cannot be read is a warning: the map still exports without it.
            let mut skybox_map = None;
            if let Some(reference) = loaded.skybox_reference() {
                let archive = install.csgo_dir().join(&reference.archive);
                let sky = Vpk::open(&archive)
                    .map_err(|e| e.to_string())
                    .and_then(|vpk| {
                        content.insert(1, vpk);
                        Map::load(&content, &reference.world).map_err(|e| e.to_string())
                    });
                match sky {
                    Ok(sky) => {
                        let (camera, scale) = sky.sky_camera().unwrap_or(([0.0; 3], 16.0));
                        let objects: usize = sky
                            .nodes
                            .iter()
                            .map(|n| n.scene_objects.len() + n.aggregates.len())
                            .sum();
                        println!(
                            "3D skybox {}: {objects} objects, scale {scale}, sky camera {camera:?} → {:?}",
                            reference.world, reference.origin
                        );
                        let transform = sky_to_world(camera, scale, &reference);
                        skybox_map = Some((sky, transform));
                    }
                    Err(e) => eprintln!("warning: 3D skybox {} skipped: {e}", archive.display()),
                }
            }
            let skybox = skybox_map.as_ref().map(|(map, transform)| Skybox {
                map,
                transform: *transform,
            });

            let options = ExportOptions { bake_model_tints };
            let report = export_map(&content, &loaded, skybox.as_ref(), &out, options)
                .map_err(|e| e.to_string())?;
            print_report(&report, started);
        }
        Command::Vpk {
            command: VpkCommand::Verify,
        } => {
            let content = Content::game(&install).map_err(|e| e.to_string())?;
            let vpk = &content.archives()[0];
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
