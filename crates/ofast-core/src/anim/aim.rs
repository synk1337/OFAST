//! Aiming the upper body where the player looks: CS2's `CNmAimCSNode`, rebuilt.
//!
//! CS2 does not aim with clips. Its graph ends in `Aim IK`, a CS-specific node that takes the view relative to the
//! body (`aim_angle_yaw`, positive to the left; `aim_angle_pitch`, positive down, both clamped to ±89°), the weapon's
//! category and type, the crouch amount and the action in progress, and bends the skeleton procedurally:
//!
//! 1. **Body**: the yaw, remapped into −55 … 65°, turns `spine_0` … `spine_3` about the model's up axis by 0.1, 0.2,
//!    0.3 and 0.4 of it (each turn carried by the bones above); the pitch, remapped into the category's spine range,
//!    turns each spine bone about its own Z axis by 0.125, 0.225, 0.25 and 0.4 of it, and the neck and head by 0.35
//!    and 0.65 of the head range. Remapping takes −90 … 0 onto `min` … 0 and 0 … 90 onto 0 … `max`.
//! 2. **Weapon**: the weapon pivot (`wpnPivot`) moves onto `spine_3`. Knives and equipment pitch the pivot; pistols
//!    and rifles pitch the weapon, shift it when looking far up or down, then turn the barrel (`wpnEnd` → `wpnTip`)
//!    onto the aim direction (less while reloading or drawing); rifles also turn the head towards the muzzle.
//! 3. **Hands**: a two-bone IK puts `hand_R` (and `hand_L`, except for knives and grenades) on the weapon's hand
//!    bones (`wpnHand_R`, `wpnHand_L`).
//!
//! The constants are CS2's as Source 2 Viewer reconstructs them (MIT, `Renderer/Gameplay/AimCS.cs`); OFAST's code is
//! its own. Left out: the weapon drop parameter, the C4-planting fade, and the weapon tilt and roll when aiming off to
//! the side, all small.

use super::layers::{conjugate, slerp};
use super::{NmSkeleton, Transform};
use crate::model::{normalize_quat, quat_mul};

/// How a weapon is aimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AimCategory {
    /// Knives: the pivot pitches, no left-hand IK.
    Knife,
    /// Pistols and most SMGs.
    Pistol,
    /// Rifles, shotguns, snipers, machine guns and some SMGs.
    Rifle,
    /// Grenades: body aim only, no left-hand IK.
    Grenade,
    /// The C4, the Healthshot.
    Equipment,
}

/// The per-weapon variations within a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Other,
    MachineGun,
    Mp5sd,
    Mp9,
    P90,
    Bizon,
    SawedOff,
    Mag7,
    Famas,
    C4,
}

/// How a weapon (category and type IDs of the graphs) is aimed.
pub fn aim_category(category: &str, weapon: &str) -> Option<AimCategory> {
    classify(category, weapon).map(|(c, _)| c)
}

fn classify(category: &str, weapon: &str) -> Option<(AimCategory, Variant)> {
    use AimCategory as C;
    let exception = match (category, weapon) {
        ("weapon_category_smg", "weapon_mp5sd") => Some((C::Pistol, Variant::Mp5sd)),
        ("weapon_category_smg", "weapon_mp9") => Some((C::Rifle, Variant::Mp9)),
        ("weapon_category_smg", "weapon_p90") => Some((C::Rifle, Variant::P90)),
        ("weapon_category_smg", "weapon_bizon") => Some((C::Rifle, Variant::Bizon)),
        ("weapon_category_smg", "weapon_ump45") => Some((C::Rifle, Variant::Other)),
        ("weapon_category_shotgun", "weapon_sawedoff") => Some((C::Rifle, Variant::SawedOff)),
        ("weapon_category_shotgun", "weapon_mag7") => Some((C::Rifle, Variant::Mag7)),
        ("weapon_category_rifle" | "weapon_category_sniper", "weapon_famas") => {
            Some((C::Rifle, Variant::Famas))
        }
        ("weapon_category_equipment", "weapon_c4") => Some((C::Equipment, Variant::C4)),
        _ => None,
    };
    exception.or(match category {
        "weapon_category_knife" => Some((C::Knife, Variant::Other)),
        "weapon_category_pistol" | "weapon_category_smg" => Some((C::Pistol, Variant::Other)),
        "weapon_category_shotgun" | "weapon_category_rifle" | "weapon_category_sniper" => {
            Some((C::Rifle, Variant::Other))
        }
        "weapon_category_machinegun" => Some((C::Rifle, Variant::MachineGun)),
        "weapon_category_grenade" => Some((C::Grenade, Variant::Other)),
        "weapon_category_equipment" => Some((C::Equipment, Variant::Other)),
        _ => None,
    })
}

/// What the aim needs at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct AimInputs {
    /// View yaw relative to the body, degrees, positive to the left.
    pub yaw: f32,
    /// View pitch, degrees, positive down.
    pub pitch: f32,
    /// Crouch amount, 0 – 1.
    pub crouch: f32,
    /// How far a draw or reload is in (0 – 1, eased): pulls the pitch towards level and loosens the barrel alignment.
    pub action_weight: f32,
    /// How much the hands hold on to the weapon (0 – 1).
    pub hand_ik_weight: f32,
    /// The weapon's category ID (`weapon_category_rifle`…).
    pub category: String,
    /// The weapon's type ID (`weapon_ak47`…).
    pub weapon: String,
}

/// Angle ranges and weights for one weapon.
#[derive(Debug, Clone, Copy)]
struct Settings {
    yaw_range: [f32; 2],
    spine_pitch: [f32; 2],
    yaw_weights: [f32; 4],
    pitch_weights: [f32; 4],
    weapon_pitch: [f32; 2],
    head_pitch: [f32; 2],
    head_split: [f32; 2],
    offset_pitch: [f32; 2],
    offset_low: [f32; 3],
    offset_high: [f32; 3],
    left_hand_ik: bool,
}

fn settings(category: AimCategory, variant: Variant, crouch: f32) -> Settings {
    let c = crouch;
    let mut s = Settings {
        yaw_range: [-55.0, 65.0],
        spine_pitch: [-12.5, 10.0],
        yaw_weights: [0.1, 0.2, 0.3, 0.4],
        pitch_weights: [0.125, 0.225, 0.25, 0.4],
        weapon_pitch: [-90.0, 90.0],
        head_pitch: [-45.0, 35.0],
        head_split: [0.35, 0.65],
        offset_pitch: [-90.0, 90.0],
        offset_low: [0.0; 3],
        offset_high: [0.0; 3],
        left_hand_ik: true,
    };
    match category {
        AimCategory::Rifle => {
            s.spine_pitch = [-25.5, 25.0];
            s.head_pitch = [-35.0, 20.0];
            s.weapon_pitch = [-15.0 * c - 75.0, 80.0];
            s.offset_pitch = [
                -30.0,
                if variant == Variant::MachineGun {
                    20.0
                } else {
                    50.0
                },
            ];
            let low = [-10.0, 3.0, -5.0];
            (s.offset_low, s.offset_high) = match variant {
                Variant::Mp9 => (low, [2.0, 0.0, 4.0]),
                Variant::P90 => ([0.0; 3], [3.0, 2.0, 4.0]),
                Variant::Bizon => (low, [-6.0, 2.0, 6.0]),
                Variant::SawedOff => (low, [6.0, 2.0, 4.0]),
                Variant::Mag7 => ([-2.0, 0.0, 0.0], [0.0; 3]),
                Variant::Famas => (low, [-5.0, 1.0, 3.0]),
                Variant::MachineGun => ([-6.0, 4.0, -16.0], [-8.0, 0.0, 14.0]),
                _ => (low, [-1.0 - 7.0 * c, 3.0, 4.0 + 8.0 * c]),
            };
        }
        AimCategory::Pistol => {
            s.spine_pitch = [-10.0 * c - 25.5, 10.0];
            s.weapon_pitch = [-40.0, 55.0];
            s.head_pitch = [10.0 * c - 35.0, 35.0];
            s.offset_pitch = [-70.0, 50.0];
            s.offset_high = if variant == Variant::Mp5sd {
                [-2.0, 0.0, 4.0]
            } else {
                [0.0, 0.0, 1.0]
            };
        }
        AimCategory::Knife => {
            s.weapon_pitch = [-35.0, 25.0];
            s.spine_pitch = [-15.5, 25.0];
            s.head_pitch = [-35.0, 20.0];
            s.left_hand_ik = false;
        }
        AimCategory::Equipment => {
            s.weapon_pitch = [-15.0, 5.0];
            s.spine_pitch = [-15.5, 15.0];
            s.head_pitch = [-35.0, 20.0];
            s.left_hand_ik = variant == Variant::C4;
        }
        AimCategory::Grenade => s.left_hand_ik = false,
    }
    s
}

/// −90 … 0 onto `range[0]` … 0, 0 … 90 onto 0 … `range[1]`.
fn remap(value: f32, range: [f32; 2]) -> f32 {
    if (range[0] - range[1]).abs() < f32::EPSILON {
        return range[0];
    }
    if value < 0.0 {
        range[0] * (1.0 - ((value + 90.0) / 90.0).clamp(0.0, 1.0))
    } else {
        range[1] * (value / 90.0).clamp(0.0, 1.0)
    }
}

fn axis_angle(axis: [f32; 3], degrees: f32) -> [f32; 4] {
    let (s, c) = (degrees.to_radians() / 2.0).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let w = q[3];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let uv = cross(u, v);
    let uuv = cross(u, uv);
    std::array::from_fn(|k| v[k] + 2.0 * (w * uv[k] + uuv[k]))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalised(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = length(a);
    (l > 1e-6).then(|| a.map(|c| c / l))
}

/// The shortest rotation taking direction `from` onto `to`.
fn from_to(from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
    let (Some(a), Some(b)) = (normalised(from), normalised(to)) else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let d = dot(a, b);
    if d < -0.999_999 {
        // Opposite: any perpendicular axis.
        let axis = normalised(cross(a, [1.0, 0.0, 0.0]))
            .or_else(|| normalised(cross(a, [0.0, 1.0, 0.0])))
            .unwrap_or([0.0, 0.0, 1.0]);
        return [axis[0], axis[1], axis[2], 0.0];
    }
    let c = cross(a, b);
    normalize_quat([c[0], c[1], c[2], 1.0 + d])
}

/// A rigid transform (rotation, then translation).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rigid {
    q: [f32; 4],
    t: [f32; 3],
}

impl Rigid {
    fn of(bone: &Transform) -> Self {
        Self {
            q: bone.rotation,
            t: bone.translation,
        }
    }

    /// `self` then `local` in its frame: a child's model transform from its parent's.
    fn then(self, local: Self) -> Self {
        let r = rotate(self.q, local.t);
        Self {
            q: normalize_quat(quat_mul(self.q, local.q)),
            t: [self.t[0] + r[0], self.t[1] + r[1], self.t[2] + r[2]],
        }
    }

    fn inverse(self) -> Self {
        let q = conjugate(self.q);
        let t = rotate(q, self.t);
        Self {
            q,
            t: [-t[0], -t[1], -t[2]],
        }
    }
}

/// The clip-skeleton bones the aim moves.
#[derive(Debug, Clone)]
pub struct AimBones {
    spine: [usize; 4],
    neck: usize,
    head: usize,
    pivot: usize,
    weapon: usize,
    tip: usize,
    end: usize,
    weapon_hand: [usize; 2],
    arm_upper: [usize; 2],
    arm_lower: [usize; 2],
    hand: [usize; 2],
    parents: Vec<Option<usize>>,
}

impl AimBones {
    /// Finds the bones in a clip skeleton (`worldmodel.vnmskel`).
    pub fn find(skeleton: &NmSkeleton) -> Option<Self> {
        let f = |name: &str| skeleton.find(name);
        Some(Self {
            spine: [f("spine_0")?, f("spine_1")?, f("spine_2")?, f("spine_3")?],
            neck: f("neck_0")?,
            head: f("head_0")?,
            pivot: f("wpnPivot")?,
            weapon: f("wpn")?,
            tip: f("wpnTip")?,
            end: f("wpnEnd")?,
            weapon_hand: [f("wpnHand_L")?, f("wpnHand_R")?],
            arm_upper: [f("arm_upper_L")?, f("arm_upper_R")?],
            arm_lower: [f("arm_lower_L")?, f("arm_lower_R")?],
            hand: [f("hand_L")?, f("hand_R")?],
            parents: skeleton.parents.clone(),
        })
    }

    fn model(&self, pose: &[Transform], bone: usize) -> Rigid {
        let local = Rigid::of(&pose[bone]);
        match self.parents.get(bone).copied().flatten() {
            Some(p) => self.model(pose, p).then(local),
            None => local,
        }
    }

    /// Sets a bone's local transform from a wanted model-space transform.
    fn set_model(&self, pose: &mut [Transform], bone: usize, model: Rigid) {
        let parent = self
            .parents
            .get(bone)
            .copied()
            .flatten()
            .map(|p| self.model(pose, p));
        let local = match parent {
            Some(p) => p.inverse().then(model),
            None => model,
        };
        pose[bone].rotation = local.q;
        pose[bone].translation = local.t;
    }
}

/// Aims `pose` (clip-skeleton bones, relative to their parents) for `input`. Does nothing for a weapon the aim does
/// not know.
pub fn aim(pose: &mut [Transform], bones: &AimBones, input: &AimInputs) {
    let Some((category, variant)) = classify(&input.category, &input.weapon) else {
        return;
    };
    let s = settings(category, variant, input.crouch.clamp(0.0, 1.0));
    let yaw = input.yaw.clamp(-89.0, 89.0);
    let mut pitch = input.pitch.clamp(-89.0, 89.0);
    let action = input.action_weight.clamp(0.0, 1.0);
    if action > 0.0 {
        let a = 40.0 * action;
        pitch =
            (a - 90.0) + ((90.0 - a) - (a - 90.0)) * ((input.pitch + 90.0) / 180.0).clamp(0.0, 1.0);
    }
    // Undoes the weapon's own rotation when the pivot takes over the weapon's model rotation.
    let pivot_local = Rigid::of(&pose[bones.pivot]);
    let weapon_fix = normalize_quat(quat_mul(
        conjugate(pivot_local.then(Rigid::of(&pose[bones.weapon])).q),
        pivot_local.q,
    ));

    // 1. The body.
    let mut aim_yaw = yaw;
    if yaw.abs() > 0.001 {
        let y = remap(yaw, s.yaw_range);
        aim_yaw = y;
        for k in 0..4 {
            let bone = bones.spine[k];
            let mut model = bones.model(pose, bone);
            model.q = normalize_quat(quat_mul(
                axis_angle([0.0, 0.0, 1.0], y * s.yaw_weights[k]),
                model.q,
            ));
            bones.set_model(pose, bone, model);
        }
    }
    if pitch.abs() > 0.001 {
        let spine_pitch = remap(pitch, s.spine_pitch);
        for k in 0..4 {
            let bone = &mut pose[bones.spine[k]];
            bone.rotation = normalize_quat(quat_mul(
                bone.rotation,
                axis_angle([0.0, 0.0, 1.0], spine_pitch * s.pitch_weights[k]),
            ));
        }
        let head_pitch = remap(pitch, s.head_pitch);
        for (bone, share) in [(bones.neck, s.head_split[0]), (bones.head, s.head_split[1])] {
            pose[bone].rotation = normalize_quat(quat_mul(
                pose[bone].rotation,
                axis_angle([0.0, 0.0, 1.0], head_pitch * share),
            ));
        }
    }
    let (ps, pc) = pitch.to_radians().sin_cos();
    let (ys, yc) = aim_yaw.to_radians().sin_cos();
    let aim_direction = [pc * yc, pc * ys, -ps];
    // The pivot sits under an identity root: its local transform is the top spine bone's model transform.
    let chest = bones.model(pose, bones.spine[3]);
    bones.set_model(pose, bones.pivot, chest);

    // 2. The weapon.
    let weapon_pitch_scale = 1.0 - 0.5 * action;
    let weapon_model = match category {
        AimCategory::Knife | AimCategory::Equipment => {
            let mut pivot = bones.model(pose, bones.pivot);
            pivot.q = normalize_quat(quat_mul(
                axis_angle([0.0, 1.0, 0.0], remap(pitch, s.weapon_pitch)),
                pivot.q,
            ));
            bones.set_model(pose, bones.pivot, pivot);
            pivot.then(Rigid::of(&pose[bones.weapon]))
        }
        AimCategory::Pistol | AimCategory::Rifle => {
            let mut pivot = bones.model(pose, bones.pivot);
            let mut weapon = pivot.then(Rigid::of(&pose[bones.weapon]));
            let wp = remap(pitch, s.weapon_pitch) * weapon_pitch_scale;
            if category == AimCategory::Pistol {
                pivot.q = normalize_quat(quat_mul(axis_angle([0.0, 1.0, 0.0], wp), pivot.q));
                weapon = pivot.then(Rigid::of(&pose[bones.weapon]));
            } else {
                // Pitch about the weapon end's own Y axis.
                let mut end = weapon.then(Rigid::of(&pose[bones.end]));
                let end_to_weapon = end.inverse().then(weapon);
                end.q = normalize_quat(quat_mul(end.q, axis_angle([0.0, 1.0, 0.0], wp)));
                weapon = end.then(end_to_weapon);
            }
            weapon = offset_by_pitch(&s, pivot, weapon, pitch);
            if action < 1.0 {
                // Barrel onto the aim direction.
                let end = weapon.then(Rigid::of(&pose[bones.end]));
                let tip = weapon.then(Rigid::of(&pose[bones.tip]));
                let end_to_weapon = end.inverse().then(weapon);
                let arc = slerp(
                    from_to(sub(tip.t, end.t), aim_direction),
                    [0.0, 0.0, 0.0, 1.0],
                    action,
                );
                let turned = Rigid {
                    q: normalize_quat(quat_mul(arc, end.q)),
                    t: end.t,
                };
                weapon = turned.then(end_to_weapon);
            }
            pivot.q = normalize_quat(quat_mul(pivot.q, weapon_fix));
            bones.set_model(pose, bones.pivot, pivot);
            let local = pivot.inverse().then(weapon);
            pose[bones.weapon].rotation = local.q;
            pose[bones.weapon].translation = local.t;
            if category == AimCategory::Rifle && pitch > -25.0 {
                head_look_at(
                    pose,
                    bones,
                    weapon.then(Rigid::of(&pose[bones.tip])).t,
                    ((pitch + 25.0) / 115.0).clamp(0.0, 1.0),
                );
            }
            weapon
        }
        AimCategory::Grenade => bones.model(pose, bones.weapon),
    };

    // 3. The hands.
    let w = input.hand_ik_weight.clamp(0.0, 1.0);
    if w > 0.001 {
        for side in [1usize, 0] {
            if side == 0 && !s.left_hand_ik {
                continue;
            }
            let target = weapon_model.then(Rigid::of(&pose[bones.weapon_hand[side]]));
            two_bone_ik(pose, bones, side, target, w);
        }
    }
}

/// Shifts the weapon in its own axes when looking far up or down.
fn offset_by_pitch(s: &Settings, pivot: Rigid, weapon: Rigid, pitch: f32) -> Rigid {
    let zero = |v: [f32; 3]| v.iter().all(|c| c.abs() <= 0.01);
    if (zero(s.offset_low) && zero(s.offset_high))
        || (s.offset_pitch[0] <= -90.0 && s.offset_pitch[1] >= 90.0)
    {
        return weapon;
    }
    let offset = if pitch <= s.offset_pitch[0] && !zero(s.offset_low) {
        let t = ((pitch + 90.0) / (s.offset_pitch[0] + 90.0)).clamp(0.0, 1.0);
        s.offset_low.map(|c| c * (1.0 - t))
    } else if pitch >= s.offset_pitch[1] && !zero(s.offset_high) {
        let t = ((pitch - s.offset_pitch[1]) / (90.0 - s.offset_pitch[1])).clamp(0.0, 1.0);
        s.offset_high.map(|c| c * t)
    } else {
        return weapon;
    };
    let mut local = pivot.inverse().then(weapon);
    let shift = rotate(local.q, offset);
    for (t, s) in local.t.iter_mut().zip(shift) {
        *t += s;
    }
    pivot.then(local)
}

/// Turns the head's forward (its local +Y) towards `target` by `t`.
fn head_look_at(pose: &mut [Transform], bones: &AimBones, target: [f32; 3], t: f32) {
    let neck = bones.model(pose, bones.neck);
    let head = neck.then(Rigid::of(&pose[bones.head]));
    let Some(direction) = normalised(sub(target, head.t)) else {
        return;
    };
    let wanted = normalize_quat(quat_mul(
        from_to(rotate(head.q, [0.0, 1.0, 0.0]), direction),
        head.q,
    ));
    let looked = Rigid {
        q: slerp(head.q, wanted, t),
        t: head.t,
    };
    let local = neck.inverse().then(looked);
    pose[bones.head].rotation = local.q;
}

/// Bends the arm `side` (0 left, 1 right) so the hand reaches `target` (blended from where it is by `weight`), and
/// gives the hand the target's rotation: the elbow angle from the law of cosines in the arm's current bending plane,
/// then the whole arm turned onto the target.
fn two_bone_ik(pose: &mut [Transform], bones: &AimBones, side: usize, target: Rigid, weight: f32) {
    let (upper, lower, hand) = (
        bones.arm_upper[side],
        bones.arm_lower[side],
        bones.hand[side],
    );
    let a = bones.model(pose, upper);
    let b = a.then(Rigid::of(&pose[lower]));
    let c = b.then(Rigid::of(&pose[hand]));
    let goal = Rigid {
        q: slerp(c.q, target.q, weight),
        t: std::array::from_fn(|k| c.t[k] + (target.t[k] - c.t[k]) * weight),
    };
    let (l1, l2) = (length(sub(b.t, a.t)), length(sub(c.t, b.t)));
    if l1 < 1e-4 || l2 < 1e-4 {
        return;
    }
    let reach = length(sub(goal.t, a.t)).clamp((l1 - l2).abs() + 1e-3, l1 + l2 - 1e-3);
    // The bending plane's normal; a straight arm keeps a normal from its upper arm's frame.
    let normal = normalised(cross(sub(b.t, a.t), sub(c.t, b.t)))
        .unwrap_or_else(|| rotate(a.q, [0.0, 0.0, 1.0]));
    let current = dot(
        normalised(sub(a.t, b.t)).unwrap_or([1.0, 0.0, 0.0]),
        normalised(sub(c.t, b.t)).unwrap_or([1.0, 0.0, 0.0]),
    )
    .clamp(-1.0, 1.0)
    .acos();
    let wanted = ((l1 * l1 + l2 * l2 - reach * reach) / (2.0 * l1 * l2))
        .clamp(-1.0, 1.0)
        .acos();
    // Opening the elbow by (wanted − current) about the plane normal (sign: the normal points so that a positive turn
    // of the lower arm about it increases the elbow angle).
    let bend = axis_angle(normal, (current - wanted).to_degrees());
    let mut lower_model = b;
    lower_model.q = normalize_quat(quat_mul(bend, b.q));
    let c_after = lower_model.then(Rigid::of(&pose[hand]));
    // Turn the whole arm at the shoulder so the hand lands on the goal.
    let swing = from_to(sub(c_after.t, a.t), sub(goal.t, a.t));
    let mut upper_model = a;
    upper_model.q = normalize_quat(quat_mul(swing, a.q));
    bones.set_model(pose, upper, upper_model);
    let lower_after = bones.model(pose, upper).then(Rigid {
        q: normalize_quat(quat_mul(conjugate(a.q), lower_model.q)),
        t: pose[lower].translation,
    });
    // Lower arm: its model rotation is the swing applied to the bent one.
    let lower_wanted = Rigid {
        q: normalize_quat(quat_mul(swing, lower_model.q)),
        t: lower_after.t,
    };
    bones.set_model(pose, lower, lower_wanted);
    let lower_now = bones.model(pose, lower);
    let hand_local = lower_now.inverse().then(goal);
    pose[hand].rotation = hand_local.q;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_spreads_each_side_over_ninety_degrees() {
        assert_eq!(remap(0.0, [-55.0, 65.0]), 0.0);
        assert_eq!(remap(90.0, [-55.0, 65.0]), 65.0);
        assert_eq!(remap(-90.0, [-55.0, 65.0]), -55.0);
        assert!((remap(45.0, [-55.0, 65.0]) - 32.5).abs() < 1e-4);
    }

    #[test]
    fn weapons_classify_like_cs2() {
        assert_eq!(
            aim_category("weapon_category_rifle", "weapon_ak47"),
            Some(AimCategory::Rifle)
        );
        assert_eq!(
            aim_category("weapon_category_smg", "weapon_mac10"),
            Some(AimCategory::Pistol)
        );
        assert_eq!(
            aim_category("weapon_category_smg", "weapon_p90"),
            Some(AimCategory::Rifle)
        );
        assert_eq!(
            aim_category("weapon_category_knife", "weapon_knife"),
            Some(AimCategory::Knife)
        );
        assert_eq!(aim_category("nothing", "weapon_ak47"), None);
    }

    #[test]
    fn rigid_transforms_invert() {
        let a = Rigid {
            q: axis_angle([0.0, 0.0, 1.0], 30.0),
            t: [1.0, 2.0, 3.0],
        };
        let identity = a.inverse().then(a);
        assert!(identity.t.iter().all(|c| c.abs() < 1e-5));
        assert!((identity.q[3].abs() - 1.0).abs() < 1e-5);
        assert!(from_to([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])[2] > 0.7);
    }

    /// A three-bone arm along +X (shoulder at the origin, 10 + 10 long) reaches a point 15 away: the hand lands on it.
    #[test]
    fn two_bone_ik_reaches_the_target() {
        let skeleton = NmSkeleton {
            bones: [
                "root",
                "spine_0",
                "spine_1",
                "spine_2",
                "spine_3",
                "neck_0",
                "head_0",
                "wpnPivot",
                "wpn",
                "wpnTip",
                "wpnEnd",
                "wpnHand_L",
                "wpnHand_R",
                "arm_upper_L",
                "arm_lower_L",
                "hand_L",
                "arm_upper_R",
                "arm_lower_R",
                "hand_R",
            ]
            .map(String::from)
            .to_vec(),
            parents: vec![
                None,
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(0),
                Some(7),
                Some(8),
                Some(8),
                Some(8),
                Some(8),
                Some(4),
                Some(13),
                Some(14),
                Some(4),
                Some(16),
                Some(17),
            ],
            reference: Vec::new(),
        };
        let bones = AimBones::find(&skeleton).expect("bones");
        let mut pose = vec![Transform::IDENTITY; skeleton.bones.len()];
        // A slightly bent right arm: elbow 10 along X, hand 10 further along X and 1 up.
        pose[17].translation = [10.0, 0.0, 0.0];
        pose[18].translation = [10.0, 0.0, 1.0];
        let target = Rigid {
            q: [0.0, 0.0, 0.0, 1.0],
            t: [9.0, 12.0, 0.0],
        };
        two_bone_ik(&mut pose, &bones, 1, target, 1.0);
        let hand = bones.model(&pose, 18).t;
        let error = length(sub(hand, target.t));
        assert!(error < 1e-2, "hand at {hand:?}");
    }
}
