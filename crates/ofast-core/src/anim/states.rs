//! What a player is doing, in the terms of CS2's graphs, decided from demo values (M4.4c).
//!
//! A demo has no graph state (decision 013): no `ground_action`, no `air_action`, no `action` ID. CS2's game code
//! computes them every frame from the pawn and its weapon; OFAST rebuilds them from what the demo does carry:
//!
//! | Graph parameter | From the demo |
//! |---|---|
//! | `action`, `attack_type` | the weapon's gameplay state (`m_iWeaponGameplayAnimState`, [`weapon_action`]) and its category |
//! | state restarts | `m_flWeaponGameplayAnimStateTimestamp` (a second swing restarts state 503 without changing it) |
//! | `attack_throw_strength` | the grenade's `m_flThrowStrength` |
//! | `move_type` | ladder (`m_MoveType` 9), air (off the ground for 3 ticks or more, or after a jump), ground |
//! | `air_action` | `air_action_jump` when the air time began with a jump (`m_nLastJumpTick`), else `air_action_start_fall` |
//! | `air_height_above_ground` | the height over the take-off point while rising, over the landing point while falling ([`AirRun`]) |
//! | `ground_action` | [`GroundAction`]: starts from standing, plants and turns on direction reversals, turning on the spot, moving |
//! | `ground_action_direction_id` | the body-relative direction ([`direction8`], [`direction4`]) |
//! | `ground_turn_angle_or_velocity` | the body's turn rate while standing (degrees/s, positive to the left) |
//! | `ladder_cycle` | the height climbed, one cycle every [`LADDER_CYCLE_HEIGHT`] units |
//!
//! The thresholds below are OFAST's choices (CS2's game code is not public); each is named and documented.

/// Below this horizontal speed (units/s) a player stands: starts begin from it, turns on the spot happen under it.
pub const STANDING_SPEED: f32 = 10.0;
/// A body turning faster than this (degrees/s) while standing turns on the spot.
pub const TURN_RATE: f32 = 15.0;
/// Off the ground for fewer ticks than this, without a jump, is a step or a slope, not air time.
pub const MIN_AIR_TICKS: usize = 3;
/// A direction reversal plants and turns when the player moved at least this fast just before (units/s).
pub const REVERSAL_SPEED: f32 = 60.0;
/// How far back a reversal looks for the old direction, seconds.
pub const REVERSAL_WINDOW: f32 = 0.25;
/// Height climbed per ladder clip cycle, units: the clips repeat every 0.8 s with a foot travelling about 20 units
/// per step, two steps per clip (an estimate from `ladder_facing_fast_*`, not checked against CS2 yet).
pub const LADDER_CYCLE_HEIGHT: f32 = 40.0;

/// What the weapons layer plays for a weapon's gameplay state: `(action, attack_type)` IDs, or `None` when the
/// layer is off (idle, shooting a gun, inspecting: the third-person graphs play nothing for those).
///
/// | State | Weapons | IDs |
/// |---|---|---|
/// | 11 | all | `action_deploy` |
/// | 800, 801 | guns | `action_reload` |
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

/// The C4's gameplay state while it is being planted.
pub const PLANT_STATE: i64 = 300;

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
    /// `PlantAndTurn` / `PlantAndTurnWalks`: the old direction reversed into `direction` (`N`, `S`, `E`, `W`) after
    /// moving at `previous_speed`.
    PlantAndTurn(&'static str, f32),
    /// `TurnOnSpot` → `Turn Loop`: standing and turning.
    Turn,
}

impl GroundAction {
    /// The `ground_action` ID.
    pub fn id(self) -> &'static str {
        match self {
            Self::Move => "ground_action_move",
            Self::Start(_) => "ground_action_start",
            Self::PlantAndTurn(..) => "ground_action_plant_and_turn",
            Self::Turn => "ground_action_turn_on_spot_loop",
        }
    }
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
    /// The player stood still at the previous tick, and was not at [`REVERSAL_SPEED`] or more over the last
    /// [`REVERSAL_WINDOW`] (a counter-strafe passes through a standstill without stopping).
    pub was_standing: bool,
    /// A direction reversal at this tick ([`reversal`]): the new direction and the speed before.
    pub reversal: Option<(&'static str, f32)>,
}

/// The ground action at a tick, given the current one, how long it has played (`elapsed`) of its `length`
/// (seconds; starts and plants end with their clip), and whether the player was already on the ground (landing or
/// stepping off a ladder never starts a run):
///
/// - a start or a plant plays to its end (a start stops early if the player stops or reverses); a turn lasts while
///   the player stays under twice [`STANDING_SPEED`] and turns at more than half [`TURN_RATE`] (both thresholds
///   with a margin, so a body hovering around them does not flicker between turning and standing);
/// - a reversal plants and turns; leaving a standstill without walking starts ([`direction8`] of the velocity);
///   standing and turning faster than [`TURN_RATE`] turns on the spot; anything else moves.
pub fn ground_action(
    current: GroundAction,
    elapsed: f32,
    length: f32,
    on_ground_before: bool,
    inputs: &GroundInputs,
) -> GroundAction {
    let standing = inputs.speed < STANDING_SPEED;
    let keep = match current {
        GroundAction::Start(_) => elapsed < length && !standing && inputs.reversal.is_none(),
        GroundAction::PlantAndTurn(..) => elapsed < length,
        GroundAction::Turn => {
            inputs.speed < 2.0 * STANDING_SPEED && inputs.turn_rate.abs() >= TURN_RATE * 0.5
        }
        GroundAction::Move => false,
    };
    if keep {
        return current;
    }
    if let Some((direction, before)) = inputs.reversal {
        return GroundAction::PlantAndTurn(direction, before);
    }
    if !standing && inputs.was_standing && !inputs.walking && on_ground_before {
        return GroundAction::Start(direction8(inputs.body));
    }
    if standing && inputs.turn_rate.abs() >= TURN_RATE {
        return GroundAction::Turn;
    }
    GroundAction::Move
}

/// A direction reversal: the velocity `now` (world, units/s) points against one of the recent velocities (`recent`,
/// within [`REVERSAL_WINDOW`]) that was at least [`REVERSAL_SPEED`] fast, and is itself under way (a third of that).
/// Returns the new direction ([`direction4`] of `body_now`, the body-relative velocity) and the fastest recent
/// speed.
pub fn reversal(
    recent: &[[f32; 2]],
    now: [f32; 2],
    body_now: [f32; 2],
) -> Option<(&'static str, f32)> {
    let length = |v: [f32; 2]| v[0].hypot(v[1]);
    let speed = length(now);
    if speed < REVERSAL_SPEED / 3.0 {
        return None;
    }
    let fastest = recent.iter().map(|&v| length(v)).fold(0.0f32, f32::max);
    let against = recent.iter().any(|&v| {
        let l = length(v);
        l >= REVERSAL_SPEED && (v[0] * now[0] + v[1] * now[1]) / (l * speed) < -0.5
    });
    against.then(|| (direction4(body_now), fastest))
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
            reversal: None,
        };
        let moving = GroundInputs {
            speed: 40.0,
            body: [0.0, 40.0],
            ..still
        };
        // Leaving a standstill starts to the left (W); walking or landing does not.
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
            GroundAction::Move
        );
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, false, &moving),
            GroundAction::Move
        );
        // A start plays out its clip, then moves.
        let running = GroundInputs {
            speed: 200.0,
            body: [200.0, 0.0],
            was_standing: false,
            ..still
        };
        let start = GroundAction::Start("W");
        assert_eq!(ground_action(start, 0.3, 0.53, true, &running), start);
        assert_eq!(
            ground_action(start, 0.6, 0.53, true, &running),
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
        // A counter-strafe: right at 200, now left at 30.
        let recent = [[0.0, -200.0], [0.0, -120.0]];
        let flip = reversal(&recent, [0.0, 30.0], [0.0, 30.0]);
        assert_eq!(flip, Some(("W", 200.0)));
        assert_eq!(
            reversal(&recent, [0.0, 10.0], [0.0, 10.0]),
            None,
            "not under way yet"
        );
        assert_eq!(
            reversal(&[[0.0, 40.0]], [0.0, -30.0], [0.0, -30.0]),
            None,
            "too slow before"
        );
        let reversing = GroundInputs {
            speed: 30.0,
            body: [0.0, 30.0],
            reversal: flip,
            ..still
        };
        assert_eq!(
            ground_action(GroundAction::Move, 0.0, 0.0, true, &reversing),
            GroundAction::PlantAndTurn("W", 200.0)
        );
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
