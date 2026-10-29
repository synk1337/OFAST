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

/// The main graph and the weapon graphs give each weapon's idle, shoot, draw and reload clips.
#[test]
fn weapon_clips_found_in_the_graphs() {
    use ofast_core::anim::graph::Graph;
    use ofast_core::anim::weapon::WeaponClips;
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");
    let graph = |path: &str| {
        let bytes = vpk
            .read_path(&format!("{path}_c"))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        Graph::parse(
            &Resource::parse(&bytes)
                .unwrap()
                .kv3(BlockKind::DATA)
                .unwrap()
                .root,
        )
    };
    let main = graph("animation/graphs/worldmodel/worldmodel.vnmgraph").expect("main graph");
    let name = |path: &String| path.rsplit('/').next().unwrap_or_default().to_owned();
    for weapon in [
        "weapon_ak47",
        "weapon_m4a1",
        "weapon_glock",
        "weapon_knife",
        "weapon_hegrenade",
        "weapon_awp",
        "weapon_knife_tactical",
        "weapon_knife_survival_bowie",
    ] {
        let clips = WeaponClips::find(&main, weapon, |p| graph(p)).expect("weapon clips");
        println!(
            "{weapon}: {:?}, idle {:?}, shoot {:?}, draw {:?}, reload {:?}",
            clips.category,
            clips.idle.iter().map(name).collect::<Vec<_>>(),
            clips.shoot.iter().map(name).collect::<Vec<_>>(),
            clips.draw.iter().map(name).collect::<Vec<_>>(),
            clips.reload.iter().map(name).collect::<Vec<_>>()
        );
        assert!(clips.category.is_some(), "{weapon} has a category");
    }
    // Which locomotion set each category runs with: the main graph's locomotion state machine.
    let machine = main
        .find_under_top("Locomotion/locomotion")
        .expect("locomotion state machine");
    for category in ofast_core::anim::weapon::CATEGORIES {
        let ids = |p: &str| (p == "weapon_category").then(|| category.to_owned());
        let state = main.select_state(machine, &ids);
        println!(
            "{category}: locomotion {:?}",
            state.map(|s| main.paths[s].clone())
        );
    }
    let ak = WeaponClips::find(&main, "weapon_ak47", |p| graph(p)).unwrap();
    assert_eq!(ak.category.as_deref(), Some("weapon_category_rifle"));
    assert!(ak.idle.iter().any(|p| p.ends_with("/idle_ak.vnmclip")));
    assert!(ak.shoot.iter().any(|p| p.ends_with("/shoot_ak.vnmclip")));
    assert!(ak.draw.iter().any(|p| p.ends_with("/draw_ak.vnmclip")));
    assert!(ak.reload.iter().any(|p| p.ends_with("/reload_ak.vnmclip")));
}

/// CS2's own graphs, evaluated by `anim::eval` for the states the bake decides (M4.4c): each plays the clip its
/// states should — knife swings and their swing-back variants, grenade throws by strength, draws, jumps moving on
/// to the landing blend, starts by direction, the defuse's enter then loop — and transitions take the graphs' times.
#[test]
fn graph_states_play_their_clips() {
    use ofast_core::anim::AnimError;
    use ofast_core::anim::eval::{Evaluator, Library, Params};
    use ofast_core::anim::graph::Graph;
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
    let knife = "animation/graphs/worldmodel/worldmodel_knife.vnmgraph+tactical.vnmgraph";
    let grenade = "animation/graphs/worldmodel/worldmodel_grenade.vnmgraph+he.vnmgraph";
    let locomotion = "animation/graphs/worldmodel/worldmodel_locomotion.vnmgraph+knife.vnmgraph";
    let main = "animation/graphs/worldmodel/worldmodel.vnmgraph";
    let glock = "animation/graphs/worldmodel/worldmodel_gun.vnmgraph+glock.vnmgraph";
    let mut library = Library::new();
    let mut load_graph = |p: &str| Graph::parse(&data(p));
    let mut load_clip = |p: &str| -> Result<Clip, AnimError> { Clip::parse(&data(p)) };
    for graph in [knife, grenade, locomotion, glock] {
        library
            .load(graph, None, &mut load_graph, &mut load_clip)
            .expect(graph);
    }
    let defuse = library
        .load_graph(main, &mut load_graph)
        .unwrap()
        .find("Defuse/Defuse")
        .expect("defuse state");
    library
        .load(main, Some(defuse), &mut load_graph, &mut load_clip)
        .expect("defuse clips");
    let layers: Vec<usize> = [
        "BodyFlinch/Flinch_WPNs",
        "HeadFlinch/Flinch_WPNs",
        "flashed/flashed",
    ]
    .iter()
    .map(|path| library.graph(main).unwrap().find(path).expect(path))
    .collect();
    for &node in &layers {
        library
            .load(main, Some(node), &mut load_graph, &mut load_clip)
            .expect("layer clips");
    }
    let eval = Evaluator::new(&library);
    // The clip a node plays `time` seconds in, by file name.
    let playing_node = |graph: &str, node: usize, params: &Params, time: f32| -> String {
        let (clip, _) = eval
            .dominant_clip(graph, node, params, time)
            .unwrap()
            .unwrap_or_else(|| panic!("{graph}: nothing plays"));
        let path = library
            .graph(graph)
            .unwrap()
            .resources
            .iter()
            .find(|p| library.clip(p).is_some_and(|c| std::ptr::eq(c, clip)))
            .cloned()
            .unwrap_or_default();
        path.rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches(".vnmclip")
            .to_owned()
    };
    // The same for a graph's root, or for the defuse state in the main graph.
    let playing = |graph: &str, params: &Params, time: f32| -> String {
        let node = eval.root(graph).unwrap_or(defuse);
        let node = if graph == main { defuse } else { node };
        playing_node(graph, node, params, time)
    };

    let attack = |kind: &str| {
        Params::default()
            .id("action", "action_attack")
            .id("attack_type", kind)
            .id("weapon_type", "weapon_knife_tactical")
            .id("weapon_category", "weapon_category_knife")
    };
    assert_eq!(
        playing(knife, &attack("attack_knife_lightmiss"), 0.1),
        "frontswing_knife"
    );
    assert_eq!(
        playing(
            knife,
            &attack("attack_knife_lightmiss").event("KnifeSwingBack"),
            0.1
        ),
        "frontswing_b_knife"
    );
    assert_eq!(
        playing(knife, &attack("attack_knife_heavybackstab"), 0.1),
        "backstab_overhead_knife"
    );
    assert_eq!(
        playing(
            knife,
            &attack("attack_knife_heavymiss").float("move_crouch_amount_eased", 1.0),
            0.1
        ),
        "frontswing_crouching_knife"
    );
    let deploy = attack("").id("action", "action_deploy");
    assert_eq!(playing(knife, &deploy, 0.1), "draw_tactical");
    let idle = attack("").id("action", "action_idle");
    assert!(
        eval.pose(knife, eval.root(knife).unwrap(), &idle, 0.1)
            .unwrap()
            .is_none(),
        "Off plays nothing"
    );
    // Off → Deploy blends in 0.1 s, Deploy → Off 0.2 s, Off → attack at once.
    let states = |graph: &str, params: &Params| {
        eval.states(graph, eval.root(graph).unwrap(), params, 0.0)
            .unwrap()
    };
    assert_eq!(
        eval.transition_time(&states(knife, &idle), &states(knife, &deploy), &deploy),
        Some(0.1)
    );
    assert!(
        (eval
            .transition_time(&states(knife, &deploy), &states(knife, &idle), &idle)
            .unwrap()
            - 0.2)
            .abs()
            < 1e-6
    );
    let swing = attack("attack_knife_lightmiss");
    assert_eq!(
        eval.transition_time(&states(knife, &idle), &states(knife, &swing), &swing),
        Some(0.0)
    );

    let throw = |kind: &str, strength: f32| {
        Params::default()
            .id("action", "action_attack")
            .id("attack_type", kind)
            .id("weapon_type", "weapon_hegrenade")
            .id("weapon_category", "weapon_category_grenade")
            .float("attack_throw_strength", strength)
    };
    assert_eq!(
        playing(grenade, &throw("attack_grenade_charge", 1.0), 0.1),
        "pullpin_grenade"
    );
    assert_eq!(
        playing(grenade, &throw("attack_grenade_throw", 1.0), 0.1),
        "throw_overhand_grenade"
    );
    assert_eq!(
        playing(grenade, &throw("attack_grenade_throw", 0.2), 0.1),
        "throw_underhand_grenade"
    );

    // A reload from an empty magazine plays the empty variant.
    let reload = |ammo: f32| {
        Params::default()
            .id("action", "action_reload")
            .id("weapon_type", "weapon_glock")
            .id("weapon_category", "weapon_category_pistol")
            .float("weapon_ammo", ammo)
    };
    assert_eq!(playing(glock, &reload(5.0), 0.5), "reload_glock");
    assert_eq!(playing(glock, &reload(0.0), 0.5), "reload_empty_glock");

    // A jump plays its clip (0.4 s), then the landing blend's in-air poses.
    let air = Params::default()
        .id("move_type", "move_type_air")
        .id("air_action", "air_action_jump")
        .float("move_speed_x", 225.0)
        .float("air_height_above_ground", 30.0);
    assert_eq!(playing(locomotion, &air, 0.1), "jump_n_knife");
    let landing = eval
        .states(locomotion, eval.root(locomotion).unwrap(), &air, 0.6)
        .unwrap();
    let paths: Vec<&str> = landing
        .iter()
        .map(|(g, n)| library.graph(g).unwrap().paths[*n].as_str())
        .collect();
    assert!(paths.contains(&"SM/InAir/SM/landing_blend"), "{paths:?}");
    let ground = Params::default()
        .id("move_type", "move_type_ground")
        .id("ground_action", "ground_action_move");
    let into_air = eval.transition_time(
        &states(locomotion, &ground),
        &states(locomotion, &air),
        &air,
    );
    let onto_ground = eval.transition_time(
        &states(locomotion, &air),
        &states(locomotion, &ground),
        &ground,
    );
    // Into the air: the transition's own duration is 0, its override (`m_nDurationOverrideNodeIdx`) 0.1 s for a jump
    // and 0.3 s for a fall.
    assert!((into_air.unwrap() - 0.1).abs() < 1e-6, "{into_air:?}");
    let fall = air.clone().id("air_action", "air_action_start_fall");
    let into_fall = eval.transition_time(
        &states(locomotion, &ground),
        &states(locomotion, &fall),
        &fall,
    );
    assert!((into_fall.unwrap() - 0.3).abs() < 1e-6, "{into_fall:?}");
    assert!((onto_ground.unwrap() - 0.2).abs() < 1e-6, "{onto_ground:?}");
    // Starts by direction.
    let start = |direction: &str| {
        Params::default()
            .id("move_type", "move_type_ground")
            .id("ground_action", "ground_action_start")
            .id("ground_action_direction_id", direction)
    };
    assert_eq!(playing(locomotion, &start("NW"), 0.1), "planted_start_nw");
    assert_eq!(playing(locomotion, &start("E"), 0.1), "planted_start_e");
    assert!(
        (eval
            .duration(locomotion, eval.root(locomotion).unwrap(), &start("N"))
            .unwrap()
            - 0.533)
            .abs()
            < 0.01
    );

    // Defusing with a rifle: the enter clip, then the loop.
    let defusing = Params::default()
        .flag("is_defusing", true)
        .id("weapon_category", "weapon_category_rifle")
        .id("weapon_type", "weapon_ak47");
    assert_eq!(playing(main, &defusing, 0.1), "defuse_enter_rifle");
    let enter = eval.duration(main, defuse, &defusing).unwrap();
    println!("defuse enter: {enter:.2} s");
    assert_eq!(playing(main, &defusing, enter + 0.1), "defuse_loop_rifle");

    // Flinches by hit and weapon, and the flashed pose.
    let hit = |category: &str, key: &str, kind: &str| {
        Params::default()
            .id("weapon_category", category)
            .id(key, kind)
    };
    assert_eq!(
        playing_node(
            main,
            layers[0],
            &hit(
                "weapon_category_rifle",
                "flinch_body_type",
                "flinch_body_chest_south"
            ),
            0.1
        ),
        "flinch_chest_rear"
    );
    assert_eq!(
        playing_node(
            main,
            layers[0],
            &hit(
                "weapon_category_pistol",
                "flinch_body_type",
                "flinch_body_leg_left"
            ),
            0.1
        ),
        "flinch_leg_left_pistol"
    );
    assert_eq!(
        playing_node(
            main,
            layers[1],
            &hit(
                "weapon_category_knife",
                "flinch_head_type",
                "flinch_head_west"
            ),
            0.1
        ),
        "flinch_head_left_knife"
    );
    assert!(
        eval.pose(main, layers[2], &Params::default(), 0.0)
            .unwrap()
            .is_some()
    );
}

/// `items_game.txt` names the weapons the demos report by index, with the names the animation graphs select on.
#[test]
fn item_names_read() {
    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");
    let text = String::from_utf8(vpk.read_path("scripts/items/items_game.txt").unwrap()).unwrap();
    let names = ofast_core::items::item_names(&text);
    println!("{} item names", names.len());
    for (index, name) in [
        (1, "weapon_deagle"),
        (4, "weapon_glock"),
        (7, "weapon_ak47"),
        (16, "weapon_m4a1"),
        (42, "weapon_knife"),
        (44, "weapon_hegrenade"),
        (49, "weapon_c4"),
        (59, "weapon_knife_t"),
        (60, "weapon_m4a1_silencer"),
        (61, "weapon_usp_silencer"),
    ] {
        assert_eq!(
            names.get(&index).map(String::as_str),
            Some(name),
            "item {index}"
        );
    }
    for index in [509, 514] {
        println!("  {index}: {:?}", names.get(&index));
    }
}

/// Every agent and glove model's procedural-bone constraints (`m_keyValueText` → `BoneConstraintList`): all of
/// them read, every bone hash resolves, only the eyes' morph constraints are left out, and applied to the bind pose
/// they change nothing — which pins down every offset and axis convention at once. Then, on `ctm_sas`, rolling the
/// right hand about its forearm turns the forearm's twist bones by their share of the roll.
#[test]
fn procedural_bones_keep_the_bind_pose() {
    use ofast_core::anim::BoneConstraints;
    use ofast_core::model::{Model, affine, compose, invert_rigid};

    let install = Cs2Install::locate().expect("CS2 install found");
    let vpk = Vpk::open(install.main_vpk()).expect("pak01_dir.vpk opens");
    let paths: Vec<String> = vpk
        .entries()
        .iter()
        .map(|e| e.path().to_owned())
        .filter(|p| p.starts_with("agents/models/") && p.ends_with(".vmdl_c"))
        .collect();
    let load = |path: &str| {
        let bytes = vpk.read_path(path).unwrap();
        Model::parse(&Resource::parse(&bytes).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"))
    };
    let (mut models, mut applied, mut worst) = (0usize, 0usize, (0.0f32, String::new()));
    for path in &paths {
        let model = load(path);
        let constraints = BoneConstraints::of(&model);
        // Glove models carry their agent's list over an arm-only skeleton: the head's constraints find no bone, the
        // upper arms' twists no parent to measure against.
        let gloves = path.contains("/arms/");
        for skipped in &constraints.skipped {
            assert!(
                skipped.contains("CBoneConstraintDotToMorph")
                    || gloves && (skipped.contains("no bone") || skipped.contains("no parent")),
                "{path}: {skipped}"
            );
        }
        if constraints.is_empty() {
            continue;
        }
        models += 1;
        applied += constraints.len();
        let bind = model.skeleton.world_transforms();
        let mut world = bind.clone();
        constraints.apply(&mut world);
        for (bone, (after, before)) in world.iter().zip(&bind).enumerate() {
            // Positions in inches, rotation matrix entries.
            let difference = (0..3)
                .flat_map(|i| (0..4).map(move |j| (i, j)))
                .map(|(i, j)| (after[i][j] - before[i][j]).abs())
                .fold(0.0f32, f32::max);
            let name = &model.skeleton.bones[bone].name;
            // Every agent shares one list, authored on a skeleton whose hands sat 6.6° from theirs (almost all of
            // it tilt, which the twist ignores): the twist left over turns the forearms' twist bones by up to
            // 0.66° at bind pose, in CS2 too. Nothing else may move.
            if !name.contains("TWIST") {
                assert!(difference < 2e-3, "{path} {name} moved by {difference}");
            }
            if difference > worst.0 {
                worst = (difference, format!("{path} {name}"));
            }
        }
    }
    println!(
        "{models} models with constraints, {applied} applied; largest change of the bind pose {:.2e} ({})",
        worst.0, worst.1
    );
    assert!(models >= 80, "CS2's agents and gloves carry constraints");
    assert!(worst.0 < 0.02, "the bind pose is kept: {worst:?}");

    // The right hand rolled 60° about its own x (the forearm's axis): the forearm's twist bones take their share.
    let sas = load("agents/models/ctm_sas/ctm_sas.vmdl_c");
    let constraints = BoneConstraints::of(&sas);
    let bones = &sas.skeleton.bones;
    let find = |name: &str| sas.skeleton.find(name).unwrap_or_else(|| panic!("{name}"));
    let mut world = sas.skeleton.world_transforms();
    let hand = find("hand_R");
    let roll = affine(
        [
            (30f32).to_radians().sin(),
            0.0,
            0.0,
            (30f32).to_radians().cos(),
        ],
        [0.0; 3],
    );
    world[hand] = compose(&world[hand], &roll);
    constraints.apply(&mut world);
    let bind = sas.skeleton.world_transforms();
    // How far a twist bone turned about its own x (signed, degrees), against its bind pose.
    let turned = |name: &str| {
        let bone = find(name);
        let parent = bones[bone].parent.expect("parent");
        let before = compose(&invert_rigid(&bind[parent]), &bind[bone]);
        let after = compose(&invert_rigid(&world[parent]), &world[bone]);
        // `before`ᵀ · `after`: the turn between the two local rotations.
        let delta = |i: usize, j: usize| (0..3).map(|k| before[k][i] * after[k][j]).sum::<f32>();
        delta(2, 1).atan2(delta(1, 1)).to_degrees()
    };
    let (twist, twist1) = (turned("arm_lower_R_TWIST"), turned("arm_lower_R_TWIST1"));
    println!("hand_R rolled 60°: arm_lower_R_TWIST {twist:.1}°, arm_lower_R_TWIST1 {twist1:.1}°");
    // Weights 0.5 and 1, the same way round as the hand (within the bind pose's 0.66°).
    assert!((twist - 30.0).abs() < 1.5, "{twist}");
    assert!((twist1 - 60.0).abs() < 1.5, "{twist1}");
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
