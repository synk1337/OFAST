//! Each player's body animation over a round, computed from the demo's rig inputs (decision 013, "design A").
//!
//! The layers follow CS2's main graph (`worldmodel.vnmgraph`, `docs/formats/animation.md`), bottom to top:
//!
//! 1. **Locomotion** (full body): the weapon's category picks one of the three sets (rifle, pistol, knife). Moving
//!    and standing use [`Locomotion`]'s 2D blends with one phase advancing tick by tick; the set's locomotion
//!    graph's other states — starts, plants and turns, turning on the spot, jumps and air time, ladders — are
//!    evaluated by [`super::eval`] from the state [`super::states`] decides, `time` seconds into it. A change of
//!    state blends over CS2's own transition time.
//! 2. **Planting** (full body): `SM0/Planting` while the C4 is planted (gameplay state 300).
//! 3. **Weapon idle pose** (additive, upper body): the weapon's own `idle_<w>` / `idle_crouch_<w>` delta, blended by
//!    the crouch amount — how this weapon is held over the set's generic carry.
//! 4. **Weapons** (model space, `UpperBody`, `UpperBody_Ladder` on ladders): the weapon's own graph
//!    (`worldmodel_gun.vnmgraph+ak47`, `worldmodel_knife.vnmgraph+tactical`, `worldmodel_grenade.vnmgraph+he`…, or
//!    the main graph's equipment states) for the action its gameplay state stands for: drawing (from the holster:
//!    the draw clips start with the hand there), reloading, knife attacks, grenade charges and throws, the
//!    silencer, the Healthshot. Actions hand over to each other, go on until the graph itself leaves them (a draw
//!    plays to its end through the inspect state that follows it) and fade out over the graphs' transition times.
//! 5. **Defuse** (model space, `Defuse` mask): `Defuse/Defuse` while defusing, its enter clip then its loop.
//! 6. **Body additives** (additive): the jump start from take-off, the landing until its clip ends, breathing
//!    while standing idle (`Breathing` mask).
//! 7. **Shot** (additive, upper body): `shoot_<w>` from each tick the weapon enters a shooting state (100 / 101).
//! 8. **Flinches** (additive): `BodyFlinch` / `HeadFlinch` for each hit (`player_hurt`: hit group and side),
//!    `FireFlinch` while burning.
//! 9. **Aim** ([`super::aim`]): the upper body turned towards the view (eye yaw relative to the body yaw, eye pitch),
//!    the weapon aligned, the hands on it — released during actions until the clip's `WPN_IK_ACTION_ENDING` event —
//!    and the weapon snapped onto the hand.
//! 10. **Flashed** (model space, `Flashed` mask, over the aim): the arms before the face while flashed.
//!
//! Poses are sampled at a fixed frame rate from the round's first tick, ready to key on a model's skeleton (after
//! [`super::Retargeter`]). Not baked yet: foot IK, ragdolls.

use std::collections::{HashMap, VecDeque};

use super::aim::{AimBones, AimInputs, aim, snap_weapon};
use super::eval::{Evaluator, Leave, Library, Params, StateId};
use super::layers::{BoneMask, add_layer, model_space_layer};
use super::locomotion::{LocomotionBlend, blend_poses};
use super::states::{
    BURN_GAP, GroundAction, GroundInputs, LADDER_CYCLE_HEIGHT, MIN_AIR_TICKS, Movement,
    PLANT_STATE, REVERSAL_SPEED, REVERSAL_WINDOW, SMALL_TURN_MAX, SMALL_TURN_MIN, STANDING_SPEED,
    air_runs, direction4, flashed_amount, flinch_types, ground_action, is_fire, ladder_angles,
    ladder_bearings, passive_action, reversal, turn_extent, weapon_action,
};
use super::{AnimError, Clip, Locomotion, Transform};
use crate::demo::events::GameEvent;
use crate::demo::players::{PlayerSample, RoundTrack};

/// Which of CS2's three locomotion sets a weapon uses (`worldmodel_locomotion.vnmgraph+<set>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponSet {
    /// Rifles, SMGs, shotguns, machine guns, snipers.
    Rifle,
    /// Pistols and the Zeus.
    Pistol,
    /// Knives, grenades, and anything else.
    Knife,
}

impl WeaponSet {
    /// All three, in a fixed order.
    pub const ALL: [WeaponSet; 3] = [WeaponSet::Rifle, WeaponSet::Pistol, WeaponSet::Knife];

    /// The set for a weapon category ID, as the main graph's `SM0/Locomotion/locomotion` picks it: pistols run with
    /// pistols, knives and grenades with knives, rifles, SMGs, shotguns, snipers and machine guns with rifles;
    /// equipment (C4, Healthshot) matches none of its states and falls back to the knife set.
    pub fn for_category(category: &str) -> Self {
        match category {
            "weapon_category_pistol" => Self::Pistol,
            "weapon_category_rifle"
            | "weapon_category_smg"
            | "weapon_category_shotgun"
            | "weapon_category_sniper"
            | "weapon_category_machinegun" => Self::Rifle,
            _ => Self::Knife,
        }
    }

    /// The set for a weapon's item definition index, when its category is unknown (`None` = nothing in hand).
    pub fn for_item(item: Option<u32>) -> Self {
        match item {
            // Desert Eagle, Dual Berettas, Five-SeveN, Glock-18, Tec-9, Zeus x27, P2000, P250, USP-S, CZ75-Auto,
            // R8 Revolver.
            Some(1 | 2 | 3 | 4 | 30 | 31 | 32 | 36 | 61 | 63 | 64) => Self::Pistol,
            // Knives (default CT 42, default T 59, golden 41, skins 500+), grenades (43 – 48), C4 (49),
            // Healthshot (57), nothing.
            Some(41 | 42 | 59 | 43..=49 | 57 | 80..) | None => Self::Knife,
            Some(_) => Self::Rifle,
        }
    }

    /// The set's name in CS2's file names (`rifle`, `pistol`, `knife`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Rifle => "rifle",
            Self::Pistol => "pistol",
            Self::Knife => "knife",
        }
    }

    /// The set's position in [`WeaponSet::ALL`].
    pub fn index(self) -> usize {
        match self {
            Self::Rifle => 0,
            Self::Pistol => 1,
            Self::Knife => 2,
        }
    }
}

/// The three locomotion sets, read from CS2's graphs.
#[derive(Debug, Clone)]
pub struct LocomotionSets {
    /// `worldmodel_locomotion.vnmgraph+rifle`.
    pub rifle: Locomotion,
    /// `worldmodel_locomotion.vnmgraph+pistol`.
    pub pistol: Locomotion,
    /// `worldmodel_locomotion.vnmgraph+knife`.
    pub knife: Locomotion,
}

impl LocomotionSets {
    /// The locomotion of one set.
    pub fn get(&self, set: WeaponSet) -> &Locomotion {
        match set {
            WeaponSet::Rifle => &self.rifle,
            WeaponSet::Pistol => &self.pistol,
            WeaponSet::Knife => &self.knife,
        }
    }
}

/// Where a weapon's weapons-layer actions are evaluated: a graph of [`GraphRig::library`] and its node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionGraph {
    /// The graph's resource path (the weapon's own graph, or the main graph for equipment).
    pub graph: String,
    /// The node (the weapon graph's root, or the equipment's state in the main graph).
    pub node: usize,
}

/// One weapon's decoded clips (from [`super::weapon::WeaponClips`]).
#[derive(Debug, Clone)]
pub struct WeaponAnimation {
    /// Type ID (`weapon_ak47`).
    pub weapon: String,
    /// Category ID (`weapon_category_rifle`).
    pub category: String,
    /// Additive idle poses: standing, crouched.
    pub idle: Option<[Clip; 2]>,
    /// Additive shot.
    pub shoot: Option<Clip>,
    /// The weapons layer's graph (draws, reloads, attacks, throws…), when [`GraphRig`] has it.
    pub actions: Option<ActionGraph>,
}

/// What the upper-body layers need: the weapons by item definition index, CS2's `UpperBody` mask, the aim's bones.
#[derive(Debug, Clone)]
pub struct UpperBody {
    /// Weapons the round's players hold, by item definition index.
    pub weapons: HashMap<u32, WeaponAnimation>,
    /// The `UpperBody` bone mask of the clip skeleton.
    pub mask: BoneMask,
    /// `UpperBody_Ladder`, the weapons layer's mask on ladders.
    pub ladder_mask: Option<BoneMask>,
    /// Bones for the aim.
    pub aim: AimBones,
    /// Clip skeleton parents (for model-space layers).
    pub parents: Vec<Option<usize>>,
}

/// CS2's graphs for the layers [`super::eval`] evaluates, loaded into one library.
#[derive(Debug, Default)]
pub struct GraphRig {
    /// The graphs and their clips.
    pub library: Library,
    /// Each set's locomotion graph, in [`WeaponSet::ALL`] order.
    pub locomotion: [String; 3],
    /// The main graph (`worldmodel.vnmgraph`).
    pub main: String,
    /// `SM0/Planting`.
    pub planting: Option<usize>,
    /// `Defuse/Defuse`.
    pub defuse: Option<usize>,
    /// `BodyAdditives/JumpAdditiveStart`.
    pub jump_start: Option<usize>,
    /// `BodyAdditives/JumpAdditiveLand`.
    pub jump_land: Option<usize>,
    /// `BodyAdditives/Breathing`.
    pub breathing: Option<usize>,
    /// Masks of the defuse layer (`Defuse`, `Defuse_Grenade` with a grenade in hand) and of breathing.
    pub defuse_mask: Option<BoneMask>,
    /// `Defuse_Grenade`.
    pub defuse_grenade_mask: Option<BoneMask>,
    /// `Breathing`.
    pub breathing_mask: Option<BoneMask>,
    /// `BodyFlinch/Flinch_WPNs`, `HeadFlinch/Flinch_WPNs`, `FireFlinch/OnFire_WPNs` (additive layers).
    pub body_flinch: Option<usize>,
    /// See [`GraphRig::body_flinch`].
    pub head_flinch: Option<usize>,
    /// See [`GraphRig::body_flinch`].
    pub fire_flinch: Option<usize>,
    /// `flashed/flashed` (model space, above the aim) and its `Flashed` mask.
    pub flashed: Option<usize>,
    /// See [`GraphRig::flashed`].
    pub flashed_mask: Option<BoneMask>,
}

/// The aim's action blend (`m_flActionBlendTimeSeconds`), seconds.
const AIM_ACTION_BLEND: f32 = 0.4;
/// The aim's hand IK blend-in (`m_flHandIKBlendInTimeSeconds`), seconds.
const HAND_IK_BLEND_IN: f32 = 0.3;
/// Blend time when the graphs give none (a new action of another weapon's graph, a state without a transition).
const DEFAULT_BLEND: f32 = 0.1;
/// Fade-out of an action when the graph gives none.
const DEFAULT_FADE_OUT: f32 = 0.2;
/// `Defuse`: Off → Defuse and back (the main graph's `Defuse` state machine).
const DEFUSE_BLEND_IN: f32 = 0.2;
const DEFUSE_BLEND_OUT: f32 = 0.4;
/// `SM0`: Locomotion ↔ Planting.
const PLANT_BLEND: f32 = 0.1;
/// `BodyAdditives`: into the jump start; breathing fades this long (CS2 cuts it in at once).
const ADDITIVE_BLEND: f32 = 0.2;
/// The fire flinch fades out over this long after [`BURN_GAP`] (`OnFire_WPNs0` → Off is 0.1 s).
const FIRE_FADE: f32 = 0.1;
/// A hit older than this (seconds) no longer flinches (CS2's flinch clips are shorter).
const FLINCH_MAX: f32 = 2.0;
/// A gameplay state's timestamp older than this (seconds) at the tick it is first seen is not its start.
const STATE_TIMESTAMP_SLACK: f32 = 1.0;

/// A state from a tick on, and the state it blends from.
#[derive(Debug, Clone, Copy)]
struct MovementSegment {
    state: Movement,
    start: u32,
    /// Length of a start or plant (its clip), seconds.
    length: f32,
    previous: Option<PreviousMovement>,
    blend: f32,
}

/// The state a movement segment blends from, with its own blend still running when a new state comes quickly
/// (a counter-strafe: move, plant and turn, move): the pose blends from that blend instead of snapping.
#[derive(Debug, Clone, Copy)]
struct PreviousMovement {
    state: Movement,
    start: u32,
    from: Option<(Movement, u32)>,
    blend: f32,
}

/// A weapons-layer action from a tick on.
#[derive(Debug, Clone, Copy)]
struct ActionSegment {
    item: u32,
    action: &'static str,
    attack: &'static str,
    /// The gameplay state and its timestamp tick that began it.
    state: i64,
    stamp: Option<u32>,
    start: u32,
    /// The tick the graph left it (it fades out from there), or it was handed over to the next action.
    end: Option<u32>,
    /// The tick the gameplay state stopped calling for it: the aim lets go from there, while the graph may go on
    /// playing it (a draw followed by state 1000 plays to its end).
    released: Option<u32>,
    blend_in: f32,
    blend_out: f32,
    /// A knife swing that follows a swing still in its `KnifeSwingBack` window (the graph's B variants).
    swing_back: bool,
    strength: f32,
    /// Rounds in the magazine when it began (`weapon_ammo`: a reload from 0 plays `reload_empty`).
    ammo: f32,
    /// `deploy_variation`, 0 – 1: a draw's variant by the graph's weights (a knife's `draw2` 8 times in 9).
    variation: f32,
    /// The layer's weight when it began (the action it took over, still blending in or fading out), and the aim's
    /// action fade then: both carry on instead of restarting at 0.
    inherited: f32,
    aim_from: f32,
}

impl ActionSegment {
    /// The layer's weight at demo tick `tick` (fractional; `interval` seconds per tick): from `inherited` up to 1
    /// over `blend_in`, down to 0 over `blend_out` after it ended.
    fn layer_weight(&self, tick: f32, interval: f32) -> f32 {
        let since = |t: u32| (tick - t as f32).max(0.0) * interval;
        let rise = if self.blend_in > 0.0 {
            self.inherited + (1.0 - self.inherited) * (since(self.start) / self.blend_in).min(1.0)
        } else {
            1.0
        };
        match self.end.filter(|&end| tick >= end as f32) {
            Some(end) => rise * (1.0 - (since(end) / self.blend_out.max(1e-3)).min(1.0)),
            None => rise,
        }
    }

    /// The aim's action fade at `tick` (before easing): from `aim_from` up to 1 over [`AIM_ACTION_BLEND`], back to
    /// 0 over it after the action is released (or ends), from where it had got to.
    fn aim_fade(&self, tick: f32, interval: f32) -> f32 {
        let since = |t: u32| (tick - t as f32).max(0.0) * interval;
        let rise = |at: f32| (self.aim_from + at / AIM_ACTION_BLEND).min(1.0);
        let stop = match (self.released, self.end) {
            (Some(r), Some(e)) => Some(r.min(e)),
            (r, e) => r.or(e),
        };
        match stop.filter(|&end| tick >= end as f32) {
            Some(end) => {
                let reached = rise(end.saturating_sub(self.start) as f32 * interval);
                reached * (1.0 - (since(end) / AIM_ACTION_BLEND).min(1.0))
            }
            None => rise(since(self.start)),
        }
    }
}

/// A repeatable value in 0 – 1 for a player's action at a tick (CS2 draws `deploy_variation` at random; OFAST
/// needs the same pick on every export).
fn variation(controller: u32, tick: u32) -> f32 {
    let mut x = (u64::from(controller) << 32 | u64::from(tick)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// A layer on from a tick, and off from another.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: u32,
    end: Option<u32>,
}

/// The body additive playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Additive {
    JumpStart,
    JumpLand,
}

/// The state of one living tick.
struct Step {
    tick: u32,
    set: WeaponSet,
    blend: LocomotionBlend,
    /// Phase reached at this tick (0 – 1).
    phase: f32,
    item: Option<u32>,
    crouch: f32,
    walking: bool,
    defusing: bool,
    /// Velocity in the body's frame, its length, and the fastest speed of the last [`REVERSAL_WINDOW`].
    body: [f32; 2],
    speed: f32,
    speed_previous: f32,
    turn_rate: f32,
    air_height: f32,
    ladder_cycle: f32,
    /// `ladder_angle_yaw` and `ladder_angle_yaw_backwards` ([`super::states::ladder_angles`]).
    ladder_angle: f32,
    ladder_back: f32,
    /// View yaw relative to the body (positive left) and pitch (positive down).
    aim_yaw: f32,
    aim_pitch: f32,
    movement: MovementSegment,
    action: Option<ActionSegment>,
    /// The action `action` took over from, while they blend.
    handover: Option<ActionSegment>,
    plant: Option<Span>,
    defuse: Option<Span>,
    additive: Option<(Additive, u32)>,
    /// Tick of the latest shot with the weapon in hand.
    shot: Option<u32>,
    /// The latest hit to the body and to the head: tick and flinch type ([`flinch_types`]).
    flinch_body: Option<(u32, &'static str)>,
    flinch_head: Option<(u32, &'static str)>,
    /// Burning: the first and the latest tick of fire damage in a row.
    burning: Option<(u32, u32)>,
    /// The latest flash: tick and blind duration, seconds.
    blind: Option<(u32, f32)>,
}

/// A world velocity (Source axes, units/s) in the body's frame: `(forward, left)` for a body facing `yaw` degrees.
pub fn body_relative(velocity: [f32; 3], yaw: f32) -> [f32; 2] {
    let (sin, cos) = yaw.to_radians().sin_cos();
    [
        velocity[0] * cos + velocity[1] * sin,
        -velocity[0] * sin + velocity[1] * cos,
    ]
}

/// An angle in −180 … 180.
fn normalize_angle(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// One player's poses (clip-skeleton bones) at `fps` frames per second from the round's first tick to its last:
/// `None` while the player has no living pawn. Without `upper`, the locomotion alone; without `graphs`, no
/// graph-evaluated states (moving and standing only, no actions).
pub fn bake_player(
    track: &RoundTrack,
    controller: u32,
    sets: &LocomotionSets,
    upper: Option<&UpperBody>,
    graphs: Option<&GraphRig>,
    fps: f32,
) -> Result<Vec<Option<Vec<Transform>>>, AnimError> {
    let baker = Baker {
        track,
        sets,
        upper,
        graphs,
        eval: graphs.map(|g| Evaluator::new(&g.library)),
    };
    let steps = baker.steps(controller)?;
    let frames = frame_samples(track, fps);
    let mut out = Vec::with_capacity(frames.len());
    for (tick, index) in frames {
        out.push(match steps.get(index).and_then(Option::as_ref) {
            Some(step) => Some(baker.render(step, tick)?),
            None => None,
        });
    }
    Ok(out)
}

/// What [`bake_player`] decides for a player, as readable changes: `(tick, what began)` for every new movement
/// state (`movement Ground(Start("NW")) after 0.1 s`), weapons-layer action (`action action_attack
/// attack_knife_heavymiss`), plant, defuse and body additive. For checking the decisions against a demo.
pub fn timeline(
    track: &RoundTrack,
    controller: u32,
    sets: &LocomotionSets,
    upper: Option<&UpperBody>,
    graphs: Option<&GraphRig>,
) -> Result<Vec<(u32, String)>, AnimError> {
    let baker = Baker {
        track,
        sets,
        upper,
        graphs,
        eval: graphs.map(|g| Evaluator::new(&g.library)),
    };
    let mut out = Vec::new();
    let mut last: Option<&Step> = None;
    let steps = baker.steps(controller)?;
    for step in steps.iter().flatten() {
        let m = &step.movement;
        if last.is_none_or(|l| l.movement.start != m.start || l.movement.state != m.state) {
            out.push((
                m.start,
                format!("movement {:?} after {:.2} s", m.state, m.blend),
            ));
        }
        if let Some(a) = step.action
            && last.is_none_or(|l| {
                l.action
                    .is_none_or(|b| b.start != a.start || b.action != a.action || b.item != a.item)
            })
        {
            let back = if a.swing_back { " (swing back)" } else { "" };
            out.push((
                a.start,
                format!(
                    "action {} {}{back} on item {} in {:.2} s",
                    a.action, a.attack, a.item, a.blend_in
                ),
            ));
        }
        for (name, now, before) in [
            ("plant", step.plant, last.and_then(|l| l.plant)),
            ("defuse", step.defuse, last.and_then(|l| l.defuse)),
        ] {
            if let Some(s) = now
                && before.is_none_or(|b| b.start != s.start)
            {
                out.push((s.start, format!("{name} begins")));
            }
        }
        if let Some((kind, start)) = step.additive
            && last.is_none_or(|l| l.additive != step.additive)
        {
            out.push((start, format!("additive {kind:?}")));
        }
        for (hit, before) in [
            (step.flinch_body, last.and_then(|l| l.flinch_body)),
            (step.flinch_head, last.and_then(|l| l.flinch_head)),
        ] {
            if let Some((start, kind)) = hit
                && before != hit
            {
                out.push((start, format!("flinch {kind}")));
            }
        }
        if let Some((start, _)) = step.burning
            && last.and_then(|l| l.burning).is_none_or(|(s, _)| s != start)
        {
            out.push((start, "burning".into()));
        }
        if let Some((start, duration)) = step.blind
            && last.and_then(|l| l.blind).is_none_or(|(s, _)| s != start)
        {
            out.push((start, format!("flashed for {duration:.2} s")));
        }
        last = Some(step);
    }
    Ok(out)
}

/// The frames of a round's animations at `fps`, from its first tick to its last: for each, the demo tick it shows
/// (fractional) and the index of the last sample at or before it in `track.samples`.
pub fn frame_samples(track: &RoundTrack, fps: f32) -> Vec<(f32, usize)> {
    let seconds = track.end_tick.saturating_sub(track.start_tick) as f32 * track.tick_interval;
    let frames = (seconds * fps).floor() as usize + 1;
    let mut index = 0usize;
    (0..frames)
        .map(|k| {
            let tick = track.start_tick as f32 + k as f32 / fps / track.tick_interval;
            while index + 1 < track.samples.len() && track.samples[index + 1].tick as f32 <= tick {
                index += 1;
            }
            (tick, index)
        })
        .collect()
}

/// Blends two poses of the same clip pair by the crouch amount (standing, crouched).
fn crouch_blend(pair: &[Clip; 2], time: f32, crouch: f32) -> Result<Vec<Transform>, AnimError> {
    let crouch = crouch.clamp(0.0, 1.0);
    let stand = pair[0].sample(time, false)?;
    if crouch <= 0.0 {
        return Ok(stand);
    }
    let crouched = pair[1].sample(time, false)?;
    Ok(blend_poses(&[(stand, 1.0 - crouch), (crouched, crouch)]))
}

/// `a` towards `b` by `w` (0 – 1), bone by bone.
fn mix(a: Vec<Transform>, b: Vec<Transform>, w: f32) -> Vec<Transform> {
    if w >= 1.0 {
        return b;
    }
    if w <= 0.0 {
        return a;
    }
    blend_poses(&[(a, 1.0 - w), (b, w)])
}

/// In-out quadratic easing (the aim node's action blend).
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}

/// The round, the data and the graphs one player is baked with.
struct Baker<'a> {
    track: &'a RoundTrack,
    sets: &'a LocomotionSets,
    upper: Option<&'a UpperBody>,
    graphs: Option<&'a GraphRig>,
    eval: Option<Evaluator<'a>>,
}

impl Baker<'_> {
    fn seconds(&self, ticks: f32) -> f32 {
        ticks.max(0.0) * self.track.tick_interval
    }

    fn weapon(&self, item: Option<u32>) -> Option<&WeaponAnimation> {
        self.upper?.weapons.get(&item?)
    }

    /// The step of every sample: decided in order, each from the one before.
    fn steps(&self, controller: u32) -> Result<Vec<Option<Step>>, AnimError> {
        let track = self.track;
        let dt = track.tick_interval;
        let players: Vec<Option<&PlayerSample>> = track
            .samples
            .iter()
            .map(|s| {
                s.players
                    .iter()
                    .find(|p| p.controller == controller && p.alive)
            })
            .collect();
        let ground: Vec<Option<(bool, f32)>> = players
            .iter()
            .map(|p| {
                p.filter(|p| !p.rig.on_ladder)
                    .map(|p| (p.rig.on_ground, p.position[2]))
            })
            .collect();
        let runs = air_runs(&ground);
        let bearings = ladder_bearings(
            &players
                .iter()
                .map(|p| p.map(|p| (p.rig.on_ladder, p.rig.velocity, p.rig.body_yaw)))
                .collect::<Vec<_>>(),
        );
        let yaws: Vec<Option<f32>> = players.iter().map(|p| p.map(|p| p.rig.body_yaw)).collect();
        let mut run_of = vec![None; players.len()];
        for run in &runs {
            for slot in &mut run_of[run.start..=run.end] {
                *slot = Some(*run);
            }
        }

        // The phase runs on through deaths and absences; it only has to be continuous while the player moves.
        let mut phase = 0.0f32;
        let mut previous: Option<(u32, f32, f32, Movement)> = None; // tick, body yaw, speed, movement
        let mut turn_rate = 0.0f32;
        let mut history: VecDeque<(u32, [f32; 2])> = VecDeque::new();
        let mut movement: Option<MovementSegment> = None;
        let mut action: Option<ActionSegment> = None;
        let mut handover: Option<ActionSegment> = None;
        let mut plant: Option<Span> = None;
        let mut defuse: Option<Span> = None;
        let mut additive: Option<(Additive, u32)> = None;
        let mut shot: Option<u32> = None;
        let mut last_state: Option<(u32, i64)> = None;
        // This player's hits and flashes, oldest first, consumed as the ticks pass.
        let mine = |name: &str| -> Vec<&GameEvent> {
            track
                .events
                .iter()
                .filter(|e| e.name == name && e.controller("userid") == Some(controller))
                .collect()
        };
        let (hurts, blinds) = (mine("player_hurt"), mine("player_blind"));
        let (mut next_hurt, mut next_blind) = (0usize, 0usize);
        let mut flinch_body: Option<(u32, &'static str)> = None;
        let mut flinch_head: Option<(u32, &'static str)> = None;
        let mut burning: Option<(u32, u32)> = None;
        let mut blind: Option<(u32, f32)> = None;
        let mut steps = Vec::with_capacity(players.len());
        for (i, (sample, player)) in track.samples.iter().zip(&players).enumerate() {
            let Some(player) = player else {
                // Dead or absent: a respawn starts afresh (nothing blends from before the gap, and the hits and
                // flashes of this time are passed over).
                while hurts.get(next_hurt).is_some_and(|e| e.tick <= sample.tick) {
                    next_hurt += 1;
                }
                while blinds
                    .get(next_blind)
                    .is_some_and(|e| e.tick <= sample.tick)
                {
                    next_blind += 1;
                }
                previous = None;
                history.clear();
                movement = None;
                action = None;
                handover = None;
                plant = None;
                defuse = None;
                additive = None;
                shot = None;
                last_state = None;
                flinch_body = None;
                flinch_head = None;
                burning = None;
                blind = None;
                steps.push(None);
                continue;
            };
            let tick = sample.tick;
            let rig = &player.rig;
            let item = rig.weapon.as_ref().map(|w| w.item);
            let weapon = self.weapon(item);
            let set = weapon.map_or(WeaponSet::for_item(item), |w| {
                WeaponSet::for_category(&w.category)
            });
            let body = body_relative(rig.velocity, rig.body_yaw);
            let speed = rig.velocity[0].hypot(rig.velocity[1]);
            let blend = self.sets.get(set).blend(body, rig.duck_amount);
            let elapsed_ticks = previous.map_or(0, |(t, ..)| tick.saturating_sub(t));
            let step_dt = elapsed_ticks as f32 * dt;
            phase = (phase + step_dt / blend.duration).rem_euclid(1.0);

            // The body's turn rate, smoothed over about a tenth of a second.
            if let Some((_, yaw, ..)) = previous
                && step_dt > 0.0
            {
                let raw = normalize_angle(rig.body_yaw - yaw) / step_dt;
                turn_rate += (raw - turn_rate) * (step_dt / 0.1).min(1.0);
            } else {
                turn_rate = 0.0;
            }
            while history
                .front()
                .is_some_and(|&(t, _)| (tick - t) as f32 * dt > REVERSAL_WINDOW)
            {
                history.pop_front();
            }
            let recent: Vec<[f32; 2]> = history.iter().map(|&(_, v)| v).collect();
            let speed_previous = recent
                .iter()
                .map(|v| v[0].hypot(v[1]))
                .fold(speed, f32::max);
            let world = [rig.velocity[0], rig.velocity[1]];

            // Movement: ladder, air or one of the ground actions.
            let run = run_of[i];
            let jumped = run.is_some_and(|r| {
                let takeoff = track.samples[r.start].tick;
                rig.last_jump_tick
                    .is_some_and(|j| j + 2 >= takeoff && j <= tick)
            });
            let airborne = run.is_some_and(|r| r.end - r.start + 1 >= MIN_AIR_TICKS || jumped);
            let current = movement.map(|m| m.state);
            let desired = if rig.on_ladder {
                Movement::Ladder
            } else if airborne {
                Movement::Air { jumped }
            } else if self.graphs.is_some() {
                let (now, elapsed, length) = match movement {
                    Some(MovementSegment {
                        state: Movement::Ground(g),
                        start,
                        length,
                        ..
                    }) => (g, self.seconds((tick - start) as f32), length),
                    _ => (GroundAction::Move, 0.0, 0.0),
                };
                // Standing: slow at the last tick and not just passing through zero (a counter-strafe crosses
                // it within a tick or two, from a run).
                let recently_fast = recent.iter().any(|v| v[0].hypot(v[1]) >= REVERSAL_SPEED);
                let inputs = GroundInputs {
                    speed,
                    body,
                    walking: rig.walking,
                    turn_rate,
                    was_standing: !recently_fast
                        && previous.is_some_and(|(_, _, s, _)| s < STANDING_SPEED),
                    reversal: reversal(&recent, world, body),
                };
                let before = matches!(current, Some(Movement::Ground(_)));
                match ground_action(now, elapsed, length, before, &inputs) {
                    // A new turn on the spot: one stepping turn when the body settles within ±45°, else the loop.
                    GroundAction::Turn
                        if !matches!(now, GroundAction::Turn | GroundAction::SmallTurn(_)) =>
                    {
                        let (angle, settled) = turn_extent(&yaws, i, dt);
                        Movement::Ground(if settled && angle.abs() < SMALL_TURN_MIN {
                            GroundAction::Move
                        } else if settled && angle.abs() <= SMALL_TURN_MAX {
                            GroundAction::SmallTurn(angle)
                        } else {
                            GroundAction::Turn
                        })
                    }
                    action => Movement::Ground(action),
                }
            } else {
                Movement::Ground(GroundAction::Move)
            };
            history.push_back((tick, world));

            let air_height = run.map_or(0.0, |r| r.height(player.position[2], rig.velocity[2]));
            let ladder = bearings[i].map_or((0.0, 180.0), |b| ladder_angles(b, rig.body_yaw));
            let mut step = Step {
                tick,
                set,
                blend,
                phase,
                item,
                crouch: rig.duck_amount,
                walking: rig.walking,
                defusing: rig.defusing,
                body,
                speed,
                speed_previous,
                turn_rate,
                air_height,
                ladder_cycle: (player.position[2] / LADDER_CYCLE_HEIGHT).rem_euclid(1.0),
                ladder_angle: ladder.0,
                ladder_back: ladder.1,
                aim_yaw: normalize_angle(player.yaw - rig.body_yaw),
                aim_pitch: player.pitch,
                movement: MovementSegment {
                    state: desired,
                    start: tick,
                    length: 0.0,
                    previous: None,
                    blend: 0.0,
                },
                action: None,
                handover: None,
                plant: None,
                defuse: None,
                additive: None,
                shot: None,
                flinch_body: None,
                flinch_head: None,
                burning: None,
                blind: None,
            };
            movement = Some(match movement {
                Some(m) if m.state == desired => m,
                old => {
                    let mut next = step.movement;
                    if let Some(old) = old {
                        next.previous = Some(PreviousMovement {
                            state: old.state,
                            start: old.start,
                            from: old.previous.map(|p| (p.state, p.start)),
                            blend: old.blend,
                        });
                        next.blend = self
                            .movement_transition(&step, old.state, desired)
                            .unwrap_or(DEFAULT_BLEND);
                    }
                    if matches!(
                        desired,
                        Movement::Ground(
                            GroundAction::Start(_)
                                | GroundAction::PlantAndTurn(..)
                                | GroundAction::SmallTurn(_)
                        )
                    ) {
                        next.length = self.movement_length(&step, desired);
                    }
                    next
                }
            });
            step.movement = movement.expect("set above");

            // Weapons layer actions.
            let wanted = rig.weapon.as_ref().and_then(|w| {
                let anim = self.weapon(Some(w.item))?;
                anim.actions.as_ref()?;
                let (name, attack) = weapon_action(w.anim_state, &anim.category)?;
                Some((w, name, attack))
            });
            match wanted {
                Some((w, name, attack)) => {
                    // The same action goes on while its state and timestamp stay (a second swing restarts the
                    // same state value with a new timestamp); a reload's outro (801) goes on with the reload.
                    let same = action.is_some_and(|a| {
                        a.end.is_none()
                            && a.item == w.item
                            && a.action == name
                            && a.attack == attack
                            && ((a.state == w.anim_state && a.stamp == w.state_tick)
                                || (a.state == 800 && w.anim_state == 801))
                    });
                    if !same {
                        // The state's own start when it is recent, else this tick.
                        let start = w
                            .state_tick
                            .filter(|&s| {
                                s <= tick && self.seconds((tick - s) as f32) < STATE_TIMESTAMP_SLACK
                            })
                            .unwrap_or(tick);
                        let mut next = ActionSegment {
                            item: w.item,
                            action: name,
                            attack,
                            state: w.anim_state,
                            stamp: w.state_tick,
                            start,
                            end: None,
                            released: None,
                            blend_in: DEFAULT_BLEND,
                            blend_out: DEFAULT_FADE_OUT,
                            swing_back: false,
                            strength: w.throw_strength,
                            ammo: w.clip.map_or(1.0, |c| c as f32),
                            variation: variation(player.controller, start),
                            inherited: 0.0,
                            aim_from: 0.0,
                        };
                        // The action it takes over: still playing, or fading out.
                        let at = start as f32;
                        let old = action.filter(|a| a.layer_weight(at, dt) > 0.0);
                        let playing = old.filter(|a| a.end.is_none_or(|end| end >= start));
                        next.swing_back = attack.starts_with("attack_knife")
                            && playing.is_some_and(|o| self.in_swing_back(&step, &o, start));
                        next.blend_in = self
                            .action_transition(&step, playing.as_ref(), &next)
                            .unwrap_or(DEFAULT_BLEND);
                        next.blend_out = self
                            .action_fade_out(&step, &next)
                            .unwrap_or(DEFAULT_FADE_OUT);
                        next.inherited = old.map_or(0.0, |o| o.layer_weight(at, dt));
                        next.aim_from = action.map_or(0.0, |o| o.aim_fade(at, dt));
                        handover = old.map(|mut o| {
                            o.end = Some(o.end.map_or(start, |end| end.min(start)));
                            o
                        });
                        action = Some(next);
                    }
                }
                None => {
                    // No new action: the one in progress goes on until its graph leaves it for what the state now
                    // says (idle, inspect, gun fire), as CS2 runs the graph; another weapon in hand ends it.
                    if let Some(a) = action.as_mut()
                        && a.end.is_none()
                    {
                        a.released.get_or_insert(tick);
                        let same_weapon = rig.weapon.as_ref().filter(|w| w.item == a.item);
                        match same_weapon.map(|w| self.action_leaves(&step, a, w.anim_state, tick))
                        {
                            Some(Some(blend)) => {
                                a.end = Some(tick);
                                a.blend_out = blend;
                            }
                            Some(None) => {}
                            None => a.end = Some(tick),
                        }
                    }
                }
            }
            step.action = action;
            step.handover = handover.filter(|_| {
                action.is_some_and(|a| self.seconds(tick as f32 - a.start as f32) < a.blend_in)
            });

            // Planting (the C4's state 300) and defusing.
            let planting = rig
                .weapon
                .as_ref()
                .is_some_and(|w| w.anim_state == PLANT_STATE);
            span(&mut plant, planting, tick);
            span(&mut defuse, rig.defusing, tick);
            step.plant = plant;
            step.defuse = defuse;

            // Body additives: the jump start while in the air, the landing after it (on the ground: CS2's
            // `air_action_land`; catching a ladder ends the jump start without one).
            let was_air = matches!(current, Some(Movement::Air { .. }));
            additive = match (desired, additive) {
                (Movement::Air { .. }, Some((Additive::JumpStart, since))) => {
                    Some((Additive::JumpStart, since))
                }
                (Movement::Air { .. }, _) => Some((Additive::JumpStart, tick)),
                (Movement::Ground(_), _) if was_air => Some((Additive::JumpLand, tick)),
                (_, Some((Additive::JumpLand, since))) => {
                    let length = self.additive_length(&step, Additive::JumpLand);
                    (self.seconds((tick - since) as f32) < length)
                        .then_some((Additive::JumpLand, since))
                }
                _ => None,
            };
            step.additive = additive;

            // A shot starts whenever the state enters 100 or 101 (they alternate shot by shot).
            let state = rig.weapon.as_ref().map(|w| (w.item, w.anim_state));
            if let Some((_, s)) = state
                && matches!(s, 100 | 101)
                && last_state != state
            {
                shot = Some(tick);
            }
            last_state = state;
            step.shot = shot;

            // Hits (flinches, burning) and flashes up to this tick.
            while let Some(hurt) = hurts.get(next_hurt).filter(|e| e.tick <= tick) {
                next_hurt += 1;
                if is_fire(hurt.string("weapon").unwrap_or("")) {
                    let first = burning
                        .filter(|&(_, last)| {
                            self.seconds(hurt.tick.saturating_sub(last) as f32) < BURN_GAP
                        })
                        .map_or(hurt.tick, |(first, _)| first);
                    burning = Some((first, hurt.tick));
                    continue;
                }
                // Fall damage (no attacker, no weapon) makes no flinch.
                if hurt.controller("attacker").is_none()
                    && hurt.string("weapon").unwrap_or("").is_empty()
                {
                    continue;
                }
                let side = self.hit_side(hurt, sample, player, controller);
                let (body, head) = flinch_types(hurt.int("hitgroup").unwrap_or(0), side);
                if let Some(kind) = body {
                    flinch_body = Some((hurt.tick, kind));
                }
                if let Some(kind) = head {
                    flinch_head = Some((hurt.tick, kind));
                }
            }
            while let Some(flash) = blinds.get(next_blind).filter(|e| e.tick <= tick) {
                next_blind += 1;
                let duration = match flash.get("blind_duration") {
                    Some(crate::demo::events::EventValue::Float(d)) => *d,
                    _ => 0.0,
                };
                // A weaker flash does not cut a stronger one short: the one that ends later counts.
                let ends = |(start, d): (u32, f32)| start as f32 * dt + d;
                if blind.is_none_or(|b| ends((flash.tick, duration)) >= ends(b)) {
                    blind = Some((flash.tick, duration));
                }
            }
            step.flinch_body = flinch_body;
            step.flinch_head = flinch_head;
            step.burning = burning;
            step.blind = blind;

            previous = Some((tick, rig.body_yaw, speed, desired));
            steps.push(Some(step));
        }
        Ok(steps)
    }

    /// Where a hit came from in the victim's body frame ([`direction4`]: `N` ahead … `W` left): against the
    /// bullet's direction (`bullet_damage` of the same tick, victim and attacker), towards the explosion for an HE
    /// grenade (`hegrenade_detonate` within two ticks), else towards the attacker, else ahead.
    fn hit_side(
        &self,
        hurt: &GameEvent,
        sample: &crate::demo::players::TickSample,
        victim: &PlayerSample,
        controller: u32,
    ) -> &'static str {
        let attacker = hurt.controller("attacker");
        let bullet = self.track.events.iter().find(|e| {
            e.name == "bullet_damage"
                && e.tick == hurt.tick
                && e.controller("victim") == Some(controller)
                && e.controller("attacker") == attacker
        });
        let float = |e: &GameEvent, key: &str| match e.get(key) {
            Some(crate::demo::events::EventValue::Float(v)) => Some(*v),
            _ => None,
        };
        let blast = (hurt.string("weapon") == Some("hegrenade"))
            .then(|| {
                self.track
                    .events
                    .iter()
                    .find(|e| e.name == "hegrenade_detonate" && e.tick.abs_diff(hurt.tick) <= 2)
            })
            .flatten()
            .and_then(|e| {
                Some([
                    float(e, "x")? - victim.position[0],
                    float(e, "y")? - victim.position[1],
                ])
            });
        let towards = bullet
            .and_then(|b| Some([-float(b, "damage_dir_x")?, -float(b, "damage_dir_y")?]))
            .or(blast)
            .or_else(|| {
                let attacker = hurt.controller("attacker")?;
                let a = sample.players.iter().find(|p| p.controller == attacker)?;
                Some([
                    a.position[0] - victim.position[0],
                    a.position[1] - victim.position[1],
                ])
            });
        match towards {
            Some(v) if v[0].hypot(v[1]) > 1e-3 => {
                direction4(body_relative([v[0], v[1], 0.0], victim.rig.body_yaw))
            }
            _ => "N",
        }
    }

    /// The locomotion graph's parameters at a step, for a movement state.
    fn locomotion_params(&self, step: &Step, state: Movement) -> Params {
        let mut params = Params::default()
            .float("move_speed_x", step.body[0])
            .float("move_speed_y", step.body[1])
            .float("move_speed_horizontal", step.speed)
            .float("move_speed_horizontal_previous", step.speed_previous)
            .float("move_crouch_amount", step.crouch)
            .float("move_crouch_amount_eased", step.crouch)
            .float("ground_turn_angle_or_velocity", step.turn_rate)
            .float("air_height_above_ground", step.air_height)
            .float("ladder_cycle", step.ladder_cycle)
            .float("ladder_angle_yaw", step.ladder_angle)
            .float("ladder_angle_yaw_backwards", step.ladder_back)
            .flag("move_is_walking", step.walking)
            .id("move_type", state.move_type());
        match state {
            Movement::Ground(ground) => {
                params = params.id("ground_action", ground.id());
                match ground {
                    GroundAction::Start(direction) => {
                        params = params.id("ground_action_direction_id", direction);
                    }
                    GroundAction::PlantAndTurn(direction, before) => {
                        params = params
                            .id("ground_action_direction_id", direction)
                            .float("move_speed_horizontal_previous", before);
                    }
                    GroundAction::SmallTurn(angle) => {
                        params = params.float("ground_turn_angle_or_velocity", angle);
                    }
                    _ => {}
                }
            }
            Movement::Air { jumped } => {
                params = params.id(
                    "air_action",
                    if jumped {
                        "air_action_jump"
                    } else {
                        "air_action_start_fall"
                    },
                );
            }
            Movement::Ladder => {}
        }
        params
    }

    /// The locomotion graph of a step's set and its root node.
    fn locomotion_graph(&self, step: &Step) -> Option<(&str, usize)> {
        let graph = &self.graphs?.locomotion[step.set.index()];
        Some((graph.as_str(), self.eval?.root(graph)?))
    }

    fn movement_transition(&self, step: &Step, from: Movement, to: Movement) -> Option<f32> {
        let eval = self.eval?;
        let (graph, root) = self.locomotion_graph(step)?;
        let states = |m: Movement| {
            eval.states(graph, root, &self.locomotion_params(step, m), 0.0)
                .ok()
        };
        eval.transition_time(
            &states(from)?,
            &states(to)?,
            &self.locomotion_params(step, to),
        )
    }

    fn movement_length(&self, step: &Step, state: Movement) -> f32 {
        let (Some(eval), Some((graph, root))) = (self.eval, self.locomotion_graph(step)) else {
            return 0.0;
        };
        eval.duration(graph, root, &self.locomotion_params(step, state))
            .unwrap_or(0.0)
    }

    /// The weapon graph's parameters for an action.
    fn action_params(&self, step: &Step, segment: &ActionSegment) -> Params {
        let mut params = Params::default()
            .id("action", segment.action)
            .id("attack_type", segment.attack)
            .float("attack_throw_strength", segment.strength)
            .float("weapon_ammo", segment.ammo)
            .float("deploy_variation", segment.variation)
            .float("move_crouch_amount", step.crouch)
            .float("move_crouch_amount_eased", step.crouch)
            .float("weapon_action_speedscale", 1.0)
            .flag("is_defusing", step.defusing);
        if let Some(weapon) = self.weapon(Some(segment.item)) {
            params = params
                .id("weapon_type", &weapon.weapon)
                .id("weapon_category", &weapon.category);
        }
        if segment.swing_back {
            params = params.event("KnifeSwingBack");
        }
        params
    }

    /// The weapon graph's states for an action, or for the layer at rest (`action_idle`).
    fn action_states(
        &self,
        step: &Step,
        segment: &ActionSegment,
        idle: bool,
    ) -> Option<Vec<StateId>> {
        let eval = self.eval?;
        let graph = self.weapon(Some(segment.item))?.actions.as_ref()?;
        let mut params = self.action_params(step, segment);
        if idle {
            params = params.id("action", "action_idle").id("attack_type", "");
        }
        eval.states(&graph.graph, graph.node, &params, 0.0).ok()
    }

    /// Blend time into `next`: from the action it takes over (same weapon), else from the layer at rest.
    fn action_transition(
        &self,
        step: &Step,
        old: Option<&ActionSegment>,
        next: &ActionSegment,
    ) -> Option<f32> {
        let eval = self.eval?;
        let to = self.action_states(step, next, false)?;
        let from = match old {
            Some(o) if o.item == next.item => self.action_states(step, o, false)?,
            _ => self.action_states(step, next, true)?,
        };
        // The same states again (a second swing): CS2 alternates two copies of the attack states to restart.
        if from == to {
            return Some(DEFAULT_BLEND);
        }
        eval.transition_time(&from, &to, &self.action_params(step, next))
    }

    fn action_fade_out(&self, step: &Step, segment: &ActionSegment) -> Option<f32> {
        let eval = self.eval?;
        let from = self.action_states(step, segment, false)?;
        let to = self.action_states(step, segment, true)?;
        let mut idle = self.action_params(step, segment);
        idle = idle.id("action", "action_idle");
        eval.transition_time(&from, &to, &idle)
    }

    /// Whether the weapon's graph leaves `segment`'s state at `tick` now that the gameplay state (`state`) starts
    /// no action ([`passive_action`]): `Some(blend)` when it does, `None` while the action goes on. When the graph
    /// cannot tell ([`Leave::Unknown`], or an error), the action ends at once with [`DEFAULT_FADE_OUT`].
    fn action_leaves(
        &self,
        step: &Step,
        segment: &ActionSegment,
        state: i64,
        tick: u32,
    ) -> Option<f32> {
        let (Some(eval), Some(graph)) = (
            self.eval,
            self.weapon(Some(segment.item))
                .and_then(|w| w.actions.as_ref()),
        ) else {
            return Some(DEFAULT_FADE_OUT);
        };
        let (name, attack) = passive_action(state);
        let from = self.action_params(step, segment);
        let now = from.clone().id("action", name).id("attack_type", attack);
        let time = self.seconds(tick.saturating_sub(segment.start) as f32);
        match eval.leaves(&graph.graph, graph.node, &from, time, &now) {
            Ok(Leave::Leaves { blend, .. }) => Some(blend),
            Ok(Leave::Stays) => None,
            Ok(Leave::Unknown) | Err(_) => Some(DEFAULT_FADE_OUT),
        }
    }

    /// Whether a new attack at `start` comes while `old`'s clip is in its `KnifeSwingBack` event.
    fn in_swing_back(&self, step: &Step, old: &ActionSegment, start: u32) -> bool {
        if old.action != "action_attack" || !old.attack.starts_with("attack_knife") {
            return false;
        }
        let (Some(eval), Some(graph)) = (
            self.eval,
            self.weapon(Some(old.item)).and_then(|w| w.actions.as_ref()),
        ) else {
            return false;
        };
        let time = self.seconds(start as f32 - old.start as f32);
        let Ok(Some((clip, t))) = eval.dominant_clip(
            &graph.graph,
            graph.node,
            &self.action_params(step, old),
            time,
        ) else {
            return false;
        };
        clip.event_span("KnifeSwingBack")
            .is_some_and(|(s, e)| t >= s && t <= e)
    }

    fn additive_params(&self, step: &Step) -> Params {
        let mut params = Params::default()
            .float("move_crouch_amount", step.crouch)
            .float("move_speed_horizontal", step.speed)
            .flag("is_defusing", step.defusing)
            .id("action", "action_idle");
        if let Some(weapon) = self.weapon(step.item) {
            params = params
                .id("weapon_type", &weapon.weapon)
                .id("weapon_category", &weapon.category);
        }
        params
    }

    fn additive_node(&self, additive: Additive) -> Option<usize> {
        let graphs = self.graphs?;
        match additive {
            Additive::JumpStart => graphs.jump_start,
            Additive::JumpLand => graphs.jump_land,
        }
    }

    fn additive_length(&self, step: &Step, additive: Additive) -> f32 {
        let (Some(eval), Some(graphs), Some(node)) =
            (self.eval, self.graphs, self.additive_node(additive))
        else {
            return 0.0;
        };
        eval.duration(&graphs.main, node, &self.additive_params(step))
            .unwrap_or(0.0)
    }

    /// The pose at demo tick `tick` (fractional) of a step: every layer of the module docs.
    fn render(&self, step: &Step, tick: f32) -> Result<Vec<Transform>, AnimError> {
        let since = |t: u32| self.seconds(tick - t as f32);
        let movement = &step.movement;
        let mut pose = self.movement_pose(step, movement.state, movement.start, tick)?;
        if let Some(previous) = movement.previous
            && movement.blend > 0.0
        {
            let w = since(movement.start) / movement.blend;
            if w < 1.0 {
                // The previous state, itself still blending from the one before when it began recently.
                let mut old = self.movement_pose(step, previous.state, previous.start, tick)?;
                if let Some((from, from_start)) = previous.from
                    && previous.blend > 0.0
                {
                    let v = since(previous.start) / previous.blend;
                    if v < 1.0 {
                        old = mix(self.movement_pose(step, from, from_start, tick)?, old, v);
                    }
                }
                pose = mix(old, pose, w);
            }
        }
        let (Some(eval), Some(graphs)) = (self.eval, self.graphs) else {
            if let Some(upper) = self.upper {
                self.upper_body(&mut pose, upper, step, tick, None)?;
            }
            return Ok(pose);
        };

        // Planting replaces the whole body.
        if let (Some(plant), Some(node)) = (step.plant, graphs.planting) {
            let w = fade(
                since(plant.start),
                plant.end.map(since),
                PLANT_BLEND,
                PLANT_BLEND,
            );
            if w > 0.0
                && let Some(planting) = eval.pose(
                    &graphs.main,
                    node,
                    &self.additive_params(step),
                    since(plant.start),
                )?
            {
                pose = mix(pose, planting, w);
            }
        }
        if let Some(upper) = self.upper {
            self.upper_body(&mut pose, upper, step, tick, Some((eval, graphs)))?;
        }
        Ok(pose)
    }

    /// The pose of a movement state that began at `start`.
    fn movement_pose(
        &self,
        step: &Step,
        state: Movement,
        start: u32,
        tick: f32,
    ) -> Result<Vec<Transform>, AnimError> {
        let locomotion = self.sets.get(step.set);
        let moving = |_: ()| {
            let extra = self.seconds(tick - step.tick as f32);
            locomotion.sample(&step.blend, step.phase + extra / step.blend.duration)
        };
        if state.is_move() {
            return moving(());
        }
        let (Some(eval), Some((graph, root))) = (self.eval, self.locomotion_graph(step)) else {
            return moving(());
        };
        let time = self.seconds(tick - start as f32);
        match eval.pose(graph, root, &self.locomotion_params(step, state), time)? {
            Some(pose) => Ok(pose),
            None => moving(()),
        }
    }

    /// The weapons layer's pose for an action at `tick`.
    fn action_pose(
        &self,
        eval: Evaluator<'_>,
        step: &Step,
        segment: &ActionSegment,
        tick: f32,
    ) -> Result<Option<Vec<Transform>>, AnimError> {
        let Some(graph) = self
            .weapon(Some(segment.item))
            .and_then(|w| w.actions.as_ref())
        else {
            return Ok(None);
        };
        let time = self.seconds(tick - segment.start as f32);
        eval.pose(
            &graph.graph,
            graph.node,
            &self.action_params(step, segment),
            time,
        )
    }

    /// How far the hands are back on the weapon during an action `time` seconds in: 0 until the clip's
    /// `WPN_IK_ACTION_ENDING` event, rising to 1 over it (0 throughout without the event).
    fn hand_ik_during(
        &self,
        eval: Evaluator<'_>,
        step: &Step,
        segment: &ActionSegment,
        time: f32,
    ) -> f32 {
        let Some(graph) = self
            .weapon(Some(segment.item))
            .and_then(|w| w.actions.as_ref())
        else {
            return 0.0;
        };
        let Ok(Some((clip, t))) = eval.dominant_clip(
            &graph.graph,
            graph.node,
            &self.action_params(step, segment),
            time,
        ) else {
            return 0.0;
        };
        match clip.event_span("WPN_IK_ACTION_ENDING") {
            Some((s, e)) if e > s => ((t - s) / (e - s)).clamp(0.0, 1.0),
            Some((s, _)) => f32::from(t >= s),
            None => 0.0,
        }
    }

    /// Layers 3 – 8 of the module docs over the body pose.
    fn upper_body(
        &self,
        pose: &mut [Transform],
        upper: &UpperBody,
        step: &Step,
        tick: f32,
        graphs: Option<(Evaluator<'_>, &GraphRig)>,
    ) -> Result<(), AnimError> {
        let since = |t: u32| self.seconds(tick - t as f32);
        let weapon = self.weapon(step.item);
        if let Some(idle) = weapon.and_then(|w| w.idle.as_ref()) {
            add_layer(
                pose,
                &crouch_blend(idle, 0.0, step.crouch)?,
                1.0,
                Some(&upper.mask),
            );
        }

        // The weapons layer, the aim's action weight and the hands.
        let mut action_weight = 0.0f32;
        let mut hand_ik = 1.0f32;
        // An action long over (faded out, the aim and hands back) needs no evaluation.
        let settled = |a: &ActionSegment| {
            a.end.is_some_and(|end| {
                tick >= end as f32
                    && since(end) >= a.blend_out.max(AIM_ACTION_BLEND).max(HAND_IK_BLEND_IN)
            })
        };
        if let (Some((eval, _)), Some(action)) = (graphs, step.action.filter(|a| !settled(a))) {
            let interval = self.track.tick_interval;
            let ended = action.end.filter(|&end| tick >= end as f32);
            let blend_in = if action.blend_in > 0.0 {
                (since(action.start) / action.blend_in).min(1.0)
            } else {
                1.0
            };
            let mask = match (step.movement.state, &upper.ladder_mask) {
                (Movement::Ladder, Some(ladder)) => ladder,
                _ => &upper.mask,
            };
            let current = self.action_pose(eval, step, &action, tick)?;
            let old = match step.handover {
                Some(h) if blend_in < 1.0 => self.action_pose(eval, step, &h, tick)?,
                _ => None,
            };
            // The new action's pose blends in over the old one's, from the weight the layer had then.
            let weight = action.layer_weight(tick, interval);
            let layer = match (old, current) {
                (Some(old), Some(current)) => Some((mix(old, current, blend_in), weight)),
                (Some(old), None) => Some((old, action.inherited * (1.0 - blend_in))),
                (None, Some(current)) => Some((current, weight)),
                (None, None) => None,
            };
            if let Some((layer, weight)) = layer {
                model_space_layer(pose, &layer, weight, mask, &upper.parents);
            }
            // The aim pulls towards level and lets go of the hands while an action plays.
            action_weight = ease(action.aim_fade(tick, interval));
            hand_ik = match ended {
                None => self.hand_ik_during(eval, step, &action, since(action.start)),
                Some(end) => {
                    let at_end = self.hand_ik_during(
                        eval,
                        step,
                        &action,
                        self.seconds(end.saturating_sub(action.start) as f32),
                    );
                    at_end + (1.0 - at_end) * (since(end) / HAND_IK_BLEND_IN).min(1.0)
                }
            };
        }

        if let Some((eval, rig)) = graphs {
            self.defuse_and_additives(
                pose,
                eval,
                rig,
                step,
                tick,
                &mut action_weight,
                &mut hand_ik,
            )?;
        }

        if let (Some(shot), Some(shoot)) = (step.shot, weapon.and_then(|w| w.shoot.as_ref())) {
            let elapsed = since(shot);
            if elapsed < shoot.duration {
                add_layer(pose, &shoot.sample(elapsed, false)?, 1.0, Some(&upper.mask));
            }
        }
        if let Some((eval, rig)) = graphs {
            self.flinches(pose, eval, rig, step, tick)?;
        }

        if let Some(weapon) = weapon {
            aim(
                pose,
                &upper.aim,
                &AimInputs {
                    yaw: step.aim_yaw,
                    pitch: step.aim_pitch,
                    crouch: step.crouch,
                    action_weight,
                    hand_ik_weight: hand_ik,
                    category: weapon.category.clone(),
                    weapon: weapon.weapon.clone(),
                },
            );
        }

        // Flashed: the arms up before the face, over the aim (the main graph's root layer), then the weapon snapped
        // back onto the hand.
        if let (Some((eval, rig)), Some((start, duration))) = (graphs, step.blind)
            && let (Some(node), Some(mask)) = (rig.flashed, rig.flashed_mask.as_ref())
        {
            let amount = flashed_amount(since(start), duration);
            // The state's layer weight: `flashed_amount` 0.1 – 1 onto 0 – 0.95.
            let weight = ((amount - 0.1) / 0.9).clamp(0.0, 1.0) * 0.95;
            if weight > 0.0
                && let Some(layer) = eval.pose(&rig.main, node, &self.additive_params(step), 0.0)?
            {
                model_space_layer(pose, &layer, weight, mask, &upper.parents);
                if weapon.is_some() {
                    snap_weapon(pose, &upper.aim);
                }
            }
        }
        Ok(())
    }

    /// The additive flinch layers: the latest body and head hit's clip from its tick to its end, and the fire
    /// flinch while burning (and [`BURN_GAP`] after the last burn).
    fn flinches(
        &self,
        pose: &mut [Transform],
        eval: Evaluator<'_>,
        rig: &GraphRig,
        step: &Step,
        tick: f32,
    ) -> Result<(), AnimError> {
        let since = |t: u32| self.seconds(tick - t as f32);
        for (node, hit, parameter) in [
            (rig.body_flinch, step.flinch_body, "flinch_body_type"),
            (rig.head_flinch, step.flinch_head, "flinch_head_type"),
        ] {
            let (Some(node), Some((start, kind))) = (node, hit) else {
                continue;
            };
            let time = since(start);
            // Flinch clips last well under this: no need to ask the graph about an old hit.
            if time >= FLINCH_MAX {
                continue;
            }
            let params = self.additive_params(step).id(parameter, kind);
            if time < eval.duration(&rig.main, node, &params)?
                && let Some(delta) = eval.pose(&rig.main, node, &params, time)?
            {
                add_layer(pose, &delta, 1.0, None);
            }
        }
        if let (Some(node), Some((first, last))) = (rig.fire_flinch, step.burning) {
            let after = since(last);
            if after < BURN_GAP + FIRE_FADE {
                let params = self.additive_params(step).flag("flinch_is_on_fire", true);
                let weight = 1.0 - ((after - BURN_GAP) / FIRE_FADE).clamp(0.0, 1.0);
                if let Some(delta) = eval.pose(&rig.main, node, &params, since(first))? {
                    add_layer(pose, &delta, weight, None);
                }
            }
        }
        Ok(())
    }

    /// Layers 5 and 6: defusing, the jump additives and breathing. Defusing and planting also level the aim and
    /// free the hands.
    #[allow(clippy::too_many_arguments)]
    fn defuse_and_additives(
        &self,
        pose: &mut [Transform],
        eval: Evaluator<'_>,
        rig: &GraphRig,
        step: &Step,
        tick: f32,
        action_weight: &mut f32,
        hand_ik: &mut f32,
    ) -> Result<(), AnimError> {
        let since = |t: u32| self.seconds(tick - t as f32);
        let params = self.additive_params(step);
        if let (Some(defuse), Some(node)) = (step.defuse, rig.defuse) {
            let w = fade(
                since(defuse.start),
                defuse.end.map(since),
                DEFUSE_BLEND_IN,
                DEFUSE_BLEND_OUT,
            );
            let grenade = self
                .weapon(step.item)
                .is_some_and(|w| w.category == "weapon_category_grenade");
            let mask = if grenade {
                rig.defuse_grenade_mask.as_ref()
            } else {
                rig.defuse_mask.as_ref()
            };
            // The defuse pose goes on while it fades out.
            let defusing = params.clone().flag("is_defusing", true);
            if w > 0.0
                && let Some(mask) = mask
                && let Some(layer) = eval.pose(&rig.main, node, &defusing, since(defuse.start))?
            {
                let parents = self.upper.map(|u| u.parents.as_slice()).unwrap_or_default();
                model_space_layer(pose, &layer, w, mask, parents);
                *action_weight = action_weight.max(ease(w));
                *hand_ik *= 1.0 - w;
            }
        }
        if let Some(plant) = step.plant {
            let w = fade(
                since(plant.start),
                plant.end.map(since),
                PLANT_BLEND,
                PLANT_BLEND,
            );
            *action_weight = action_weight.max(ease(w));
            *hand_ik *= 1.0 - w;
        }
        if let Some((additive, start)) = step.additive
            && let Some(node) = self.additive_node(additive)
        {
            let w = match additive {
                Additive::JumpStart => (since(start) / ADDITIVE_BLEND).min(1.0),
                Additive::JumpLand => 1.0,
            };
            if let Some(delta) = eval.pose(&rig.main, node, &params, since(start))? {
                add_layer(pose, &delta, w, None);
            }
        }
        // Breathing: standing still on the ground with nothing else going on.
        let idle = step.speed <= 5.0
            && step.movement.state == Movement::Ground(GroundAction::Move)
            && step.defuse.is_none_or(|d| d.end.is_some())
            && step.action.is_none_or(|a| a.end.is_some());
        if idle && let (Some(node), Some(mask)) = (rig.breathing, rig.breathing_mask.as_ref()) {
            let still = since(step.movement.start)
                .min(step.action.and_then(|a| a.end).map_or(f32::MAX, since));
            let w = (still / ADDITIVE_BLEND).min(1.0);
            let time = self.seconds(tick - self.track.start_tick as f32);
            if let Some(delta) = eval.pose(&rig.main, node, &params, time)? {
                add_layer(pose, &delta, w, Some(mask));
            }
        }
        Ok(())
    }
}

/// A layer's weight `on` seconds after it began (rising over `blend_in`), and `off` seconds after it ended (falling
/// over `blend_out`).
fn fade(on: f32, off: Option<f32>, blend_in: f32, blend_out: f32) -> f32 {
    let rise = if blend_in > 0.0 {
        (on / blend_in).min(1.0)
    } else {
        1.0
    };
    let fall = off.map_or(1.0, |off| 1.0 - (off / blend_out.max(1e-3)).min(1.0));
    rise * fall
}

/// Opens a span when `on` starts, closes it when `on` stops, keeps it closed afterwards (until the next start).
fn span(span: &mut Option<Span>, on: bool, tick: u32) {
    match (on, span.as_mut()) {
        (true, Some(s)) if s.end.is_none() => {}
        (true, _) => {
            *span = Some(Span {
                start: tick,
                end: None,
            })
        }
        (false, Some(s)) if s.end.is_none() => s.end = Some(tick),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_turns_into_the_body_frame() {
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
        // Facing +y (yaw 90°) and moving +y: straight ahead.
        assert!(close(body_relative([0.0, 250.0, 0.0], 90.0), [250.0, 0.0]));
        // Facing +x and moving −y: strafing right (negative left).
        assert!(close(body_relative([0.0, -200.0, 0.0], 0.0), [0.0, -200.0]));
        // Facing −x (yaw 180°) and moving +x: backwards.
        assert!(close(
            body_relative([100.0, 0.0, 5.0], 180.0),
            [-100.0, 0.0]
        ));
    }

    #[test]
    fn weapons_pick_their_locomotion_set() {
        assert_eq!(WeaponSet::for_item(Some(7)), WeaponSet::Rifle); // AK-47
        assert_eq!(WeaponSet::for_item(Some(4)), WeaponSet::Pistol); // Glock-18
        assert_eq!(WeaponSet::for_item(Some(509)), WeaponSet::Knife); // a knife skin
        assert_eq!(WeaponSet::for_item(None), WeaponSet::Knife);
        assert_eq!(
            WeaponSet::for_category("weapon_category_smg"),
            WeaponSet::Rifle
        );
        assert_eq!(
            WeaponSet::for_category("weapon_category_grenade"),
            WeaponSet::Knife
        );
        assert_eq!(
            WeaponSet::for_category("weapon_category_pistol"),
            WeaponSet::Pistol
        );
        for (i, set) in WeaponSet::ALL.into_iter().enumerate() {
            assert_eq!(set.index(), i);
        }
    }

    #[test]
    fn angles_wrap_into_half_turns() {
        assert_eq!(normalize_angle(190.0), -170.0);
        assert_eq!(normalize_angle(-200.0), 160.0);
        assert_eq!(normalize_angle(45.0), 45.0);
    }

    #[test]
    fn actions_carry_on_from_the_one_they_take_over() {
        let interval = 1.0 / 64.0;
        let mut action = ActionSegment {
            item: 7,
            action: "action_attack",
            attack: "attack_grenade_throw",
            state: 202,
            stamp: Some(100),
            start: 100,
            end: None,
            released: None,
            blend_in: 0.1,
            blend_out: 0.2,
            swing_back: false,
            strength: 1.0,
            ammo: 1.0,
            variation: 0.5,
            inherited: 0.6,
            aim_from: 0.75,
        };
        // The layer starts where the old one was (0.6) and reaches 1 after the blend (6.4 ticks).
        assert!((action.layer_weight(100.0, interval) - 0.6).abs() < 1e-6);
        assert!((action.layer_weight(103.2, interval) - 0.8).abs() < 1e-4);
        assert_eq!(action.layer_weight(110.0, interval), 1.0);
        // The aim's fade goes on from 0.75: full 0.1 s later.
        assert!((action.aim_fade(100.0, interval) - 0.75).abs() < 1e-6);
        assert_eq!(action.aim_fade(106.4, interval), 1.0);
        // Ended at 106.4 (fade reached 1): half way down 0.2 s later, gone after 0.4 s.
        action.end = Some(110);
        assert!((action.layer_weight(116.4, interval) - 0.5).abs() < 1e-4);
        assert!((action.aim_fade(122.8, interval) - 0.5).abs() < 1e-4);
        assert_eq!(action.aim_fade(140.0, interval), 0.0);
        // Variations are repeatable and spread over 0 – 1.
        assert_eq!(variation(3, 25164), variation(3, 25164));
        let picks: Vec<f32> = (0..1000).map(|t| variation(5, 25000 + t)).collect();
        assert!(picks.iter().all(|v| (0.0..1.0).contains(v)));
        let low = picks.iter().filter(|&&v| v < 0.5).count();
        assert!((400..600).contains(&low), "{low}");
    }

    #[test]
    fn layers_fade_in_and_out() {
        assert_eq!(fade(0.05, None, 0.1, 0.2), 0.5);
        assert_eq!(fade(1.0, None, 0.1, 0.2), 1.0);
        assert_eq!(fade(1.0, Some(0.1), 0.1, 0.2), 0.5);
        assert_eq!(fade(1.0, Some(0.3), 0.1, 0.2), 0.0);
        assert_eq!(fade(0.0, None, 0.0, 0.2), 1.0);
        let mut s = None;
        span(&mut s, true, 5);
        span(&mut s, true, 6);
        span(&mut s, false, 9);
        span(&mut s, false, 10);
        assert!(matches!(
            s,
            Some(Span {
                start: 5,
                end: Some(9)
            })
        ));
        span(&mut s, true, 12);
        assert!(matches!(
            s,
            Some(Span {
                start: 12,
                end: None
            })
        ));
        assert!((ease(0.25) - 0.125).abs() < 1e-6 && ease(1.0) == 1.0);
    }
}
