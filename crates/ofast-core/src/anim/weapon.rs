//! Which clips CS2 plays for a weapon, found by walking its third-person graphs.
//!
//! The main graph (`animation/graphs/worldmodel/worldmodel.vnmgraph`) selects by two ID parameters,
//! `weapon_category` (`weapon_category_rifle`, `_pistol`, `_knife`, `_smg`, `_shotgun`, `_sniper`, `_machinegun`,
//! `_grenade`, `_equipment`) and `weapon_type` (the item's name in `items_game.txt`: `weapon_ak47`, `weapon_m4a1` for
//! the M4A4, `weapon_knife`…):
//!
//! | Graph state machine | Per weapon | Layer |
//! |---|---|---|
//! | `SM0/Locomotion/Idle Poses` → category → `SM` → weapon | a crouch blend of two poses, `idle_<w>` / `idle_crouch_<w>` (additive) | added over the locomotion, upper body |
//! | `Weapon Shoot` → category → `SM` → weapon | `shoot_<w>` (additive) | added, upper body |
//! | `Weapons/State/Weapons` → category → `SM` → weapon | a referenced graph (`worldmodel_gun.vnmgraph+ak47`, `worldmodel_knife.vnmgraph+default_ct`…) whose `SM/Deploy` and `SM/Reload` states play `draw_<w>` / `draw_crouch_<w>` and `reload_<w>` / `reload_crouch_<w>` | replaces the upper body in model space while it plays |
//!
//! [`WeaponClips::find`] returns the clip paths; the category is the one whose state machine knows the weapon.

use super::AnimError;
use super::graph::Graph;

/// The weapon categories of the main graph, in its own spelling.
pub const CATEGORIES: [&str; 9] = [
    "weapon_category_rifle",
    "weapon_category_pistol",
    "weapon_category_knife",
    "weapon_category_smg",
    "weapon_category_shotgun",
    "weapon_category_sniper",
    "weapon_category_machinegun",
    "weapon_category_grenade",
    "weapon_category_equipment",
];

/// Paths of the clips CS2 plays for one weapon (empty when the graph has none).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaponClips {
    /// The weapon's name (`weapon_ak47`).
    pub weapon: String,
    /// Its category (`weapon_category_rifle`), if the graph knows the weapon.
    pub category: Option<String>,
    /// Additive idle poses, standing then crouched.
    pub idle: Vec<String>,
    /// Additive shot(s).
    pub shoot: Vec<String>,
    /// Draw, standing then crouched.
    pub draw: Vec<String>,
    /// Reload, standing then crouched (and empty-magazine variants when the graph has them).
    pub reload: Vec<String>,
}

impl WeaponClips {
    /// Finds a weapon's clips in the main graph and, for the draw and reload, the weapon graph `load_graph` returns
    /// for a resource path.
    pub fn find(
        main: &Graph,
        weapon: &str,
        mut load_graph: impl FnMut(&str) -> Result<Graph, AnimError>,
    ) -> Result<Self, AnimError> {
        let mut out = Self {
            weapon: weapon.to_owned(),
            ..Self::default()
        };
        let idle_machine = main.find("SM0/Locomotion/Idle Poses");
        // The category whose idle-pose state machine has a state for this weapon.
        for category in CATEGORIES {
            let ids = |parameter: &str| match parameter {
                "weapon_category" => Some(category.to_owned()),
                "weapon_type" => Some(weapon.to_owned()),
                _ => None,
            };
            let Some(state) = idle_machine.and_then(|m| weapon_state(main, m, &ids)) else {
                continue;
            };
            out.category = Some(category.to_owned());
            out.idle = main
                .clips_under(state)
                .into_iter()
                .map(|(p, _)| p)
                .collect();
            // The shoot states also wait for an attack (primary fire).
            let attacking = |parameter: &str| match parameter {
                "action" => Some("action_attack".to_owned()),
                "attack_type" => Some("attack_gun_primaryfire".to_owned()),
                other => ids(other),
            };
            if let Some(state) = main
                .find("Weapon Shoot")
                .and_then(|m| weapon_state(main, m, &attacking))
            {
                out.shoot = main
                    .clips_under(state)
                    .into_iter()
                    .map(|(p, _)| p)
                    .collect();
            }
            if let Some(state) = main
                .find("Weapons/State/Weapons")
                .and_then(|m| weapon_state(main, m, &ids))
                && let Some(path) = referenced_graph_under(main, state)
            {
                let graph = load_graph(&path)?;
                for (state_path, clips) in
                    [("SM/Deploy", &mut out.draw), ("SM/Reload", &mut out.reload)]
                {
                    if let Some(node) = graph.find(state_path) {
                        let mut found: Vec<String> = Vec::new();
                        for (path, _) in graph.clips_under(node) {
                            if !found.contains(&path) {
                                found.push(path);
                            }
                        }
                        *clips = found;
                    }
                }
            }
            break;
        }
        Ok(out)
    }
}

/// Category state → its inner state machine → the weapon's state, under a top-level state machine.
fn weapon_state(
    graph: &Graph,
    machine: usize,
    ids: &dyn Fn(&str) -> Option<String>,
) -> Option<usize> {
    let category = graph.select_state(machine, ids)?;
    let inner = first_state_machine(graph, category, 0)?;
    graph.select_state(inner, ids)
}

/// The first state machine at or under `node` (through single-child nodes).
fn first_state_machine(graph: &Graph, node: usize, depth: usize) -> Option<usize> {
    if depth > 6 {
        return None;
    }
    if graph.class(node) == "CNmStateMachineNode" {
        return Some(node);
    }
    graph
        .child(node, "m_nChildNodeIdx")
        .and_then(|c| first_state_machine(graph, c, depth + 1))
}

/// The graph a referenced-graph node at or under `node` plays.
fn referenced_graph_under(graph: &Graph, node: usize) -> Option<String> {
    let mut current = node;
    for _ in 0..6 {
        if let Some(path) = graph.referenced_graph(current) {
            return Some(path.to_owned());
        }
        current = graph.child(current, "m_nChildNodeIdx")?;
    }
    None
}
