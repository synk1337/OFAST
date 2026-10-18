//! Oracle tests for the animation clip decoder against the developer's real CS2 install.
//!
//! Every `.vnmclip_c` in the main archive is decoded, nested (secondary) clips included. A misread layout shows up
//! three ways: frames that do not tile the pose stream exactly (each frame holds 3 values per dynamic rotation,
//! 3 per dynamic translation, 1 per dynamic scale), rotations that are not unit quaternions, or a stream that runs
//! out before the last frame.
//!
//! Only runs with `cargo test --release --features game-data --test cs2_anim -- --nocapture`.

#![cfg(feature = "game-data")]

use std::collections::BTreeMap;

use ofast_core::anim::Clip;
use ofast_core::game::Cs2Install;
use ofast_core::resource::{BlockKind, Resource};
use ofast_core::vpk::Vpk;

/// `u16` values one frame of `clip` takes in the pose stream.
fn frame_size(clip: &Clip) -> usize {
    clip.tracks
        .iter()
        .map(|t| {
            3 * usize::from(!t.rotation_static)
                + 3 * usize::from(!t.translation_static)
                + usize::from(!t.scale_static)
        })
        .sum()
}

/// Checks one clip and its nested clips; returns how many clips were checked.
fn check(path: &str, clip: &Clip, worst_norm: &mut f32) -> usize {
    let size = frame_size(clip);
    assert_eq!(
        clip.offsets.len(),
        clip.frames,
        "{path}: one offset per frame"
    );
    for (f, &offset) in clip.offsets.iter().enumerate() {
        assert_eq!(
            offset,
            f * size,
            "{path}: frame {f} starts after the previous one"
        );
    }
    assert_eq!(
        clip.data.len(),
        clip.frames * size,
        "{path}: frames fill the stream"
    );
    for f in 0..clip.frames {
        let pose = clip
            .decode_frame(f)
            .unwrap_or_else(|e| panic!("{path} frame {f}: {e}"));
        assert_eq!(pose.len(), clip.tracks.len());
        for bone in pose {
            let norm = bone.rotation.iter().map(|c| c * c).sum::<f32>().sqrt();
            *worst_norm = worst_norm.max((norm - 1.0).abs());
        }
    }
    1 + clip
        .secondary
        .iter()
        .map(|nested| check(path, nested, worst_norm))
        .sum::<usize>()
}

/// The three locomotion graphs give the moving-on-the-ground blends described in `anim::locomotion`: standing =
/// idle + 8 runs at 225 units/s + 8 walks at 136, crouched = crouch idle + 8 crouch walks at 96, all clips decoded.
#[test]
fn locomotion_graphs_read() {
    use ofast_core::anim::Locomotion;
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");
    let data = |path: &str| {
        let bytes = vpk
            .read_path(&format!("{path}_c"))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        Resource::parse(&bytes)
            .unwrap()
            .kv3(BlockKind::DATA)
            .unwrap()
            .root
    };
    for set in ["rifle", "pistol", "knife"] {
        let graph = data(&format!(
            "animation/graphs/worldmodel/worldmodel_locomotion.vnmgraph+{set}.vnmgraph"
        ));
        let loco = Locomotion::from_graph(&graph, |path: &str| Clip::parse(&data(path)))
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        let name = |space: &ofast_core::anim::locomotion::Blend2D, i: usize| {
            loco.clips[space.sources[i].clip]
                .0
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        println!(
            "{set}: variation {}, {} clips, standing {} points, crouched {} points",
            loco.variation,
            loco.clips.len(),
            loco.standing.points.len(),
            loco.crouched.points.len()
        );
        assert_eq!(loco.standing.points.len(), 17);
        assert_eq!(loco.crouched.points.len(), 9);
        assert_eq!(loco.standing.points[1], [225.0, 0.0]);
        assert_eq!(name(&loco.standing, 1), format!("run_n_{set}.vnmclip"));
        assert_eq!(name(&loco.standing, 9), format!("walk_n_{set}.vnmclip"));
        assert_eq!(loco.crouched.points[1], [96.0, 0.0]);
        assert_eq!(name(&loco.crouched, 1), format!("crouch_n_{set}.vnmclip"));
        assert_eq!(name(&loco.standing, 0), format!("idle_{set}.vnmclip"));
        // A run forward at full speed is the run clip alone, one cycle in its own length.
        let run = loco.blend([225.0, 0.0], 0.0);
        assert_eq!(run.clips.len(), 1);
        assert!((run.duration - 0.7333).abs() < 1e-3, "{}", run.duration);
    }
}

#[test]
fn every_clip_decodes() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");
    let paths: Vec<String> = vpk
        .entries()
        .iter()
        .map(|e| e.path().to_owned())
        .filter(|p| p.ends_with(".vnmclip_c"))
        .collect();
    assert!(paths.len() > 1000, "CS2 has thousands of clips");

    let (mut clips, mut additive, mut frames) = (0usize, 0usize, 0usize);
    let mut worst_norm = 0.0f32;
    let mut skeletons: BTreeMap<String, usize> = BTreeMap::new();
    for path in &paths {
        let bytes = vpk.read_path(path).unwrap();
        let resource = Resource::parse(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let data = resource
            .kv3(BlockKind::DATA)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let clip = Clip::parse(&data.root).unwrap_or_else(|e| panic!("{path}: {e}"));
        clips += check(path, &clip, &mut worst_norm);
        additive += usize::from(clip.is_additive());
        frames += clip.frames;
        *skeletons.entry(clip.skeleton.clone()).or_default() += 1;
    }
    println!(
        "{} clip files ({clips} clips with nested ones), {additive} additive, {frames} frames, largest |rotation \
         length − 1| {worst_norm:.2e}",
        paths.len()
    );
    for (skeleton, count) in skeletons.iter().rev().take(12) {
        println!("  {count:5}  {skeleton}");
    }
    assert!(worst_norm < 1e-3, "rotations decode to unit quaternions");
}
