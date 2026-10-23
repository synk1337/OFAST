//! Developer tool: decode a 2D texture as stored (all four channels, no conversion) and write it as a PNG, and print
//! its size, format and stored average colour (reflectivity), and a sprite sheet's sequences (frames, time, where
//! the first frame sits in pixels).
//!
//! ```bash
//! cargo run --release --example texture_png -- materials/shared_base/plaster/plaster_facade_01_color_tga_8dd9ce15.vtex_c test-data/plaster.png
//! ```

use ofast_core::content::Content;
use ofast_core::export::gltf::write_png;
use ofast_core::game::Cs2Install;
use ofast_core::resource::Resource;
use ofast_core::texture::Texture;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(path), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: texture_png <texture .vtex_c> <out.png>");
        std::process::exit(2);
    };
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let data = content.read(path).expect("texture reads");
    let resource = Resource::parse(&data).expect("resource parses");
    let texture = Texture::parse(&resource).expect("texture parses");
    println!(
        "{}x{} {:?}, average colour (reflectivity) {:?}",
        texture.width, texture.height, texture.format, texture.reflectivity
    );
    let (w, h) = (texture.width as f32, texture.height as f32);
    for s in &texture.sheet {
        let first = s.frames.first().and_then(|f| f.images.first());
        println!(
            "  sequence {} {:?}: {} frames, {} s{}{}, first frame uncropped at {:?} px",
            s.id,
            s.name,
            s.frames.len(),
            s.total_time,
            if s.clamp { ", clamped" } else { "" },
            if s.params.is_empty() {
                String::new()
            } else {
                format!(", {:?}", s.params)
            },
            // In pixels, unrounded: where the corners sit within their texels.
            first.map(|i| [
                i.uncropped[0][0] * w,
                i.uncropped[0][1] * h,
                i.uncropped[1][0] * w,
                i.uncropped[1][1] * h
            ])
        );
    }
    let image = texture.decode_rgba8(&data).expect("texture decodes");
    write_png(std::path::Path::new(out), &image).expect("PNG writes");
}
