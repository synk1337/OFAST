//! Developer tool: the model entities a demo has solid at one tick, and whether their models' collision stops
//! bullets. Used to find what besides the world's collision (and the rotating doors) bullets meet: `func_brush`
//! blockers, `prop_dynamic` doors and the like.
//!
//! ```bash
//! # round 5, 20 s after its start: a census by class, then each solid entity whose model stops bullets
//! cargo run --release --example solid_entities -- path/to/match.dem 5 20
//! # only those within 300 units of a point
//! cargo run --release --example solid_entities -- path/to/match.dem 5 20 984,80,-416 300
//! ```
//!
//! Per entity: its class, index, model, origin and angles, `m_Collision`'s solid type (0 none, 1 BSP, 2 box,
//! 3 oriented box, 6 the model's physics…) and flags (4: not solid, 8: a trigger), its collision group and
//! interaction layers (`m_nInteractsAs`, a bit mask), `m_fEffects` (32: not drawn), and its model's `PHYS`: how many
//! hulls and triangles stop bullets ([`CollisionAttribute::stops_bullets`]), and its collision groups and tags.

use std::collections::{BTreeMap, HashMap};

use ofast_core::content::Content;
use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::players::list_rounds;
use ofast_core::game::Cs2Install;
use ofast_core::physics::PhysicsData;
use ofast_core::physics::ragdoll::WorldCollision;
use ofast_core::resource::Resource;

const MODEL: &str = "CBodyComponent.m_skeletonInstance.m_modelState.m_hModel";

/// A model's bullet-stopping hulls and triangles, and its collision attributes as "group[tags]".
fn physics_summary(content: &Content, path: &str) -> String {
    let physics = content
        .read_resource(path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            let resource = Resource::parse(&bytes).map_err(|e| e.to_string())?;
            PhysicsData::from_resource(&resource).map_err(|e| e.to_string())
        });
    match physics {
        Ok(Some(physics)) => {
            let (hulls, triangles) =
                WorldCollision::from_physics_where(&physics, |a| a.stops_bullets()).counts();
            let attributes: Vec<String> = physics
                .attributes
                .iter()
                .map(|a| format!("{}{:?}", a.group, a.interact_as))
                .collect();
            format!("bullets: {hulls} hulls {triangles} triangles; {attributes:?}")
        }
        Ok(None) => "no PHYS".into(),
        Err(e) => format!("unreadable: {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let round: u32 = args.get(1).map_or(1, |r| r.parse().expect("round number"));
    let seconds: f32 = args.get(2).map_or(10.0, |s| s.parse().expect("seconds"));
    let near: Option<([f32; 3], f32)> = args.get(3).map(|p| {
        let v: Vec<f32> = p.split(',').map(|s| s.parse().expect("x,y,z")).collect();
        let radius = args
            .get(4)
            .map(|r| r.parse().expect("radius"))
            .unwrap_or(300.0);
        ([v[0], v[1], v[2]], radius)
    });

    let mut parser = DemoParser::new(&data).expect("demo header");
    let map = parser.header().map_name().to_owned();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::for_map(&install, &map).expect("map archive opens");
    let models = content.resource_paths("vmdl");

    let info = list_rounds(&data)
        .expect("rounds")
        .into_iter()
        .find(|r| r.round == round)
        .expect("round in the demo");
    let target = info.start_tick + (seconds * 64.0) as u32;
    while let Some(frame) = parser.next_frame().expect("frame") {
        if frame.tick.is_some_and(|t| t >= target) && frame.entities_changed {
            break;
        }
    }
    println!("{map}, round {round}, tick {target}");
    let serializers = parser.serializers().expect("send tables");
    let classes = parser.classes();
    let field = |class_id: u32, names: &[&str]| {
        let serializer = classes.get(class_id as usize)?.as_ref()?.serializer;
        names
            .iter()
            .find_map(|name| serializers.path_of(serializer, name))
    };
    let mut census: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    let mut summaries: HashMap<String, String> = HashMap::new();
    let mut rows = Vec::new();
    for entity in parser.entities().iter() {
        let class = parser.class_name(entity.class_id).unwrap_or("?").to_owned();
        let get = |names: &[&str]| field(entity.class_id, names).and_then(|p| entity.get(&p));
        let number = |names: &[&str]| get(names).and_then(FieldValue::as_u64);
        let (Some(id), Some(solid)) = (number(&[MODEL]), number(&["m_Collision.m_nSolidType"]))
        else {
            continue;
        };
        let flags = number(&["m_Collision.m_usSolidFlags"]).unwrap_or(0);
        let counts = census.entry(class.clone()).or_default();
        counts[0] += 1;
        if solid == 0 || flags & 4 != 0 {
            continue;
        }
        counts[1] += 1;
        let axis = |k: usize| -> Option<f32> {
            let xyz = ["X", "Y", "Z"][k];
            let cell = number(&[
                &format!("CBodyComponent.m_cell{xyz}"),
                &format!("CBodyComponent.m_skeletonInstance.m_vecOrigin.m_cell{xyz}"),
            ])? as f32;
            let offset = get(&[
                &format!("CBodyComponent.m_vec{xyz}"),
                &format!("CBodyComponent.m_skeletonInstance.m_vecOrigin.m_vec{xyz}"),
            ])
            .and_then(FieldValue::as_f32)
            .unwrap_or(0.0);
            Some(cell * 512.0 - 16384.0 + offset)
        };
        let origin = [axis(0), axis(1), axis(2)].map(|c| c.unwrap_or(f32::NAN));
        if let Some((point, radius)) = near
            && (0..3)
                .map(|k| (origin[k] - point[k]).powi(2))
                .sum::<f32>()
                .sqrt()
                > radius
        {
            continue;
        }
        let angles = get(&[
            "CBodyComponent.m_angRotation",
            "CBodyComponent.m_skeletonInstance.m_angRotation",
        ])
        .and_then(FieldValue::as_vector3)
        .unwrap_or_default();
        let path = models
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("{id:016x}"));
        let summary = summaries
            .entry(path.clone())
            .or_insert_with(|| physics_summary(&content, &path))
            .clone();
        if summary.starts_with("bullets: 0 hulls 0 triangles") {
            continue;
        }
        counts[2] += 1;
        rows.push(format!(
            "#{:<5} {class:<24} {path}\n        at {:?} turned {:?}; solid {solid} flags {flags} group {} \
             interacts as {:#x} effects {}\n        {summary}",
            entity.index,
            origin.map(|c| c.round()),
            angles.map(|c| c.round()),
            number(&["m_Collision.m_collisionAttribute.m_nCollisionGroup"]).unwrap_or(0),
            number(&["m_Collision.m_collisionAttribute.m_nInteractsAs"]).unwrap_or(0),
            number(&["m_fEffects"]).unwrap_or(0),
        ));
    }
    println!("class: entities with a model, solid, solid with collision that stops bullets");
    for (class, [all, solid, bullets]) in &census {
        println!("  {class:<28} {all:>4} {solid:>4} {bullets:>4}");
    }
    for row in rows {
        println!("{row}");
    }
}
