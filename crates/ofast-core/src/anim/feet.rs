//! Feet on the ground: CS2's foot IK, rebuilt for stairs and slopes.
//!
//! CS2's main graph puts each ankle on a target just under the aim (`CNmFootIKNode` "Foot IK" in
//! `worldmodel.vnmgraph`: effectors `ankle_L` / `ankle_R`, targets the control parameters `ik_left_foot` and
//! `ik_right_foot` that the game's code sets from the ground, in character space). Demos carry neither the targets
//! nor the ground the game traced, so OFAST places the feet itself, from the map's collision ([`plant_feet`]):
//!
//! 1. Wherever a player stands (on the ground, not on a ladder), the ground under each foot: rays down at the ankle,
//!    the middle of the foot and its ball, the middle height of the three, against the ground the caller gives (the
//!    export: the map's drawn surfaces facing up, `export::ground`, which have the steps a clip ramp covers), and the
//!    median over a few frames (a foot swaying over a step's edge would flicker between two heights).
//! 2. A foot down (its ankle within [`PLANT_LIFT`] of its height at bind pose, where the clips put a planted foot)
//!    takes the ground under it. A foot in the air between two steps goes from the ground it left to the ground it
//!    lands on (smoothstep over the swing), so it rises onto a stair as it swings instead of jumping up the moment it
//!    passes over the edge.
//! 3. A foot over ground more than [`MAX_STEP`] below the ground under the other foot, that drop in one go between
//!    them (on stairs the ground between the feet goes down step by step), hangs over a ledge, as in CS2: it keeps the
//!    animation's pose. The pelvis comes down only as far as the feet that carry it cannot reach their ground by
//!    straightening their legs (to [`STRAIGHT`] of their length; never up, at most [`MAX_STEP`]), smoothed over
//!    [`PELVIS_SMOOTH_S`], and it eases back up to CS2's pose around a hit on the player ([`HIT_KEEP_S`]: the
//!    server's hitboxes have no lowered body). A player's origin rests on the highest ground under its 32-inch box (a
//!    step's top, the uphill side of a slope) or on a clip ramp laid over stairs (de_anubis's starts up to 18 inches
//!    over the floor before the first step), and climbs a stair one step a tick: without this the body floats over the
//!    lower steps and pops up each step. A foot in the air goes no lower than the body brings it: on a steep clip ramp
//!    the origin climbs 25 inches while one foot is down; that foot's leg reaches down towards its step as far as it
//!    goes, then the foot lifts.
//! 4. Each leg reaches its ankle's target with a two-bone IK: the thigh and the shin turn in the leg's own bend plane
//!    (the knee where the animation points it), their lengths kept. The foot keeps the animation's orientation,
//!    tilted to the ground's slope while it is down (at most [`MAX_TILT_DEG`]). Each foot's height in the world is
//!    smoothed over [`FOOT_SMOOTH_S`] where it takes or loses its ground.
//!
//! It eases in over [`BLEND_S`] after a landing and out before a jump or a ladder. `OFAST_FEET=0` turns it off (to
//! compare); `OFAST_FEET_DUMP=<file>` appends each standing frame's legs before and after, and the ground along the
//! way, for side views (`tools/compare/feet_views.py`). Details and checks: `docs/formats/animation.md`, "Feet on the
//! ground".

use crate::model::{Affine, Skeleton};

/// Where a ray down for the ground under a foot starts, above the player's origin (inches): above the highest step a
/// foot may stand on that the origin is not on.
pub const RAY_UP: f32 = 20.0;

/// The most a foot is raised above the animation (inches): a stair step is 8 to 18. Ground higher than this under a
/// foot is a wall or a box the foot is in front of: none for that foot.
pub const MAX_UP: f32 = 18.0;

/// How far below the player's origin the ground is looked for (inches): a clip ramp's height over the floor in front
/// of its stairs (18 on de_anubis) and more. Further down, the player stands on something the ground does not have.
pub const MAX_BELOW: f32 = 24.0;

/// The most one foot's ground lies below the other's (inches), and the most the pelvis comes down: a step and a half.
/// Lower, the foot hangs over a ledge.
pub const MAX_STEP: f32 = 14.0;

/// How straight a leg gets reaching down to its ground (of its full length): a bent leg straightens before the body
/// comes down, and the body comes down only for what the legs cannot reach (the server's hitboxes, which the demo's
/// hits were on, stay where CS2 has them on gentle slopes).
pub const STRAIGHT: f32 = 0.98;

/// A foot is down while its ankle is within this of its height at bind pose (inches): the clips' planted feet are
/// at that height, a swinging foot 3 to 10 inches above it.
pub const PLANT_LIFT: f32 = 2.0;

/// The longest a foot is in the air between two steps on the ground (seconds); longer, it follows the ground under
/// it.
pub const MAX_SWING_S: f32 = 0.8;

/// How much the pelvis's height is smoothed (seconds: the standard deviation of a Gaussian over the frames).
pub const PELVIS_SMOOTH_S: f32 = 0.08;

/// How long the feet ease into the ground after a landing, and out of it before a jump (seconds).
pub const BLEND_S: f32 = 0.15;

/// The most a foot down tilts to the ground's slope (degrees).
pub const MAX_TILT_DEG: f32 = 25.0;

/// How long before and after a hit on a player the body eases back up to where CS2's server posed it (seconds). The
/// server's graph has the foot IK (its leg hits fit OFAST's IK'd legs better than the animation's) but nothing that
/// lowers the body: with the pelvis lowered, a hit's blood would show above a head (de_anubis round 14, tick 138704:
/// 4 inches, the player on a clip 2 inches over the drawn floor). The legs still reach their ground as far as they go.
pub const HIT_KEEP_S: f32 = 0.3;

/// How much each foot's height in the world is smoothed (seconds, a Gaussian's standard deviation): where it takes or
/// loses its ground it goes over in a few frames.
pub const FOOT_SMOOTH_S: f32 = 0.03;

/// The ground under a foot is the median of this many frames each side and its own (a foot over an edge, swaying,
/// would flicker between two heights).
const GROUND_MEDIAN: usize = 3;

/// No ground, among heights (below any).
const NONE: f32 = -1.0e9;

/// A foot moved by more than this counts in [`FeetReport::moved`] (inches).
const MOVED: f32 = 1.0;

type V3 = [f32; 3];
type M3 = [[f32; 3]; 3];

/// The bones [`plant_feet`] moves, found by name in an agent's skeleton.
#[derive(Debug, Clone)]
pub struct Legs {
    /// The pelvis and every bone under it (moved with it).
    body: Vec<usize>,
    legs: [Leg; 2],
    /// The ankles' height above the floor at bind pose (inches), where the clips put a planted foot.
    floor_ankle: f32,
}

#[derive(Debug, Clone)]
struct Leg {
    hip: usize,
    knee: usize,
    ankle: usize,
    ball: Option<usize>,
    /// The thigh's bone and everything under it (the knee, the foot, the thigh's twist bones).
    thigh: Vec<usize>,
    /// The shin's bone and everything under it.
    shin: Vec<usize>,
    /// The ankle and everything under it.
    foot: Vec<usize>,
}

impl Legs {
    /// The legs of a skeleton with CS2's agent bones (`pelvis`, `leg_upper_L`, `leg_lower_L`, `ankle_L`, `ball_L`
    /// and the `_R` ones); `None` without them.
    pub fn of(skeleton: &Skeleton) -> Option<Self> {
        let under = |root: usize| -> Vec<usize> {
            // Parents come before their children: one pass marks every bone whose parent is marked.
            let mut inside = vec![false; skeleton.bones.len()];
            inside[root] = true;
            for (i, bone) in skeleton.bones.iter().enumerate() {
                if let Some(p) = bone.parent
                    && inside[p]
                {
                    inside[i] = true;
                }
            }
            (0..inside.len()).filter(|&i| inside[i]).collect()
        };
        let leg = |side: &str| -> Option<Leg> {
            let hip = skeleton.find(&format!("leg_upper_{side}"))?;
            let knee = skeleton.find(&format!("leg_lower_{side}"))?;
            let ankle = skeleton.find(&format!("ankle_{side}"))?;
            Some(Leg {
                hip,
                knee,
                ankle,
                ball: skeleton.find(&format!("ball_{side}")),
                thigh: under(hip),
                shin: under(knee),
                foot: under(ankle),
            })
        };
        let pelvis = skeleton.find("pelvis")?;
        let legs = [leg("L")?, leg("R")?];
        let bind = skeleton.world_transforms();
        let floor_ankle = legs.iter().map(|l| bind[l.ankle][2][3]).sum::<f32>() / 2.0;
        Some(Legs {
            body: under(pelvis),
            legs,
            floor_ankle,
        })
    }

    /// The bones an `OFAST_FEET_DUMP` line lists: the pelvis, then each leg's hip, knee, ankle and ball (left first).
    fn dumped(&self) -> Vec<usize> {
        let mut bones = vec![self.body[0]];
        for leg in &self.legs {
            bones.extend([leg.hip, leg.knee, leg.ankle]);
            bones.extend(leg.ball);
        }
        bones
    }
}

/// Where a player is on one frame, for [`plant_feet`].
#[derive(Debug, Clone, Copy)]
pub struct Stance {
    /// The player's actor: the origin (feet) and the body's yaw ([`crate::export`]'s placement of the agent).
    pub actor: Affine,
    /// On the ground and not on a ladder: the feet stand on something.
    pub standing: bool,
    /// The demo has a bullet hitting the player on this frame: the body stays where CS2's server posed it (its
    /// hitboxes, which the hit is on), the pelvis not lowered ([`HIT_KEEP_S`]).
    pub hit: bool,
}

/// What [`plant_feet`] did over one player's frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FeetReport {
    /// Frames standing (where the feet were placed).
    pub standing: usize,
    /// Frames with a foot moved by more than an inch.
    pub moved: usize,
    /// Frames with a foot short of its target by more than an inch (out of the leg's reach).
    pub short: usize,
    /// Frames with a foot hanging over a ledge ([`MAX_STEP`]).
    pub hanging: usize,
    /// The most the pelvis came down (inches).
    pub pelvis_drop: f32,
}

impl FeetReport {
    /// Adds another player's.
    pub fn add(&mut self, other: FeetReport) {
        self.standing += other.standing;
        self.moved += other.moved;
        self.short += other.short;
        self.hanging += other.hanging;
        self.pelvis_drop = self.pelvis_drop.max(other.pelvis_drop);
    }
}

/// Whether feet are placed: unless the environment says `OFAST_FEET=0`.
pub fn enabled() -> bool {
    std::env::var("OFAST_FEET").map_or(true, |v| v != "0")
}

/// One foot on one standing frame, as the animation has it.
#[derive(Debug, Clone, Copy)]
struct FootFrame {
    /// The ankle's position (model space).
    ankle: V3,
    /// How much the foot is down: 1 at the planted height, 0 from [`PLANT_LIFT`] above it.
    down: f32,
    /// The ground under the foot (world height) and its normal (world), if any.
    ground: Option<(f32, V3)>,
    /// How far down the ankle reaches from the animation's pose by straightening its leg to [`STRAIGHT`] of its
    /// length, the hip where it is (inches).
    slack: f32,
}

/// Places the feet of one player's frames on the ground (see the [module docs](self)): `worlds` are the agent's bones
/// in model space, frame by frame (moved in place), `stances` where the player is on each frame (`None` without a
/// living pawn), `ground` a ray down: from a point, how far it goes before it meets the world within a reach, and
/// the surface's normal. `fps` is the frames' rate.
pub fn plant_feet(
    worlds: &mut [Vec<Affine>],
    stances: &[Option<Stance>],
    legs: &Legs,
    ground: &dyn Fn(V3, f32) -> Option<(f32, V3)>,
    fps: f32,
) -> FeetReport {
    let frames = worlds.len().min(stances.len());
    let standing = |f: usize| stances[f].filter(|s| s.standing);
    let origin_z = |f: usize| stances[f].map_or(0.0, |s| s.actor[2][3]);

    // 1. The feet as the animation has them, and the ground under them.
    let feet: Vec<Option<[FootFrame; 2]>> = (0..frames)
        .map(|f| {
            let stance = standing(f)?;
            let world = &worlds[f];
            Some(std::array::from_fn(|i| {
                let leg = &legs.legs[i];
                let ankle = position(&world[leg.ankle]);
                let lift = ankle[2] - legs.floor_ankle;
                let down = 1.0 - smoothstep(0.5 * PLANT_LIFT, PLANT_LIFT, lift);
                let hip = position(&world[leg.hip]);
                let knee = position(&world[leg.knee]);
                let length = STRAIGHT * (length(sub(knee, hip)) + length(sub(ankle, knee)));
                let across = (ankle[0] - hip[0]).hypot(ankle[1] - hip[1]);
                let lowest = hip[2] - (length * length - across * across).max(0.0).sqrt();
                let slack = (ankle[2] - lowest).max(0.0);
                let origin = stance.actor[2][3];
                let top = origin + RAY_UP;
                // Ground within MAX_BELOW under the origin and MAX_UP over it; none under a wall.
                let under = |p: V3| {
                    let at = apply(&stance.actor, p);
                    ground([at[0], at[1], top], RAY_UP + MAX_BELOW)
                        .map(|(d, n)| (top - d, n))
                        .filter(|&(z, _)| z - origin <= MAX_UP)
                };
                // Under the ankle, the middle of the foot and its ball: the middle height of the three, where most
                // of the foot is (a foot over a step's edge stands on the step only when most of it is over it).
                let ball = leg.ball.map(|b| position(&world[b]));
                let mut hits: Vec<(f32, V3)> =
                    [Some(ankle), ball.map(|b| scale(add(ankle, b), 0.5)), ball]
                        .into_iter()
                        .flatten()
                        .map(|p| under(p).unwrap_or((NONE, [0.0, 0.0, 1.0])))
                        .collect();
                hits.sort_by(|a, b| a.0.total_cmp(&b.0));
                let ground = Some(hits[hits.len() / 2]).filter(|(z, _)| *z > NONE);
                FootFrame {
                    ankle,
                    down,
                    ground,
                    slack,
                }
            }))
        })
        .collect();
    // The ground under each foot over a few frames (median): a foot over a step's edge or a ledge's, swaying with the
    // idle, would flicker between the two heights, and between a ground and none.
    let mut feet = feet;
    let runs = standing_runs(&feet);
    for i in 0..2 {
        for &(start, end) in &runs {
            let heights: Vec<f32> = (start..end)
                .map(|f| {
                    feet[f]
                        .and_then(|foot| foot[i].ground)
                        .map_or(NONE, |(z, _)| z)
                })
                .collect();
            for (f, frame) in feet.iter_mut().enumerate().take(end).skip(start) {
                let lo = f.saturating_sub(GROUND_MEDIAN).max(start);
                let hi = (f + GROUND_MEDIAN + 1).min(end);
                let mut window: Vec<f32> = heights[lo - start..hi - start].to_vec();
                window.sort_by(f32::total_cmp);
                let median = window[window.len() / 2];
                if let Some(foot) = frame.as_mut() {
                    let normal = foot[i].ground.map_or([0.0, 0.0, 1.0], |(_, n)| n);
                    foot[i].ground = (median > NONE).then_some((median, normal));
                }
            }
        }
    }

    // 2. Each foot's ground: under it while it is down, from one step to the next while it swings.
    let swing = (MAX_SWING_S * fps).round().max(1.0) as usize;
    let mut targets: [Vec<Option<f32>>; 2] = std::array::from_fn(|i| {
        let planted = |f: usize| {
            feet[f]
                .filter(|foot| foot[i].down >= 0.5)
                .and_then(|foot| foot[i].ground)
                .map(|(z, _)| z)
        };
        (0..frames)
            .map(|f| {
                let foot = feet[f]?;
                if let Some(z) = planted(f) {
                    return Some(z);
                }
                // The steps around this swing, within the same stretch on the ground.
                let before = (f.saturating_sub(swing)..f)
                    .rev()
                    .take_while(|&g| feet[g].is_some())
                    .find_map(|g| planted(g).map(|z| (g, z)));
                let after = (f + 1..(f + swing + 1).min(frames))
                    .take_while(|&g| feet[g].is_some())
                    .find_map(|g| planted(g).map(|z| (g, z)));
                match (before, after) {
                    (Some((a, za)), Some((b, zb))) => {
                        let t = (f - a) as f32 / (b - a) as f32;
                        Some(za + (zb - za) * smoothstep(0.0, 1.0, t))
                    }
                    (Some((_, z)), None) | (None, Some((_, z))) => Some(z),
                    (None, None) => foot[i].ground.map(|(z, _)| z),
                }
            })
            .collect()
    });
    // A foot over ground more than a step below the ground under the other foot, with that drop in one place between
    // them, hangs over a ledge (the grounds under the feet as they are, not a swinging foot's way to its next step).
    // Feet a long stride apart on a stair are steps apart, the ground between them going down step by step: no
    // ledge. Decided over a few frames (median), so that a foot does not flicker between hanging and standing.
    let ledge: Vec<Option<usize>> = (0..frames)
        .map(|f| {
            let foot = feet[f]?;
            let stance = standing(f)?;
            let (l, r) = (foot[0].ground?.0, foot[1].ground?.0);
            if (l - r).abs() <= MAX_STEP {
                return None;
            }
            let at = |i: usize| apply(&stance.actor, foot[i].ankle);
            drop_between(at(0), at(1), stance.actor[2][3], ground).then_some(usize::from(r < l))
        })
        .collect();
    let mut hanging = 0;
    for &(start, end) in &runs {
        for (side, target) in targets.iter_mut().enumerate() {
            for (f, at) in target.iter_mut().enumerate().take(end).skip(start) {
                let lo = f.saturating_sub(GROUND_MEDIAN).max(start);
                let hi = (f + GROUND_MEDIAN + 1).min(end);
                let votes = ledge[lo..hi].iter().filter(|&&l| l == Some(side)).count();
                if 2 * votes > hi - lo {
                    *at = None;
                    hanging += 1;
                }
            }
        }
    }

    // 3. The pelvis: down to the lower foot's ground, its height smoothed within each stretch on the ground, kept up
    // around the frames the player is hit; and the easing in and out of each stretch.
    let mut pelvis = vec![0.0f32; frames];
    let mut blend = vec![0.0f32; frames];
    let ease = (BLEND_S * fps).max(1.0);
    let keep = (HIT_KEEP_S * fps).max(1.0);
    let hits: Vec<usize> = (0..frames)
        .filter(|&f| stances[f].is_some_and(|s| s.hit))
        .collect();
    let kept_up = |f: usize| {
        hits.iter()
            .map(|&h| 1.0 - smoothstep(0.0, keep, h.abs_diff(f) as f32))
            .fold(0.0f32, f32::max)
    };
    for &(start, end) in &runs {
        // The pelvis's height in the world, before smoothing: down as far as the feet that carry the body (those down;
        // both when neither is: a run's flight) cannot reach their ground by straightening their legs.
        let height: Vec<f32> = (start..end)
            .map(|g| {
                let need = |i: usize, carrying: bool| {
                    let foot = feet[g]?[i];
                    let z = targets[i][g].filter(|_| !carrying || foot.down >= 0.5)?;
                    Some(z - origin_z(g) + foot.slack)
                };
                let carrying = [need(0, true), need(1, true)];
                let needs = if carrying.iter().any(Option::is_some) {
                    carrying
                } else {
                    [need(0, false), need(1, false)]
                };
                let lowest = needs.into_iter().flatten().fold(0.0f32, f32::min);
                origin_z(g) + lowest.max(-MAX_STEP)
            })
            .collect();
        let smooth = gaussian(&height, PELVIS_SMOOTH_S * fps);
        for g in start..end {
            pelvis[g] =
                (smooth[g - start] - origin_z(g)).clamp(-MAX_STEP, 0.0) * (1.0 - kept_up(g));
            let into = (g - start + 1) as f32 / ease;
            let out_of = (end - g) as f32 / ease;
            blend[g] = smoothstep(0.0, 1.0, into.min(out_of).min(1.0));
        }
    }
    // How far each foot moves (before the easing): onto its ground, but a foot down no lower than its straightened leg
    // reaches from where the body brings it, and a foot in the air no lower than the body brings it (a rear foot on
    // a steep stair lifts with the body rather than stretching back down to the step it left); with the pelvis when
    // it has no ground (hanging, or nothing under it). Its height in the world smoothed over a few frames: where a
    // foot takes or loses its ground (a ledge, the edge of what the ground has), it goes over in FOOT_SMOOTH_S rather
    // than at once.
    let raw = |f: usize, i: usize| {
        let (down, slack) = feet[f].map_or((0.0, 0.0), |foot| (foot[i].down, foot[i].slack));
        targets[i][f].map_or(pelvis[f], |z| {
            (z - origin_z(f)).clamp(pelvis[f] - slack * down, MAX_UP)
        })
    };
    let mut offsets = [vec![0.0f32; frames], vec![0.0f32; frames]];
    for (i, out) in offsets.iter_mut().enumerate() {
        for &(start, end) in &runs {
            let world: Vec<f32> = (start..end).map(|f| origin_z(f) + raw(f, i)).collect();
            for (k, z) in gaussian(&world, FOOT_SMOOTH_S * fps)
                .into_iter()
                .enumerate()
            {
                out[start + k] = z - origin_z(start + k);
            }
        }
    }
    let offset = |f: usize, i: usize| offsets[i][f];

    // 4. Each frame: the body down, the legs onto their targets, the feet tilted.
    let mut report = FeetReport::default();
    let mut dump = std::env::var_os("OFAST_FEET_DUMP").and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let dumped = legs.dumped();
    for f in 0..frames {
        let (Some(foot), Some(stance)) = (feet[f], standing(f)) else {
            continue;
        };
        report.standing += 1;
        let b = blend[f];
        let drop = b * pelvis[f];
        report.pelvis_drop = report.pelvis_drop.max(-drop);
        let world = &mut worlds[f];
        let original: Vec<Affine> = legs.legs.iter().map(|leg| world[leg.ankle]).collect();
        let before: Vec<V3> = match dump {
            Some(_) => dumped
                .iter()
                .map(|&b| apply(&stance.actor, position(&world[b])))
                .collect(),
            None => Vec::new(),
        };
        for &bone in &legs.body {
            world[bone][2][3] += drop;
        }
        let mut moved = false;
        let mut short = false;
        for (i, leg) in legs.legs.iter().enumerate() {
            let lift = b * offset(f, i);
            let target = [foot[i].ankle[0], foot[i].ankle[1], foot[i].ankle[2] + lift];
            moved |= lift.abs() > MOVED;
            short |= reach(world, leg, target) > MOVED;
            // The foot as the animation turned it, tilted to the ground while down on it.
            let tilt = foot[i]
                .ground
                .filter(|_| targets[i][f].is_some())
                .map(|(_, n)| tilt_to(unrotate(&stance.actor, n), foot[i].down * b))
                .unwrap_or(IDENTITY);
            let wanted = mul(&tilt, &rotation(&original[i]));
            let now = rotation(&world[leg.ankle]);
            let turn = mul(&wanted, &transpose(&now));
            let pivot = position(&world[leg.ankle]);
            rotate(world, &leg.foot, &turn, pivot);
        }
        report.moved += usize::from(moved);
        report.short += usize::from(short);
        if let Some(file) = dump.as_mut() {
            let after: Vec<V3> = dumped
                .iter()
                .map(|&b| apply(&stance.actor, position(&worlds[f][b])))
                .collect();
            dump_frame(file, f, &before, &after, stances, ground);
        }
    }
    report.hanging = hanging;
    report
}

/// One line of the `OFAST_FEET_DUMP` file, for side views of the legs over the ground
/// (`tools/compare/feet_views.py`): the frame, the bones [`Legs::dumped`] lists after and before the feet were placed
/// (world, inches), the player's origin, the height of the ground every 2 inches from 40 behind the origin to 40
/// ahead along the way the player goes (`nan` for none), and that way (x, y). Fields are separated by `|`; a frame
/// number lower than the one before starts the next player.
fn dump_frame(
    file: &mut std::fs::File,
    f: usize,
    before: &[V3],
    after: &[V3],
    stances: &[Option<Stance>],
    ground: &dyn Fn(V3, f32) -> Option<(f32, V3)>,
) {
    use std::fmt::Write as _;
    use std::io::Write as _;
    let Some(stance) = stances[f] else {
        return;
    };
    let mut line = format!("{f}");
    for bones in [after, before] {
        for p in bones {
            let _ = write!(line, " {:.2} {:.2} {:.2}", p[0], p[1], p[2]);
        }
        line.push_str(" |");
    }
    let o = position(&stance.actor);
    let _ = write!(line, " {:.2} {:.2} {:.2} |", o[0], o[1], o[2]);
    // The way the player goes: from the frame before to the frame after (where they face when still).
    let at = |g: Option<usize>| g.and_then(|g| stances.get(g).copied().flatten());
    let (p0, p1) = (
        at(f.checked_sub(1)).map_or(o, |s| position(&s.actor)),
        at(Some(f + 1)).map_or(o, |s| position(&s.actor)),
    );
    let d = sub(p1, p0);
    let l = d[0].hypot(d[1]);
    let way = if l > 1e-3 {
        [d[0] / l, d[1] / l]
    } else {
        [stance.actor[0][0], stance.actor[1][0]]
    };
    for k in -20..=20 {
        let s = k as f32 * 2.0;
        let top = o[2] + 40.0;
        let z = ground([o[0] + way[0] * s, o[1] + way[1] * s, top], 80.0)
            .map_or(f32::NAN, |(d, _)| top - d);
        let _ = write!(line, " {z:.2}");
    }
    let _ = writeln!(file, "{line} | {:.3} {:.3}", way[0], way[1]);
}

/// Turns `leg` so that its ankle reaches `target` (model space): the thigh about the hip, then the shin about the
/// knee, in the plane of the knee's bend; the leg's lengths kept. Returns how far the ankle stays short (inches; 0
/// within reach).
fn reach(world: &mut [Affine], leg: &Leg, target: V3) -> f32 {
    let hip = position(&world[leg.hip]);
    let knee = position(&world[leg.knee]);
    let ankle = position(&world[leg.ankle]);
    let thigh = length(sub(knee, hip));
    let shin = length(sub(ankle, knee));
    let to = sub(target, hip);
    let wanted = length(to);
    let longest = (thigh + shin) * 0.9995;
    let shortest = (thigh - shin).abs() + 0.01;
    let distance = wanted.clamp(shortest, longest);
    if wanted < 1e-4 || thigh < 1e-4 || shin < 1e-4 {
        return 0.0;
    }
    let along = scale(to, 1.0 / wanted);
    // Where the knee points: its offset from the line hip → ankle as animated (for a straight leg, where the foot
    // points), then square to the line hip → target.
    let now = unit(sub(ankle, hip));
    let mut pole = sub(sub(knee, hip), scale(now, dot(sub(knee, hip), now)));
    if length(pole) < 1e-3 {
        let forward = leg
            .ball
            .map_or([1.0, 0.0, 0.0], |b| sub(position(&world[b]), ankle));
        pole = sub(forward, scale(now, dot(forward, now)));
    }
    let bend = sub(pole, scale(along, dot(pole, along)));
    if length(bend) < 1e-6 {
        return 0.0;
    }
    let bend = scale(bend, 1.0 / length(bend));
    let cos = ((thigh * thigh + distance * distance - shin * shin) / (2.0 * thigh * distance))
        .clamp(-1.0, 1.0);
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let new_knee = add(hip, scale(add(scale(along, cos), scale(bend, sin)), thigh));
    let first = between(unit(sub(knee, hip)), unit(sub(new_knee, hip)));
    rotate(world, &leg.thigh, &first, hip);
    let ankle = position(&world[leg.ankle]);
    let reached = add(hip, scale(along, distance));
    let second = between(unit(sub(ankle, new_knee)), unit(sub(reached, new_knee)));
    rotate(world, &leg.shin, &second, new_knee);
    (wanted - distance).max(0.0)
}

/// The rotation that tilts a foot from level to the ground with normal `normal` (model space), by `amount` (0 – 1),
/// at most [`MAX_TILT_DEG`].
fn tilt_to(normal: V3, amount: f32) -> M3 {
    let up = [0.0, 0.0, 1.0];
    let n = unit(normal);
    let axis = cross(up, n);
    let s = length(axis);
    if s < 1e-6 || amount <= 0.0 {
        return IDENTITY;
    }
    let angle = s.atan2(dot(up, n)).min(MAX_TILT_DEG.to_radians()) * amount.min(1.0);
    axis_angle(scale(axis, 1.0 / s), angle)
}

const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Whether the ground drops by more than [`MAX_STEP`] in one place between the feet at `a` and `b` (world): a ray down
/// every [`LEDGE_SPACING`] inches from one to the other, from [`RAY_UP`] over the player's `origin` height, the ground
/// missing anywhere counting as a drop.
fn drop_between(a: V3, b: V3, origin: f32, ground: &dyn Fn(V3, f32) -> Option<(f32, V3)>) -> bool {
    let top = origin + RAY_UP;
    let span = (b[0] - a[0]).hypot(b[1] - a[1]);
    let steps = (span / LEDGE_SPACING).ceil().max(1.0) as usize;
    let mut last: Option<f32> = None;
    for k in 0..=steps {
        let t = k as f32 / steps as f32;
        let at = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, top];
        let Some((d, _)) = ground(at, RAY_UP + MAX_BELOW) else {
            return true;
        };
        let z = top - d;
        if last.is_some_and(|l| (z - l).abs() > MAX_STEP) {
            return true;
        }
        last = Some(z);
    }
    false
}

/// How far apart the rays [`drop_between`] casts between the feet are (inches): less than a stair's tread.
const LEDGE_SPACING: f32 = 6.0;

/// The stretches of frames a player stands (start, end exclusive).
fn standing_runs<T>(frames: &[Option<T>]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut f = 0;
    while f < frames.len() {
        if frames[f].is_none() {
            f += 1;
            continue;
        }
        let start = f;
        while f < frames.len() && frames[f].is_some() {
            f += 1;
        }
        runs.push((start, f));
    }
    runs
}

/// `values` smoothed by a Gaussian of standard deviation `sigma` (in values), its weights cut at the ends.
fn gaussian(values: &[f32], sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(0.5);
    let radius = (3.0 * sigma).ceil() as usize;
    (0..values.len())
        .map(|g| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for (h, v) in values
                .iter()
                .enumerate()
                .take((g + radius + 1).min(values.len()))
                .skip(g.saturating_sub(radius))
            {
                let x = (h as f32 - g as f32) / sigma;
                let w = (-0.5 * x * x).exp();
                sum += w * v;
                weight += w;
            }
            sum / weight
        })
        .collect()
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn position(m: &Affine) -> V3 {
    [m[0][3], m[1][3], m[2][3]]
}

fn rotation(m: &Affine) -> M3 {
    std::array::from_fn(|i| [m[i][0], m[i][1], m[i][2]])
}

/// `m` applied to the point `p`.
fn apply(m: &Affine, p: V3) -> V3 {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * p[k]).sum::<f32>() + m[i][3])
}

/// A world direction in the actor's frame (its rotation undone).
fn unrotate(actor: &Affine, v: V3) -> V3 {
    std::array::from_fn(|i| (0..3).map(|k| actor[k][i] * v[k]).sum())
}

/// Turns `bones` by `r` about `pivot` (model space).
fn rotate(world: &mut [Affine], bones: &[usize], r: &M3, pivot: V3) {
    for &bone in bones {
        let m = world[bone];
        let p = sub(position(&m), pivot);
        let moved = add(pivot, mul_v(r, p));
        let turned = mul(r, &rotation(&m));
        world[bone] = std::array::from_fn(|i| [turned[i][0], turned[i][1], turned[i][2], moved[i]]);
    }
}

/// The smallest rotation taking the unit vector `a` onto the unit vector `b`.
fn between(a: V3, b: V3) -> M3 {
    let axis = cross(a, b);
    let s = length(axis);
    let c = dot(a, b);
    if s < 1e-7 {
        // Parallel (no turn) or opposite (never for a knee within its reach).
        return IDENTITY;
    }
    axis_angle(scale(axis, 1.0 / s), s.atan2(c))
}

/// The rotation by `angle` (radians) about the unit `axis` (Rodrigues).
fn axis_angle(axis: V3, angle: f32) -> M3 {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn mul_v(a: &M3, v: V3) -> V3 {
    std::array::from_fn(|i| (0..3).map(|k| a[i][k] * v[k]).sum())
}

fn transpose(a: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| a[j][i]))
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

fn unit(a: V3) -> V3 {
    let l = length(a);
    if l < 1e-9 { a } else { scale(a, 1.0 / l) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bone, affine};

    /// A pelvis 40 inches up, a leg on each side: hip, knee a little forward (bent), ankle at the bind height 4.3,
    /// the ball of the foot ahead of it.
    fn skeleton() -> Skeleton {
        let bone = |name: &str, parent: Option<usize>, position: V3| Bone {
            name: name.into(),
            parent,
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            flags: 0,
        };
        let mut bones = vec![
            bone("root_motion", None, [0.0; 3]),
            bone("pelvis", Some(0), [0.0, 0.0, 37.0]),
        ];
        for (side, y) in [("L", 4.0), ("R", -4.0)] {
            let hip = bones.len();
            bones.push(bone(&format!("leg_upper_{side}"), Some(1), [0.0, y, -2.0]));
            bones.push(bone(
                &format!("leg_lower_{side}"),
                Some(hip),
                [3.0, 0.0, -15.5],
            ));
            bones.push(bone(
                &format!("ankle_{side}"),
                Some(hip + 1),
                [-3.0, 0.0, -15.2],
            ));
            bones.push(bone(
                &format!("ball_{side}"),
                Some(hip + 2),
                [5.0, 0.0, -3.0],
            ));
        }
        Skeleton { bones }
    }

    /// A floor at height 0 where y < 0 and at `left` where y ≥ 0 (a step between the feet).
    fn floor(left: f32) -> impl Fn(V3, f32) -> Option<(f32, V3)> {
        move |from: V3, reach: f32| {
            let z = if from[1] >= 0.0 { left } else { 0.0 };
            let d = from[2] - z;
            (0.0..=reach).contains(&d).then_some((d, [0.0, 0.0, 1.0]))
        }
    }

    fn standing(frames: usize, z: f32) -> Vec<Option<Stance>> {
        vec![
            Some(Stance {
                actor: affine([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, z]),
                standing: true,
                hit: false,
            });
            frames
        ]
    }

    fn bone_z(world: &[Affine], skeleton: &Skeleton, name: &str) -> f32 {
        world[skeleton.find(name).unwrap()][2][3]
    }

    fn distance(world: &[Affine], skeleton: &Skeleton, a: &str, b: &str) -> f32 {
        let pa = position(&world[skeleton.find(a).unwrap()]);
        let pb = position(&world[skeleton.find(b).unwrap()]);
        length(sub(pa, pb))
    }

    #[test]
    fn feet_on_a_flat_floor_stay_as_animated() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let bind = skeleton.world_transforms();
        let mut worlds = vec![bind.clone(); 40];
        let report = plant_feet(&mut worlds, &standing(40, 0.0), &legs, &floor(0.0), 64.0);
        assert_eq!(report.standing, 40);
        assert_eq!(report.moved, 0);
        for world in &worlds {
            for (a, b) in world.iter().zip(&bind) {
                for i in 0..3 {
                    for j in 0..4 {
                        assert!((a[i][j] - b[i][j]).abs() < 1e-3, "{a:?} vs {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_foot_over_a_lower_step_goes_down_with_the_pelvis() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let bind = skeleton.world_transforms();
        let mut worlds = vec![bind.clone(); 64];
        // The left foot over a step 8 inches below the origin's.
        let report = plant_feet(&mut worlds, &standing(64, 0.0), &legs, &floor(-8.0), 64.0);
        assert_eq!(report.short, 0);
        let mid = &worlds[32];
        let ankle = bind[skeleton.find("ankle_L").unwrap()][2][3];
        assert!((bone_z(mid, &skeleton, "ankle_L") - (ankle - 8.0)).abs() < 0.05);
        assert!((bone_z(mid, &skeleton, "ankle_R") - ankle).abs() < 0.05);
        assert!((bone_z(mid, &skeleton, "pelvis") - 29.0).abs() < 0.05);
        assert!((report.pelvis_drop - 8.0).abs() < 0.05);
        // The legs keep their lengths; the right knee bends to keep its foot up.
        for side in ["L", "R"] {
            for (a, b) in [("leg_upper", "leg_lower"), ("leg_lower", "ankle")] {
                let (a, b) = (format!("{a}_{side}"), format!("{b}_{side}"));
                let before = distance(&bind, &skeleton, &a, &b);
                assert!((distance(mid, &skeleton, &a, &b) - before).abs() < 1e-3);
            }
        }
        // The foot keeps its orientation (level ground): the ball stays ahead of the ankle as animated.
        let ball = position(&mid[skeleton.find("ball_R").unwrap()]);
        let heel = position(&mid[skeleton.find("ankle_R").unwrap()]);
        assert!((ball[0] - heel[0] - 5.0).abs() < 1e-3 && (ball[2] - heel[2] + 3.0).abs() < 1e-3);
        // Easing in and out at the ends of the stretch on the ground.
        assert!(bone_z(&worlds[0], &skeleton, "pelvis") > 35.0);
        assert!(bone_z(&worlds[63], &skeleton, "pelvis") > 35.0);
    }

    #[test]
    fn the_body_stays_up_where_the_player_is_hit() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let bind = skeleton.world_transforms();
        let mut worlds = vec![bind.clone(); 128];
        let mut stances = standing(128, 0.0);
        if let Some(s) = stances[64].as_mut() {
            s.hit = true;
        }
        plant_feet(&mut worlds, &stances, &legs, &floor(-8.0), 64.0);
        let pelvis = |f: usize| bone_z(&worlds[f], &skeleton, "pelvis");
        // As the server posed it on the hit, down 8 inches (the left foot's step) half a second away.
        assert!((pelvis(64) - bone_z(&bind, &skeleton, "pelvis")).abs() < 0.01);
        assert!((pelvis(32) - (bone_z(&bind, &skeleton, "pelvis") - 8.0)).abs() < 0.05);
        // Eased: no jump from frame to frame.
        for f in 33..96 {
            assert!((pelvis(f) - pelvis(f - 1)).abs() < 1.0);
        }
    }

    #[test]
    fn a_foot_over_a_ledge_hangs_and_the_body_stays_up() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let bind = skeleton.world_transforms();
        let mut worlds = vec![bind.clone(); 64];
        // The floor under the left foot 20 inches down: more than a step below the right foot's.
        let report = plant_feet(&mut worlds, &standing(64, 0.0), &legs, &floor(-20.0), 64.0);
        assert!(report.hanging > 0);
        for name in ["pelvis", "ankle_L", "ankle_R"] {
            assert!(
                (bone_z(&worlds[32], &skeleton, name) - bone_z(&bind, &skeleton, name)).abs()
                    < 0.01
            );
        }
    }

    #[test]
    fn feet_two_steps_apart_on_a_stair_both_stand() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let mut worlds = vec![skeleton.world_transforms(); 64];
        // A stair down along -y, 8-inch steps 4 inches deep: the left foot (y = 4) two steps below the right foot
        // (y = -4), 16 inches, more than a ledge's 14, but step by step.
        let stair = |from: V3, reach: f32| {
            let z = -8.0 * (from[1] / 4.0).floor();
            let d = from[2] - z;
            (0.0..=reach).contains(&d).then_some((d, [0.0, 0.0, 1.0]))
        };
        let report = plant_feet(&mut worlds, &standing(64, 0.0), &legs, &stair, 64.0);
        assert_eq!(report.hanging, 0);
    }

    #[test]
    fn a_ground_flickering_for_a_frame_does_not_move_a_foot() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let bind = skeleton.world_transforms();
        let mut worlds = vec![bind.clone(); 64];
        // Under the left foot, the floor; on three lone frames a step 8 inches up (a foot over its edge, swaying).
        // The player is 100 inches further along x each frame, so a ray tells its frame by where it is cast.
        let mut stances = standing(64, 0.0);
        for (f, stance) in stances.iter_mut().enumerate() {
            if let Some(s) = stance {
                s.actor[0][3] = f as f32 * 100.0;
            }
        }
        let flicker = |from: V3, reach: f32| {
            let f = (from[0] / 100.0).round() as usize;
            let z = if from[1] >= 0.0 && [10, 30, 31].contains(&f) {
                8.0
            } else {
                0.0
            };
            let d = from[2] - z;
            (0.0..=reach).contains(&d).then_some((d, [0.0, 0.0, 1.0]))
        };
        plant_feet(&mut worlds, &stances, &legs, &flicker, 64.0);
        let ankle = bone_z(&bind, &skeleton, "ankle_L");
        for world in &worlds {
            assert!((bone_z(world, &skeleton, "ankle_L") - ankle).abs() < 0.01);
        }
    }

    #[test]
    fn a_bent_leg_reaches_down_before_the_body_comes_down() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        // The body 3 inches lower, the feet where they were: the knees bent more (a crouch-walk's pose).
        let mut bent = skeleton.world_transforms();
        let feet: Vec<V3> = legs.legs.iter().map(|l| position(&bent[l.ankle])).collect();
        for &bone in &legs.body {
            bent[bone][2][3] -= 3.0;
        }
        for (leg, ankle) in legs.legs.iter().zip(feet) {
            reach(&mut bent, leg, ankle);
        }
        let mut worlds = vec![bent.clone(); 64];
        plant_feet(&mut worlds, &standing(64, 0.0), &legs, &floor(-8.0), 64.0);
        let mid = &worlds[32];
        // The left leg straightens most of the 8 inches down: the body comes down by what is left.
        let drop = bone_z(&bent, &skeleton, "pelvis") - bone_z(mid, &skeleton, "pelvis");
        assert!(drop > 0.5 && drop < 6.0, "{drop}");
        let ankle = bone_z(&bent, &skeleton, "ankle_L");
        assert!((bone_z(mid, &skeleton, "ankle_L") - (ankle - 8.0)).abs() < 0.05);
    }

    #[test]
    fn a_swinging_foot_goes_smoothly_from_one_step_to_the_next() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let bind = skeleton.world_transforms();
        let ankle_l = skeleton.find("ankle_L").unwrap();
        // The left foot swings 10 inches forward (from 5 behind its hip to 5 ahead) between frames 20 and 40,
        // lifted up to 6 inches, over the edge of a step 8 inches high at x = 2.5 (its toes 5 ahead of its ankle).
        let frames = 64;
        let mut worlds: Vec<Vec<Affine>> = (0..frames)
            .map(|f| {
                let mut world = bind.clone();
                let t = ((f as f32 - 20.0) / 20.0).clamp(0.0, 1.0);
                let lift = 6.0 * (t * std::f32::consts::PI).sin();
                let target = add(position(&world[ankle_l]), [10.0 * t - 5.0, 0.0, lift]);
                reach(&mut world, &legs.legs[0], target);
                world
            })
            .collect();
        let step = |from: V3, reach: f32| {
            let z = if from[1] >= 0.0 && from[0] >= 2.5 {
                8.0
            } else {
                0.0
            };
            let d = from[2] - z;
            (0.0..=reach).contains(&d).then_some((d, [0.0, 0.0, 1.0]))
        };
        plant_feet(&mut worlds, &standing(frames, 0.0), &legs, &step, 64.0);
        let heights: Vec<f32> = worlds.iter().map(|w| w[ankle_l][2][3]).collect();
        // No frame-to-frame jump bigger than the swing's own speed.
        for w in heights.windows(2) {
            assert!((w[1] - w[0]).abs() < 1.5, "{heights:?}");
        }
        // On the step, as high as the step, once down.
        assert!((heights[50] - heights[10] - 8.0).abs() < 0.1, "{heights:?}");
    }

    #[test]
    fn a_leg_reaches_its_target_with_its_lengths_kept() {
        let skeleton = skeleton();
        let legs = Legs::of(&skeleton).unwrap();
        let mut world = skeleton.world_transforms();
        let leg = &legs.legs[0];
        let target = [3.0, 4.0, 12.0];
        let thigh = length(sub(position(&world[leg.knee]), position(&world[leg.hip])));
        assert_eq!(reach(&mut world, leg, target), 0.0);
        assert!(length(sub(position(&world[leg.ankle]), target)) < 1e-3);
        assert!(
            (length(sub(position(&world[leg.knee]), position(&world[leg.hip]))) - thigh).abs()
                < 1e-3
        );
        // The knee still bends forward.
        assert!(position(&world[leg.knee])[0] > position(&world[leg.hip])[0]);
        // Out of reach: straight, short of it.
        assert!(reach(&mut world, leg, [0.0, 4.0, -40.0]) > 30.0);
    }

    #[test]
    fn a_foot_down_on_a_slope_tilts_with_it() {
        let normal = unit([-0.34, 0.0, 0.94]); // 20 degrees, rising toward +x
        let r = tilt_to(normal, 1.0);
        let forward = mul_v(&r, [1.0, 0.0, 0.0]);
        assert!(forward[2] > 0.3, "{forward:?}");
        // Steeper than MAX_TILT_DEG: held there.
        let steep = tilt_to(unit([-1.0, 0.0, 1.0]), 1.0);
        let up = mul_v(&steep, [0.0, 0.0, 1.0]);
        assert!((up[2] - MAX_TILT_DEG.to_radians().cos()).abs() < 1e-4);
    }
}
