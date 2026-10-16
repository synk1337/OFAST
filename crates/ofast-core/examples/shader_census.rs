//! Developer tool: which CS2 material shaders every map uses (world, props, entities; not the 3D skybox), to see
//! what OFAST must handle to import any official map. Prints, per shader, how many maps and materials use it, then
//! the shaders per map.
//!
//! ```bash
//! cargo run --release --example shader_census                       # every map archive in game/csgo/maps
//! cargo run --release --example shader_census -- de_inferno de_nuke  # some maps
//! ```

use std::collections::{BTreeMap, BTreeSet};

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::map::Map;
use ofast_core::model::Model;
use ofast_core::resource::{BlockKind, Resource};

fn main() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let mut maps: Vec<String> = std::env::args().skip(1).collect();
    if maps.is_empty() {
        // Playable maps: skip the vanity (main menu) scenes, previews and helper archives.
        let dir = install.csgo_dir().join("maps");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("maps folder reads")
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".vpk").map(str::to_owned))
            .filter(|n| {
                !n.ends_with("_vanity")
                    && !n.starts_with("workshop_preview")
                    && !matches!(n.as_str(), "graphics_settings" | "lobby_mapveto")
            })
            .collect();
        names.sort();
        maps = names;
    }

    // shader -> map -> material count
    let mut usage: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for name in &maps {
        let Ok(content) = Content::for_map(&install, name) else {
            eprintln!("{name}: archive does not open");
            continue;
        };
        let Ok(map) = Map::load(&content, name) else {
            eprintln!("{name}: map does not load");
            continue;
        };
        let mut models: BTreeSet<(String, bool)> = BTreeSet::new(); // (path, aggregate: first mesh only)
        for node in &map.nodes {
            for o in &node.scene_objects {
                models.insert((o.model.clone(), false));
            }
            for a in &node.aggregates {
                models.insert((a.model.clone(), true));
            }
        }
        for e in &map.entities {
            if let Some(model) = e.string("model").filter(|m| m.ends_with(".vmdl")) {
                models.insert((model.to_owned(), false));
            }
        }
        let mut materials = BTreeSet::new();
        for (path, aggregate) in &models {
            let Ok(data) = content.read_resource(path) else {
                continue;
            };
            let Ok(resource) = Resource::parse(&data) else {
                continue;
            };
            let Ok(model) = Model::parse(&resource) else {
                continue;
            };
            let meshes: Vec<_> = if *aggregate {
                model.meshes.iter().take(1).collect()
            } else {
                model.default_meshes().collect()
            };
            for mesh in meshes {
                materials.extend(mesh.draw_calls.iter().map(|d| d.material.clone()));
            }
        }
        let mut per_shader: BTreeMap<String, usize> = BTreeMap::new();
        for material in &materials {
            let shader = content
                .read_resource(material)
                .ok()
                .and_then(|data| {
                    let resource = Resource::parse(&data).ok()?;
                    let root = resource.kv3(BlockKind::DATA).ok()?.root;
                    root.get("m_shaderName")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "(unreadable)".into());
            *per_shader.entry(shader).or_default() += 1;
        }
        println!(
            "{name}: {} models, {} materials: {}",
            models.len(),
            materials.len(),
            per_shader
                .iter()
                .map(|(s, n)| format!("{} {n}", s.trim_end_matches(".vfx")))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (shader, count) in per_shader {
            usage.entry(shader).or_default().insert(name.clone(), count);
        }
    }
    println!("\nshader: maps using it, materials in total");
    let mut rows: Vec<_> = usage.iter().collect();
    rows.sort_by_key(|(_, maps)| std::cmp::Reverse(maps.values().sum::<usize>()));
    for (shader, maps) in rows {
        println!(
            "{:<36} {:>2} maps {:>5} materials",
            shader,
            maps.len(),
            maps.values().sum::<usize>()
        );
    }
}
