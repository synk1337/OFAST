//! Developer tool: how CS2's character and weapon materials store roughness — which tells which texture channel the
//! glTF export must read.
//!
//! ```bash
//! cargo run --release --example material_channels
//! ```
//!
//! For every `csgo_character` and `csgo_weapon` material in the game: the `g_tNormal` texture's format (a
//! two-channel one, ATI2N / BC5, has no blue channel and so no roughness in it), whether the material has a
//! `g_tAnisoGloss` texture (anisotropic roughness, two directions in R and G), and the format of `g_tMetalness`.
//! Prints the combinations and how many materials have each, with an example.

use std::collections::BTreeMap;

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::kv3::Value;
use ofast_core::resource::{BlockKind, Resource};
use ofast_core::texture::Texture;

fn main() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let format = |path: &str| -> String {
        let Ok(bytes) = content.read_resource(path) else {
            return "missing".into();
        };
        let Ok(resource) = Resource::parse(&bytes) else {
            return "unparsed".into();
        };
        Texture::parse(&resource).map_or("unparsed".into(), |t| format!("{:?}", t.format))
    };
    let mut combos: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let paths: Vec<String> = content.archives()[0]
        .entries()
        .iter()
        .map(|e| e.path().to_owned())
        .filter(|p| p.ends_with(".vmat_c"))
        .collect();
    for path in paths {
        let Ok(bytes) = content.read(&path) else {
            continue;
        };
        let Ok(resource) = Resource::parse(&bytes) else {
            continue;
        };
        let Ok(data) = resource.kv3(BlockKind::DATA) else {
            continue;
        };
        let root = data.root;
        let shader = root
            .get("m_shaderName")
            .and_then(Value::as_str)
            .unwrap_or("");
        if shader != "csgo_character.vfx" && shader != "csgo_weapon.vfx" {
            continue;
        }
        let textures: BTreeMap<String, String> = root
            .get("m_textureParams")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                Some((
                    t.get("m_name")?.as_str()?.to_owned(),
                    t.get("m_pValue")?.as_str()?.to_owned(),
                ))
            })
            .collect();
        let normal = textures
            .get("g_tNormal")
            .map_or("none".into(), |p| format(p));
        let metal = textures
            .get("g_tMetalness")
            .map_or("none".into(), |p| format(p));
        let key = format!(
            "{shader:20} normal {normal:8} aniso {:5} metalness {metal}",
            textures.contains_key("g_tAnisoGloss")
        );
        let entry = combos.entry(key).or_insert((0, path.clone()));
        entry.0 += 1;
    }
    for (key, (count, example)) in combos {
        println!("{count:5}  {key}   e.g. {example}");
    }
}
