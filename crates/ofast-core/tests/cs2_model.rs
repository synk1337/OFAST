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
