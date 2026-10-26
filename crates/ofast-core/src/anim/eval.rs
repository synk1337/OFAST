//! Evaluating the parts of CS2's compiled animation graphs that OFAST bakes from: the pose a node produces for
//! given control parameter values, some time into its state (M4.4c).
//!
//! # What this is and is not
//!
//! AnimGraph 2 (Esoterica's design) runs a graph every frame with memory: state machines remember their state and
//! blend through timed transitions, springs and eases smooth values over time, events travel between clips and
//! conditions. A demo carries no graph state, only gameplay values (decision 013), so OFAST does not run graphs
//! as a whole. The bake ([`super::bake`]) decides from the demo which state a player is in and since when; this
//! module then evaluates that state's tree the way CS2's nodes compute their pose, `time` seconds after it began:
//!
//! | Node | Here |
//! |---|---|
//! | `CNmClipNode` | the clip at `time × m_flSpeedMultiplier`, wrapping when `m_bAllowLooping`, else held at its end |
//! | `CNmAnimationPoseNode` | one still pose of the clip at a value node's time (`m_nPoseTimeValueNodeIdx`): a fraction (0 – 1), a frame (`m_bUseFramesAsInput`) or a value in `m_inputTimeRemapRange` |
//! | `CNmBlend1DNode` | the two sources around the input value in `m_parameterization.m_blendRanges`, linearly |
//! | `CNmBlend2DNode` | barycentric weights in the node's triangulation ([`super::locomotion::Blend2D`]) |
//! | `CNmSelectorNode`, `CNmClipSelectorNode` | the first option whose condition holds |
//! | `CNmParameterizedClipSelectorNode`, `CNmParameterizedSelectorNode` | with `m_bHasWeightsSet`, the option a parameter in 0 – 1 falls on in `m_optionWeights` proportions; else the option whose index the parameter is; options without a clip skipped when `m_bIgnoreInvalidOptions` |
//! | `CNmDurationScaleNode` | the child stretched to `m_flDefaultInputValue` (or its input) seconds |
//! | `CNmSpeedScaleNode` | the child played `m_flDefaultInputValue` (or its input) times faster |
//! | `CNmStateNode` | its child; an off state (no child) gives nothing |
//! | `CNmStateMachineNode` | the first state whose entry condition holds, else `m_nDefaultStateIndex`; past the state's length, a transition whose condition is "state completed" moves on to its target (a jump into the landing blend, a defuse's enter into its loop); no other transitions |
//! | `CNmLayerBlendNode` | the base, then each layer that gives a pose: additive layers added, overlay layers blended over it, by the layer's weight and bone mask — or, for a state machine layer, its current state's (`m_nLayerWeightNodeIdx`, `m_nLayerBoneMaskNodeIdx`); masks come from [`Library::insert_mask`], a layer whose mask the library lacks covers every bone |
//! | `CNmReferencedGraphNode` | the referenced graph's root (`m_nRootNodeIdx`), in the same [`Library`] |
//! | `CNmZeroPoseNode` | nothing (an additive layer at rest) |
//! | `CNmScaleNode` | its child, each bone's scale times its weight in the node's mask while the enable condition holds (the viewmodels hide the silencer this way) |
//! | `CNmTwoBoneIKNode` | its child: the bake moves the hands itself ([`super::viewmodel`]) |
//!
//! Blends keep their sources in step by normalised time: the blend lasts the weighted average of its sources'
//! lengths, and each source is sampled at the same fraction of its own length — how AnimGraph 2 synchronises
//! blends without sync events, as [`super::locomotion`] does for the ground blends. Value nodes: control
//! parameters, constants (floats and IDs), curves (Hermite splines), float maths, switches (floats and IDs), ID
//! maps and remaps; springs, eases and caches pass their input through (they only smooth over time); a state's
//! current sync event ID has no value. Conditions: ID, float and range comparisons, booleans, `and` / `or` / `not`,
//! and events (true when listed in [`Params::events`]); timers are false.
//!
//! [`Evaluator::secondary`] evaluates the same tree for a secondary skeleton: the weapon bones a viewmodel clip
//! moves besides the arms (`m_secondaryAnimations`: the magazine, the bolt).
//!
//! Between two states the bake blends their poses over CS2's own transition time, found by
//! [`Evaluator::transition_time`] from the states each pose comes from ([`Evaluator::states`]).

use std::collections::{HashMap, HashSet};

use super::graph::{Graph, Transition};
use super::layers::{BoneMask, add_layer};
use super::locomotion::{Blend2D, BlendSource, blend_poses};
use super::{AnimError, Clip, Transform};
use crate::kv3::Value;

/// Control parameter values and active events for one evaluation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    /// Float parameters (`move_speed_x`, `move_crouch_amount`, `air_height_above_ground`, …); unset ones are 0.
    pub floats: HashMap<String, f32>,
    /// ID parameters (`action` = `action_attack`, `weapon_type` = `weapon_ak47`, …); unset ones are the empty ID.
    pub ids: HashMap<String, String>,
    /// Bool parameters (`move_is_walking`, `is_defusing`, …); unset ones are false.
    pub bools: HashMap<String, bool>,
    /// Events considered present (`KnifeSwingBack`, …); every other event condition is false.
    pub events: HashSet<String>,
}

impl Params {
    /// Sets a float parameter.
    pub fn float(mut self, name: &str, value: f32) -> Self {
        self.floats.insert(name.to_owned(), value);
        self
    }

    /// Sets an ID parameter.
    pub fn id(mut self, name: &str, value: &str) -> Self {
        self.ids.insert(name.to_owned(), value.to_owned());
        self
    }

    /// Sets a bool parameter.
    pub fn flag(mut self, name: &str, value: bool) -> Self {
        self.bools.insert(name.to_owned(), value);
        self
    }

    /// Marks an event as present.
    pub fn event(mut self, id: &str) -> Self {
        self.events.insert(id.to_owned());
        self
    }
}

/// Graphs and the clips they play, loaded once: a graph is found by its resource path (as other graphs store it,
/// without `_c`), a clip likewise.
#[derive(Debug, Default)]
pub struct Library {
    graphs: HashMap<String, Graph>,
    clips: HashMap<String, Clip>,
    /// `(graph, start node)` already walked by [`Library::load`] (`usize::MAX` for the root): each is walked once,
    /// which also ends graphs that reference each other.
    walked: HashSet<(String, usize)>,
    /// Bone masks by ID, on the clips' own skeleton, for the layers that name one ([`Library::insert_mask`]).
    masks: HashMap<String, BoneMask>,
    /// Bone masks on a secondary skeleton: `(skeleton path, mask ID)`.
    secondary_masks: HashMap<(String, String), BoneMask>,
}

impl Library {
    /// An empty library.
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads graph `path` alone (no clips), unless it is already loaded.
    pub fn load_graph<E>(
        &mut self,
        path: &str,
        load_graph: &mut impl FnMut(&str) -> Result<Graph, E>,
    ) -> Result<&Graph, E> {
        if !self.graphs.contains_key(path) {
            let graph = load_graph(path)?;
            self.graphs.insert(path.to_owned(), graph);
        }
        Ok(&self.graphs[path])
    }

    /// Loads graph `path` and every clip that `node` (the graph's root when `None`) can play, following referenced
    /// graphs (loaded from their roots). Graphs and clips already loaded are not read again. `load_graph` and
    /// `load_clip` read one resource by path.
    pub fn load<E>(
        &mut self,
        path: &str,
        node: Option<usize>,
        load_graph: &mut impl FnMut(&str) -> Result<Graph, E>,
        load_clip: &mut impl FnMut(&str) -> Result<Clip, E>,
    ) -> Result<(), E> {
        if !self
            .walked
            .insert((path.to_owned(), node.unwrap_or(usize::MAX)))
        {
            return Ok(());
        }
        self.load_graph(path, load_graph)?;
        let graph = &self.graphs[path];
        let Some(start) = node.or(graph.root) else {
            return Ok(());
        };
        // Every node reachable from `start`: its clips, and the graphs its referenced-graph nodes play.
        let mut clips = Vec::new();
        let mut referenced = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            if n >= graph.nodes.len() || !seen.insert(n) {
                continue;
            }
            if let Some((clip, _)) = graph.clip(n) {
                clips.push(clip.to_owned());
            }
            if let Some(other) = graph.referenced_graph(n) {
                referenced.push(other.to_owned());
            }
            stack.extend(node_children(graph, n));
        }
        for clip in clips {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.clips.entry(clip) {
                let decoded = load_clip(slot.key())?;
                slot.insert(decoded);
            }
        }
        for other in referenced {
            self.load(&other, None, load_graph, load_clip)?;
        }
        Ok(())
    }

    /// A loaded graph.
    pub fn graph(&self, path: &str) -> Option<&Graph> {
        self.graphs.get(path)
    }

    /// A loaded clip.
    pub fn clip(&self, path: &str) -> Option<&Clip> {
        self.clips.get(path)
    }

    /// Every loaded clip and its path.
    pub fn clips(&self) -> impl Iterator<Item = (&str, &Clip)> {
        self.clips.iter().map(|(path, clip)| (path.as_str(), clip))
    }

    /// Adds an already decoded clip (tests, clips loaded elsewhere).
    pub fn insert_clip(&mut self, path: &str, clip: Clip) {
        self.clips.insert(path.to_owned(), clip);
    }

    /// Adds an already parsed graph (tests).
    pub fn insert_graph(&mut self, path: &str, graph: Graph) {
        self.graphs.insert(path.to_owned(), graph);
    }

    /// Adds a bone mask for the layers that name `id` (a layer's `CNmBoneMaskNode`, or a layer state's
    /// `m_nLayerBoneMaskNodeIdx`): on the clips' own skeleton, or with `skeleton`, on that secondary skeleton (a
    /// weapon's, [`Evaluator::secondary`]). Without masks, layers apply to every bone.
    pub fn insert_mask(&mut self, skeleton: Option<&str>, id: &str, mask: BoneMask) {
        match skeleton {
            Some(s) => {
                self.secondary_masks
                    .insert((s.to_owned(), id.to_owned()), mask);
            }
            None => {
                self.masks.insert(id.to_owned(), mask);
            }
        }
    }
}

/// Every node index a node reads poses from (children, states, blend sources, options, layer inputs).
fn node_children(graph: &Graph, node: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for key in ["m_nChildNodeIdx", "m_nBaseNodeIdx"] {
        out.extend(graph.child(node, key));
    }
    for key in ["m_sourceNodeIndices", "m_optionNodeIndices"] {
        out.extend(graph.children(node, key));
    }
    out.extend(graph.states(node).into_iter().map(|(state, _)| state));
    out.extend(layer_inputs(graph, node).into_iter().map(|l| l.input));
    out
}

/// One layer of a layer blend (`m_layerDefinition`).
#[derive(Debug, Clone, Copy)]
struct Layer {
    /// The node giving the layer's pose.
    input: usize,
    /// Added over the pose below (`m_blendMode` `Additive`), else blended over it (`Overlay`).
    additive: bool,
    /// The layer's weight value node (`m_nWeightValueNodeIdx`); 1 without.
    weight: Option<usize>,
    /// The layer's bone mask value node (`m_nBoneMaskValueNodeIdx`); a state machine layer can name one per state.
    mask: Option<usize>,
}

/// A layer blend's layers.
fn layer_inputs(graph: &Graph, node: usize) -> Vec<Layer> {
    graph
        .nodes
        .get(node)
        .and_then(|n| n.get("m_layerDefinition"))
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|layer| {
            let index = |key: &str| {
                layer
                    .get(key)
                    .and_then(Value::as_i64)
                    .and_then(|i| usize::try_from(i).ok())
            };
            Some(Layer {
                input: index("m_nInputNodeIdx")?,
                additive: layer.get("m_blendMode").and_then(Value::as_str) == Some("Additive"),
                weight: index("m_nWeightValueNodeIdx"),
                mask: index("m_nBoneMaskValueNodeIdx"),
            })
        })
        .collect()
}

/// A state a pose comes from: `(graph path, state node)`.
pub type StateId = (String, usize);

/// What [`Evaluator::leaves`] finds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Leave {
    /// The machine stays in its state.
    Stays,
    /// It leaves for `state` (a state node), blending over `blend` seconds.
    Leaves {
        /// The target state node.
        state: usize,
        /// The transition's blend time, seconds.
        blend: f32,
    },
    /// No state machine or state was found to ask.
    Unknown,
}

/// What a node plays next, `time` seconds into it.
enum Next<'l> {
    /// A node with its own pose (clip, pose, zero pose) or none.
    Leaf,
    /// One child at its own time: `(graph, node, time)`.
    One(&'l str, usize, f32),
    /// Weighted sources of a blend in the same graph: `(node, weight, time)`.
    Blend(Vec<(usize, f32, f32)>),
    /// A layer blend: the base, then its layers, all at the same time.
    Layers(usize, Vec<Layer>),
    /// Nothing plays (an off state, a selector without a valid option).
    Nothing,
}

/// Evaluates nodes of the graphs in a [`Library`].
#[derive(Debug, Clone, Copy)]
pub struct Evaluator<'l> {
    library: &'l Library,
    /// With a secondary skeleton's path, poses are of that skeleton ([`Evaluator::secondary`]).
    secondary: Option<&'l str>,
}

/// Deepest node nesting followed (graphs are trees of a few dozen levels at most).
const MAX_DEPTH: usize = 64;

impl<'l> Evaluator<'l> {
    /// An evaluator over `library`.
    pub fn new(library: &'l Library) -> Self {
        Self {
            library,
            secondary: None,
        }
    }

    /// An evaluator whose poses are of the secondary skeleton `skeleton` (a weapon's own bones under a viewmodel's
    /// `wpn`): every clip plays its secondary animation on that skeleton (`m_secondaryAnimations`), blended and
    /// layered exactly like the clips themselves, with the masks' weights for that skeleton. Times, durations,
    /// states and conditions stay those of the clips: a clip without such an animation gives nothing, so it drops
    /// out of blends, and the weapon keeps the pose of the clips that have one.
    pub fn secondary(library: &'l Library, skeleton: &'l str) -> Self {
        Self {
            library,
            secondary: Some(skeleton),
        }
    }

    /// The pose a clip gives at `time` (seconds of the clip): its own, or its secondary animation on
    /// [`Evaluator::secondary`]'s skeleton (`None` when it has none).
    fn sample_clip(
        &self,
        clip: &Clip,
        time: f32,
        looping: bool,
    ) -> Result<Option<Vec<Transform>>, AnimError> {
        match self.secondary {
            None => Ok(Some(clip.sample(time, looping)?)),
            Some(skeleton) => match clip.secondary.iter().find(|c| c.skeleton == skeleton) {
                Some(secondary) => Ok(Some(secondary.sample(time, looping)?)),
                None => Ok(None),
            },
        }
    }

    /// The bone mask that mask value node `node` (`CNmBoneMaskNode`) names, for the skeleton this evaluator poses:
    /// `None` when `node` is no mask node, `Some(None)` when the library has no such mask for that skeleton.
    fn mask(&self, path: &str, node: usize) -> Option<Option<&'l BoneMask>> {
        let (_, graph) = self.graph(path).ok()?;
        if graph.class(node) != "CNmBoneMaskNode" {
            return None;
        }
        let id = graph.nodes.get(node)?.get("m_boneMaskID")?.as_str()?;
        Some(match self.secondary {
            None => self.library.masks.get(id),
            Some(skeleton) => self
                .library
                .secondary_masks
                .get(&(skeleton.to_owned(), id.to_owned())),
        })
    }

    /// A layer's weight and mask `time` seconds in: its own, or for a state machine layer, those of the state it is
    /// in (`m_nLayerWeightNodeIdx`, `m_nLayerBoneMaskNodeIdx`). A mask the library lacks covers every bone of the
    /// clips' own skeleton, and none of a secondary skeleton: CS2's masks list weights for some weapon skeletons
    /// only (`GunEmpty`'s for the pistols and the SCAR-20, not the AK-47's).
    fn layer_weight_and_mask(
        &self,
        path: &'l str,
        layer: &Layer,
        params: &Params,
        time: f32,
        depth: usize,
    ) -> Result<(f32, Option<&'l BoneMask>), AnimError> {
        let (_, graph) = self.graph(path)?;
        let mut weight = layer.weight.map_or(1.0, |w| self.float(path, w, params));
        let mut named = layer.mask.and_then(|m| self.mask(path, m));
        if graph.class(layer.input) == "CNmStateMachineNode"
            && let Some((state, _)) = self.machine_state(path, layer.input, params, time, depth)?
        {
            if let Some(w) = graph.child(state, "m_nLayerWeightNodeIdx") {
                weight *= self.float(path, w, params);
            }
            if named.is_none() {
                named = graph
                    .child(state, "m_nLayerBoneMaskNodeIdx")
                    .and_then(|m| self.mask(path, m));
            }
        }
        if matches!(named, Some(None)) && self.secondary.is_some() {
            weight = 0.0;
        }
        Ok((weight.clamp(0.0, 1.0), named.flatten()))
    }

    /// The library's own copy of a graph path, and the graph.
    fn graph(&self, path: &str) -> Result<(&'l str, &'l Graph), AnimError> {
        self.library
            .graphs
            .get_key_value(path)
            .map(|(k, g)| (k.as_str(), g))
            .ok_or_else(|| AnimError::Graph(format!("{path}: graph not loaded")))
    }

    fn clip(&self, path: &str) -> Result<&'l Clip, AnimError> {
        self.library
            .clip(path)
            .ok_or_else(|| AnimError::Graph(format!("{path}: clip not loaded")))
    }

    /// A graph's root node.
    pub fn root(&self, graph: &str) -> Option<usize> {
        self.library.graph(graph)?.root
    }

    /// The node at `path` (editor path, `SM/Ground/Standing/Starts`) of a graph.
    pub fn find(&self, graph: &str, path: &str) -> Option<usize> {
        self.library.graph(graph)?.find(path)
    }

    /// The pose of `node` (one transform per clip-skeleton bone) `time` seconds after it began, or `None` when it
    /// produces nothing (an off state, a zero pose).
    pub fn pose(
        &self,
        graph: &str,
        node: usize,
        params: &Params,
        time: f32,
    ) -> Result<Option<Vec<Transform>>, AnimError> {
        let (path, _) = self.graph(graph)?;
        self.pose_at(path, node, params, time, 0)
    }

    /// The length of `node` in seconds for these parameters (0 for still poses and nodes without time).
    pub fn duration(&self, graph: &str, node: usize, params: &Params) -> Result<f32, AnimError> {
        let (path, _) = self.graph(graph)?;
        self.duration_at(path, node, params, 0)
    }

    /// The states the pose of `node` comes from `time` seconds in, outer states first (every state machine on the
    /// way, through blends and layers).
    pub fn states(
        &self,
        graph: &str,
        node: usize,
        params: &Params,
        time: f32,
    ) -> Result<Vec<StateId>, AnimError> {
        let (path, _) = self.graph(graph)?;
        let mut out = Vec::new();
        self.states_at(path, node, params, time, 0, &mut out)?;
        Ok(out)
    }

    /// The clip that weighs most in the pose of `node` `time` seconds in, and the time reached in that clip
    /// (seconds of the clip) — for the clip's events.
    pub fn dominant_clip(
        &self,
        graph: &str,
        node: usize,
        params: &Params,
        time: f32,
    ) -> Result<Option<(&'l Clip, f32)>, AnimError> {
        let (mut path, _) = self.graph(graph)?;
        let (mut node, mut time) = (node, time);
        for depth in 0..MAX_DEPTH {
            let (_, g) = self.graph(path)?;
            if g.class(node) == "CNmClipNode" {
                let Some((clip, speed)) = g.clip(node) else {
                    return Ok(None);
                };
                let clip = self.clip(clip)?;
                let mut t = time * speed;
                if looping(g, node) && clip.duration > 0.0 {
                    t = t.rem_euclid(clip.duration);
                }
                return Ok(Some((clip, t.clamp(0.0, clip.duration))));
            }
            match self.next(path, node, params, time, depth)? {
                Next::One(p, n, t) => (path, node, time) = (p, n, t),
                Next::Blend(sources) => {
                    let Some(&(n, _, t)) = sources.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else {
                        return Ok(None);
                    };
                    (node, time) = (n, t);
                }
                Next::Layers(base, _) => node = base,
                Next::Leaf | Next::Nothing => return Ok(None),
            }
        }
        Ok(None)
    }

    /// Whether the state machine `node` plays (`node` itself, or the first machine under its states and referenced
    /// graphs), in the state that `from` chose `time` seconds ago, leaves it for `params`: the first of that state's
    /// transitions whose condition holds ([`Leave::Leaves`] with the target state node and the blend time), or a
    /// "state completed" transition that has already taken it into a state that plays nothing (blend 0). This is
    /// how CS2 runs a machine (entry conditions only choose the state on entry), so an action goes on until the
    /// graph itself lets go of it. [`Leave::Unknown`] when no machine or state is found.
    pub fn leaves(
        &self,
        graph: &str,
        node: usize,
        from: &Params,
        time: f32,
        params: &Params,
    ) -> Result<Leave, AnimError> {
        let (mut path, _) = self.graph(graph)?;
        let mut machine = node;
        for _ in 0..MAX_DEPTH {
            let (_, g) = self.graph(path)?;
            match g.class(machine) {
                "CNmStateMachineNode" => break,
                "CNmStateNode" | "CNmScaleNode" | "CNmTwoBoneIKNode" => {
                    match g.child(machine, "m_nChildNodeIdx") {
                        Some(child) => machine = child,
                        None => return Ok(Leave::Unknown),
                    }
                }
                // The machine under the layers (a viewmodel's actions, under its bullets' layer).
                "CNmLayerBlendNode" => match g.child(machine, "m_nBaseNodeIdx") {
                    Some(base) => machine = base,
                    None => return Ok(Leave::Unknown),
                },
                "CNmReferencedGraphNode" => {
                    let Some((other, g)) = g
                        .referenced_graph(machine)
                        .and_then(|other| self.graph(other).ok())
                    else {
                        return Ok(Leave::Unknown);
                    };
                    let Some(root) = g.root else {
                        return Ok(Leave::Unknown);
                    };
                    (path, machine) = (other, root);
                }
                _ => return Ok(Leave::Unknown),
            }
        }
        let (_, g) = self.graph(path)?;
        if g.class(machine) != "CNmStateMachineNode" {
            return Ok(Leave::Unknown);
        }
        let (Some((entered, _)), Some((state, t))) = (
            self.machine_state(path, machine, from, 0.0, 0)?,
            self.machine_state(path, machine, from, time, 0)?,
        ) else {
            return Ok(Leave::Unknown);
        };
        // Completed into a state that plays nothing: the action is over.
        if state != entered && self.pose_at(path, state, from, t, 0)?.is_none() {
            return Ok(Leave::Leaves { state, blend: 0.0 });
        }
        let states = g.states(machine);
        let Some(index) = states.iter().position(|&(s, _)| s == state) else {
            return Ok(Leave::Unknown);
        };
        Ok(g.transitions(machine, index)
            .into_iter()
            .filter(|t| t.target < states.len())
            .find(|t| t.condition.is_some_and(|c| self.condition(path, c, params)))
            .map_or(Leave::Stays, |t| Leave::Leaves {
                state: states[t.target].0,
                blend: self.transition_duration(path, &t, params),
            }))
    }

    /// CS2's blend time from the states `from` to the states `to` ([`Evaluator::states`]): the transition, in the
    /// machine where the two first differ, from the old state to the new one — the first whose condition holds for
    /// `params`, else the first to that target. `None` when the states are the same or no transition joins them.
    pub fn transition_time(
        &self,
        from: &[StateId],
        to: &[StateId],
        params: &Params,
    ) -> Option<f32> {
        let (old, new) = from.iter().zip(to).find(|(a, b)| a != b)?;
        if old.0 != new.0 {
            return None;
        }
        let (path, graph) = self.graph(&new.0).ok()?;
        // The machine that has both states.
        (0..graph.nodes.len())
            .filter(|&m| graph.class(m) == "CNmStateMachineNode")
            .find_map(|machine| {
                let states = graph.states(machine);
                let source = states.iter().position(|&(s, _)| s == old.1)?;
                let target = states.iter().position(|&(s, _)| s == new.1)?;
                let transitions: Vec<_> = graph
                    .transitions(machine, source)
                    .into_iter()
                    .filter(|t| t.target == target)
                    .collect();
                let chosen = transitions
                    .iter()
                    .find(|t| t.condition.is_some_and(|c| self.condition(path, c, params)))
                    .or(transitions.first())?;
                Some(self.transition_duration(path, chosen, params))
            })
    }

    /// CS2's blend time when an action starts over in the same states ([`Evaluator::states`]): its graph restarts
    /// it by switching to a twin state (`ATK0` ↔ `ATK2` for shots, `Deploy0` ↔ `Deploy2` for draws, `inspect0` ↔
    /// `inspect1`) on `action_reset`. The first transition, innermost machine first, out of one of `states` that
    /// holds with `action_reset` set and not without it. `None` when there is none.
    pub fn restart_time(&self, states: &[StateId], params: &Params) -> Option<f32> {
        let reset = params.clone().flag("action_reset", true);
        let unset = params.clone().flag("action_reset", false);
        states.iter().rev().find_map(|(path, state)| {
            let (path, graph) = self.graph(path).ok()?;
            (0..graph.nodes.len())
                .filter(|&m| graph.class(m) == "CNmStateMachineNode")
                .find_map(|machine| {
                    let index = graph
                        .states(machine)
                        .iter()
                        .position(|&(s, _)| s == *state)?;
                    graph
                        .transitions(machine, index)
                        .into_iter()
                        .find(|t| {
                            t.condition.is_some_and(|c| {
                                self.condition(path, c, &reset) && !self.condition(path, c, &unset)
                            })
                        })
                        .map(|t| self.transition_duration(path, &t, &reset))
                })
        })
    }

    /// The events of the clip weighing most in `node`'s pose `time` seconds in ([`Evaluator::dominant_clip`]) that
    /// are on at that time: what conditions on events see (the draw's `WPN_INSPECT_USE_DRAW_VERSION` when an inspect
    /// starts during it, `WPN_DISABLE_LEFT_HAND_IK` while a hand leaves the weapon).
    pub fn events_at(
        &self,
        graph: &str,
        node: usize,
        params: &Params,
        time: f32,
    ) -> Result<HashSet<String>, AnimError> {
        let Some((clip, t)) = self.dominant_clip(graph, node, params, time)? else {
            return Ok(HashSet::new());
        };
        // Event times are fractions of the clip.
        Ok(clip
            .events
            .iter()
            .filter(|e| {
                let start = e.start * clip.duration;
                t >= start && t <= start + e.duration * clip.duration
            })
            .map(|e| e.id.clone())
            .collect())
    }

    /// The events the states `states` ([`Evaluator::states`]) send while playing (`m_executeEvents`, as the
    /// viewmodel's `Idle` sends `WPN_STATE_IDLE`): what a "fully in state" graph event condition of the next
    /// action sees when these states had fully blended in.
    pub fn state_events(&self, states: &[StateId]) -> HashSet<String> {
        let mut out = HashSet::new();
        for (path, state) in states {
            let Ok((_, graph)) = self.graph(path) else {
                continue;
            };
            let listed = graph
                .nodes
                .get(*state)
                .and_then(|n| n.get("m_executeEvents"))
                .and_then(Value::as_array)
                .unwrap_or_default();
            out.extend(listed.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        out
    }

    /// A transition's blend time: its duration override (a float value node) when it has one, else its duration.
    fn transition_duration(&self, path: &str, transition: &Transition, params: &Params) -> f32 {
        transition
            .duration_override
            .map_or(transition.duration, |n| {
                self.float(path, n, params).max(0.0)
            })
    }

    /// What `node` plays next, `time` seconds into it (`depth`: the nesting reached, for [`MAX_DEPTH`]).
    fn next(
        &self,
        path: &'l str,
        node: usize,
        params: &Params,
        time: f32,
        depth: usize,
    ) -> Result<Next<'l>, AnimError> {
        let (_, graph) = self.graph(path)?;
        Ok(match graph.class(node) {
            // A scale node (bones of a mask scaled, [`Evaluator::pose`] applies it) and a two-bone IK node (a hand
            // onto its target, which the bake applies) change their child's pose after it is computed.
            "CNmStateNode" | "CNmScaleNode" | "CNmTwoBoneIKNode" => {
                match graph.child(node, "m_nChildNodeIdx") {
                    Some(child) => Next::One(path, child, time),
                    None => Next::Nothing,
                }
            }
            "CNmDurationScaleNode" => match graph.child(node, "m_nChildNodeIdx") {
                Some(child) => {
                    let desired = self.scale_input(path, node, params);
                    let length = self.duration_at(path, child, params, depth + 1)?;
                    let t = if desired > 0.0 {
                        time * length / desired
                    } else {
                        0.0
                    };
                    Next::One(path, child, t)
                }
                None => Next::Nothing,
            },
            "CNmSpeedScaleNode" => match graph.child(node, "m_nChildNodeIdx") {
                Some(child) => Next::One(path, child, time * self.scale_input(path, node, params)),
                None => Next::Nothing,
            },
            "CNmStateMachineNode" => match self.machine_state(path, node, params, time, depth)? {
                Some((state, t)) => Next::One(path, state, t),
                None => Next::Nothing,
            },
            "CNmSelectorNode"
            | "CNmClipSelectorNode"
            | "CNmParameterizedClipSelectorNode"
            | "CNmParameterizedSelectorNode" => match self.chosen_option(path, node, params) {
                Some(option) => Next::One(path, option, time),
                None => Next::Nothing,
            },
            "CNmReferencedGraphNode" => match graph.referenced_graph(node) {
                Some(other) => {
                    let (other, g) = self.graph(other)?;
                    match g.root {
                        Some(root) => Next::One(other, root, time),
                        None => Next::Nothing,
                    }
                }
                None => Next::Nothing,
            },
            "CNmBlend1DNode" | "CNmBlend2DNode" => {
                let weights = self.blend_weights(path, node, params)?;
                let mut length = 0.0;
                let mut lengths = Vec::with_capacity(weights.len());
                for &(source, weight) in &weights {
                    let l = self.duration_at(path, source, params, depth + 1)?;
                    length += weight * l;
                    lengths.push(l);
                }
                let phase = if length > 0.0 {
                    let p = time / length;
                    if looping(graph, node) {
                        p.rem_euclid(1.0)
                    } else {
                        p.clamp(0.0, 1.0)
                    }
                } else {
                    0.0
                };
                Next::Blend(
                    weights
                        .into_iter()
                        .zip(lengths)
                        .map(|((source, weight), l)| {
                            (source, weight, if length > 0.0 { phase * l } else { time })
                        })
                        .collect(),
                )
            }
            "CNmLayerBlendNode" => match graph.child(node, "m_nBaseNodeIdx") {
                Some(base) => Next::Layers(base, layer_inputs(graph, node)),
                None => Next::Nothing,
            },
            _ => Next::Leaf,
        })
    }

    fn pose_at(
        &self,
        path: &'l str,
        node: usize,
        params: &Params,
        time: f32,
        depth: usize,
    ) -> Result<Option<Vec<Transform>>, AnimError> {
        if depth > MAX_DEPTH {
            return Err(AnimError::Graph(format!(
                "{path}: node {node} nested too deep"
            )));
        }
        let (_, graph) = self.graph(path)?;
        // The masked bones' scales times their mask weight, while the enable condition holds: the viewmodels'
        // silencer hidden (`silencer` mask: the weapon's `silencer` bone at 0) unless it is on or being shown.
        if graph.class(node) == "CNmScaleNode" {
            let Some(child) = graph.child(node, "m_nChildNodeIdx") else {
                return Ok(None);
            };
            let Some(mut pose) = self.pose_at(path, child, params, time, depth + 1)? else {
                return Ok(None);
            };
            let enabled = graph
                .child(node, "m_nEnableNodeIdx")
                .is_none_or(|c| self.condition_at(path, c, params, depth + 1));
            if enabled
                && let Some(mask) = graph
                    .child(node, "m_nMaskNodeIdx")
                    .and_then(|m| self.mask(path, m))
                    .flatten()
            {
                for (bone, t) in pose.iter_mut().enumerate() {
                    t.scale *= mask.0.get(bone).copied().unwrap_or(1.0);
                }
            }
            return Ok(Some(pose));
        }
        match self.next(path, node, params, time, depth)? {
            Next::Leaf => match graph.class(node) {
                "CNmClipNode" => {
                    let Some((clip, speed)) = graph.clip(node) else {
                        return Ok(None);
                    };
                    let clip = self.clip(clip)?;
                    self.sample_clip(clip, time * speed, looping(graph, node))
                }
                "CNmAnimationPoseNode" => {
                    let Some((clip, _)) = graph.clip(node) else {
                        return Ok(None);
                    };
                    let clip = self.clip(clip)?;
                    let fraction = self.pose_fraction(path, node, params, clip, depth);
                    self.sample_clip(clip, fraction * clip.duration, false)
                }
                _ => Ok(None),
            },
            Next::One(p, n, t) => self.pose_at(p, n, params, t, depth + 1),
            Next::Blend(sources) => {
                let mut poses = Vec::with_capacity(sources.len());
                let mut total = 0.0;
                for (source, weight, t) in sources {
                    if let Some(pose) = self.pose_at(path, source, params, t, depth + 1)? {
                        total += weight;
                        poses.push((pose, weight));
                    }
                }
                // Sources that give nothing drop out; the others share their weight.
                for (_, w) in &mut poses {
                    *w /= total.max(1e-6);
                }
                Ok((!poses.is_empty()).then(|| blend_poses(&poses)))
            }
            Next::Layers(base, layers) => {
                let Some(mut pose) = self.pose_at(path, base, params, time, depth + 1)? else {
                    return Ok(None);
                };
                for layer in layers {
                    let Some(top) = self.pose_at(path, layer.input, params, time, depth + 1)?
                    else {
                        continue;
                    };
                    let (weight, mask) =
                        self.layer_weight_and_mask(path, &layer, params, time, depth + 1)?;
                    if layer.additive {
                        add_layer(&mut pose, &top, weight, mask);
                    } else if weight >= 1.0 && mask.is_none() {
                        pose = top;
                    } else {
                        overlay(&mut pose, &top, weight, mask);
                    }
                }
                Ok(Some(pose))
            }
            Next::Nothing => Ok(None),
        }
    }

    fn states_at(
        &self,
        path: &'l str,
        node: usize,
        params: &Params,
        time: f32,
        depth: usize,
        out: &mut Vec<StateId>,
    ) -> Result<(), AnimError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let (_, graph) = self.graph(path)?;
        if graph.class(node) == "CNmStateNode" {
            out.push((path.to_owned(), node));
        }
        match self.next(path, node, params, time, depth)? {
            Next::One(p, n, t) => self.states_at(p, n, params, t, depth + 1, out),
            Next::Blend(sources) => {
                for (source, _, t) in sources {
                    self.states_at(path, source, params, t, depth + 1, out)?;
                }
                Ok(())
            }
            Next::Layers(base, layers) => {
                self.states_at(path, base, params, time, depth + 1, out)?;
                for layer in layers {
                    self.states_at(path, layer.input, params, time, depth + 1, out)?;
                }
                Ok(())
            }
            Next::Leaf | Next::Nothing => Ok(()),
        }
    }

    fn duration_at(
        &self,
        path: &'l str,
        node: usize,
        params: &Params,
        depth: usize,
    ) -> Result<f32, AnimError> {
        if depth > MAX_DEPTH {
            return Err(AnimError::Graph(format!(
                "{path}: node {node} nested too deep"
            )));
        }
        let (_, graph) = self.graph(path)?;
        Ok(match graph.class(node) {
            "CNmClipNode" => match graph.clip(node) {
                Some((clip, speed)) => self.clip(clip)?.duration / speed.max(1e-3),
                None => 0.0,
            },
            "CNmBlend1DNode" | "CNmBlend2DNode" => {
                let mut total = 0.0;
                for (source, weight) in self.blend_weights(path, node, params)? {
                    total += weight * self.duration_at(path, source, params, depth + 1)?;
                }
                total
            }
            "CNmDurationScaleNode" => self.scale_input(path, node, params),
            "CNmSpeedScaleNode" => match graph.child(node, "m_nChildNodeIdx") {
                Some(child) => {
                    self.duration_at(path, child, params, depth + 1)?
                        / self.scale_input(path, node, params).max(1e-3)
                }
                None => 0.0,
            },
            // A still pose, or nothing.
            "CNmAnimationPoseNode" | "CNmZeroPoseNode" => 0.0,
            _ => match self.next(path, node, params, 0.0, depth)? {
                Next::One(p, n, _) => self.duration_at(p, n, params, depth + 1)?,
                Next::Layers(base, _) => self.duration_at(path, base, params, depth + 1)?,
                _ => 0.0,
            },
        })
    }

    /// The state a machine is in `time` seconds after it was entered, and the time in that state.
    fn machine_state(
        &self,
        path: &'l str,
        machine: usize,
        params: &Params,
        time: f32,
        depth: usize,
    ) -> Result<Option<(usize, f32)>, AnimError> {
        let (_, graph) = self.graph(path)?;
        let states = graph.states(machine);
        if states.is_empty() {
            return Ok(None);
        }
        let mut index = states
            .iter()
            .position(|&(_, entry)| entry.is_some_and(|c| self.condition(path, c, params)))
            .unwrap_or_else(|| graph.default_state(machine).min(states.len() - 1));
        let mut t = time;
        // Completed states move on ("state completed" transitions), as long as time remains.
        for _ in 0..states.len() {
            let Some(next) = graph.transitions(machine, index).into_iter().find(|tr| {
                tr.condition
                    .is_some_and(|c| self.is_completion(path, c, params))
            }) else {
                break;
            };
            let length = self.duration_at(path, states[index].0, params, depth + 1)?;
            if length <= 0.0 || t < length || next.target >= states.len() {
                break;
            }
            t -= length;
            index = next.target;
        }
        Ok(Some((states[index].0, t)))
    }

    /// Whether a transition condition is "the state completed" (alone, or with other conditions that hold).
    fn is_completion(&self, path: &str, node: usize, params: &Params) -> bool {
        let Ok((_, graph)) = self.graph(path) else {
            return false;
        };
        match graph.class(node) {
            "CNmStateCompletedConditionNode" => true,
            "CNmAndNode" => {
                let parts = graph.children(node, "m_conditionNodeIndices");
                parts
                    .iter()
                    .any(|&c| graph.class(c) == "CNmStateCompletedConditionNode")
                    && parts.iter().all(|&c| {
                        graph.class(c) == "CNmStateCompletedConditionNode"
                            || self.condition(path, c, params)
                    })
            }
            _ => false,
        }
    }

    /// The option a selector plays for these parameters.
    fn chosen_option(&self, path: &str, node: usize, params: &Params) -> Option<usize> {
        let (_, graph) = self.graph(path).ok()?;
        let options = graph.children(node, "m_optionNodeIndices");
        if !matches!(
            graph.class(node),
            "CNmParameterizedClipSelectorNode" | "CNmParameterizedSelectorNode"
        ) {
            let conditions = graph.children(node, "m_conditionNodeIndices");
            return options
                .iter()
                .zip(&conditions)
                .find(|&(_, &c)| self.condition(path, c, params))
                .map(|(&option, _)| option);
        }
        let raw = graph.nodes.get(node)?;
        let flag = |key: &str| raw.get(key).and_then(Value::as_bool).unwrap_or(false);
        let valid =
            |option: usize| !flag("m_bIgnoreInvalidOptions") || graph.clip(option).is_some();
        let value = graph
            .child(node, "m_parameterNodeIdx")
            .map_or(0.0, |p| self.float(path, p, params));
        if flag("m_bHasWeightsSet") {
            let weights = numbers(raw.get("m_optionWeights"));
            let (options, weights): (Vec<usize>, Vec<f32>) = options
                .iter()
                .enumerate()
                .filter(|&(_, &o)| valid(o))
                .map(|(i, &o)| (o, weights.get(i).copied().unwrap_or(1.0)))
                .unzip();
            return pick_weighted(&options, &weights, value.clamp(0.0, 1.0));
        }
        let index = (value.round().max(0.0) as usize).min(options.len().checked_sub(1)?);
        Some(options[index])
            .filter(|&o| valid(o))
            .or_else(|| options.iter().copied().find(|&o| valid(o)))
    }

    /// `(source node, weight)` of a 1D or 2D blend for these parameters, weights summing to 1.
    fn blend_weights(
        &self,
        path: &str,
        node: usize,
        params: &Params,
    ) -> Result<Vec<(usize, f32)>, AnimError> {
        let (_, graph) = self.graph(path)?;
        let raw = &graph.nodes[node];
        let sources = graph.children(node, "m_sourceNodeIndices");
        if graph.class(node) == "CNmBlend2DNode" {
            let values = numbers(raw.get("m_values"));
            let ints = |key: &str| -> Vec<usize> {
                raw.get(key)
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|v| usize::try_from(v.as_i64()?).ok())
                    .collect()
            };
            let space = Blend2D {
                points: values.as_chunks::<2>().0.to_vec(),
                sources: (0..sources.len())
                    .map(|clip| BlendSource { clip, speed: 1.0 })
                    .collect(),
                triangles: ints("m_indices").as_chunks::<3>().0.to_vec(),
                hull: ints("m_hullIndices"),
            };
            let points = space.points.len();
            if points != sources.len()
                || sources.is_empty()
                || space.triangles.iter().flatten().any(|&i| i >= points)
                || space.hull.iter().any(|&i| i >= points)
            {
                return Err(AnimError::Graph(format!(
                    "{path}: 2D blend {node} is inconsistent"
                )));
            }
            let input = |key: &str| {
                graph
                    .child(node, key)
                    .map_or(0.0, |n| self.float(path, n, params))
            };
            let p = [
                input("m_nInputParameterNodeIdx0"),
                input("m_nInputParameterNodeIdx1"),
            ];
            return Ok(space
                .weights(p)
                .into_iter()
                .map(|(i, w)| (sources[i], w))
                .collect());
        }
        // 1D: the range the input falls in (the nearest one outside them all), linear between its two sources.
        let value = graph
            .child(node, "m_nInputParameterValueNodeIdx")
            .map_or(0.0, |n| self.float(path, n, params));
        let ranges = raw
            .get("m_parameterization")
            .and_then(|p| p.get("m_blendRanges"))
            .and_then(Value::as_array)
            .unwrap_or_default();
        let mut best: Option<(f32, usize, usize, f32)> = None;
        for range in ranges {
            let index = |key: &str| {
                range
                    .get(key)
                    .and_then(Value::as_i64)
                    .and_then(|i| usize::try_from(i).ok())
            };
            let (Some(i0), Some(i1)) = (index("m_nInputIdx0"), index("m_nInputIdx1")) else {
                continue;
            };
            let bound = |key: &str| {
                range
                    .get("m_parameterValueRange")
                    .and_then(|r| r.get(key))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32
            };
            let (min, max) = (bound("m_flMin"), bound("m_flMax"));
            let t = if max > min {
                (value - min) / (max - min)
            } else {
                0.0
            };
            let outside = if t < 0.0 {
                -t
            } else if t > 1.0 {
                t - 1.0
            } else {
                0.0
            };
            if best.is_none_or(|b| outside < b.0) {
                best = Some((outside, i0, i1, t.clamp(0.0, 1.0)));
            }
        }
        let Some((_, i0, i1, t)) = best else {
            return Ok(sources.first().map(|&s| (s, 1.0)).into_iter().collect());
        };
        let (Some(&a), Some(&b)) = (sources.get(i0), sources.get(i1)) else {
            return Err(AnimError::Graph(format!(
                "{path}: 1D blend {node} has no source {i1}"
            )));
        };
        Ok(if t <= 0.0 {
            vec![(a, 1.0)]
        } else if t >= 1.0 {
            vec![(b, 1.0)]
        } else {
            vec![(a, 1.0 - t), (b, t)]
        })
    }

    /// Where in its clip a pose node is, as a fraction (0 – 1).
    fn pose_fraction(
        &self,
        path: &str,
        node: usize,
        params: &Params,
        clip: &Clip,
        depth: usize,
    ) -> f32 {
        let Ok((_, graph)) = self.graph(path) else {
            return 0.0;
        };
        let value = match graph.child(node, "m_nPoseTimeValueNodeIdx") {
            Some(input) => self.float_at(path, input, params, depth + 1),
            None => graph.number(node, "m_flUserSpecifiedTime").unwrap_or(0.0) as f32,
        };
        let raw = &graph.nodes[node];
        let range = raw.get("m_inputTimeRemapRange");
        let bound = |k: &str| {
            range
                .and_then(|r| r.get(k))
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32
        };
        let (min, max) = (bound("m_flMin"), bound("m_flMax"));
        let fraction = if raw.get("m_bUseFramesAsInput").and_then(Value::as_bool) == Some(true) {
            value / (clip.frames.saturating_sub(1).max(1)) as f32
        } else if max > min {
            (value - min) / (max - min)
        } else {
            value
        };
        fraction.clamp(0.0, 1.0)
    }

    /// The value a duration or speed scale node uses: its input, else `m_flDefaultInputValue`.
    fn scale_input(&self, path: &str, node: usize, params: &Params) -> f32 {
        let Ok((_, graph)) = self.graph(path) else {
            return 1.0;
        };
        match graph.child(node, "m_nInputValueNodeIdx") {
            Some(input) => self.float(path, input, params),
            None => graph.number(node, "m_flDefaultInputValue").unwrap_or(1.0) as f32,
        }
    }

    /// A float value node's value.
    pub fn float(&self, path: &str, node: usize, params: &Params) -> f32 {
        self.float_at(path, node, params, 0)
    }

    fn float_at(&self, path: &str, node: usize, params: &Params, depth: usize) -> f32 {
        let Ok((_, graph)) = self.graph(path) else {
            return 0.0;
        };
        if depth > MAX_DEPTH || node >= graph.nodes.len() {
            return 0.0;
        }
        let raw = &graph.nodes[node];
        let input = || {
            graph
                .child(node, "m_nInputValueNodeIdx")
                .map_or(0.0, |n| self.float_at(path, n, params, depth + 1))
        };
        let number = |key: &str| graph.number(node, key).unwrap_or(0.0) as f32;
        match graph.class(node) {
            "CNmControlParameterFloatNode" => graph
                .parameters
                .get(node)
                .and_then(|name| params.floats.get(name))
                .copied()
                .unwrap_or(0.0),
            "CNmConstFloatNode" => number("m_flValue"),
            // A op B, B a value node or the constant `m_flValueB` (`weapon_ammo_max − weapon_ammo`: rounds to
            // reload).
            "CNmFloatMathNode" => {
                let operand = |key: &str| {
                    graph
                        .child(node, key)
                        .map(|n| self.float_at(path, n, params, depth + 1))
                };
                let a = operand("m_nInputValueNodeIdxA").unwrap_or(0.0);
                let b = operand("m_nInputValueNodeIdxB").unwrap_or(number("m_flValueB"));
                let flag = |key: &str| raw.get(key).and_then(Value::as_bool).unwrap_or(false);
                // The operators CS2's graphs use are `Mod` (198 times in the viewmodel graphs: odd or even rounds
                // for the belts' bullets) and `Sub`; one not listed here gives 0.
                let mut value = match raw.get("m_operator").and_then(Value::as_str) {
                    Some("Add") => a + b,
                    Some("Sub") => a - b,
                    Some("Mul") => a * b,
                    Some("Div") if b != 0.0 => a / b,
                    Some("Mod") if b != 0.0 => a % b,
                    _ => 0.0,
                };
                if flag("m_bReturnAbsoluteResult") {
                    value = value.abs();
                }
                if flag("m_bReturnNegatedResult") {
                    value = -value;
                }
                value
            }
            "CNmFloatCurveNode" => curve(raw.get("m_curve"), input()),
            "CNmFloatSpringNode" | "CNmFloatEaseNode" | "CNmCachedFloatNode" => input(),
            "CNmFloatSwitchNode" => {
                let switch = graph
                    .child(node, "m_nSwitchValueNodeIdx")
                    .is_some_and(|c| self.condition_at(path, c, params, depth + 1));
                let (key, fallback) = if switch {
                    ("m_nTrueValueNodeIdx", "m_flTrueValue")
                } else {
                    ("m_nFalseValueNodeIdx", "m_flFalseValue")
                };
                match graph.child(node, key) {
                    Some(value) => self.float_at(path, value, params, depth + 1),
                    None => number(fallback),
                }
            }
            "CNmIDToFloatNode" => {
                let id = graph
                    .child(node, "m_nInputValueNodeIdx")
                    .and_then(|n| self.id(path, n, params));
                let ids: Vec<&str> = raw
                    .get("m_IDs")
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                let values = numbers(raw.get("m_values"));
                id.and_then(|id| ids.iter().position(|&i| i == id))
                    .and_then(|i| values.get(i).copied())
                    .unwrap_or(number("m_defaultValue"))
            }
            "CNmFloatRemapNode" => {
                let range = |key: &str| {
                    let r = raw.get(key);
                    let at = |k: &str| {
                        r.and_then(|r| r.get(k))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0) as f32
                    };
                    (at("m_flBegin"), at("m_flEnd"))
                };
                let (a, b) = range("m_inputRange");
                let (c, d) = range("m_outputRange");
                let t = if (b - a).abs() > f32::EPSILON {
                    ((input() - a) / (b - a)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                c + (d - c) * t
            }
            _ => 0.0,
        }
    }

    /// An ID value node's value: a control parameter, a constant, or a switch between two. The current sync event of
    /// a state (`CNmCurrentSyncEventIDNode`) needs the graph's running state, which OFAST does not keep: no value.
    fn id(&self, path: &str, node: usize, params: &Params) -> Option<String> {
        self.id_at(path, node, params, 0)
    }

    fn id_at(&self, path: &str, node: usize, params: &Params, depth: usize) -> Option<String> {
        let (_, graph) = self.graph(path).ok()?;
        if depth > MAX_DEPTH {
            return None;
        }
        let raw = graph.nodes.get(node)?;
        let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_owned);
        match graph.class(node) {
            "CNmControlParameterIDNode" => graph
                .parameters
                .get(node)
                .and_then(|name| params.ids.get(name))
                .cloned(),
            "CNmConstIDNode" => text("m_value"),
            "CNmIDSwitchNode" => {
                let switch = graph
                    .child(node, "m_nSwitchValueNodeIdx")
                    .is_some_and(|c| self.condition_at(path, c, params, depth + 1));
                let (key, fallback) = if switch {
                    ("m_nTrueValueNodeIdx", "m_trueValue")
                } else {
                    ("m_nFalseValueNodeIdx", "m_falseValue")
                };
                match graph.child(node, key) {
                    Some(value) => self.id_at(path, value, params, depth + 1),
                    None => text(fallback),
                }
            }
            _ => None,
        }
    }

    /// Whether a condition node holds for these parameters.
    pub fn condition(&self, path: &str, node: usize, params: &Params) -> bool {
        self.condition_at(path, node, params, 0)
    }

    fn condition_at(&self, path: &str, node: usize, params: &Params, depth: usize) -> bool {
        let Ok((_, graph)) = self.graph(path) else {
            return false;
        };
        if depth > MAX_DEPTH || node >= graph.nodes.len() {
            return false;
        }
        let raw = &graph.nodes[node];
        let text = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or("");
        let input = || graph.child(node, "m_nInputValueNodeIdx");
        let float_input = || input().map_or(0.0, |n| self.float_at(path, n, params, depth + 1));
        match graph.class(node) {
            "CNmIDComparisonNode" => {
                // An unset ID is the empty ID, which some conditions list (`action in ["", action_idle]`).
                let value = input()
                    .and_then(|n| self.id(path, n, params))
                    .unwrap_or_default();
                id_matches(raw, &value) == (text("m_comparison") != "DoesntMatch")
            }
            "CNmAndNode" => {
                let all = graph.children(node, "m_conditionNodeIndices");
                !all.is_empty()
                    && all
                        .iter()
                        .all(|&c| self.condition_at(path, c, params, depth + 1))
            }
            "CNmOrNode" => graph
                .children(node, "m_conditionNodeIndices")
                .iter()
                .any(|&c| self.condition_at(path, c, params, depth + 1)),
            "CNmNotNode" => input().is_some_and(|c| !self.condition_at(path, c, params, depth + 1)),
            "CNmFloatComparisonNode" => {
                let lhs = float_input();
                let rhs = match graph.child(node, "m_nComparandValueNodeIdx") {
                    Some(c) => self.float_at(path, c, params, depth + 1),
                    None => graph.number(node, "m_flComparisonValue").unwrap_or(0.0) as f32,
                };
                let epsilon = graph.number(node, "m_flEpsilon").unwrap_or(0.0) as f32;
                match text("m_comparison") {
                    "GreaterThanEqual" => lhs >= rhs,
                    "LessThanEqual" => lhs <= rhs,
                    "GreaterThan" => lhs > rhs,
                    "LessThan" => lhs < rhs,
                    "NearEqual" => (lhs - rhs).abs() <= epsilon,
                    _ => false,
                }
            }
            "CNmFloatRangeComparisonNode" => {
                let value = float_input();
                let range = raw.get("m_range");
                let at = |k: &str| {
                    range
                        .and_then(|r| r.get(k))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0) as f32
                };
                let (min, max) = (at("m_flMin"), at("m_flMax"));
                if raw
                    .get("m_bIsInclusiveCheck")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
                {
                    (min..=max).contains(&value)
                } else {
                    value > min && value < max
                }
            }
            "CNmControlParameterBoolNode" => graph
                .parameters
                .get(node)
                .and_then(|name| params.bools.get(name))
                .copied()
                .unwrap_or(false),
            // ID events list their IDs in `m_eventIDs`; graph events their rules in `m_conditions[].m_eventID`.
            "CNmIDEventConditionNode" | "CNmGraphEventConditionNode" => {
                let listed = raw
                    .get("m_eventIDs")
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str);
                let ruled = raw
                    .get("m_conditions")
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|c| c.get("m_eventID").and_then(Value::as_str));
                listed.chain(ruled).any(|id| params.events.contains(id))
            }
            _ => false,
        }
    }
}

/// Blends `layer` over `pose` bone by bone (parent space) by `weight` times the mask's weight of the bone: an
/// overlay layer.
fn overlay(pose: &mut [Transform], layer: &[Transform], weight: f32, mask: Option<&BoneMask>) {
    for (bone, (p, l)) in pose.iter_mut().zip(layer).enumerate() {
        let w = weight * mask.map_or(1.0, |m| m.0.get(bone).copied().unwrap_or(0.0));
        if w > 0.0 {
            *p = p.lerp(l, w.min(1.0));
        }
    }
}

/// Whether a clip or blend node loops (`m_bAllowLooping`).
fn looping(graph: &Graph, node: usize) -> bool {
    graph
        .nodes
        .get(node)
        .and_then(|n| n.get("m_bAllowLooping"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether `id` is among an ID comparison's `m_comparisionIDs` (CS2's spelling).
fn id_matches(raw: &Value, id: &str) -> bool {
    raw.get("m_comparisionIDs")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .any(|v| v.as_str() == Some(id))
}

fn numbers(value: Option<&Value>) -> Vec<f32> {
    let mut out = Vec::new();
    if let Some(v) = value {
        crate::map::numbers(v, &mut out);
    }
    out
}

/// The option a value in 0 – 1 selects when options take `weights` shares of the range (equal shares without
/// weights).
fn pick_weighted(options: &[usize], weights: &[f32], value: f32) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let shares: Vec<f32> = if weights.len() == options.len() && weights.iter().sum::<f32>() > 0.0 {
        weights.to_vec()
    } else {
        vec![1.0; options.len()]
    };
    let total: f32 = shares.iter().sum();
    let mut edge = 0.0;
    for (option, share) in options.iter().zip(&shares) {
        edge += share / total;
        if value < edge {
            return Some(*option);
        }
    }
    options.last().copied()
}

/// A `m_curve` (Hermite spline: points `x`, `y` with incoming and outgoing slopes) at `x`, held flat outside.
fn curve(curve: Option<&Value>, x: f32) -> f32 {
    let points: Vec<[f32; 4]> = curve
        .and_then(|c| c.get("m_spline"))
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let at = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
            [
                at("x"),
                at("y"),
                at("m_flSlopeIncoming"),
                at("m_flSlopeOutgoing"),
            ]
        })
        .collect();
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return x;
    };
    if x <= first[0] {
        return first[1];
    }
    if x >= last[0] {
        return last[1];
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if x <= b[0] {
            let width = b[0] - a[0];
            if width <= 0.0 {
                return b[1];
            }
            let t = (x - a[0]) / width;
            let (t2, t3) = (t * t, t * t * t);
            // Cubic Hermite with the slopes scaled to the segment's width.
            return (2.0 * t3 - 3.0 * t2 + 1.0) * a[1]
                + (t3 - 2.0 * t2 + t) * a[3] * width
                + (-2.0 * t3 + 3.0 * t2) * b[1]
                + (t3 - t2) * b[2] * width;
        }
    }
    last[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A KV3 value from JSON (objects, arrays, numbers, strings, booleans).
    fn kv3(json: serde_json::Value) -> Value {
        match json {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::Float(n.as_f64().unwrap_or(0.0)),
            },
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(a) => Value::Array(a.into_iter().map(kv3).collect()),
            serde_json::Value::Object(o) => {
                Value::Object(o.into_iter().map(|(k, v)| (k, kv3(v))).collect())
            }
        }
    }

    /// A clip whose one bone moves along x from `from` to `to` over `duration` seconds (two frames).
    fn moving_clip(from: f32, to: f32, duration: f32) -> Clip {
        Clip {
            frames: 2,
            duration,
            tracks: vec![crate::anim::TrackSettings {
                translation_range: [(from, to - from), (0.0, 0.0), (0.0, 0.0)],
                scale_range: (1.0, 0.0),
                constant_rotation: [0.0, 0.0, 0.0, 1.0],
                rotation_static: true,
                translation_static: false,
                scale_static: true,
            }],
            data: vec![0, 0, 0, 65535, 0, 0],
            offsets: vec![0, 3],
            ..Clip::default()
        }
    }

    /// A graph exercising the node kinds the bake uses:
    ///
    /// - node 4, `SM`: state `Stand` (entry `NOT crouched`: a 1D blend on `speed` of clips a and b) or `Crouch`
    ///   (entry `crouched`: a pose of clip b at a curve of `speed`);
    /// - node 13: a weighted selector (1 : 3) on `variation`;
    /// - node 17, `Defuse`: no entry conditions, default state `Enter` (clip a, not looping), which moves on to
    ///   `Loop` (clip b, looping) once completed;
    /// - node 24: an index selector on `index` whose second option has no clip.
    fn library() -> Library {
        // Control parameter `i` is node `i`: `index` is node 23.
        let mut parameters = vec!["speed", "crouched", "variation", "kind"];
        parameters.extend(std::iter::repeat_n("", 19));
        parameters.push("index");
        let graph = serde_json::json!({
            "m_nRootNodeIdx": 4,
            "m_controlParameterIDs": parameters,
            "m_nodePaths": ["speed", "crouched", "variation", "kind", "SM", "SM/Stand", "SM/Stand/Blend", "a", "b",
                "SM/Crouch", "SM/Crouch/Pose", "curve", "not", "selector", "c", "d", "is_b",
                "Defuse", "Defuse/Loop", "Defuse/Loop/b", "Defuse/Enter", "Defuse/Enter/a", "completed",
                "index", "pick", "pick/a", "pick/none", "pick/b"],
            "m_resources": ["a.vnmclip", "b.vnmclip"],
            "m_nodes": [
                { "_class": "CNmControlParameterFloatNode::CDefinition" },
                { "_class": "CNmControlParameterBoolNode::CDefinition" },
                { "_class": "CNmControlParameterFloatNode::CDefinition" },
                { "_class": "CNmControlParameterIDNode::CDefinition" },
                { "_class": "CNmStateMachineNode::CDefinition", "m_nDefaultStateIndex": 0, "m_stateDefinitions": [
                    { "m_nStateNodeIdx": 5, "m_nEntryConditionNodeIdx": 12, "m_transitionDefinitions": [
                        { "m_nTargetStateIdx": 1, "m_nConditionNodeIdx": 1, "m_nTransitionNodeIdx": -1 } ] },
                    { "m_nStateNodeIdx": 9, "m_nEntryConditionNodeIdx": 1 } ] },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 6 },
                { "_class": "CNmBlend1DNode::CDefinition", "m_sourceNodeIndices": [7, 8],
                  "m_nInputParameterValueNodeIdx": 0, "m_bAllowLooping": false,
                  "m_parameterization": { "m_blendRanges": [ { "m_nInputIdx0": 0, "m_nInputIdx1": 1,
                      "m_parameterValueRange": { "m_flMin": 100.0, "m_flMax": 200.0 } } ] } },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 0, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 1, "m_flSpeedMultiplier": 0.5 },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 10 },
                { "_class": "CNmAnimationPoseNode::CDefinition", "m_nDataSlotIdx": 1, "m_nPoseTimeValueNodeIdx": 11,
                  "m_inputTimeRemapRange": { "m_flMin": 3.4e38, "m_flMax": -3.4e38 } },
                { "_class": "CNmFloatCurveNode::CDefinition", "m_nInputValueNodeIdx": 0, "m_curve": { "m_spline": [
                    { "x": 0.0, "y": 1.0, "m_flSlopeIncoming": 0.0, "m_flSlopeOutgoing": -0.02 },
                    { "x": 50.0, "y": 0.0, "m_flSlopeIncoming": -0.02, "m_flSlopeOutgoing": 0.0 } ] } },
                { "_class": "CNmNotNode::CDefinition", "m_nInputValueNodeIdx": 1 },
                { "_class": "CNmParameterizedClipSelectorNode::CDefinition", "m_optionNodeIndices": [14, 15],
                  "m_optionWeights": [1, 3], "m_parameterNodeIdx": 2, "m_bHasWeightsSet": true },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 0, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 1, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmIDComparisonNode::CDefinition", "m_nInputValueNodeIdx": 3,
                  "m_comparison": "Matches", "m_comparisionIDs": ["kind_b"] },
                { "_class": "CNmStateMachineNode::CDefinition", "m_nDefaultStateIndex": 1, "m_stateDefinitions": [
                    { "m_nStateNodeIdx": 18, "m_nEntryConditionNodeIdx": -1 },
                    { "m_nStateNodeIdx": 20, "m_nEntryConditionNodeIdx": -1, "m_transitionDefinitions": [
                        { "m_nTargetStateIdx": 0, "m_nConditionNodeIdx": 22, "m_nTransitionNodeIdx": -1 } ] } ] },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 19 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 1, "m_flSpeedMultiplier": 1.0,
                  "m_bAllowLooping": true },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 21 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 0, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmStateCompletedConditionNode::CDefinition" },
                { "_class": "CNmControlParameterFloatNode::CDefinition" },
                { "_class": "CNmParameterizedClipSelectorNode::CDefinition", "m_optionNodeIndices": [25, 26, 27],
                  "m_parameterNodeIdx": 23, "m_bIgnoreInvalidOptions": true, "m_bHasWeightsSet": false },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 0, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": -1, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 1, "m_flSpeedMultiplier": 1.0 }
            ]
        });
        let mut library = Library::new();
        library.insert_graph("g", Graph::parse(&kv3(graph)).expect("graph"));
        // Clip a: x 0 → 1 over 1 s; clip b: x 2 → 4 over 2 s.
        library.insert_clip("a.vnmclip", moving_clip(0.0, 1.0, 1.0));
        library.insert_clip("b.vnmclip", moving_clip(2.0, 4.0, 2.0));
        library
    }

    fn x(pose: Option<Vec<Transform>>) -> f32 {
        pose.expect("a pose")[0].translation[0]
    }

    /// A track at a fixed translation `(x, 0, 0)`.
    fn still_track(x: f32) -> crate::anim::TrackSettings {
        crate::anim::TrackSettings {
            translation_range: [(x, 0.0), (0.0, 0.0), (0.0, 0.0)],
            scale_range: (1.0, 0.0),
            constant_rotation: [0.0, 0.0, 0.0, 1.0],
            rotation_static: true,
            translation_static: true,
            scale_static: true,
        }
    }

    /// A graph laid out like a viewmodel weapon graph (`viewmodel_gun.vnmgraph+…`), two bones:
    ///
    /// - node 4 (root), `Scale`: bones scaled by mask `tip` unless `silenced`;
    /// - node 5, a layer blend: base `actions` (node 8: `ATK0` / `ATK2`, both clip a, which switch to each other on
    ///   `action_reset` during an attack, over 0.25 s while `silenced`, else 0.5 s: a duration override; `ATK0`
    ///   sends `ATK_ON` while it plays), one overlay state machine layer (node 13) whose `Empty` state (when
    ///   30 − `ammo` ≥ 30) holds clip b's pose, masked by its state's mask `tip`;
    /// - node 24: whether the ID switch on `silenced` gives `on`; node 27: whether `ammo` is even (`Mod` 2).
    ///
    /// Clip a: bone 0 x 0 → 1 over 2 s, bone 1 at 5, an event `E` over 20 – 40 % of it (0.4 – 0.8 s), and secondary
    /// animations on `weapon.vnmskel` (one bone at 7) and `other.vnmskel` (one bone at 9). Clip b: bones at 10 and
    /// 20, and on `other.vnmskel` 11; mask `tip` has weights for `weapon.vnmskel` only.
    fn viewmodel_library() -> Library {
        let parameters = vec!["ammo", "silenced", "action_reset", "action"];
        let graph = serde_json::json!({
            "m_nRootNodeIdx": 4,
            "m_controlParameterIDs": parameters,
            "m_nodePaths": ["ammo", "silenced", "action_reset", "action", "Scale", "Layers", "tip", "not",
                "actions", "actions/ATK0", "actions/ATK2", "reset", "transition", "Empty", "Empty/Empty",
                "Empty/Off", "is_empty", "missing", "Empty/Empty/pose", "a", "attacking", "thirty", "switch", "off",
                "is_on", "override", "parity", "even"],
            "m_resources": ["a.vnmclip", "b.vnmclip"],
            "m_nodes": [
                { "_class": "CNmControlParameterFloatNode::CDefinition" },
                { "_class": "CNmControlParameterBoolNode::CDefinition" },
                { "_class": "CNmControlParameterBoolNode::CDefinition" },
                { "_class": "CNmControlParameterIDNode::CDefinition" },
                { "_class": "CNmScaleNode::CDefinition", "m_nChildNodeIdx": 5, "m_nMaskNodeIdx": 6,
                  "m_nEnableNodeIdx": 7 },
                { "_class": "CNmLayerBlendNode::CDefinition", "m_nBaseNodeIdx": 8, "m_layerDefinition": [
                    { "m_nInputNodeIdx": 13, "m_nWeightValueNodeIdx": -1, "m_nBoneMaskValueNodeIdx": -1,
                      "m_bIsStateMachineLayer": true, "m_blendMode": "Overlay" } ] },
                { "_class": "CNmBoneMaskNode::CDefinition", "m_boneMaskID": "tip" },
                { "_class": "CNmNotNode::CDefinition", "m_nInputValueNodeIdx": 1 },
                { "_class": "CNmStateMachineNode::CDefinition", "m_nDefaultStateIndex": 0, "m_stateDefinitions": [
                    { "m_nStateNodeIdx": 9, "m_nEntryConditionNodeIdx": -1, "m_transitionDefinitions": [
                        { "m_nTargetStateIdx": 1, "m_nConditionNodeIdx": 11, "m_nTransitionNodeIdx": 12 } ] },
                    { "m_nStateNodeIdx": 10, "m_nEntryConditionNodeIdx": -1, "m_transitionDefinitions": [
                        { "m_nTargetStateIdx": 0, "m_nConditionNodeIdx": 11, "m_nTransitionNodeIdx": 12 } ] } ] },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 19, "m_executeEvents": ["ATK_ON"] },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 19 },
                { "_class": "CNmAndNode::CDefinition", "m_conditionNodeIndices": [2, 20] },
                { "_class": "CNmTransitionNode::CDefinition", "m_flDuration": 0.1, "m_nDurationOverrideNodeIdx": 25 },
                { "_class": "CNmStateMachineNode::CDefinition", "m_nDefaultStateIndex": 1, "m_stateDefinitions": [
                    { "m_nStateNodeIdx": 14, "m_nEntryConditionNodeIdx": 16 },
                    { "m_nStateNodeIdx": 15, "m_nEntryConditionNodeIdx": -1 } ] },
                { "_class": "CNmStateNode::CDefinition", "m_nChildNodeIdx": 18, "m_nLayerBoneMaskNodeIdx": 6 },
                { "_class": "CNmStateNode::CDefinition" },
                { "_class": "CNmFloatComparisonNode::CDefinition", "m_nInputValueNodeIdx": 17,
                  "m_comparison": "GreaterThanEqual", "m_flComparisonValue": 30.0 },
                { "_class": "CNmFloatMathNode::CDefinition", "m_nInputValueNodeIdxA": 21,
                  "m_nInputValueNodeIdxB": 0, "m_operator": "Sub" },
                { "_class": "CNmAnimationPoseNode::CDefinition", "m_nDataSlotIdx": 1,
                  "m_nPoseTimeValueNodeIdx": -1, "m_flUserSpecifiedTime": 0.0 },
                { "_class": "CNmClipNode::CDefinition", "m_nDataSlotIdx": 0, "m_flSpeedMultiplier": 1.0 },
                { "_class": "CNmIDComparisonNode::CDefinition", "m_nInputValueNodeIdx": 3,
                  "m_comparison": "Matches", "m_comparisionIDs": ["action_attack"] },
                { "_class": "CNmConstFloatNode::CDefinition", "m_flValue": 30.0 },
                { "_class": "CNmIDSwitchNode::CDefinition", "m_nSwitchValueNodeIdx": 1,
                  "m_nTrueValueNodeIdx": -1, "m_nFalseValueNodeIdx": 23, "m_trueValue": "on",
                  "m_falseValue": "" },
                { "_class": "CNmConstIDNode::CDefinition", "m_value": "off" },
                { "_class": "CNmIDComparisonNode::CDefinition", "m_nInputValueNodeIdx": 22,
                  "m_comparison": "Matches", "m_comparisionIDs": ["on"] },
                { "_class": "CNmFloatSwitchNode::CDefinition", "m_nSwitchValueNodeIdx": 1,
                  "m_nTrueValueNodeIdx": -1, "m_nFalseValueNodeIdx": -1, "m_flTrueValue": 0.25,
                  "m_flFalseValue": 0.5 },
                { "_class": "CNmFloatMathNode::CDefinition", "m_nInputValueNodeIdxA": 0,
                  "m_nInputValueNodeIdxB": -1, "m_flValueB": 2.0, "m_operator": "Mod" },
                { "_class": "CNmFloatComparisonNode::CDefinition", "m_nInputValueNodeIdx": 26,
                  "m_comparison": "NearEqual", "m_flComparisonValue": 0.0, "m_flEpsilon": 0.01 }
            ]
        });
        let mut library = Library::new();
        library.insert_graph("v", Graph::parse(&kv3(graph)).expect("graph"));
        let still = |skeleton: &str, x: f32| Clip {
            skeleton: skeleton.into(),
            frames: 1,
            tracks: vec![still_track(x)],
            offsets: vec![0],
            ..Clip::default()
        };
        let mut a = moving_clip(0.0, 1.0, 2.0);
        a.tracks.push(still_track(5.0));
        a.events.push(crate::anim::ClipEvent {
            id: "E".into(),
            start: 0.2,
            duration: 0.2,
        });
        a.secondary = vec![still("weapon.vnmskel", 7.0), still("other.vnmskel", 9.0)];
        library.insert_clip("a.vnmclip", a);
        library.insert_clip(
            "b.vnmclip",
            Clip {
                frames: 1,
                tracks: vec![still_track(10.0), still_track(20.0)],
                offsets: vec![0],
                secondary: vec![still("other.vnmskel", 11.0)],
                ..Clip::default()
            },
        );
        library.insert_mask(None, "tip", BoneMask(vec![0.0, 1.0]));
        library.insert_mask(Some("weapon.vnmskel"), "tip", BoneMask(vec![0.0]));
        library
    }

    #[test]
    fn viewmodel_graphs_mask_scale_and_restart() {
        let library = viewmodel_library();
        let eval = Evaluator::new(&library);
        let firing = Params::default()
            .float("ammo", 5.0)
            .flag("silenced", true)
            .id("action", "action_attack");
        let pose = eval.pose("v", 4, &firing, 1.0).unwrap().expect("a pose");
        let xs = |p: &[Transform]| p.iter().map(|t| t.translation[0]).collect::<Vec<_>>();
        let scales = |p: &[Transform]| p.iter().map(|t| t.scale).collect::<Vec<_>>();
        assert_eq!(xs(&pose), [0.5, 5.0]);
        assert_eq!(scales(&pose), [1.0, 1.0]);
        // Out of ammo (30 − 0 ≥ 30): the empty pose over the bones of its state's mask only.
        let empty = firing.clone().float("ammo", 0.0);
        assert_eq!(
            xs(&eval.pose("v", 4, &empty, 1.0).unwrap().unwrap()),
            [0.5, 20.0]
        );
        // Not silenced: the scale node takes the mask's weights as scales.
        let bare = firing.clone().flag("silenced", false);
        assert_eq!(
            scales(&eval.pose("v", 4, &bare, 0.5).unwrap().unwrap()),
            [0.0, 1.0]
        );

        // The weapon's own bone: clip a's secondary animation, scaled by the mask's weight for its skeleton; clip b
        // has none, so the empty layer leaves it.
        let weapon = Evaluator::secondary(&library, "weapon.vnmskel");
        let w = weapon.pose("v", 4, &firing, 0.5).unwrap().unwrap();
        assert_eq!((xs(&w), scales(&w)), (vec![7.0], vec![1.0]));
        assert_eq!(
            scales(&weapon.pose("v", 4, &bare, 0.5).unwrap().unwrap()),
            [0.0]
        );
        assert_eq!(
            xs(&weapon.pose("v", 4, &empty, 0.5).unwrap().unwrap()),
            [7.0]
        );
        // A skeleton the layer's mask lists no weights for: the layer leaves it (clip b's 11 does not show).
        let other = Evaluator::secondary(&library, "other.vnmskel");
        assert_eq!(
            xs(&other.pose("v", 4, &empty, 0.5).unwrap().unwrap()),
            [9.0]
        );

        // A second shot restarts the attack through its twin state, over the override's 0.25 s while silenced and
        // 0.5 s otherwise; outside an attack nothing restarts. `ATK0` sends its event while it plays.
        let states = eval.states("v", 4, &firing, 0.0).unwrap();
        assert_eq!(states.first(), Some(&("v".to_owned(), 9)));
        assert_eq!(eval.restart_time(&states, &firing), Some(0.25));
        assert_eq!(eval.restart_time(&states, &bare), Some(0.5));
        let idle = firing.clone().id("action", "action_idle");
        assert_eq!(eval.restart_time(&states, &idle), None);
        assert!(eval.state_events(&states).contains("ATK_ON"));
        // The machine under the scale node and the layers is the one asked.
        assert_eq!(
            eval.leaves(
                "v",
                4,
                &firing,
                0.1,
                &firing.clone().flag("action_reset", true)
            )
            .unwrap(),
            Leave::Leaves {
                state: 10,
                blend: 0.25
            }
        );
        // Clip a's event is on over 20 – 40 % of its 2 s: 0.4 – 0.8 s.
        assert!(eval.events_at("v", 4, &firing, 0.6).unwrap().contains("E"));
        assert!(eval.events_at("v", 4, &firing, 0.3).unwrap().is_empty());
        assert!(eval.events_at("v", 4, &firing, 0.9).unwrap().is_empty());
        // The ID switch: `on` while silenced, the constant `off` otherwise.
        assert!(eval.condition("v", 24, &firing));
        assert!(!eval.condition("v", 24, &bare));
        // Float maths: `ammo Mod 2` is 0 for an even count.
        assert!(eval.condition("v", 27, &firing.clone().float("ammo", 4.0)));
        assert!(!eval.condition("v", 27, &firing));
    }

    #[test]
    fn blends_run_their_sources_in_step() {
        let library = library();
        let eval = Evaluator::new(&library);
        // Halfway through the blend range: half of each clip. The blend lasts the weighted average of a (1 s) and
        // b (2 s at half speed: 4 s), 2.5 s; 1.25 s in, both sources are halfway: a at 0.5, b at 3.
        let standing = Params::default().float("speed", 150.0);
        assert!((eval.duration("g", 4, &standing).unwrap() - 2.5).abs() < 1e-5);
        let at = |t: f32| x(eval.pose("g", 4, &standing, t).unwrap());
        assert!((at(1.25) - 1.75).abs() < 1e-4, "{}", at(1.25));
        // Not looping: held at the end.
        assert!((at(10.0) - 2.5).abs() < 1e-4, "{}", at(10.0));
        // Below and above the range: one clip only.
        let slow = Params::default().float("speed", 50.0);
        assert!((x(eval.pose("g", 4, &slow, 0.5).unwrap()) - 0.5).abs() < 1e-4);
        let fast = Params::default().float("speed", 500.0);
        assert!((x(eval.pose("g", 4, &fast, 2.0).unwrap()) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn machines_enter_complete_and_report_their_states() {
        let library = library();
        let eval = Evaluator::new(&library);
        // Crouched: the second state; its pose node is at the curve's value (speed 25 → 0.5: x = 3).
        let crouched = Params::default()
            .flag("crouched", true)
            .float("speed", 25.0);
        assert!((x(eval.pose("g", 4, &crouched, 0.0).unwrap()) - 3.0).abs() < 1e-3);
        assert_eq!(
            eval.states("g", 4, &crouched, 0.0).unwrap(),
            vec![("g".into(), 9)]
        );
        // Defuse: no entry condition holds, so the default state (Enter, 1 s) plays, then Loop (b, looping).
        let none = Params::default();
        assert!((x(eval.pose("g", 17, &none, 0.5).unwrap()) - 0.5).abs() < 1e-4);
        assert!((x(eval.pose("g", 17, &none, 1.5).unwrap()) - 2.5).abs() < 1e-4);
        assert!(
            (x(eval.pose("g", 17, &none, 3.5).unwrap()) - 2.5).abs() < 1e-4,
            "looped"
        );
        assert_eq!(
            eval.states("g", 17, &none, 0.5).unwrap(),
            vec![("g".into(), 20)]
        );
        assert_eq!(
            eval.states("g", 17, &none, 1.5).unwrap(),
            vec![("g".into(), 18)]
        );
        // Stand → Crouch has a transition (no transition node here: 0 s); Crouch → Stand has none.
        let stand = eval.states("g", 4, &none, 0.0).unwrap();
        let crouch = eval.states("g", 4, &crouched, 0.0).unwrap();
        assert_eq!(eval.transition_time(&stand, &crouch, &crouched), Some(0.0));
        assert_eq!(eval.transition_time(&crouch, &stand, &none), None);
        // Run as CS2 runs it: standing leaves for crouched (state node 9), crouched never leaves.
        assert_eq!(
            eval.leaves("g", 4, &none, 0.3, &crouched).unwrap(),
            Leave::Leaves {
                state: 9,
                blend: 0.0
            }
        );
        assert_eq!(
            eval.leaves("g", 4, &none, 0.3, &none).unwrap(),
            Leave::Stays
        );
        assert_eq!(
            eval.leaves("g", 4, &crouched, 0.3, &none).unwrap(),
            Leave::Stays
        );
        // A clip node is no machine: nothing to ask.
        assert_eq!(
            eval.leaves("g", 7, &none, 0.3, &none).unwrap(),
            Leave::Unknown
        );
        assert_eq!(eval.transition_time(&stand, &stand, &none), None);
        // The dominant clip and its time: Enter's clip a, 0.25 s in.
        let (clip, t) = eval.dominant_clip("g", 17, &none, 0.25).unwrap().unwrap();
        assert!((clip.duration - 1.0).abs() < 1e-6 && (t - 0.25).abs() < 1e-6);
    }

    #[test]
    fn curves_selectors_and_conditions() {
        let library = library();
        let eval = Evaluator::new(&library);
        let curve_at = |speed: f32| eval.float("g", 11, &Params::default().float("speed", speed));
        assert!((curve_at(0.0) - 1.0).abs() < 1e-5);
        assert!((curve_at(25.0) - 0.5).abs() < 1e-5, "{}", curve_at(25.0));
        assert!((curve_at(80.0)).abs() < 1e-5);
        // Weights 1 : 3: a value below 0.25 picks the first option.
        let pick = |v: f32| {
            x(eval
                .pose("g", 13, &Params::default().float("variation", v), 0.0)
                .unwrap())
        };
        assert_eq!(pick(0.1), 0.0);
        assert_eq!(pick(0.5), 2.0);
        // Index selection; option 1 has no clip and is skipped.
        let index = |v: f32| {
            x(eval
                .pose("g", 24, &Params::default().float("index", v), 0.0)
                .unwrap())
        };
        assert_eq!(index(0.0), 0.0);
        assert_eq!(index(2.0), 2.0);
        assert_eq!(index(1.0), 0.0);
        assert!(eval.condition("g", 16, &Params::default().id("kind", "kind_b")));
        assert!(!eval.condition("g", 16, &Params::default().id("kind", "kind_a")));
        assert!(!eval.condition("g", 16, &Params::default()));
    }

    #[test]
    fn loading_follows_the_nodes_it_can_play() {
        let source = library();
        let mut library = Library::new();
        let mut graph_reads = Vec::new();
        let mut clip_reads = Vec::new();
        library
            .load::<AnimError>(
                "g",
                Some(17),
                &mut |path| {
                    graph_reads.push(path.to_owned());
                    Ok(source.graph(path).unwrap().clone())
                },
                &mut |path| {
                    clip_reads.push(path.to_owned());
                    Ok(source.clip(path).unwrap().clone())
                },
            )
            .unwrap();
        clip_reads.sort();
        assert_eq!(graph_reads, ["g"]);
        assert_eq!(clip_reads, ["a.vnmclip", "b.vnmclip"]);
        assert!(library.clip("a.vnmclip").is_some());
    }
}
