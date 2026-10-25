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

/// The reference pose (`model::refpose`): de_nuke's metal door, whose root bone is turned 90° about x at bind pose
/// and to (0.5, 0.5, 0.5, 0.5) in its `ref` animation, is drawn where its own physics hull is once moved to the
/// reference pose (at bind pose it stands 90° off: a closed door looked open); de_inferno's breakable door has the
/// same skeleton and animation but no bones of its own on its mesh, so it is drawn as stored, where its hull is.
#[test]
fn doors_stand_where_their_physics_is() {
    use ofast_core::content::Content;
    use ofast_core::physics::{PhysicsData, ShapeKind};
    let install = Cs2Install::locate().expect("CS2 install found");
    let check = |map: &str, path: &str, corrected: bool| {
        let content = Content::for_map(&install, map).expect("map archive opens");
        let resource_bytes = content.read_resource(path).expect("model reads");
        let resource = Resource::parse(&resource_bytes).unwrap();
        let model = Model::parse(&resource).unwrap();
        let mesh = model.default_meshes().next().expect("a mesh");
        let correction = model.reference_correction(mesh);
        assert_eq!(correction.is_some(), corrected, "{path}");
        let apply = |p: [f32; 3]| -> [f32; 3] {
            correction.map_or(p, |m| {
                std::array::from_fn(|i| (0..3).map(|k| m[i][k] * p[k]).sum::<f32>() + m[i][3])
            })
        };
        // The physics hull's bounds (model space: these parts follow no bone).
        let physics = PhysicsData::from_resource(&resource)
            .unwrap()
            .expect("PHYS");
        assert!(physics.bones.is_empty());
        let (hull_min, hull_max) = physics
            .parts
            .iter()
            .flat_map(|p| &p.shapes)
            .find_map(|s| match &s.kind {
                ShapeKind::Hull { min, max, .. } => Some((*min, *max)),
                _ => None,
            })
            .expect("a hull");
        // Every vertex within the hull's bounds grown by the handles' 7 inches.
        let mut checked = 0;
        for draw in &mesh.draw_calls {
            for p in mesh.geometry(draw).unwrap().positions {
                let p = apply(p);
                for k in 0..3 {
                    assert!(
                        p[k] >= hull_min[k] - 7.0 && p[k] <= hull_max[k] + 7.0,
                        "{path}: vertex {p:?} outside the hull {hull_min:?} – {hull_max:?}"
                    );
                }
                checked += 1;
            }
        }
        println!("{path}: {checked} vertices within its physics hull");
    };
    check(
        "de_nuke",
        "models/props/de_nuke/hr_nuke/metal_door_001/metal_door_001_br.vmdl",
        true,
    );
    check(
        "de_inferno",
        "models/breakable_props/break_wooden_door_01/break_wooden_door_01.vmdl",
        false,
    );
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

/// The `PHYS` blocks: every agent's ragdoll reads, its parts follow bones of the agent at the agent's own bind pose,
/// its joints link parts whose frames meet; de_nuke's world collision reads, with every shape inside its bounds.
#[test]
fn ragdolls_and_world_collision_read() {
    use ofast_core::content::Content;
    use ofast_core::model::compose;
    use ofast_core::physics::ragdoll::{Ragdoll, WorldCollision, joint_angles};
    use ofast_core::physics::{PhysicsData, ShapeKind};

    let vpk = vpk();
    let paths: Vec<String> = vpk
        .entries()
        .iter()
        .map(|e| e.path().to_owned())
        .filter(|p| {
            p.starts_with("agents/models/") && !p.contains("/shared/") && p.ends_with(".vmdl_c")
        })
        .collect();
    let (mut agents, mut worst_bind, mut worst_joint) = (0usize, 0.0f32, 0.0f32);
    let (mut worst_swing, mut worst_twist) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for path in &paths {
        let resource_bytes = vpk.read_path(path).unwrap();
        let resource = Resource::parse(&resource_bytes).unwrap();
        let model = Model::parse(&resource).unwrap();
        let Some(physics) = PhysicsData::from_resource(&resource).unwrap() else {
            continue;
        };
        agents += 1;
        assert_eq!(physics.bones.len(), physics.parts.len(), "{path}");
        assert_eq!(physics.bind_pose.len(), physics.parts.len(), "{path}");
        let bind = model.skeleton.world_transforms();
        for (part, name) in physics.parts.iter().zip(&physics.bones) {
            assert!(part.mass > 0.0, "{path} {name}: mass");
            assert!(
                part.shapes.iter().all(|s| matches!(
                    s.kind,
                    ShapeKind::Capsule { .. } | ShapeKind::Sphere { .. } | ShapeKind::Hull { .. }
                )),
                "{path} {name}: shapes"
            );
        }
        for (i, name) in physics.bones.iter().enumerate() {
            // The part's bone, by name ignoring case (the ragdoll spells `leg_upper_l`, the skeleton `leg_upper_L`).
            let bone = model
                .skeleton
                .bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(name))
                .unwrap_or_else(|| panic!("{path}: no bone {name}"));
            let difference = (0..3)
                .flat_map(|r| (0..4).map(move |c| (r, c)))
                .map(|(r, c)| (physics.bind_pose[i][r][c] - bind[bone][r][c]).abs())
                .fold(0.0f32, f32::max);
            worst_bind = worst_bind.max(difference);
        }
        for joint in &physics.joints {
            // At bind pose both frames of a joint are the same point (the joint's pivot).
            let a = compose(&physics.bind_pose[joint.body1], &joint.frame1);
            let b = compose(&physics.bind_pose[joint.body2], &joint.frame2);
            let gap = (0..3)
                .map(|k| (a[k][3] - b[k][3]).powi(2))
                .sum::<f32>()
                .sqrt();
            worst_joint = worst_joint.max(gap);
            // At bind pose each joint sits within its own limits, measured the way the solver measures them
            // (swing between the frames' z axes, twist of their x axes about the mean z): the axis convention.
            let (swing, twist) = joint_angles(
                joint,
                &physics.bind_pose[joint.body1],
                &physics.bind_pose[joint.body2],
            );
            // A joint without a swing limit is a hinge: no swing.
            worst_swing = worst_swing.max(swing - joint.swing.map_or(0.0, |(_, max)| max));
            if let Some((min, max)) = joint.twist {
                worst_twist = worst_twist.max(min - twist).max(twist - max);
            }
        }
    }
    println!(
        "{agents} agents with a ragdoll; part bind poses within {worst_bind:.4} of the skeleton's, joint frames meet within {worst_joint:.4} inch; at bind pose swings at most {worst_swing:.3} rad and twists at most {worst_twist:.3} rad past their limits"
    );
    assert!(agents >= 70, "{agents} agents with a ragdoll");
    assert!(
        worst_swing < 0.05 && worst_twist < 0.05,
        "bind poses within the joints' limits"
    );

    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, "de_nuke").expect("de_nuke archives open");
    let bytes = content
        .read_resource("maps/de_nuke/world_physics.vmdl")
        .expect("world physics reads");
    let world = PhysicsData::from_resource(&Resource::parse(&bytes).unwrap())
        .unwrap()
        .expect("a PHYS block");
    let (mut hulls, mut meshes, mut triangles, mut default) = (0usize, 0usize, 0usize, 0usize);
    let mut worst_corner = f32::NEG_INFINITY;
    for shape in world.parts.iter().flat_map(|p| &p.shapes) {
        default += usize::from(world.attribute(shape).is_some_and(|a| a.is_default()));
        let (vertices, min, max) = match &shape.kind {
            ShapeKind::Hull {
                vertices,
                planes,
                min,
                max,
            } => {
                hulls += 1;
                // Every corner lies on or inside every face plane (to float rounding at world coordinates).
                for v in vertices {
                    for (n, d) in planes {
                        let side = n[0] * v[0] + n[1] * v[1] + n[2] * v[2] - d;
                        worst_corner = worst_corner.max(side);
                    }
                }
                (vertices, min, max)
            }
            ShapeKind::Mesh {
                vertices,
                triangles: t,
                min,
                max,
                ..
            } => {
                meshes += 1;
                triangles += t.len();
                (vertices, min, max)
            }
            _ => continue,
        };
        for v in vertices {
            for k in 0..3 {
                assert!(
                    v[k] >= min[k] - 0.1 && v[k] <= max[k] + 0.1,
                    "a vertex outside its bounds"
                );
            }
        }
    }
    println!(
        "de_nuke world physics: {} parts, {hulls} hulls, {meshes} meshes ({triangles} triangles), {default} Default shapes, {} attributes",
        world.parts.len(),
        world.attributes.len()
    );
    println!("hull corners at most {worst_corner:.3} inch outside their own planes");
    assert!(hulls + meshes > 1000 && default > 1000);
    assert!(worst_corner < 0.2, "hull planes match their corners");

    // The SAS ragdoll dropped at bind pose 20 inches above de_nuke's CT spawn floor: it settles on the floor in a
    // few seconds, joints holding, nothing through the floor.
    let collision = WorldCollision::from_physics(&world);
    println!(
        "de_nuke collision: {:?} solid hulls and triangles",
        collision.counts()
    );
    let sas_bytes = vpk
        .read_path("agents/models/ctm_sas/ctm_sas.vmdl_c")
        .unwrap();
    let sas = PhysicsData::from_resource(&Resource::parse(&sas_bytes).unwrap())
        .unwrap()
        .expect("ragdoll");
    // CT spawn on de_nuke: a player stood at (2584, −504) on the floor at z = −352 (Aurora vs FURIA, round 5).
    const FLOOR: f32 = -352.0;
    let lift = ofast_core::model::affine([0.0, 0.0, 0.0, 1.0], [2584.0, -504.0, FLOOR + 20.0]);
    let start: Vec<_> = sas.bind_pose.iter().map(|m| compose(&lift, m)).collect();
    let mut ragdoll = Ragdoll::new(&sas, &start, &[]);
    let mut frames = 0;
    let started = std::time::Instant::now();
    let mut speeds = Vec::new();
    while !ragdoll.at_rest() && frames < 64 * 8 {
        ragdoll.step(1.0 / 64.0, &collision);
        frames += 1;
        if frames % 32 == 0 {
            speeds.push(format!("{:.0}", ragdoll.speed()));
        }
    }
    println!("speed every half second: {}", speeds.join(" "));
    let now = ragdoll.frames();
    let lowest = now.iter().map(|m| m[2][3]).fold(f32::MAX, f32::min);
    let highest = now.iter().map(|m| m[2][3]).fold(f32::MIN, f32::max);
    let mut gap = 0.0f32;
    for joint in &sas.joints {
        let a = compose(&now[joint.body1], &joint.frame1);
        let b = compose(&now[joint.body2], &joint.frame2);
        gap = gap.max(
            (0..3)
                .map(|k| (a[k][3] - b[k][3]).powi(2))
                .sum::<f32>()
                .sqrt(),
        );
    }
    println!(
        "SAS ragdoll: still after {frames} frames ({:.0} ms), parts between z {lowest:.1} and {highest:.1}, joints apart at most {gap:.2} inch",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert!(ragdoll.at_rest(), "comes to rest");
    assert!(lowest > FLOOR - 1.0, "nothing sinks through the floor");
    assert!(highest < FLOOR + 20.0, "lying down");
    assert!(gap < 1.0, "joints hold");
}
