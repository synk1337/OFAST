//! A compiled AnimGraph 2 graph (`.vnmgraph_c`, KV3 in the `DATA` block), read enough to find clips by state.
//!
//! | Key | Value |
//! |---|---|
//! | `m_nodes` | node definitions, each with `_class` (`CNmStateMachineNode::CDefinition`, `CNmClipNode::CDefinition`…) and the indices of the nodes it reads |
//! | `m_nodePaths` | each node's editor path (`Weapons/State/Weapons/Rifles/SM/AK`) |
//! | `m_controlParameterIDs` | the graph's inputs; control parameter `i` is node `i` |
//! | `m_resources` | data slots: clips and referenced graphs |
//! | `m_referencedGraphSlots` | which data slot each `CNmReferencedGraphNode` plays (`m_nNodeIdx`, `m_dataSlotIdx`) |
//!
//! OFAST does not run graphs. It walks them to find which clips CS2 plays for a weapon: a state machine's states
//! carry entry conditions (`m_stateDefinitions[].m_nEntryConditionNodeIdx`) built from ID comparisons on control
//! parameters (`weapon_category in [weapon_category_rifle]`, `weapon_type in [weapon_ak47]`) and `and` / `or` /
//! `not` nodes, which [`Graph::condition`] evaluates for given parameter values.

use super::AnimError;
use crate::kv3::Value;

/// A graph's nodes and tables.
#[derive(Debug, Clone)]
pub struct Graph {
    /// Node definitions (`m_nodes`).
    pub nodes: Vec<Value>,
    /// Editor path of each node.
    pub paths: Vec<String>,
    /// Control parameter names; parameter `i` is node `i`.
    pub parameters: Vec<String>,
    /// Data slots: clip and graph resource paths.
    pub resources: Vec<String>,
    /// `(referenced graph node, data slot)`.
    pub referenced_graphs: Vec<(usize, usize)>,
}

fn strings(root: &Value, key: &'static str) -> Vec<String> {
    root.get(key)
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn index(value: Option<&Value>) -> Option<usize> {
    usize::try_from(value?.as_i64()?).ok()
}

impl Graph {
    /// Reads a graph's `DATA` block.
    pub fn parse(root: &Value) -> Result<Self, AnimError> {
        let nodes = root
            .get("m_nodes")
            .and_then(Value::as_array)
            .ok_or(AnimError::MissingKey("m_nodes"))?
            .to_vec();
        let referenced_graphs = root
            .get("m_referencedGraphSlots")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|slot| {
                Some((
                    index(slot.get("m_nNodeIdx"))?,
                    index(slot.get("m_dataSlotIdx"))?,
                ))
            })
            .collect();
        Ok(Self {
            nodes,
            paths: strings(root, "m_nodePaths"),
            parameters: strings(root, "m_controlParameterIDs"),
            resources: strings(root, "m_resources"),
            referenced_graphs,
        })
    }

    /// A node's class without the `::CDefinition` suffix (`CNmClipNode`), empty if out of range.
    pub fn class(&self, node: usize) -> &str {
        self.nodes
            .get(node)
            .and_then(|n| n.get("_class"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_end_matches("::CDefinition")
    }

    /// The node with this editor path.
    pub fn find(&self, path: &str) -> Option<usize> {
        self.paths.iter().position(|p| p == path)
    }

    /// A node index field (`m_nChildNodeIdx`…).
    pub fn child(&self, node: usize, key: &str) -> Option<usize> {
        index(self.nodes.get(node)?.get(key))
    }

    /// A node's list of indices (`m_sourceNodeIndices`…).
    pub fn children(&self, node: usize, key: &str) -> Vec<usize> {
        self.nodes
            .get(node)
            .and_then(|n| n.get(key))
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| index(Some(v)))
            .collect()
    }

    /// A number field of a node.
    pub fn number(&self, node: usize, key: &str) -> Option<f64> {
        self.nodes.get(node)?.get(key)?.as_f64()
    }

    /// Whether a condition node holds when the ID parameters have the values `ids` gives (`None` for a parameter it
    /// does not know). Conditions on anything else (floats, events, unknown parameters) are false.
    pub fn condition(&self, node: usize, ids: &dyn Fn(&str) -> Option<String>) -> bool {
        match self.class(node) {
            "CNmIDComparisonNode" => {
                let Some(input) = self.child(node, "m_nInputValueNodeIdx") else {
                    return false;
                };
                let Some(value) = self.parameters.get(input).and_then(|p| ids(p)) else {
                    return false;
                };
                let listed = self
                    .nodes
                    .get(node)
                    .and_then(|n| n.get("m_comparisionIDs"))
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .any(|v| v.as_str() == Some(value.as_str()));
                let matches = self
                    .nodes
                    .get(node)
                    .and_then(|n| n.get("m_comparison"))
                    .and_then(Value::as_str)
                    != Some("DoesntMatch");
                listed == matches
            }
            "CNmAndNode" => {
                let all = self.children(node, "m_conditionNodeIndices");
                !all.is_empty() && all.iter().all(|&c| self.condition(c, ids))
            }
            "CNmOrNode" => self
                .children(node, "m_conditionNodeIndices")
                .iter()
                .any(|&c| self.condition(c, ids)),
            "CNmNotNode" => self
                .child(node, "m_nInputValueNodeIdx")
                .is_some_and(|c| !self.condition(c, ids)),
            _ => false,
        }
    }

    /// The states of a state machine: `(state node, entry condition node)`.
    pub fn states(&self, machine: usize) -> Vec<(usize, Option<usize>)> {
        self.nodes
            .get(machine)
            .and_then(|n| n.get("m_stateDefinitions"))
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                Some((
                    index(s.get("m_nStateNodeIdx"))?,
                    index(s.get("m_nEntryConditionNodeIdx")),
                ))
            })
            .collect()
    }

    /// The first state of a state machine whose entry condition holds for `ids`.
    pub fn select_state(
        &self,
        machine: usize,
        ids: &dyn Fn(&str) -> Option<String>,
    ) -> Option<usize> {
        self.states(machine)
            .into_iter()
            .find(|&(_, entry)| entry.is_some_and(|c| self.condition(c, ids)))
            .map(|(state, _)| state)
    }

    /// The clip a clip or pose node plays: `(resource path, speed multiplier)`.
    pub fn clip(&self, node: usize) -> Option<(&str, f32)> {
        match self.class(node) {
            "CNmClipNode" | "CNmAnimationPoseNode" => {
                let slot = self.child(node, "m_nDataSlotIdx")?;
                let speed = self.number(node, "m_flSpeedMultiplier").unwrap_or(1.0) as f32;
                Some((self.resources.get(slot)?.as_str(), speed))
            }
            _ => None,
        }
    }

    /// The graph resource a referenced-graph node plays.
    pub fn referenced_graph(&self, node: usize) -> Option<&str> {
        let slot = self
            .referenced_graphs
            .iter()
            .find(|&&(n, _)| n == node)
            .map(|&(_, slot)| slot)?;
        self.resources.get(slot).map(String::as_str)
    }

    /// Every clip under `node`, depth first (through states, blends, selectors, scales), as `(path, speed)`.
    pub fn clips_under(&self, node: usize) -> Vec<(String, f32)> {
        let mut out = Vec::new();
        self.collect_clips(node, 0, &mut out);
        out
    }

    fn collect_clips(&self, node: usize, depth: usize, out: &mut Vec<(String, f32)>) {
        if depth > 16 {
            return;
        }
        if let Some((path, speed)) = self.clip(node) {
            out.push((path.to_owned(), speed));
            return;
        }
        for key in ["m_nChildNodeIdx", "m_nStateNodeIdx"] {
            if let Some(c) = self.child(node, key) {
                self.collect_clips(c, depth + 1, out);
            }
        }
        for key in ["m_sourceNodeIndices", "m_optionNodeIndices"] {
            for c in self.children(node, key) {
                self.collect_clips(c, depth + 1, out);
            }
        }
        for (state, _) in self.states(node) {
            self.collect_clips(state, depth + 1, out);
        }
    }
}
