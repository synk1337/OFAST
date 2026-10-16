//! Developer tool: decode a cube map texture (e.g. a sky) and write its six faces side by side as a PNG preview
//! (tone-mapped), labelled in stored order +X, −X, +Y, −Y, +Z, −Z.
//!
//! ```bash
//! cargo run --release --example sky_faces -- materials/skybox/sky_de_mirage_exr_71e5f2a1.vtex_c test-data/sky_faces.png
//! ```

use ofast_core::content::Content;
use ofast_core::export::gltf::write_png;
use ofast_core::game::Cs2Install;
use ofast_core::resource::Resource;
use ofast_core::texture::{Image, Texture};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let data = content.read(&args[0]).expect("texture reads");
    let resource = Resource::parse(&data).expect("resource parses");
    let texture = Texture::parse(&resource).expect("texture parses");
    println!(
        "{}x{} {:?} flags {:?}",
        texture.width, texture.height, texture.format, texture.flags
    );
    let faces = texture.decode_slices_f32(&data).expect("faces decode");
    let (w, h) = (faces[0].width, faces[0].height);
    let mut rgba = vec![0u8; w * faces.len() * h * 4];
    let mut max = 0f32;
    for (f, face) in faces.iter().enumerate() {
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let o = (y * w * faces.len() + f * w + x) * 4;
                for c in 0..3 {
                    let v = face.rgba[i + c];
                    max = max.max(v);
                    // Reinhard tone map, then sRGB-ish gamma, for a readable preview.
                    let t = v / (1.0 + v);
                    rgba[o + c] = (t.powf(1.0 / 2.2) * 255.0) as u8;
                }
                rgba[o + 3] = 255;
            }
        }
    }
    println!("{} faces, brightest value {max:.2}", faces.len());
    write_png(
        std::path::Path::new(&args[1]),
        &Image::new(w * faces.len(), h, rgba),
    )
    .expect("png writes");
}
