//! Oracle tests for the model reader and meshopt decoder against the developer's real CS2 install.
//!
//! - The AK-47 is checked in detail (meshes, draw calls, sensible size, unit normals).
//! - A sample of every model in the game is decoded end to end: the meshopt decoders must accept every buffer and
//!   produce exactly the announced number of bytes, and every draw call must reference valid vertices. A decoder
//!   that misread the format would fail or produce out-of-range indices.
//!
//! Only runs with `cargo test --release --features game-data --test cs2_model -- --nocapture`.
//! Set `OFAST_FULL_SCAN=1` to decode every model instead of a sample.

#![cfg(feature = "game-data")]

use ofast_core::game::Cs2Install;
use ofast_core::model::Model;
use ofast_core::resource::Resource;
use ofast_core::vpk::Vpk;

fn vpk() -> Vpk {
    let install = Cs2Install::locate().expect("CS2 install found");
    Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens")
}

#[test]
fn ak47_decodes_to_a_sensible_rifle() {
    let vpk = vpk();
    let data = vpk
        .read_path("weapons/models/ak47/weapon_rif_ak47.vmdl_c")
        .unwrap();
    let resource = Resource::parse(&data).unwrap();
    let model = Model::parse(&resource).unwrap();

    let names: Vec<_> = model.default_meshes().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["body_hd"], "default look is the HD body");

    let mesh = model.default_meshes().next().unwrap();
    let main = &mesh.draw_calls[0];
    assert_eq!(
        main.material,
        "weapons/models/ak47/materials/weapon_rif_ak47.vmat"
    );
    let geometry = mesh.geometry(main).unwrap();
    assert_eq!(geometry.indices.len(), main.index_count as usize);
    assert!(
        geometry
            .indices
            .iter()
            .all(|&i| (i as usize) < geometry.positions.len())
    );
    assert_eq!(geometry.normals.len(), geometry.positions.len());
    assert_eq!(geometry.uvs.len(), 2, "two UV sets");

    // Bounding box: an AK-47 is roughly 35 inches long.
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &geometry.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let size: Vec<f32> = (0..3).map(|k| max[k] - min[k]).collect();
    println!(
        "AK-47 body_hd: {} vertices, {} triangles, size {size:?} inches",
        geometry.positions.len(),
        geometry.indices.len() / 3
    );
    let longest = size.iter().copied().fold(0.0, f32::max);
    assert!(
        (25.0..50.0).contains(&longest),
        "longest side {longest} inches"
    );

    for n in &geometry.normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-3, "normal {n:?}");
    }
    for uv in &geometry.uvs[0] {
        assert!(
            uv.iter().all(|c| c.is_finite() && (-4.0..=5.0).contains(c)),
            "uv {uv:?}"
        );
    }
}

#[test]
fn sampled_models_decode_completely() {
    let vpk = vpk();
    let models: Vec<_> = vpk
        .entries()
        .iter()
        .filter(|e| e.path().ends_with(".vmdl_c"))
        .collect();
    let limit = if std::env::var_os("OFAST_FULL_SCAN").is_some() {
        models.len()
    } else {
        400
    };
    let step = (models.len() / limit).max(1);

    let (mut decoded, mut draws, mut triangles) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for entry in models.iter().step_by(step).take(limit) {
        let data = vpk.read(entry).unwrap();
        let result = Resource::parse(&data)
            .map_err(|e| e.to_string())
            .and_then(|resource| Model::parse(&resource).map_err(|e| e.to_string()));
        let model = match result {
            Ok(model) => model,
            Err(e) => {
                failures.push(format!("{}: {e}", entry.path()));
                continue;
            }
        };
        decoded += 1;
        for mesh in &model.meshes {
            for vb in &mesh.vertex_buffers {
                assert_eq!(
                    vb.data.len(),
                    vb.count * vb.stride,
                    "{} {}",
                    entry.path(),
                    mesh.name
                );
            }
            for draw in &mesh.draw_calls {
                match mesh.geometry(draw) {
                    Ok(g) if g.indices.len() == draw.index_count as usize => {
                        draws += 1;
                        triangles += g.indices.len() / 3;
                    }
                    Ok(g) => failures.push(format!(
                        "{} {}: {} of {} indices in range",
                        entry.path(),
                        mesh.name,
                        g.indices.len(),
                        draw.index_count
                    )),
                    Err(e) => failures.push(format!("{} {}: {e}", entry.path(), mesh.name)),
                }
            }
        }
    }
    println!(
        "models decoded: {decoded} of {}, draw calls: {draws}, triangles: {triangles}",
        models.len().min(limit)
    );
    for line in failures.iter().take(20) {
        println!("FAIL {line}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
}

/// Agents carry their weapons on attachments: `weapon` on the `wpn` bone with no offset, the holsters on body bones.
/// Weapons carry a `weapon` root bone and a `weapon_holster_center` attachment (`export::loadout`). Glove models are
/// bone-merged onto agents by name, so their bones' bind poses must be the agents' — checked here for every glove
/// model against both default agents (worst gap printed).
#[test]
fn attachments_and_glove_skeletons_fit_the_agents() {
    use ofast_core::export::loadout::{AgentMounts, WeaponFrames};
    let vpk = vpk();
    let model = |path: &str| {
        let data = vpk
            .read_path(path)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        Model::parse(&Resource::parse(&data).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"))
    };
    // Every agent model: those with the `weapon` attachment under agents/models (not the shared parts).
    let agents: Vec<(String, Model)> = vpk
        .entries()
        .iter()
        .map(|e| e.path().to_owned())
        .filter(|p| {
            p.starts_with("agents/models/") && !p.contains("/shared/") && p.ends_with(".vmdl_c")
        })
        .map(|p| {
            let m = model(&p);
            (p, m)
        })
        .filter(|(_, m)| m.attachment("weapon").is_some())
        .collect();
    println!("{} agent models", agents.len());
    assert!(agents.len() >= 20, "{} agents", agents.len());
    for (path, agent) in &agents {
        for name in [
            "weapon",
            "primary",
            "primary_smg",
            "pistol",
            "knife",
            "c4",
            "grenade0",
            "grenade1",
            "grenade2",
            "grenade3",
            "grenade4",
            "eholster",
        ] {
            if let Some(a) = agent.attachment(name) {
                assert!(!a.ignore_rotation, "{path} {name}: follows positions only");
            }
        }
        let mounts = AgentMounts::of(agent);
        let (bone, offset) = mounts.hand.expect("weapon attachment");
        assert_eq!(agent.skeleton.bones[bone].name, "wpn", "{path}");
        assert!(
            offset
                .iter()
                .flatten()
                .zip([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-5),
            "{offset:?}"
        );
        for name in ["primary", "pistol", "knife", "c4", "grenade0", "grenade4"] {
            assert!(mounts.holsters.contains_key(name), "{path} {name}");
        }
    }
    for weapon in [
        "weapons/models/ak47/weapon_rif_ak47.vmdl_c",
        "weapons/models/glock18/weapon_pist_glock18.vmdl_c",
        "weapons/models/knife/knife_bowie/weapon_knife_bowie.vmdl_c",
        "weapons/models/grenade/hegrenade/weapon_hegrenade.vmdl_c",
        "weapons/models/c4/weapon_c4.vmdl_c",
    ] {
        let frames = WeaponFrames::of(&model(weapon)).expect("weapon bone");
        assert!(frames.holster_center.is_some(), "{weapon}: holster centre");
    }
    let gloves: Vec<String> = vpk
        .entries()
        .iter()
        .map(|e| e.path().to_owned())
        .filter(|p| p.starts_with("agents/models/shared/arms/glove_") && p.ends_with(".vmdl_c"))
        .collect();
    assert!(gloves.len() >= 10, "{gloves:?}");
    // Worst gap over all gloves and agents: distance (inches) and angle (degrees) between shared bones' bind poses.
    let (mut worst_gap, mut worst_angle) = ((0.0f32, String::new()), (0.0f32, String::new()));
    for glove in &gloves {
        let glove_model = model(glove);
        let glove_world = glove_model.skeleton.world_transforms();
        for (path, agent) in &agents {
            let agent_world = agent.skeleton.world_transforms();
            let mut shared = 0;
            for (i, bone) in glove_model.skeleton.bones.iter().enumerate() {
                let Some(j) = agent.skeleton.find(&bone.name) else {
                    continue;
                };
                shared += 1;
                let (g, a) = (&glove_world[i], &agent_world[j]);
                let gap = (0..3)
                    .map(|k| (g[k][3] - a[k][3]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                // Angle of a⁻¹·g: trace of aᵀ·g = 1 + 2 cos θ.
                let trace: f32 = (0..3)
                    .flat_map(|r| (0..3).map(move |c| (r, c)))
                    .filter(|(r, c)| r == c)
                    .map(|(c, _)| (0..3).map(|k| a[k][c] * g[k][c]).sum::<f32>())
                    .sum();
                let angle = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees();
                let at = format!("{glove} on {path} at {}", bone.name);
                if gap > worst_gap.0 {
                    worst_gap = (gap, at.clone());
                }
                if angle > worst_angle.0 {
                    worst_angle = (angle, at);
                }
            }
            assert!(shared > 10, "{glove} on {path}: shares {shared} bones");
        }
    }
    println!(
        "{} gloves on {} agents: worst bind gap {:.3} in ({}), worst angle {:.2}° ({})",
        gloves.len(),
        agents.len(),
        worst_gap.0,
        worst_gap.1,
        worst_angle.0,
        worst_angle.1
    );
    assert!(worst_gap.0 < 0.05, "bind positions differ: {worst_gap:?}");
    assert!(
        worst_angle.0 < 1.0,
        "bind rotations differ: {worst_angle:?}"
    );
}
