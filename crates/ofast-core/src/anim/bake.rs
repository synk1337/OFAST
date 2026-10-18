//! Each player's body animation over a round, computed from the demo's rig inputs (decision 013, "design A").
//!
//! The layers follow CS2's main graph (`worldmodel.vnmgraph`, `docs/formats/animation.md`), bottom to top:
//!
//! 1. **Locomotion** ([`Locomotion`]): the weapon's category picks one of the three sets (rifle, pistol, knife), the
//!    velocity in the body's frame and the crouch amount pick the clips; one phase advances tick by tick.
//! 2. **Weapon idle pose** (additive, upper body): the weapon's own `idle_<w>` / `idle_crouch_<w>` delta, blended by
//!    the crouch amount — how this weapon is held over the set's generic carry.
//! 3. **Weapon action** (model space, upper body): while the weapon is drawn (gameplay state 11) or reloaded
//!    (800 / 801), its `draw_*` / `reload_*` clip from the state's first tick, fading in and out.
//! 4. **Shot** (additive, upper body): `shoot_<w>` from each tick the weapon enters a shooting state (100 / 101).
//! 5. **Aim** ([`super::aim`]): the upper body turned towards the view (eye yaw relative to the body yaw, eye pitch),
//!    the weapon aligned, the hands on it.
//!
//! Poses are sampled at a fixed frame rate from the round's first tick, ready to key on a model's skeleton (after
//! [`super::Retargeter`]). Not baked yet: knife attacks, grenade throws, planting and defusing, jumps, ladders,
//! flinches, ragdolls.

use std::collections::HashMap;

use super::aim::{AimBones, AimInputs, aim};
use super::layers::{BoneMask, add_layer, model_space_layer};
use super::locomotion::{LocomotionBlend, blend_poses};
use super::{AnimError, Clip, Locomotion, Transform};
use crate::demo::players::RoundTrack;

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
    /// Draw: standing, crouched.
    pub draw: Option<[Clip; 2]>,
    /// Reload: standing, crouched.
    pub reload: Option<[Clip; 2]>,
}

/// What the upper-body layers need: the weapons by item definition index, CS2's `UpperBody` mask, the aim's bones.
#[derive(Debug, Clone)]
pub struct UpperBody {
    /// Weapons the round's players hold, by item definition index.
    pub weapons: HashMap<u32, WeaponAnimation>,
    /// The `UpperBody` bone mask of the clip skeleton.
    pub mask: BoneMask,
    /// Bones for the aim.
    pub aim: AimBones,
    /// Clip skeleton parents (for model-space layers).
    pub parents: Vec<Option<usize>>,
}

/// Blend-in time of a weapon action layer, seconds.
const ACTION_FADE_IN: f32 = 0.15;
/// Blend-out time after a weapon action, seconds.
const ACTION_FADE_OUT: f32 = 0.25;
/// The aim's action blend (`m_flActionBlendTimeSeconds`), seconds.
const AIM_ACTION_BLEND: f32 = 0.4;
/// The aim's hand IK blend-in (`m_flHandIKBlendInTimeSeconds`), seconds.
const HAND_IK_BLEND_IN: f32 = 0.3;

/// A weapon action in progress or just finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Draw,
    Reload,
}

/// An action: what, on which weapon, from which tick, until which tick (`None` while it lasts).
#[derive(Debug, Clone, Copy)]
struct Action {
    kind: ActionKind,
    item: u32,
    start: u32,
    end: Option<u32>,
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

/// The state of one living tick.
struct Step {
    tick: u32,
    set: WeaponSet,
    blend: LocomotionBlend,
    /// Phase reached at this tick (0 – 1).
    phase: f32,
    item: Option<u32>,
    crouch: f32,
    /// View yaw relative to the body (positive left) and pitch (positive down).
    aim_yaw: f32,
    aim_pitch: f32,
    /// The current or most recent weapon action.
    action: Option<Action>,
    /// Tick of the latest shot with the weapon in hand.
    shot: Option<u32>,
}

/// One player's poses (clip-skeleton bones) at `fps` frames per second from the round's first tick to its last:
/// `None` while the player has no living pawn. Without `upper`, the locomotion alone.
pub fn bake_player(
    track: &RoundTrack,
    controller: u32,
    sets: &LocomotionSets,
    upper: Option<&UpperBody>,
    fps: f32,
) -> Result<Vec<Option<Vec<Transform>>>, AnimError> {
    // The phase runs on through deaths and absences; it only has to be continuous while the player moves.
    let mut phase = 0.0f32;
    let mut previous: Option<u32> = None;
    let mut action: Option<Action> = None;
    let mut shot: Option<u32> = None;
    let mut last_state: Option<(u32, i64)> = None;
    let steps: Vec<Option<Step>> = track
        .samples
        .iter()
        .map(|sample| {
            let player = sample
                .players
                .iter()
                .find(|p| p.controller == controller && p.alive)?;
            let rig = &player.rig;
            let item = rig.weapon.as_ref().map(|w| w.item);
            let weapon = item.and_then(|i| upper?.weapons.get(&i));
            let set = weapon.map_or(WeaponSet::for_item(item), |w| {
                WeaponSet::for_category(&w.category)
            });
            let blend = sets
                .get(set)
                .blend(body_relative(rig.velocity, rig.body_yaw), rig.duck_amount);
            let dt = previous.map_or(0.0, |t| {
                sample.tick.saturating_sub(t) as f32 * track.tick_interval
            });
            phase = (phase + dt / blend.duration).rem_euclid(1.0);
            previous = Some(sample.tick);

            // Weapon actions and shots, from the weapon's gameplay animation state.
            let state = rig.weapon.as_ref().map(|w| (w.item, w.anim_state));
            let kind = match state.map(|(_, s)| s) {
                Some(11) => Some(ActionKind::Draw),
                Some(800 | 801) => Some(ActionKind::Reload),
                _ => None,
            };
            match (kind, item) {
                (Some(kind), Some(item)) => {
                    let continuing =
                        action.is_some_and(|a| a.kind == kind && a.item == item && a.end.is_none());
                    if !continuing {
                        action = Some(Action {
                            kind,
                            item,
                            start: sample.tick,
                            end: None,
                        });
                    }
                }
                _ => {
                    if let Some(a) = action.as_mut()
                        && a.end.is_none()
                    {
                        a.end = Some(sample.tick);
                    }
                }
            }
            // A shot starts whenever the state enters 100 or 101 (they alternate shot by shot).
            if let Some((_, s)) = state
                && matches!(s, 100 | 101)
                && last_state != state
            {
                shot = Some(sample.tick);
            }
            last_state = state;

            Some(Step {
                tick: sample.tick,
                set,
                blend,
                phase,
                item,
                crouch: rig.duck_amount,
                aim_yaw: normalize_angle(player.yaw - rig.body_yaw),
                aim_pitch: player.pitch,
                action,
                shot,
            })
        })
        .collect();

    let seconds = track.end_tick.saturating_sub(track.start_tick) as f32 * track.tick_interval;
    let frames = (seconds * fps).floor() as usize + 1;
    let mut out = Vec::with_capacity(frames);
    let mut index = 0usize;
    for k in 0..frames {
        // The demo tick this frame shows (fractional), and the last sample at or before it.
        let tick = track.start_tick as f32 + k as f32 / fps / track.tick_interval;
        while index + 1 < track.samples.len() && track.samples[index + 1].tick as f32 <= tick {
            index += 1;
        }
        let pose = match steps.get(index).and_then(Option::as_ref) {
            Some(step) => {
                let extra = (tick - step.tick as f32).max(0.0) * track.tick_interval;
                let phase = step.phase + extra / step.blend.duration;
                let mut pose = sets.get(step.set).sample(&step.blend, phase)?;
                if let Some(upper) = upper {
                    layer_upper_body(&mut pose, upper, step, tick, track.tick_interval)?;
                }
                Some(pose)
            }
            None => None,
        };
        out.push(pose);
    }
    Ok(out)
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

/// Layers 2 – 5 of the module docs over a locomotion pose, at demo tick `tick` (fractional).
fn layer_upper_body(
    pose: &mut [Transform],
    upper: &UpperBody,
    step: &Step,
    tick: f32,
    interval: f32,
) -> Result<(), AnimError> {
    let since = |t: u32| (tick - t as f32).max(0.0) * interval;
    let weapon = step.item.and_then(|i| upper.weapons.get(&i));
    if let Some(idle) = weapon.and_then(|w| w.idle.as_ref()) {
        add_layer(
            pose,
            &crouch_blend(idle, 0.0, step.crouch)?,
            1.0,
            Some(&upper.mask),
        );
    }

    // The action layer, its weight and the aim's action weight.
    let mut action_weight = 0.0f32;
    let mut hand_ik = 1.0f32;
    if let Some(action) = step.action {
        let clips = upper
            .weapons
            .get(&action.item)
            .and_then(|w| match action.kind {
                ActionKind::Draw => w.draw.as_ref(),
                ActionKind::Reload => w.reload.as_ref(),
            });
        let elapsed = since(action.start);
        let (fade, aim_fade, ik) = match action.end {
            None => (
                (elapsed / ACTION_FADE_IN).min(1.0),
                (elapsed / AIM_ACTION_BLEND).min(1.0),
                0.0,
            ),
            Some(end) => {
                let after = since(end);
                (
                    1.0 - (after / ACTION_FADE_OUT).min(1.0),
                    1.0 - (after / AIM_ACTION_BLEND).min(1.0),
                    (after / HAND_IK_BLEND_IN).min(1.0),
                )
            }
        };
        hand_ik = ik;
        // In-out quadratic easing, as the aim node's action blend.
        action_weight = if aim_fade < 0.5 {
            2.0 * aim_fade * aim_fade
        } else {
            1.0 - (-2.0 * aim_fade + 2.0).powi(2) / 2.0
        };
        if let Some(clips) = clips
            && fade > 0.0
            && step.item == Some(action.item)
        {
            let layer = crouch_blend(clips, elapsed, step.crouch)?;
            model_space_layer(pose, &layer, fade, &upper.mask, &upper.parents);
        }
    }

    if let (Some(shot), Some(shoot)) = (step.shot, weapon.and_then(|w| w.shoot.as_ref())) {
        let elapsed = since(shot);
        if elapsed < shoot.duration {
            add_layer(pose, &shoot.sample(elapsed, false)?, 1.0, Some(&upper.mask));
        }
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
    Ok(())
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
    }

    #[test]
    fn angles_wrap_into_half_turns() {
        assert_eq!(normalize_angle(190.0), -170.0);
        assert_eq!(normalize_angle(-200.0), 160.0);
        assert_eq!(normalize_angle(45.0), 45.0);
    }
}
