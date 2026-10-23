//! Each player's first-person view over a round: CS2's viewmodel (the arms and the weapon in hand), baked from the
//! demo's weapon states (M5, decision 016).
//!
//! # What CS2 plays
//!
//! The viewmodel is one rig, `viewmodel.vnmskel` (56 bones: `root_motion` at the camera, `wpn` where the weapon
//! sits, `wpnHand_L` / `_R` where the hands hold it, the arms and fingers), animated by
//! `animation/graphs/viewmodel/viewmodel.vnmgraph`. That graph picks the weapon's own graph by `weapon_category` and
//! `weapon_type` (`viewmodel_gun.vnmgraph+ak47`, `viewmodel_knife.vnmgraph+default_ct`, `viewmodel_grenade…`; the
//! C4 and the Healthshot have their states in the main graph), then puts the hands on the weapon with two
//! two-bone IK nodes (`RightHandIK`: `hand_R` onto `wpnHand_R`; `LeftHandIK`: `hand_L` onto `wpnHand_L`, off while a
//! clip has the `WPN_DISABLE_LEFT_HAND_IK` event, blending over 0.2 s). A weapon graph is a state machine
//! (`actions`): `Attack` (shots, knife swings, throws), and `Actions` with `Idle`, `Deploying`, `Reload`, `Inspect`
//! and the silencer's `SilencerOn` / `SilencerOff`. Its clips (`animation/anims/viewmodel/<set>/<weapon>/`) are
//! authored in the camera's frame — the AK-47's idle holds `wpn` 14.8 inches ahead of the eye, 5.2 to the right and
//! 2.5 down — and each carries a secondary animation for the weapon's own bones (`ak47.vnmskel`: the bolt, the
//! magazine), attached under `wpn`.
//!
//! # What OFAST does
//!
//! Like the body's weapons layer ([`super::bake`]), from the weapon in hand and its gameplay state
//! (`m_iWeaponGameplayAnimState` and its timestamp): every change of state is a **segment** — an `action` and
//! `attack_type` ([`weapon_action`], [`passive_action`]: idle 50, drawing 11, shooting 100 / 101 with a new
//! timestamp per shot, reloading 800 then its outro 801, inspecting 1000 then 1001, knife swings, grenade throws),
//! from the tick the state began. A segment's pose is the weapon graph evaluated by [`super::eval`] that long after
//! it began, for the arms and, through [`Evaluator::secondary`], for the weapon's bones; it blends in from the
//! segment before over CS2's transition time between their states ([`Evaluator::transition_time`]), or the twin
//! states' when an action starts over (a shot after a shot: [`Evaluator::restart_time`]). The events on in the old
//! segment's clip when the new one begins go with it (an inspect begun during a draw starts from the draw's
//! version: `WPN_INSPECT_USE_DRAW_VERSION`). A new weapon in hand starts afresh: CS2 swaps the viewmodel's weapon.
//! Then the hand IK.
//!
//! What a demo cannot tell (decision 016): CS2's client also sways the viewmodel behind fast turns, bobs it while
//! running and dips it on landing (hard-coded, no networked state); none of that is baked yet.

use std::collections::{HashMap, HashSet};

use super::aim::reach_bone;
use super::bake::{DEFAULT_BLEND, STATE_TIMESTAMP_SLACK, frame_samples, mix, variation};
use super::eval::{Evaluator, Library, Params};
use super::states::{passive_action, weapon_action};
use super::{AnimError, NmSkeleton, Transform};
use crate::demo::players::{ActiveWeapon, PlayerSample, RoundTrack};

/// The viewmodel graph: picks each weapon's graph and puts the hands on it.
pub const VIEWMODEL_GRAPH: &str = "animation/graphs/viewmodel/viewmodel.vnmgraph";
/// The arms model whose skeleton the viewmodel clips are for (82 bones: the 56 of `viewmodel.vnmskel`, the twist
/// bones, and a body nobody sees).
pub const ARMS_MODEL: &str = "weapons/models/shared/arms/weapon_arms.vmdl";
/// `LeftHandIK`'s blend time (`m_flBlendTimeSeconds`) in and out of `WPN_DISABLE_LEFT_HAND_IK`.
const LEFT_HAND_IK_BLEND: f32 = 0.2;
/// The event during which the left hand leaves the weapon.
const LEFT_HAND_OFF: &str = "WPN_DISABLE_LEFT_HAND_IK";
/// The event during which the silencer shows although it is not on (screwing it on or off).
const SHOW_SILENCER: &str = "WPN_SHOW_SILENCER";

/// One weapon's first-person animation: its graph, and the skeleton of its own bones.
#[derive(Debug, Clone)]
pub struct ViewWeapon {
    /// Type ID (`weapon_ak47`), the graphs' `weapon_type`.
    pub weapon: String,
    /// Category ID (`weapon_category_rifle`), the graphs' `weapon_category`.
    pub category: String,
    /// The graph its actions are evaluated in (its own, or [`VIEWMODEL_GRAPH`] for the C4 and the Healthshot), and
    /// the node (the graph's root, or the weapon's state).
    pub graph: String,
    /// See [`ViewWeapon::graph`].
    pub node: usize,
    /// The weapon's own animation skeleton, a secondary skeleton of the viewmodel's under `wpn`
    /// (`animation/skeletons/weapons/ak47.vnmskel`), when its clips animate one: path and bones.
    pub skeleton: Option<(String, NmSkeleton)>,
}

/// What [`bake_viewmodel`] needs from CS2: the graphs, the rig and the weapons.
#[derive(Debug)]
pub struct ViewmodelRig {
    /// The weapons' graphs and their clips, with the viewmodel skeleton's masks (primary and secondary).
    pub library: Library,
    /// The clip skeleton (`viewmodel.vnmskel`).
    pub skeleton: NmSkeleton,
    /// The weapons the round's players hold, by item definition index.
    pub weapons: HashMap<u32, ViewWeapon>,
}

/// The bones the hand IK moves and aims at.
#[derive(Debug, Clone, Copy)]
struct Hands {
    /// `[upper, lower, hand]` of the left arm, then the right.
    arms: [[usize; 3]; 2],
    /// `wpnHand_L`, `wpnHand_R`.
    targets: [usize; 2],
}

impl Hands {
    /// The IK nodes name only their effector (`hand_L`, `hand_R`): the chain is the hand, its parent and its
    /// grandparent — in `viewmodel.vnmskel` `armUpperShoulder_L` → `arm_lower_L` → `hand_L` (`arm_upper_L` is a
    /// helper bone under the forearm).
    fn find(skeleton: &NmSkeleton) -> Option<Self> {
        let f = |name: &str| skeleton.find(name);
        let arm = |hand: usize| -> Option<[usize; 3]> {
            let lower = skeleton.parents.get(hand).copied().flatten()?;
            let upper = skeleton.parents.get(lower).copied().flatten()?;
            Some([upper, lower, hand])
        };
        Some(Self {
            arms: [arm(f("hand_L")?)?, arm(f("hand_R")?)?],
            targets: [f("wpnHand_L")?, f("wpnHand_R")?],
        })
    }
}

/// One frame of a player's first-person view.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewmodelFrame {
    /// The weapon in hand (item definition index).
    pub item: u32,
    /// The arms, one transform per bone of [`ViewmodelRig::skeleton`], relative to its parent; `root_motion` is the
    /// camera (x forward, y left, z up; inches).
    pub arms: Vec<Transform>,
    /// The weapon's own bones ([`ViewWeapon::skeleton`]), relative to their parents, the root relative to `wpn`; a
    /// scale of 0 hides a bone (the M4A1-S's silencer while it is off).
    pub weapon: Option<Vec<Transform>>,
}

/// A weapon action from a tick on: what the graph is asked to play, and how it blends in.
#[derive(Debug, Clone)]
struct Segment {
    item: u32,
    action: &'static str,
    attack: &'static str,
    /// The gameplay state and its timestamp tick that began it.
    state: i64,
    stamp: Option<u32>,
    /// The demo tick it began.
    start: u32,
    /// Rounds in the magazine and in reserve, and the silencer, when it began (the silencer is read again every
    /// tick for the pose: it goes on in the middle of the attach clip).
    ammo: f32,
    reserve: f32,
    silenced: bool,
    /// The weapon's CS:GO-era model (mesh group bit 1, for older skins).
    legacy: bool,
    strength: f32,
    /// `*_variation`, 0 – 1: which of the graph's options plays (two draws, two inspects).
    variation: f32,
    /// Events on in the segment before when this one began: its clip's, and its states' when they had fully
    /// blended in (`WPN_STATE_IDLE`: an inspect from idle blends in over 0.4 s, from anything else 0.65 s).
    events: HashSet<String>,
    /// Index of the segment it blends in from (same weapon), and over how long.
    from: Option<usize>,
    blend: f32,
}

/// Whether gameplay state `now` goes on with `before` rather than starting anew: a reload's outro (801) or an
/// inspect's second part (1001) continue what they follow.
fn continues(before: i64, now: i64) -> bool {
    matches!((before, now), (800, 801) | (1000, 1001))
}

/// Whether a category is a gun's (the viewmodel graph's `weapon_is_gun`: not a knife, grenade or equipment).
fn is_gun(category: &str) -> bool {
    !matches!(
        category,
        "weapon_category_knife" | "weapon_category_grenade" | "weapon_category_equipment"
    )
}

/// A segment's graph parameters (the viewmodel graph's control parameters, which its weapon graphs share).
/// `ammo_max` is the magazine's size (`weapon_ammo_max`, which the demo does not carry: the largest magazine seen).
fn params(weapon: &ViewWeapon, segment: &Segment, ammo_max: f32) -> Params {
    let mut params = Params::default()
        .id("weapon_type", &weapon.weapon)
        .id("weapon_category", &weapon.category)
        .flag("weapon_is_gun", is_gun(&weapon.category))
        .id("action", segment.action)
        .id("attack_type", segment.attack)
        .float("weapon_ammo", segment.ammo)
        .float("weapon_ammo_max", ammo_max.max(segment.ammo))
        .float("weapon_ammo_reserve", segment.reserve)
        .flag("weapon_is_silenced", segment.silenced)
        .flag("weapon_is_legacy_model", segment.legacy)
        .float("attack_throw_strength", segment.strength)
        .float("weapon_action_speedscale", 1.0);
    for variation in [
        "deploy_variation",
        "inspect_variation",
        "idle_variation",
        "attack_variation",
    ] {
        params = params.float(variation, segment.variation);
    }
    params.events = segment.events.clone();
    params
}

/// The rounds in reserve for the graphs: a weapon without reserve ammunition reads as having some (its empty pose,
/// slide back, is for a gun out of everything).
fn reserve_of(weapon: &ActiveWeapon) -> f32 {
    weapon.reserve.map_or(1.0, |r| r as f32)
}

/// The magazine size of each weapon held in the round, by item: the most rounds any of them had in its magazine.
fn magazines(track: &RoundTrack) -> HashMap<u32, f32> {
    let mut out: HashMap<u32, f32> = HashMap::new();
    for w in track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .filter_map(|p| p.rig.weapon.as_ref())
    {
        if let Some(clip) = w.clip {
            let max = out.entry(w.item).or_default();
            *max = max.max(clip as f32);
        }
    }
    out
}

/// What a sample says for the viewmodel: its segment and whether the silencer is on.
type SampleState = Option<(usize, bool)>;

struct Baker<'a> {
    track: &'a RoundTrack,
    rig: &'a ViewmodelRig,
    eval: Evaluator<'a>,
    hands: Option<Hands>,
    /// [`magazines`].
    magazines: HashMap<u32, f32>,
}

impl Baker<'_> {
    fn seconds(&self, ticks: f32) -> f32 {
        ticks.max(0.0) * self.track.tick_interval
    }

    fn params(&self, segment: &Segment) -> Params {
        let weapon = &self.rig.weapons[&segment.item];
        let max = self.magazines.get(&segment.item).copied().unwrap_or(0.0);
        params(weapon, segment, max)
    }

    /// Every segment of a player's round, and each sample's segment and silencer (`None` while dead, absent, or
    /// holding a weapon the rig does not know).
    fn segments(&self, controller: u32) -> Result<(Vec<Segment>, Vec<SampleState>), AnimError> {
        let mut segments: Vec<Segment> = Vec::new();
        let mut of_sample = Vec::with_capacity(self.track.samples.len());
        let mut current: Option<usize> = None;
        for sample in &self.track.samples {
            let tick = sample.tick;
            let player: Option<&PlayerSample> = sample
                .players
                .iter()
                .find(|p| p.controller == controller && p.alive);
            let Some((p, w, weapon)) = player.and_then(|p| {
                let w = p.rig.weapon.as_ref()?;
                Some((p, w, self.rig.weapons.get(&w.item)?))
            }) else {
                current = None;
                of_sample.push(None);
                continue;
            };
            let (action, attack) = weapon_action(w.anim_state, &weapon.category)
                .unwrap_or_else(|| passive_action(w.anim_state));
            let same = current.map(|c| &segments[c]).is_some_and(|s| {
                s.item == w.item
                    && s.action == action
                    && s.attack == attack
                    && (action == "action_idle"
                        || (s.state == w.anim_state && s.stamp == w.state_tick)
                        || continues(s.state, w.anim_state))
            });
            if !same {
                let start = w
                    .state_tick
                    .filter(|&s| {
                        s <= tick && self.seconds((tick - s) as f32) < STATE_TIMESTAMP_SLACK
                    })
                    .unwrap_or(tick);
                let legacy = p
                    .weapons
                    .iter()
                    .find(|c| c.entity == w.entity)
                    .is_some_and(|c| c.mesh_groups & 2 != 0 && c.mesh_groups != u64::MAX);
                let mut next = Segment {
                    item: w.item,
                    action,
                    attack,
                    state: w.anim_state,
                    stamp: w.state_tick,
                    start,
                    ammo: w.clip.map_or(1.0, |c| c as f32),
                    reserve: reserve_of(w),
                    silenced: w.silenced,
                    legacy,
                    strength: w.throw_strength,
                    variation: variation(controller, start),
                    events: HashSet::new(),
                    from: None,
                    blend: 0.0,
                };
                if let Some(c) = current.filter(|&c| segments[c].item == w.item) {
                    let old = &segments[c];
                    let old_params = self.params(old);
                    let at = self.seconds(start as f32 - old.start as f32);
                    let from = self
                        .eval
                        .states(&weapon.graph, weapon.node, &old_params, at)?;
                    next.events =
                        self.eval
                            .events_at(&weapon.graph, weapon.node, &old_params, at)?;
                    if at >= old.blend {
                        next.events.extend(self.eval.state_events(&from));
                    }
                    let new_params = self.params(&next);
                    let to = self
                        .eval
                        .states(&weapon.graph, weapon.node, &new_params, 0.0)?;
                    next.blend = if from == to {
                        self.eval.restart_time(&to, &new_params).unwrap_or(0.0)
                    } else {
                        self.eval
                            .transition_time(&from, &to, &new_params)
                            .unwrap_or(DEFAULT_BLEND)
                    };
                    next.from = Some(c);
                }
                segments.push(next);
                current = Some(segments.len() - 1);
            }
            of_sample.push(current.map(|c| (c, w.silenced)));
        }
        Ok((segments, of_sample))
    }

    /// One segment's parameters at `time` seconds into it: the silencer as it is now (`silenced`), and the clip's
    /// `WPN_SHOW_SILENCER` while it is on (the silencer shows while being screwed on and off: the graph's scale node
    /// hides it unless the weapon is silenced or this event is on). Other events of the clip playing are not added:
    /// the graph's entry conditions, evaluated again at every time here, would read them as if the state were
    /// entered anew.
    fn params_at(&self, segment: &Segment, time: f32, silenced: bool) -> Result<Params, AnimError> {
        let weapon = &self.rig.weapons[&segment.item];
        let mut params = self.params(segment).flag("weapon_is_silenced", silenced);
        if self
            .eval
            .events_at(&weapon.graph, weapon.node, &params, time)?
            .contains(SHOW_SILENCER)
        {
            params.events.insert(SHOW_SILENCER.to_owned());
        }
        Ok(params)
    }

    /// The arms and the weapon's bones of one segment `time` seconds in, before the hand IK.
    fn segment_pose(
        &self,
        segment: &Segment,
        params: &Params,
        time: f32,
    ) -> Result<(Vec<Transform>, Option<Vec<Transform>>), AnimError> {
        let weapon = &self.rig.weapons[&segment.item];
        let arms = self
            .eval
            .pose(&weapon.graph, weapon.node, params, time)?
            .unwrap_or_else(|| self.rig.skeleton.reference.clone());
        let bones = match &weapon.skeleton {
            Some((path, skeleton)) => Some(
                Evaluator::secondary(&self.rig.library, path)
                    .pose(&weapon.graph, weapon.node, params, time)?
                    .unwrap_or_else(|| skeleton.reference.clone()),
            ),
            None => None,
        };
        Ok((arms, bones))
    }

    /// How far the left hand holds the weapon `time` seconds into a segment: 0 during `WPN_DISABLE_LEFT_HAND_IK`,
    /// easing over [`LEFT_HAND_IK_BLEND`] in and out of it (the share of 5 samples over the last 0.2 s at which the
    /// event was off).
    fn left_hand_weight(
        &self,
        segment: &Segment,
        params: &Params,
        time: f32,
    ) -> Result<f32, AnimError> {
        let weapon = &self.rig.weapons[&segment.item];
        const SAMPLES: usize = 5;
        let mut on = 0usize;
        for k in 0..SAMPLES {
            let t = time - LEFT_HAND_IK_BLEND * k as f32 / (SAMPLES - 1) as f32;
            if t >= 0.0
                && self
                    .eval
                    .events_at(&weapon.graph, weapon.node, params, t)?
                    .contains(LEFT_HAND_OFF)
            {
                on += 1;
            }
        }
        Ok(1.0 - on as f32 / SAMPLES as f32)
    }

    fn frame(
        &self,
        segments: &[Segment],
        (index, silenced): (usize, bool),
        tick: f32,
    ) -> Result<ViewmodelFrame, AnimError> {
        let segment = &segments[index];
        let time = self.seconds(tick - segment.start as f32);
        let params = self.params_at(segment, time, silenced)?;
        let (mut arms, mut weapon) = self.segment_pose(segment, &params, time)?;
        if let Some(from) = segment.from.map(|f| &segments[f])
            && segment.blend > 0.0
            && time < segment.blend
        {
            let w = time / segment.blend;
            let old_time = self.seconds(tick - from.start as f32);
            let old_params = self.params_at(from, old_time, silenced)?;
            let (old_arms, old_weapon) = self.segment_pose(from, &old_params, old_time)?;
            arms = mix(old_arms, arms, w);
            weapon = match (old_weapon, weapon) {
                (Some(old), Some(new)) => Some(mix(old, new, w)),
                (_, new) => new,
            };
        }
        if let Some(hands) = self.hands {
            let parents = &self.rig.skeleton.parents;
            reach_bone(&mut arms, parents, hands.arms[1], hands.targets[1], 1.0);
            let left = self.left_hand_weight(segment, &params, time)?;
            reach_bone(&mut arms, parents, hands.arms[0], hands.targets[0], left);
        }
        Ok(ViewmodelFrame {
            item: segment.item,
            arms,
            weapon,
        })
    }
}

/// One player's first-person view at `fps` frames per second from the round's first tick to its last
/// ([`frame_samples`]): `None` while the player has no living pawn or holds a weapon `rig` does not know.
pub fn bake_viewmodel(
    track: &RoundTrack,
    controller: u32,
    rig: &ViewmodelRig,
    fps: f32,
) -> Result<Vec<Option<ViewmodelFrame>>, AnimError> {
    let baker = Baker {
        track,
        rig,
        eval: Evaluator::new(&rig.library),
        hands: Hands::find(&rig.skeleton),
        magazines: magazines(track),
    };
    let (segments, of_sample) = baker.segments(controller)?;
    frame_samples(track, fps)
        .into_iter()
        .map(
            |(tick, index)| match of_sample.get(index).copied().flatten() {
                Some(state) => baker.frame(&segments, state, tick).map(Some),
                None => Ok(None),
            },
        )
        .collect()
}

/// What [`bake_viewmodel`] decides for a player, as readable changes: `(tick, what began)` for every segment
/// (`item 7: action_attack attack_gun_primaryfire (state 100) after 0.00 s`). For checking the decisions against a
/// demo.
pub fn viewmodel_timeline(
    track: &RoundTrack,
    controller: u32,
    rig: &ViewmodelRig,
) -> Result<Vec<(u32, String)>, AnimError> {
    let baker = Baker {
        track,
        rig,
        eval: Evaluator::new(&rig.library),
        hands: None,
        magazines: magazines(track),
    };
    let (segments, _) = baker.segments(controller)?;
    Ok(segments
        .iter()
        .map(|s| {
            let events = if s.events.is_empty() {
                String::new()
            } else {
                let mut list: Vec<&String> = s.events.iter().collect();
                list.sort();
                format!(" with {list:?}")
            };
            (
                s.start,
                format!(
                    "item {}: {} {} (state {}) after {:.2} s{events}",
                    s.item, s.action, s.attack, s.state, s.blend
                ),
            )
        })
        .collect())
}
