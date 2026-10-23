//! Developer tool: what a player's pawn networks about its death and ragdoll, tick by tick — the life state, the
//! death time, the ragdoll flags, the damage that killed it (bone, force, position, weapon), the server's ragdoll
//! origin and the networked ragdoll pose (`m_RagdollPose.m_Transforms`) — printed when they change.
//!
//! ```bash
//! cargo run --release --example ragdoll_probe -- path/to/match.dem 34380 34700 KSCERATO
//! ```
//!
//! Arguments: demo, first and last demo tick, player name.

use ofast_core::demo::DemoParser;
use ofast_core::demo::decoder::FieldValue;
use ofast_core::demo::entity::handle_to_index;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0]).expect("demo reads");
    let (first, last): (u32, u32) = (
        args[1].parse().expect("first tick"),
        args[2].parse().expect("last tick"),
    );
    let name = &args[3];
    let mut parser = DemoParser::new(&data).expect("demo header");
    let mut previous: Vec<(String, String)> = Vec::new();
    while let Some(frame) = parser.next_frame().expect("frame parses") {
        let Some(tick) = frame.tick else { continue };
        if tick < first || !frame.entities_changed {
            continue;
        }
        if tick > last {
            break;
        }
        let serializers = parser.serializers().expect("serializers");
        let controller_class = serializers
            .by_name("CCSPlayerController")
            .expect("controller");
        let pawn_class = serializers.by_name("CCSPlayerPawn").expect("pawn");
        let path = |class, field: &str| serializers.path_of(class, field);
        let entities = parser.entities();
        let Some(controller) = entities.iter().find(|e| {
            parser.class_name(e.class_id) == Some("CCSPlayerController")
                && path(controller_class, "m_iszPlayerName")
                    .and_then(|p| e.get(&p))
                    .and_then(|v| match v {
                        FieldValue::String(s) => Some(s.as_ref() == name.as_str()),
                        _ => None,
                    })
                    == Some(true)
        }) else {
            continue;
        };
        let Some(pawn) = path(controller_class, "m_hPlayerPawn")
            .and_then(|p| controller.get(&p))
            .and_then(|v| match v {
                FieldValue::Unsigned(h) => Some(*h),
                _ => None,
            })
            .and_then(handle_to_index)
            .and_then(|i| entities.get(i))
        else {
            continue;
        };
        let mut now: Vec<(String, String)> = Vec::new();
        for field in [
            "m_lifeState",
            "m_flDeathTime",
            "m_bRagdollEnabled",
            "m_bRagdollClientSide",
            "m_bClientSideRagdoll",
            "m_nRagdollDamageBone",
            "m_vRagdollDamageForce",
            "m_vRagdollDamagePosition",
            "m_szRagdollDamageWeaponName",
            "m_bRagdollDamageHeadshot",
            "m_vRagdollServerOrigin",
            "m_vecForce",
            "m_qDeathEyeAngles",
        ] {
            let value = path(pawn_class, field)
                .and_then(|p| pawn.get(&p))
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| "-".into());
            now.push((field.to_owned(), value));
        }
        // The networked ragdoll pose: every field below `m_RagdollPose.m_Transforms`.
        if let Some(root) = path(pawn_class, "m_RagdollPose.m_Transforms") {
            let prefix = root.indices().to_vec();
            let mut transforms: Vec<String> = pawn
                .fields()
                .filter(|(p, _)| p.indices().starts_with(&prefix))
                .map(|(p, v)| format!("{:?}={v:?}", &p.indices()[prefix.len()..]))
                .collect();
            transforms.sort();
            let shown = transforms.len().min(4);
            now.push((
                "m_RagdollPose.m_Transforms".into(),
                format!(
                    "{} fields; first: {}",
                    transforms.len(),
                    transforms[..shown].join("; ")
                ),
            ));
        }
        for (field, value) in &now {
            if !previous.iter().any(|(f, v)| f == field && v == value) {
                println!("{tick:6} {field} = {value}");
            }
        }
        previous = now;
    }
}
