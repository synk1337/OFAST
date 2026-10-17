//! Developer tool: the draw calls that carry their own tint (`m_vTintColor`, `m_flAlpha` in a model's `MDAT`), per
//! map, and how they meet the placement's tint (scene object, aggregate fragment, entity `rendercolor`).
//!
//! Hammer stores the colour picked for a mesh face or overlay there, not in the material: de_nuke's yellow road
//! paint is a white material drawn with a yellow draw call (see `docs/formats/material.md`, "Draw-call tint").
//!
//! ```bash
//! cargo run --release --example drawcall_tints -- de_nuke            # summary + every tinted material
//! cargo run --release --example drawcall_tints -- de_nuke de_dust2   # several maps
//! ```
//!
//! Output per map: how many draw calls of each placement kind are tinted, then one line per (material, draw tint)
//! with the tint as linear floats and as sRGB bytes (gamma 2.2 and the sRGB curve), and for aggregate fragments
//! their own tint next to it (to see whether the fragment tint already contains the draw call's).

use std::collections::BTreeMap;
use std::rc::Rc;

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::map::Map;
use ofast_core::model::{DrawCall, Model};
use ofast_core::resource::{BlockKind, Resource};

fn load(content: &Content, path: &str) -> Option<Model> {
    let data = content.read_resource(path).ok()?;
    let resource = Resource::parse(&data).ok()?;
    Model::parse(&resource).ok()
}

/// The material's shader name, `?` when it does not read.
fn shader(content: &Content, material: &str) -> String {
    content
        .read_resource(material)
        .ok()
        .and_then(|data| {
            let resource = Resource::parse(&data).ok()?;
            let root = resource.kv3(BlockKind::DATA).ok()?.root;
            root.get("m_shaderName")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "?".into())
}

fn srgb_curve(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn bytes(rgb: [f32; 3], encode: impl Fn(f32) -> f32) -> String {
    let b = rgb.map(|c| encode(c.clamp(0.0, 1.0)) * 255.0);
    format!("{:.1},{:.1},{:.1}", b[0], b[1], b[2])
}

#[derive(Default)]
struct Seen {
    /// Placements of this (material, tint).
    count: usize,
    /// Placement kinds and their own tints (sRGB 0–255), deduplicated.
    placements: BTreeMap<String, usize>,
}

fn main() {
    let install = Cs2Install::locate().expect("CS2 install found");
    for name in std::env::args().skip(1) {
        let content = Content::for_map(&install, &name).expect("map archive opens");
        let map = Map::load(&content, &name).expect("map loads");
        let mut cache: BTreeMap<String, Option<Rc<Model>>> = BTreeMap::new();
        let mut model = |path: &str| -> Option<Rc<Model>> {
            cache
                .entry(path.to_owned())
                .or_insert_with(|| load(&content, path).map(Rc::new))
                .clone()
        };

        // (material, draw tint as text) -> what placed it
        let mut seen: BTreeMap<(String, String), Seen> = BTreeMap::new();
        let mut totals: BTreeMap<&str, (usize, usize)> = BTreeMap::new(); // kind -> (draws, tinted)
        let mut note = |kind: &'static str, draw: &DrawCall, placement: String| {
            let total = totals.entry(kind).or_default();
            total.0 += 1;
            if draw.is_untinted() {
                return;
            }
            total.1 += 1;
            let key = (
                draw.material.clone(),
                format!(
                    "lin {:.4},{:.4},{:.4} a {:.2} | g2.2 {} | srgb {}",
                    draw.tint[0],
                    draw.tint[1],
                    draw.tint[2],
                    draw.alpha,
                    bytes(draw.tint, |c| c.powf(1.0 / 2.2)),
                    bytes(draw.tint, srgb_curve)
                ),
            );
            let entry = seen.entry(key).or_default();
            entry.count += 1;
            *entry
                .placements
                .entry(format!("{kind} {placement}"))
                .or_default() += 1;
        };

        let rgb = |t: [f32; 4]| {
            format!(
                "{:.0},{:.0},{:.0}",
                t[0] * 255.0,
                t[1] * 255.0,
                t[2] * 255.0
            )
        };
        for node in &map.nodes {
            for object in &node.scene_objects {
                let kind = if object.flags & ofast_core::map::object_flags::OVERLAY != 0 {
                    "overlay"
                } else {
                    "scene object"
                };
                if let Some(m) = model(&object.model) {
                    for mesh in m.default_meshes() {
                        for draw in &mesh.draw_calls {
                            note(kind, draw, rgb(object.tint));
                        }
                    }
                }
            }
            for aggregate in &node.aggregates {
                let Some(m) = model(&aggregate.model) else {
                    continue;
                };
                let Some(mesh) = m.meshes.first() else {
                    continue;
                };
                for fragment in &aggregate.fragments {
                    if let Some(draw) = mesh.draw_calls.get(fragment.draw_call) {
                        note("fragment", draw, rgb(fragment.tint));
                    }
                }
            }
        }
        for entity in &map.entities {
            if let Some(path) = entity.string("model").filter(|m| m.ends_with(".vmdl"))
                && let Some(m) = model(path)
            {
                let tint = entity
                    .vector3("rendercolor")
                    .map(|c| format!("{:.0},{:.0},{:.0}", c[0], c[1], c[2]))
                    .unwrap_or_else(|| "none".into());
                for mesh in m.default_meshes() {
                    for draw in &mesh.draw_calls {
                        note("entity", draw, tint.clone());
                    }
                }
            }
        }

        println!("== {name}");
        for (kind, (draws, tinted)) in &totals {
            println!("  {kind}: {draws} draw calls placed, {tinted} with a draw-call tint");
        }
        for ((material, tint), s) in &seen {
            let placements: Vec<String> = s
                .placements
                .iter()
                .map(|(p, n)| format!("{p} ×{n}"))
                .collect();
            println!(
                "  {material} ({})\n      {tint}\n      ×{} placed by: {}",
                shader(&content, material),
                s.count,
                placements.join("; ")
            );
        }
    }
}
