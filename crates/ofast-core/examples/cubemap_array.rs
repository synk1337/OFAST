//! Developer tool: a map's baked cube map array (`maps/<map>/cubemaps/env_cubemap_array.vtex_c`, the reflections
//! its `env_combined_light_probe_volume` and `env_cubemap_box` entities point into): format, size, how many cubes,
//! the mean radiance of each cube, and the entities with their array index.
//!
//! ```bash
//! cargo run --release --example cubemap_array -- de_nuke
//! ```

use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::map::Map;
use ofast_core::resource::Resource;
use ofast_core::texture::Texture;

fn main() {
    let name = std::env::args().nth(1).expect("usage: cubemap_array <map>");
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &name).expect("archives open");
    let path = format!("maps/{name}/cubemaps/env_cubemap_array.vtex_c");
    let data = content.read(&path).expect("cube map array reads");
    let texture =
        Texture::parse(&Resource::parse(&data).expect("resource parses")).expect("texture parses");
    println!(
        "{path}: {}x{} depth {} {:?}, {} mips, flags {:?}, reflectivity {:?}",
        texture.width,
        texture.height,
        texture.depth,
        texture.format,
        texture.mip_count,
        texture.flags,
        texture.reflectivity
    );
    let slices = texture.decode_slices_f32(&data).expect("slices decode");
    println!("{} slices ({} cubes)", slices.len(), slices.len() / 6);
    for (cube, faces) in slices.chunks(6).enumerate() {
        let mut sum = [0.0f64; 4];
        let mut count = 0usize;
        for face in faces {
            for p in face.rgba.chunks(4) {
                for k in 0..4 {
                    sum[k] += f64::from(p[k]);
                }
                count += 1;
            }
        }
        let mean = sum.map(|s| s / count.max(1) as f64);
        // Each face's mean luminance, in the order +X, −X, +Y, −Y, +Z, −Z (the sky's convention: +Z up).
        let faces_luma: Vec<String> = faces
            .iter()
            .map(|f| {
                let n = (f.rgba.len() / 4).max(1) as f32;
                let l: f32 = f
                    .rgba
                    .chunks(4)
                    .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
                    .sum();
                format!("{:.2}", l / n)
            })
            .collect();
        println!(
            "cube {cube:3}: mean rgb {:.3} {:.3} {:.3}  a {:.3}  faces {}",
            mean[0],
            mean[1],
            mean[2],
            mean[3],
            faces_luma.join(" ")
        );
    }
    let map = Map::load(&content, &name).expect("map loads");
    for class in [
        "env_combined_light_probe_volume",
        "env_cubemap_box",
        "env_cubemap",
    ] {
        for e in map.entities_of(class) {
            println!(
                "{class} index {:?} origin {:?} angles {:?} box {:?} .. {:?} priority {:?}",
                e.number("array_index"),
                e.origin(),
                e.angles(),
                e.vector3("box_mins"),
                e.vector3("box_maxs"),
                e.number("priority")
            );
        }
    }
}
