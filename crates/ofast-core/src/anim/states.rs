//! What a player is doing, in the terms of CS2's graphs, decided from demo values (M4.4c).
//!
//! A demo has no graph state (decision 013): no `ground_action`, no `air_action`, no `action` ID. CS2's game code
//! computes them every frame from the pawn and its weapon; OFAST rebuilds them from what the demo does carry:
//!
//! | Graph parameter | From the demo |
//! |---|---|
//! | `action`, `attack_type` | the weapon's gameplay state (`m_iWeaponGameplayAnimState`, [`weapon_action`], [`passive_action`]) and its category |
//! | state restarts | `m_flWeaponGameplayAnimStateTimestamp` (a second swing restarts state 503 without changing it) |
//! | `attack_throw_strength` | the grenade's `m_flThrowStrength` |
//! | `move_type` | ladder (`m_MoveType` 9), air (off the ground for 3 ticks or more, or after a jump), ground |
//! | `air_action` | `air_action_jump` when the air time began with a jump (`m_nLastJumpTick`), else `air_action_start_fall` |
//! | `air_height_above_ground` | the height over the take-off point while rising, over the landing point while falling ([`AirRun`]) |
//! | `ground_action` | [`GroundAction`]: starts from standing, plants and turns on counter-strafes (the velocity's change beyond friction against the motion, [`plant_triggers`]), turning on the spot, moving; held as long as CS2 holds them ([`START_HOLD`], [`PLANT_HOLD`]) |
//! | `ground_action_direction_id` | the body-relative direction: of the velocity for a start ([`direction8`]), of the counter-strafe's acceleration for a plant ([`direction4`]) |
//! | `move_speed_horizontal_previous` | a plant's speed before the counter-strafe |
//! | `ground_turn_angle_or_velocity` | the body's turn rate while standing (degrees/s, positive to the left), or a small turn's whole angle ([`turn_extent`]) |
//! | `ladder_cycle` | the height climbed, one cycle every [`LADDER_CYCLE_HEIGHT`] units |
//! | `ladder_angle_yaw`, `_backwards` | the ladder's direction from the body ([`ladder_bearings`], [`ladder_angles`]) |
//! | `flinch_body_type`, `flinch_head_type` | `player_hurt`'s hit group and the side the bullet came from (`bullet_damage`) ([`flinch_types`]) |
//! | `flinch_is_on_fire` | fire damage ([`is_fire`]) within [`BURN_GAP`] |
//! | `flashed_amount` | the flash's duration, the pawn's `m_flFlashDuration` ([`flashed_amount`]) |
//!
//! The thresholds below are OFAST's choices (CS2's game code is not public); each is named and documented.

/// Below this horizontal speed (units/s) a player stands: starts begin from it, turns on the spot happen under it.
pub const STANDING_SPEED: f32 = 10.0;
/// A body turning faster than this (degrees/s) while standing turns on the spot.
pub const TURN_RATE: f32 = 15.0;
/// Off the ground for fewer ticks than this, without a jump, is a step or a slope, not air time.
pub const MIN_AIR_TICKS: usize = 3;
/// CS2's ground friction (`sv_friction`): each tick takes `max(speed, FRICTION_STOP_SPEED) × FRICTION × interval`
/// off the horizontal speed. With it, a demo's velocities tell what the movement keys did ([`counter_strafe`]).
pub const FRICTION: f32 = 5.2;
/// CS2's `sv_stopspeed` (units/s): below it friction takes off as much as at this speed.
pub const FRICTION_STOP_SPEED: f32 = 80.0;
/// A counter-strafe plants only when the player moved at least this fast before it (units/s).
pub const PLANT_SPEED: f32 = 20.0;
/// The acceleration beyond friction of a counter-strafe is at least this (units/s², 1 unit a tick at 64 ticks/s:
/// what is left of a key let go is friction alone).
pub const PLANT_ACCELERATION: f32 = 64.0;
/// And at most this (units/s², 26 units a tick): the movement keys add at most `sv_accelerate` (5.5) × the
/// weapon's speed a second (21.5 units a tick with a knife); faster changes are hits, landings and walls.
pub const PLANT_ACCELERATION_MAX: f32 = 1664.0;
/// A counter-strafe's acceleration points at least this far from the motion (degrees).
pub const PLANT_ANGLE: f32 = 135.0;
/// CS2 plays a plant for this long after it is set off, or set off again in the same direction (seconds): its
/// `ground_action` goes back to `ground_action_move` 13 ticks on (74 % of 1,500 plants of four rounds), and the
/// plant state lets go once 0.1 s have passed in it (`PLANT_AND_TURN_COMPLETE`).
pub const PLANT_HOLD: f32 = 0.2;
/// CS2 plays a start for this long (seconds): `Move` comes back 7 ticks on (6 when walking).
pub const START_HOLD: f32 = 0.1;
/// Players crouched this far never plant (CS2's crouched counter-strafes keep the crouch walk).
pub const PLANT_CROUCH: f32 = 0.5;
/// Height climbed per ladder clip cycle, units: the clips repeat every 0.8 s with a foot travelling about 20 units
/// per step, two steps per clip (an estimate from `ladder_facing_fast_*`, not checked against CS2 yet).
pub const LADDER_CYCLE_HEIGHT: f32 = 40.0;

/// The weapons-layer action a weapon's gameplay state starts: `(action, attack_type)` IDs, or `None` for states that
/// start none (idle, shooting a gun, inspecting: [`passive_action`] gives their IDs).
///
/// | State | Weapons | IDs |
/// |---|---|---|
/// | 11 | all | `action_deploy` |
/// | 800, 801 | guns | `action_reload` |
/// | 802, 803 | guns | `action_reload_silent` (demos of CS2 build 10955 on: a reload while walking, its `reload_silent` clips) |
/// | 600, 601 | guns | `action_silencer_attach`, `_detach` |
/// | 200, 201, 202 | grenades | `action_attack` + `attack_grenade_charge`, `_ready`, `_throw` |
/// | 500 – 505 | knives | `action_attack` + `attack_knife_lighthit`, `_lightmiss`, `_heavyhit`, `_heavymiss`, `_lightbackstab`, `_heavybackstab` |
/// | 400 | Healthshot | `action_healthshot_inject` |
///
/// The C4 plant (300) is not a weapons-layer action: it replaces the whole body ([`PLANT_STATE`]).
pub fn weapon_action(state: i64, category: &str) -> Option<(&'static str, &'static str)> {
    let knife = category == "weapon_category_knife";
    let grenade = category == "weapon_category_grenade";
    Some(match state {
        11 => ("action_deploy", ""),
        800 | 801 if !knife && !grenade => ("action_reload", ""),
        802 | 803 if !knife && !grenade => ("action_reload_silent", ""),
        600 => ("action_silencer_attach", ""),
        601 => ("action_silencer_detach", ""),
        200 if grenade => ("action_attack", "attack_grenade_charge"),
        201 if grenade => ("action_attack", "attack_grenade_ready"),
        202 if grenade => ("action_attack", "attack_grenade_throw"),
        500 if knife => ("action_attack", "attack_knife_lighthit"),
        501 if knife => ("action_attack", "attack_knife_lightmiss"),
        502 if knife => ("action_attack", "attack_knife_heavyhit"),
        503 if knife => ("action_attack", "attack_knife_heavymiss"),
        504 if knife => ("action_attack", "attack_knife_lightbackstab"),
        505 if knife => ("action_attack", "attack_knife_heavybackstab"),
        400 => ("action_healthshot_inject", ""),
        _ => return None,
    })
}

/// Whether gameplay state `now` is the outro of the reload `before` (801 after 800, 803 after 802), which goes on
/// with it rather than starting anew.
pub fn reload_outro(before: i64, now: i64) -> bool {
    matches!((before, now), (800, 801) | (802, 803))
}

/// The `(action, attack_type)` IDs of a gameplay state that starts no weapons-layer action ([`weapon_action`] gives
/// `None`): `action_inspect` (1000, 1001: the inspect and its last part — the inspect key is often pressed during a
/// draw, so 1000 comes one to 28 ticks into many draws; CS2's first-person view shows the inspect then),
/// `action_attack` + `attack_gun_primaryfire` / `_secondaryfire` / `_charge` (guns firing: 100 – 103),
/// `action_c4_plant` (300), else `action_idle`. An action in progress ends only when its graph would leave it for
/// these: a knife's `Deploy` leaves only for `action_idle` (or another action), so a draw followed by state 1000
/// plays to its end — CS2 shows the Huntsman raised by the face 23 ticks after its draw began, in state 1000.
pub fn passive_action(state: i64) -> (&'static str, &'static str) {
    match state {
        1000 | 1001 => ("action_inspect", ""),
        100 | 102 => ("action_attack", "attack_gun_primaryfire"),
        101 => ("action_attack", "attack_gun_secondaryfire"),
        103 => ("action_attack", "attack_gun_charge"),
        PLANT_STATE => ("action_c4_plant", ""),
        _ => ("action_idle", ""),
    }
}

/// The C4's gameplay state while it is being planted.
pub const PLANT_STATE: i64 = 300;

/// The flinch a hit plays: `(flinch_body_type, flinch_head_type)` IDs from `player_hurt`'s hit group and the side
/// the hit came from (`side`: [`direction4`] in the victim's body frame). The graphs' `BodyFlinch` plays
/// `flinch_chest` for `flinch_body_chest_north` (hit from ahead), `flinch_chest_rear` for `_south`,
/// `flinch_chest_right` for `_east`, `flinch_chest_left` for `_west`; stomach hits only front or rear; arms and legs
/// by side; `HeadFlinch` the four `flinch_head_*`.
///
/// | Hit group | Flinch |
/// |---|---|
/// | 1 head | `flinch_head_<side>` |
/// | 2 chest, 0 generic (grenades), 8 neck, 10 gear | `flinch_body_chest_<side>` |
/// | 3 stomach | `flinch_body_stomach_south` from behind, else `_north` |
/// | 4, 5 arms | `flinch_body_arm_left`, `_right` |
/// | 6, 7 legs | `flinch_body_leg_left`, `_right` |
pub fn flinch_types(hitgroup: i64, side: &str) -> (Option<&'static str>, Option<&'static str>) {
    let pick = |n: &'static str, s: &'static str, e: &'static str, w: &'static str| match side {
        "S" => s,
        "E" => e,
        "W" => w,
        _ => n,
    };
    match hitgroup {
        1 => (
            None,
            Some(pick(
                "flinch_head_north",
                "flinch_head_south",
                "flinch_head_east",
                "flinch_head_west",
            )),
        ),
        3 => (
            Some(if side == "S" {
                "flinch_body_stomach_south"
            } else {
                "flinch_body_stomach_north"
            }),
            None,
        ),
        4 => (Some("flinch_body_arm_left"), None),
        5 => (Some("flinch_body_arm_right"), None),
        6 => (Some("flinch_body_leg_left"), None),
        7 => (Some("flinch_body_leg_right"), None),
        _ => (
            Some(pick(
                "flinch_body_chest_north",
                "flinch_body_chest_south",
                "flinch_body_chest_east",
                "flinch_body_chest_west",
            )),
            None,
        ),
    }
}

/// Whether a `player_hurt` weapon is fire (burning: `FireFlinch`).
pub fn is_fire(weapon: &str) -> bool {
    matches!(weapon, "inferno" | "molotov" | "incgrenade")
}

/// Fire damage less than this apart (seconds) is one burn; the fire flinch lasts this long after the last. CS2 hurts
/// a player standing in fire every 12 – 13 ticks, 0.2 s (Aurora vs FURIA, Nuke, rounds 2 – 3).
pub const BURN_GAP: f32 = 0.5;

/// How flashed a player is `elapsed` seconds into a blindness of `duration` seconds (`m_flFlashDuration`), 0 – 1: fully
/// until the last 3 seconds, then fading with the square of the time left — CS:GO's flash overlay, assumed for
/// CS2's `flashed_amount`, whose layer shows above 0.1 (a 0.76 s blindness never does).
pub fn flashed_amount(elapsed: f32, duration: f32) -> f32 {
    let left = duration - elapsed;
    if elapsed < 0.0 || left <= 0.0 {
        0.0
    } else if left > 3.0 {
        1.0
    } else {
        (left / 3.0).powi(2)
    }
}

/// The eight directions of `ground_action_direction_id` for a body-relative velocity `(forward, left)`: `N` ahead,
/// `W` to the left, `E` to the right (as the graphs' 2D blends place `jump_e` at `(0, −225)`).
pub fn direction8(body: [f32; 2]) -> &'static str {
    let angle = body[1].atan2(body[0]).to_degrees();
    let index = ((angle / 45.0).round() as i32).rem_euclid(8) as usize;
    ["N", "NW", "W", "SW", "S", "SE", "E", "NE"][index]
}

/// The four directions plants and turns use (`N`, `S`, `E`, `W`), by the larger component.
pub fn direction4(body: [f32; 2]) -> &'static str {
    if body[0].abs() >= body[1].abs() {
        if body[0] >= 0.0 { "N" } else { "S" }
    } else if body[1] >= 0.0 {
        "W"
    } else {
        "E"
    }
}

/// The ground states of the locomotion graph OFAST rebuilds (`SM/Ground/Standing/…`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GroundAction {
    /// `Move` (and `Idle` at no speed): the 2D blends of [`super::Locomotion`].
    Move,
    /// `Starts`: `planted_start_<direction>`, from standing into a run.
    Start(&'static str),
    /// `PlantAndTurn` / `PlantAndTurnWalks`: a counter-strafe turning to `direction` (`N`, `S`, `E`, `W`) after
    /// moving at `previous_speed` (the graph picks the start, slow or fast clip by it: under 80, to 170, over), and
    /// whether the walk it came from was in its cross-legged half (`USE_CROSSLEG`).
    PlantAndTurn(&'static str, f32, bool),
    /// `TurnOnSpot` → `Turn Loop`: standing and turning, `ground_turn_angle_or_velocity` the turn rate.
    Turn,
    /// `TurnOnSpot` → `Small Turn`: one stepping turn of this many degrees (positive left, up to
    /// [`SMALL_TURN_MAX`]), `turn_left_23/34/45` or `turn_right_*`, played once (1.3 s).
    SmallTurn(f32),
}

/// The largest turn on the spot played as one `Small Turn` (the graph's blend reaches ±45°); larger ones loop.
pub const SMALL_TURN_MAX: f32 = 45.0;
/// A turn smaller than this (degrees) is no turn: the body just stands (the smallest clip turns 23°; a brief spike
/// of the turn rate otherwise started 0° turns that held the player in place for 1.3 s).
pub const SMALL_TURN_MIN: f32 = 10.0;

impl GroundAction {
    /// The `ground_action` ID.
    pub fn id(self) -> &'static str {
        match self {
            Self::Move => "ground_action_move",
            Self::Start(_) => "ground_action_start",
            Self::PlantAndTurn(..) => "ground_action_plant_and_turn",
            Self::Turn => "ground_action_turn_on_spot_loop",
            Self::SmallTurn(_) => "ground_action_turn_on_spot",
        }
    }
}

/// A turn seen at sample `from`: how far the body turns in all, in degrees (positive left), and whether it settles.
/// The yaw steps of `yaws` (one per tick) are summed from where the turn began — up to a quarter of a second
/// before `from`, where the smoothed rate that detected it was still catching up — until the turn rate stays below
/// half [`TURN_RATE`] for 3 ticks (settled) or the player is gone (settled), or for at most a second (not settled:
/// a slow pan, which loops). `interval`: seconds per tick.
pub fn turn_extent(yaws: &[Option<f32>], from: usize, interval: f32) -> (f32, bool) {
    let wrap = |a: f32| (a + 180.0).rem_euclid(360.0) - 180.0;
    let turning = |k: usize| match (
        yaws.get(k.wrapping_sub(1)).copied().flatten(),
        yaws.get(k).copied().flatten(),
    ) {
        (Some(a), Some(b)) => wrap(b - a).abs() / interval >= TURN_RATE * 0.5,
        _ => false,
    };
    let quarter = (0.25 / interval).round() as usize;
    let mut begin = from;
    while begin > 0 && from - begin < quarter && turning(begin) {
        begin -= 1;
    }
    let limit = (1.0 / interval).round() as usize;
    let (mut total, mut quiet) = (0.0f32, 0usize);
    for k in begin + 1..yaws.len().min(begin + 1 + limit) {
        let (Some(a), Some(b)) = (yaws[k - 1], yaws[k]) else {
            return (total, true);
        };
        let step = wrap(b - a);
        if step.abs() / interval < TURN_RATE * 0.5 {
            quiet += 1;
            if quiet >= 3 {
                return (total, true);
            }
        } else {
            quiet = 0;
        }
        total += step;
    }
    (total, begin + 1 + limit > yaws.len())
}

/// What the whole body's locomotion does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Movement {
    /// On the ground.
    Ground(GroundAction),
    /// In the air; `jumped` when it began with a jump.
    Air {
        /// The air time began with a jump.
        jumped: bool,
    },
    /// On a ladder.
    Ladder,
}

impl Movement {
    /// The `move_type` ID.
    pub fn move_type(self) -> &'static str {
        match self {
            Self::Ground(_) => "move_type_ground",
            Self::Air { .. } => "move_type_air",
            Self::Ladder => "move_type_ladder",
        }
    }

    /// Whether this is the move blend that [`super::Locomotion`] samples with its own continuous phase.
    pub fn is_move(self) -> bool {
        self == Self::Ground(GroundAction::Move)
    }
}

/// What [`ground_action`] decides from, at one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundInputs {
    /// Horizontal speed, units/s.
    pub speed: f32,
    /// Velocity in the body's frame, `(forward, left)`.
    pub body: [f32; 2],
    /// Walking (`m_bIsWalking`).
    pub walking: bool,
    /// The body's turn rate, degrees/s, positive to the left.
    pub turn_rate: f32,
    /// The player stood still at the previous tick.
    pub was_standing: bool,
    /// A plant set off at this tick ([`plant_triggers`]): the direction it turns to ([`direction4`] of the
    /// body-relative acceleration) and the speed before.
    pub plant: Option<(&'static str, f32)>,
    /// The walk or run the player is in has its cross-legged half on (`USE_CROSSLEG`, for a plant's clip).
    pub crossleg: bool,
}

/// The ground action at a tick, given the current one, how long it has played (`elapsed`; a plant's since it was
/// last set off) of its `length` (seconds: [`START_HOLD`], [`PLANT_HOLD`], a small turn's clip), and whether the
/// player was already on the ground (landing or stepping off a ladder never starts a run). As CS2 decides them
/// (its pose recipes show when it plants and starts, demo.md's "Pose recipe"):
///
/// - a start lasts [`START_HOLD`] (less if the player stops or plants); a plant [`PLANT_HOLD`] from the last time
///   it was set off in its direction, and one set off in another direction takes over (CS2's `E2W` ↔ `W2E`); a
///   turn lasts while the player stays under twice [`STANDING_SPEED`] and turns at more than half [`TURN_RATE`]
///   (both thresholds with a margin, so a body hovering around them does not flicker between turning and
///   standing);
/// - a counter-strafe plants and turns ([`plant_triggers`]; cross-legged when it comes from the cross-legged half
///   of a walk); leaving a standstill starts ([`direction8`] of the velocity), walking too; standing and turning
///   faster than [`TURN_RATE`] turns on the spot; anything else moves.
pub fn ground_action(
    current: GroundAction,
    elapsed: f32,
    length: f32,
    on_ground_before: bool,
    inputs: &GroundInputs,
) -> GroundAction {
    let standing = inputs.speed < STANDING_SPEED;
    let keep = match current {
        GroundAction::Start(_) => elapsed < length && !standing && inputs.plant.is_none(),
        GroundAction::PlantAndTurn(direction, ..) => {
            elapsed < length && inputs.plant.is_none_or(|(d, _)| d == direction)
        }
        GroundAction::Turn => {
            inputs.speed < 2.0 * STANDING_SPEED && inputs.turn_rate.abs() >= TURN_RATE * 0.5
        }
        GroundAction::SmallTurn(_) => {
            elapsed < length && inputs.speed < 2.0 * STANDING_SPEED && inputs.plant.is_none()
        }
        GroundAction::Move => false,
    };
    if keep {
        return current;
    }
    if let Some((direction, before)) = inputs.plant {
        let crossleg = inputs.crossleg && current == GroundAction::Move;
        return GroundAction::PlantAndTurn(direction, before, crossleg);
    }
    if !standing && inputs.was_standing && on_ground_before {
        return GroundAction::Start(direction8(inputs.body));
    }
    if standing && inputs.turn_rate.abs() >= TURN_RATE {
        return GroundAction::Turn;
    }
    GroundAction::Move
}

/// What a tick's friction leaves of a horizontal velocity (units/s; [`FRICTION`], [`FRICTION_STOP_SPEED`]).
pub fn after_friction(velocity: [f32; 2], interval: f32) -> [f32; 2] {
    let speed = velocity[0].hypot(velocity[1]);
    if speed < 0.1 {
        return [0.0, 0.0];
    }
    let drop = speed.max(FRICTION_STOP_SPEED) * FRICTION * interval;
    let k = (speed - drop).max(0.0) / speed;
    [velocity[0] * k, velocity[1] * k]
}

/// The movement keys' acceleration from one tick's velocity to the next (world, units/s → units/s²): the change
/// beyond friction, when it is a counter-strafe — between [`PLANT_ACCELERATION`] and [`PLANT_ACCELERATION_MAX`],
/// at least [`PLANT_ANGLE`] from the motion, the player moving at [`PLANT_SPEED`] or more before.
pub fn counter_strafe(before: [f32; 2], now: [f32; 2], interval: f32) -> Option<[f32; 2]> {
    let speed = before[0].hypot(before[1]);
    if speed < PLANT_SPEED || interval <= 0.0 {
        return None;
    }
    let left = after_friction(before, interval);
    let a = [(now[0] - left[0]) / interval, (now[1] - left[1]) / interval];
    let size = a[0].hypot(a[1]);
    if !(PLANT_ACCELERATION..=PLANT_ACCELERATION_MAX).contains(&size) {
        return None;
    }
    let cos = (a[0] * before[0] + a[1] * before[1]) / (size * speed);
    (cos <= PLANT_ANGLE.to_radians().cos()).then_some(a)
}

/// One tick of what [`plant_triggers`] reads: the demo tick, the horizontal velocity (world, units/s), and whether
/// the player could plant (on the ground, off ladders, crouched less than [`PLANT_CROUCH`]).
pub type PlantSample = Option<(u32, [f32; 2], bool)>;

/// The ticks CS2 plants at, from one player's samples: where a counter-strafe ([`counter_strafe`]) begins (not at
/// the tick before; both ticks able to plant) and is confirmed by the next tick (a counter-strafe there too, or the
/// speed still falling: a single tick of it is the speed clamped to a walk), with its acceleration. Against the
/// plants in CS2's pose recipes (four rounds of build 10955, 3,416 plants of moving players): 73 % on the same
/// tick, 86 % within a tick (CS2 is a tick earlier when the key went down late in a tick), 489 set off where CS2
/// did not plant. A key let go is friction alone and plants nothing, as in CS2.
pub fn plant_triggers(samples: &[PlantSample], interval: f32) -> Vec<Option<[f32; 2]>> {
    let counter: Vec<Option<[f32; 2]>> = (0..samples.len())
        .map(|i| {
            let (Some((t0, v0, able0)), Some((t1, v1, able1))) =
                (i.checked_sub(1).and_then(|j| samples[j]), samples[i])
            else {
                return None;
            };
            (t1 == t0 + 1 && able0 && able1)
                .then(|| counter_strafe(v0, v1, interval))
                .flatten()
        })
        .collect();
    (0..samples.len())
        .map(|i| {
            let a = counter[i]?;
            if i > 0 && counter[i - 1].is_some() {
                return None;
            }
            let confirmed = match (samples[i], samples.get(i + 1).copied().flatten()) {
                (Some((t, v, _)), Some((t1, v1, _))) if t1 == t + 1 => {
                    counter[i + 1].is_some() || v1[0].hypot(v1[1]) < v[0].hypot(v[1]) - 1.0
                }
                _ => true,
            };
            confirmed.then_some(a)
        })
        .collect()
}

/// Ticks before mounting a ladder whose velocity gives the ladder's direction.
pub const LADDER_APPROACH_TICKS: usize = 16;
/// Below this mean approach speed (units/s) the ladder is taken to be where the body faced when mounting.
pub const LADDER_APPROACH_SPEED: f32 = 30.0;

/// Ticks on a ladder whose mean vertical velocity tells climbing up from climbing down.
pub const LADDER_CLIMB_TICKS: usize = 8;

/// The ladder's direction (world yaw, degrees: where the ladder is from the player) for every sample on a ladder,
/// from `(on a ladder, velocity, body yaw)` samples (`None` where the player is absent). Demos carry no ladder
/// normal. A player climbing up walks into the ladder, so the mean horizontal velocity over the
/// [`LADDER_APPROACH_TICKS`] before mounting and the first two on it points at it; one climbing down (the first
/// [`LADDER_CLIMB_TICKS`] on it descend) walked off the platform above, away from the wall, so it points the other
/// way. When that velocity is slower than [`LADDER_APPROACH_SPEED`], the body yaw when mounting. On ladders the
/// body yaw follows the view (round 5: a player looking 80° to the side while climbing, another climbing with his
/// back to the ladder), so CS2 turns the legs back to the ladder by `ladder_angle_yaw` ([`ladder_angles`]).
pub fn ladder_bearings(samples: &[Option<(bool, [f32; 3], f32)>]) -> Vec<Option<f32>> {
    let mut out = vec![None; samples.len()];
    let mut i = 0;
    while i < samples.len() {
        if !samples[i].is_some_and(|(ladder, ..)| ladder) {
            i += 1;
            continue;
        }
        let start = i;
        while i < samples.len() && samples[i].is_some_and(|(ladder, ..)| ladder) {
            i += 1;
        }
        let window = &samples[start.saturating_sub(LADDER_APPROACH_TICKS)..(start + 2).min(i)];
        let (mut vx, mut vy, mut n) = (0.0f32, 0.0f32, 0usize);
        for (_, v, _) in window.iter().flatten() {
            vx += v[0];
            vy += v[1];
            n += 1;
        }
        let mounting = samples[start].map_or(0.0, |(_, _, yaw)| yaw);
        let climb = &samples[start..(start + LADDER_CLIMB_TICKS).min(i)];
        let vz: f32 =
            climb.iter().flatten().map(|(_, v, _)| v[2]).sum::<f32>() / climb.len().max(1) as f32;
        let bearing = if n > 0 && vx.hypot(vy) / n as f32 >= LADDER_APPROACH_SPEED {
            let walked = vy.atan2(vx).to_degrees();
            if vz < 0.0 { walked + 180.0 } else { walked }
        } else {
            mounting
        };
        let bearing = (bearing + 180.0).rem_euclid(360.0) - 180.0;
        for slot in &mut out[start..i] {
            *slot = Some(bearing);
        }
    }
    out
}

/// `(ladder_angle_yaw, ladder_angle_yaw_backwards)` for a body facing `body_yaw` on a ladder at `bearing` (both
/// world yaws): the ladder's direction from the body, positive to the left (`ladder_facing_fast_45` puts the feet
/// to the front-left), and the same seen from behind (the `back` clips' blend, beyond ±125°: `ladder_back_fast_45`,
/// at −45 on it, puts the feet behind-left, at +135).
pub fn ladder_angles(bearing: f32, body_yaw: f32) -> (f32, f32) {
    let wrap = |a: f32| (a + 180.0).rem_euclid(360.0) - 180.0;
    let angle = wrap(bearing - body_yaw);
    (angle, wrap(angle - 180.0))
}

/// One stretch off the ground: from sample `start` to sample `end` (inclusive), its take-off and landing heights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirRun {
    /// First sample off the ground.
    pub start: usize,
    /// Last sample off the ground.
    pub end: usize,
    /// Feet height at take-off (the last sample on the ground before, else the first in the air).
    pub takeoff: f32,
    /// Feet height at landing (the first sample on the ground after, else the last in the air).
    pub landing: f32,
}

impl AirRun {
    /// Height above the ground at feet height `z`, rising (`vz > 0`, over the take-off point) or falling (over the
    /// landing point); never below 0.
    pub fn height(&self, z: f32, vz: f32) -> f32 {
        (z - if vz > 0.0 { self.takeoff } else { self.landing }).max(0.0)
    }
}

/// The air runs in a sequence of `(on the ground, feet height)` samples (`None` where the player is absent or on a
/// ladder, which ends a run).
pub fn air_runs(samples: &[Option<(bool, f32)>]) -> Vec<AirRun> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    let close = |start: usize, end: usize, runs: &mut Vec<AirRun>| {
        let z = |i: usize| samples.get(i).copied().flatten().map(|(_, z)| z);
        let takeoff = start.checked_sub(1).and_then(z).or(z(start)).unwrap_or(0.0);
        let landing = z(end + 1).or(z(end)).unwrap_or(0.0);
        runs.push(AirRun {
            start,
            end,
            takeoff,
            landing,
        });
    };
    for (i, sample) in samples.iter().enumerate() {
        match (sample, start) {
            (Some((false, _)), None) => start = Some(i),
            (Some((false, _)), Some(_)) => {}
            (_, Some(s)) => {
                close(s, i - 1, &mut runs);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        close(s, samples.len() - 1, &mut runs);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameplay_states_map_to_graph_actions() {
        assert_eq!(
            weapon_action(11, "weapon_category_rifle"),
            Some(("action_deploy", ""))
        );
        assert_eq!(
            weapon_action(800, "weapon_category_rifle"),
            Some(("action_reload", ""))
        );
        assert_eq!(
            weapon_action(803, "weapon_category_pistol"),
            Some(("action_reload_silent", ""))
        );
        // Gun shots and inspects leave the weapons layer off.
        assert_eq!(weapon_action(100, "weapon_category_rifle"), None);
        assert_eq!(weapon_action(1000, "weapon_category_knife"), None);
        assert_eq!(
            weapon_action(503, "weapon_category_knife"),
            Some(("action_attack", "attack_knife_heavymiss"))
        );
        assert_eq!(
            weapon_action(202, "weapon_category_grenade"),
            Some(("action_attack", "attack_grenade_throw"))
        );
        // A knife state on a rifle is not an attack.
        assert_eq!(weapon_action(503, "weapon_category_rifle"), None);
        assert_eq!(
            weapon_action(PLANT_STATE, "weapon_category_equipment"),
            None
        );
        // What those states tell the graphs instead.
        assert_eq!(passive_action(1000), ("action_inspect", ""));
        assert_eq!(
            passive_action(100),
            ("action_attack", "attack_gun_primaryfire")
        );
        assert_eq!(passive_action(50), ("action_idle", ""));
        assert_eq!(passive_action(PLANT_STATE), ("action_c4_plant", ""));
    }

    #[test]
    fn hits_pick_flinches_and_flashes_fade() {
        assert_eq!(flinch_types(1, "W"), (None, Some("flinch_head_west")));
        assert_eq!(
            flinch_types(2, "S"),
            (Some("flinch_body_chest_south"), None)
        );
        assert_eq!(
            flinch_types(0, "N"),
            (Some("flinch_body_chest_north"), None)
        );
        assert_eq!(
            flinch_types(3, "E"),
            (Some("flinch_body_stomach_north"), None)
        );
        assert_eq!(flinch_types(7, "N"), (Some("flinch_body_leg_right"), None));
        assert!(is_fire("inferno") && !is_fire("ak47"));
        assert_eq!(flashed_amount(1.0, 5.0), 1.0);
        assert!((flashed_amount(3.5, 5.0) - 0.25).abs() < 1e-6);
        assert_eq!(flashed_amount(6.0, 5.0), 0.0);
        assert!(flashed_amount(0.0, 0.76) < 0.1);
    }

    #[test]
    fn directions_follow_the_graph_axes() {
        assert_eq!(direction8([200.0, 0.0]), "N");
        assert_eq!(direction8([0.0, 200.0]), "W");
        assert_eq!(direction8([0.0, -200.0]), "E");
        assert_eq!(direction8([-150.0, -150.0]), "SE");
        assert_eq!(direction8([150.0, 140.0]), "NW");
        assert_eq!(direction4([20.0, -100.0]), "E");
        assert_eq!(direction4([-100.0, 20.0]), "S");
    }

    #[test]
    fn ground_actions_start_plant_and_turn() {
        let still = GroundInputs {
            speed: 0.0,
            body: [0.0, 0.0],
            walking: false,
            turn_rate: 0.0,
            was_standing: true,
            plant: None,
            crossleg: false,
        };
        let moving = GroundInputs {
            speed: 40.0,
            body: [0.0, 40.0],
            ..still
        };
        // Leaving a standstill starts to the left (W), walking too; landing does not.
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, true, &moving),
            GroundAction::Start("W")
        );
        let walking = GroundInputs {
            walking: true,
            ..moving
        };
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, true, &walking),
            GroundAction::Start("W")
        );
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, false, &moving),
            GroundAction::Move
        );
        // A start lasts START_HOLD, then moves.
        let running = GroundInputs {
            speed: 200.0,
            body: [200.0, 0.0],
            was_standing: false,
            ..still
        };
        let start = GroundAction::Start("W");
        assert_eq!(
            ground_action(start, 0.05, START_HOLD, true, &running),
            start
        );
        assert_eq!(
            ground_action(start, 0.11, START_HOLD, true, &running),
            GroundAction::Move
        );
        // Turning on the spot, with some hysteresis.
        let turning = GroundInputs {
            turn_rate: -40.0,
            ..still
        };
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, true, &turning),
            GroundAction::Turn
        );
        let slowing = GroundInputs {
            turn_rate: 10.0,
            ..still
        };
        assert_eq!(
            ground_action(GroundAction::Turn, 0.5, 0.0, true, &slowing),
            GroundAction::Turn
        );
        // Shuffling a little (15 units/s) does not end a turn; walking off (25) does.
        let shuffling = GroundInputs {
            speed: 15.0,
            turn_rate: -40.0,
            was_standing: false,
            ..still
        };
        assert_eq!(
            ground_action(GroundAction::Turn, 0.5, 0.0, true, &shuffling),
            GroundAction::Turn
        );
        let off = GroundInputs {
            speed: 25.0,
            ..shuffling
        };
        assert_eq!(
            ground_action(GroundAction::Turn, 0.5, 0.0, true, &off),
            GroundAction::Move
        );
        assert_eq!(
            ground_action(GroundAction::Move, 0.5, 0.0, true, &slowing),
            GroundAction::Move
        );
        // A counter-strafe from a walk in its cross-legged half plants cross-legged, and lasts PLANT_HOLD.
        let braking = GroundInputs {
            speed: 99.0,
            body: [0.0, -99.0],
            was_standing: false,
            plant: Some(("W", 107.0)),
            crossleg: true,
            ..still
        };
        let plant = GroundAction::PlantAndTurn("W", 107.0, true);
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, true, &braking),
            plant
        );
        let after = GroundInputs {
            plant: None,
            ..braking
        };
        assert_eq!(ground_action(plant, 0.15, PLANT_HOLD, true, &after), plant);
        assert_eq!(
            ground_action(plant, 0.21, PLANT_HOLD, true, &after),
            GroundAction::Move
        );
        // Set off again in its own direction it goes on (the bake restarts its hold); the other way it turns over,
        // and not cross-legged (that comes from a walk only).
        assert_eq!(
            ground_action(plant, 0.15, PLANT_HOLD, true, &braking),
            plant
        );
        let back = GroundInputs {
            plant: Some(("E", 107.0)),
            ..braking
        };
        assert_eq!(
            ground_action(plant, 0.15, PLANT_HOLD, true, &back),
            GroundAction::PlantAndTurn("E", 107.0, false)
        );
    }

    #[test]
    fn counter_strafes_plant() {
        let interval = 1.0 / 64.0;
        // NiKo on anubis round 14 (ticks 133798 – 133806, sideways units/s): holding the key, letting go (friction
        // alone: no plant), the opposite key (a plant, confirmed as the speed goes on falling), through zero.
        let speeds = [-89.0, -101.0, -107.0, -99.0, -74.0, -49.0, -24.0, 1.0, 13.0];
        let samples: Vec<PlantSample> = speeds
            .iter()
            .enumerate()
            .map(|(k, &v)| Some((133798 + k as u32, [0.0, v], true)))
            .collect();
        let fired: Vec<usize> = plant_triggers(&samples, interval)
            .iter()
            .enumerate()
            .filter_map(|(k, a)| a.map(|_| k))
            .collect();
        assert_eq!(fired, [4], "the counter-strafe's first tick");
        let a = plant_triggers(&samples, interval)[4].unwrap();
        assert!(a[1] > 0.0, "pushing to the left (+y): {a:?}");
        // Friction alone: 107 → 98.3 in a tick; a perpendicular key is no counter-strafe.
        assert!(counter_strafe([0.0, 107.0], [0.0, 98.3], interval).is_none());
        assert!(counter_strafe([0.0, 107.0], [15.0, 98.3], interval).is_none());
        // A clamp to the walking speed (one tick of it, the speed holding after) is no plant.
        let clamp: Vec<PlantSample> = [183.0, 168.0, 155.0, 142.0, 120.0, 120.0, 120.0]
            .iter()
            .enumerate()
            .map(|(k, &v)| Some((k as u32, [v, 0.0], true)))
            .collect();
        assert!(plant_triggers(&clamp, interval).iter().all(Option::is_none));
        // Crouched, nothing plants.
        let crouched: Vec<PlantSample> = samples
            .iter()
            .map(|s| s.map(|(t, v, _)| (t, v, false)))
            .collect();
        assert!(
            plant_triggers(&crouched, interval)
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn turns_are_measured_ahead() {
        let interval = 1.0 / 64.0;
        // 2° a tick (128°/s) for 15 ticks, then still: a 30° turn that settles. Seen 5 ticks into it (the smoothed
        // rate lags), it is still measured from its beginning.
        let mut yaws: Vec<Option<f32>> = vec![Some(10.0); 4];
        yaws.extend((1..16).map(|k| Some(10.0 + 2.0 * k as f32)));
        yaws.extend(vec![Some(40.0); 10]);
        let (total, settled) = turn_extent(&yaws, 8, interval);
        assert!((total - 30.0).abs() < 1e-3 && settled, "{total} {settled}");
        // Across ±180 and to the right, ending with the samples.
        let right: Vec<Option<f32>> = (0..40).map(|k| Some(-170.0 - 3.0 * k as f32)).collect();
        let (total, settled) = turn_extent(&right, 0, interval);
        assert!((total + 117.0).abs() < 1e-2 && settled, "{total}");
        // A slow pan (0.6° a tick, 38°/s) for 3 s does not settle within a second: a loop, not a small turn.
        let pan: Vec<Option<f32>> = (0..192).map(|k| Some(0.6 * k as f32)).collect();
        let (total, settled) = turn_extent(&pan, 0, interval);
        assert!(!settled && total < 45.0, "{total}");
        // A small turn plays to its end while the player stands.
        let still = GroundInputs {
            speed: 0.0,
            body: [0.0, 0.0],
            walking: false,
            turn_rate: 0.0,
            was_standing: true,
            plant: None,
            crossleg: false,
        };
        let small = GroundAction::SmallTurn(30.0);
        assert_eq!(ground_action(small, 0.5, 1.3, true, &still), small);
        assert_eq!(
            ground_action(small, 1.4, 1.3, true, &still),
            GroundAction::Move
        );
    }

    #[test]
    fn ladders_face_where_the_player_walked() {
        // Walking +x into a ladder, then climbing up while looking 80° to the right.
        let mut samples = vec![Some((false, [200.0, 0.0, 0.0], 0.0)); 20];
        samples.extend(vec![Some((true, [0.0, 0.0, 150.0], -80.0)); 10]);
        samples.push(Some((false, [0.0, 0.0, 0.0], -80.0)));
        let bearings = ladder_bearings(&samples);
        assert_eq!(bearings[19], None);
        assert!((bearings[25].unwrap()).abs() < 1e-3);
        assert_eq!(bearings[30], None);
        // The ladder is 80° to the left of the body: the feet turn left.
        let (angle, back) = ladder_angles(bearings[25].unwrap(), -80.0);
        assert!((angle - 80.0).abs() < 1e-3 && (back + 100.0).abs() < 1e-3);
        // Back to the ladder, a little to the left behind: the back blend's +135 side… seen as −45.
        let (angle, back) = ladder_angles(0.0, -135.0);
        assert!((angle - 135.0).abs() < 1e-3 && (back + 45.0).abs() < 1e-3);
        // Walking +x off a platform onto a ladder and climbing down: the ladder is behind, at 180°.
        let mut down = vec![Some((false, [150.0, 0.0, 0.0], 0.0)); 20];
        down.extend(vec![Some((true, [0.0, 0.0, -150.0], 0.0)); 10]);
        assert!((ladder_bearings(&down)[25].unwrap().abs() - 180.0).abs() < 1e-3);
        // Mounted standing still: the body's yaw then.
        let still = vec![Some((true, [0.0, 0.0, 0.0], 33.0)); 3];
        assert_eq!(ladder_bearings(&still)[1], Some(33.0));
    }

    #[test]
    fn air_runs_know_takeoff_and_landing() {
        let samples = [
            Some((true, 0.0)),
            Some((false, 10.0)),
            Some((false, 30.0)),
            Some((false, 45.0)),
            Some((true, 40.0)),
            None,
            Some((false, 5.0)),
        ];
        let runs = air_runs(&samples);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].start, runs[0].end), (1, 3));
        assert_eq!((runs[0].takeoff, runs[0].landing), (0.0, 40.0));
        // Rising: over the take-off point; falling: over the landing point (onto a box 40 units up).
        assert_eq!(runs[0].height(30.0, 100.0), 30.0);
        assert_eq!(runs[0].height(45.0, -50.0), 5.0);
        assert_eq!(runs[0].height(35.0, -50.0), 0.0);
        // A run cut by the end of the samples lands where it was last seen.
        assert_eq!((runs[1].start, runs[1].end, runs[1].landing), (6, 6, 5.0));
    }
}
