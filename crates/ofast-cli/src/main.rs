//! `ofast` — command-line access to OFAST's CS2 readers and exporters.
//!
//! ```text
//! ofast find ak47                                  list files in CS2's archives whose path contains "ak47"
//! ofast export-model weapons/models/ak47/weapon_rif_ak47.vmdl_c --out exports/ak47
//! ofast export-map de_mirage --out exports/de_mirage   (--bake-model-tints: tints in the textures, for Blender)
//! ofast demo-rounds match.dem                      a demo's rounds and their ticks
//! ofast export-demo match.dem --round 3 --out exports/demos   one round as a replay file for Unreal
//!                                       (--players: and the players as animated agents with their weapons;
//!                                        --shots 152112:orbit --smooth 0.5 --hlae path.xml: its cameras)
//! ofast camera-to-hlae round.cameras.json --camera 0 --out shot.xml   a camera as an HLAE campath (or .cam)
//! ofast vpk verify                                 check CS2's archive checksums
//! ```
//!
//! CS2 is found automatically (Steam's default library), or through `--cs2 <folder>` / `OFAST_CS2_DIR`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use ofast_core::camera::hlae::{Campath, write_cam, write_campath};
use ofast_core::camera::shots::ShotKind;
use ofast_core::content::Content;
use ofast_core::demo::players::{list_rounds, track_round};
use ofast_core::export::ExportOptions;
use ofast_core::export::cameras::{CameraPlan, HLAE_ASPECT, export_cameras, read_cameras};
use ofast_core::export::gltf::{ExportReport, export_model_with_clips};
use ofast_core::export::map::{SkyboxExport, export_installed_map};
use ofast_core::export::players::export_players;
use ofast_core::export::replay::write_replay;
use ofast_core::game::Cs2Install;

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
        /// Kills shown through another camera than the killer's eyes: `TICK:KIND,…` (the kill's tick, from the
        /// replay file's `kills`), KIND one of pov, victim, behind, orbit, tripod, above. Written to
        /// `<demo>_r<round>.cameras.json` with every player's eyes and the camera cuts.
        #[arg(long, value_delimiter = ',')]
        shots: Vec<String>,
        /// First-person smoothing, 0 (none) to 1 (strong); the view stays on the aim at every shot.
        #[arg(long, default_value_t = 0.0)]
        smooth: f64,
        /// An HLAE camera file to cut to over its time in the round: a `mirv_campath` path (`.xml`) or a `mirv_camio`
        /// recording (`.cam`). Repeatable.
        #[arg(long)]
        hlae: Vec<PathBuf>,
    },
    /// Write one camera of a cameras file (`export-demo`'s, or one written from Unreal by
    /// `tools/unreal/export_camera.py`) as an HLAE file: `.xml` a `mirv_campath` path, `.cam` a `mirv_camio`
    /// recording.
    CameraToHlae {
        /// The cameras file.
        cameras: PathBuf,
        /// Which camera: its number in the file (from 0) or its name.
        #[arg(long, default_value = "0")]
        camera: String,
        /// The HLAE file to write (`.xml` or `.cam`).
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

/// The cameras `export-demo` was asked for: `--shots TICK:KIND,…`, `--smooth`, `--hlae`.
fn camera_plan(shots: &[String], smoothing: f64, hlae: &[PathBuf]) -> Result<CameraPlan, String> {
    let mut plan = CameraPlan {
        smoothing,
        hlae: hlae.to_vec(),
        ..CameraPlan::default()
    };
    for shot in shots {
        let (tick, kind) = shot
            .split_once(':')
            .ok_or_else(|| format!("--shots {shot:?}: TICK:KIND expected"))?;
        let tick: u32 = tick
            .trim()
            .parse()
            .map_err(|_| format!("--shots {shot:?}: {tick:?} is not a tick"))?;
        let kind = ShotKind::parse(kind.trim()).ok_or_else(|| {
            let kinds: Vec<&str> = ShotKind::ALL.iter().map(|k| k.name()).collect();
            format!("--shots {shot:?}: {kind:?} is not one of {kinds:?}")
        })?;
        plan.shots.insert(tick, kind);
    }
    Ok(plan)
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
            shots,
            smooth,
            hlae,
        } => {
            let plan = camera_plan(shots, *smooth, hlae)?;
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
            // With the players, CS2's content: the map's archive too, for its collision (the dead fall on it, the
            // shot cameras keep out of walls); the game's alone if it is missing.
            let content = if *players {
                let install = match &cli.cs2 {
                    Some(root) => Cs2Install::from_root(root),
                    None => Cs2Install::locate(),
                }
                .ok_or("CS2 not found — pass --cs2 <folder> or set OFAST_CS2_DIR")?;
                Some(
                    Content::for_map(&install, &track.map_name)
                        .or_else(|_| Content::game(&install))
                        .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            let cameras = export_cameras(content.as_ref(), &track, &plan, name, out)
                .map_err(|e| e.to_string())?;
            println!(
                "wrote {} ({} cameras, {} cuts)",
                cameras.path.display(),
                cameras.cameras,
                cameras.cuts
            );
            for warning in &cameras.warnings {
                println!("warning: {warning}");
            }
            if let Some(content) = &content {
                let report =
                    export_players(content, &track, name, out).map_err(|e| e.to_string())?;
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
        Command::CameraToHlae {
            cameras,
            camera,
            out,
        } => {
            let text = std::fs::read_to_string(cameras)
                .map_err(|e| format!("{}: {e}", cameras.display()))?;
            let (clock, list) =
                read_cameras(&text).map_err(|e| format!("{}: {e}", cameras.display()))?;
            let chosen = camera
                .parse::<usize>()
                .ok()
                .and_then(|i| list.get(i))
                .or_else(|| list.iter().find(|c| &c.name == camera))
                .ok_or_else(|| {
                    let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
                    format!("no camera {camera:?} (the file has {names:?})")
                })?;
            let extension = out.extension().and_then(|e| e.to_str()).unwrap_or("");
            let written = match extension.to_ascii_lowercase().as_str() {
                "cam" => write_cam(&chosen.keys, clock, HLAE_ASPECT),
                "xml" => write_campath(&Campath::from_keys(&chosen.keys, clock)),
                _ => return Err(format!("{}: .xml or .cam expected", out.display())),
            };
            std::fs::write(out, written).map_err(|e| format!("{}: {e}", out.display()))?;
            println!(
                "wrote {} ({}: {} keys)",
                out.display(),
                chosen.name,
                chosen.keys.len()
            );
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
        Command::DemoRounds { .. } | Command::ExportDemo { .. } | Command::CameraToHlae { .. } => {
            unreachable!("handled above")
        }
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
            let options = ExportOptions { bake_model_tints };
            let (report, skybox) =
                export_installed_map(&install, &map, &out, options).map_err(|e| e.to_string())?;
            match skybox {
                SkyboxExport::Exported {
                    world,
                    objects,
                    scale,
                    camera,
                    origin,
                } => println!(
                    "3D skybox {world}: {objects} objects, scale {scale}, sky camera {camera:?} → {origin:?}"
                ),
                // A skybox that cannot be read is a warning: the map still exports without it.
                SkyboxExport::Skipped(why) => eprintln!("warning: 3D skybox {why} skipped"),
                SkyboxExport::None => {}
            }
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
