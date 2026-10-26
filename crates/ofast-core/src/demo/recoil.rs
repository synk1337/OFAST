//! A gun's recoil: the aim punch a player's shots build, rebuilt from each gun's recoil keys.
//!
//! CS2 does not send a player's aim punch to the demo (`CCSPlayer_AimPunchServices`' fields stay unsent on SourceTV),
//! but every shot shows it: the fire-bullets message's aim is the eye angles plus twice the punch
//! ([`super::shots::Shot::angles`]). The punch follows a spring CS2 drives from each shot's kick, as Source 2 Viewer
//! rebuilds it (`Renderer/Gameplay/AimPunchServices.cs`, `RecoilPattern.cs`): a shot's kick, from the gun's recoil
//! table at its recoil index ([`RecoilPattern`]), pushes a velocity that decays exponentially, and the punch
//! integrates that velocity while it decays back to nothing, exponentially and linearly ([`AimPunch`]). The bullets
//! go along the eyes plus twice the punch ([`BULLET_SCALE`]); the first-person camera turns by 0.45 of that
//! ([`VIEW_SCALE`]), the viewmodel another 0.325 past it ([`VIEWMODEL_SCALE`]).

use super::shots::UniformRandom;

/// Scale from the punch to where the bullets go.
pub const BULLET_SCALE: f32 = 2.0;
/// Scale from the punch to the first-person camera: 0.45 of the bullets' offset.
pub const VIEW_SCALE: f32 = BULLET_SCALE * 0.45;
/// Scale from the punch to the first-person weapon, which turns another 0.325 of the bullets' offset past the
/// camera.
pub const VIEWMODEL_SCALE: f32 = VIEW_SCALE + BULLET_SCALE * 0.325;

/// The velocity's exponential decay rate (per second).
const VELOCITY_DECAY: f32 = 4.5;
/// The punch's exponential decay rate (per second).
const EXPONENTIAL_DECAY: f32 = 8.0;
/// The punch's linear decay (degrees per second).
const LINEAR_DECAY: f32 = 18.0;

/// The share of a shot's kick that goes into the view punch ([`view_punch_at`]).
pub const VIEW_PUNCH_SCALE: f32 = 0.055;
/// The view punch's exponential decay rate (per second): CS:GO's `view_punch_decay`.
pub const VIEW_PUNCH_DECAY: f32 = 18.0;

/// A view punch as the server last set it: (pitch, yaw) degrees, and when, in demo ticks to a fraction
/// ([`super::players::RigInputs::view_punch`]).
pub type ViewPunch = ([f32; 2], f64);

/// The view punch at `time` (demo ticks, fractional) from the one last set (`set`), decayed since; nothing before it
/// was set. Each shot adds [`VIEW_PUNCH_SCALE`] of its kick to what is left of the last one, and the server sends the
/// sum.
pub fn view_punch_at(set: Option<ViewPunch>, time: f64, interval: f32) -> [f32; 2] {
    let Some((angle, when)) = set else {
        return [0.0; 2];
    };
    let since = ((time - when).max(0.0) as f32) * interval;
    let keep = (-VIEW_PUNCH_DECAY * since).exp();
    angle.map(|a| a * keep)
}

/// Kicks per mode in a recoil table; a recoil index wraps past it.
pub const PATTERN_LENGTH: usize = 64;
/// How much of the way from one kick to the next a full-auto gun's kick goes.
const FULL_AUTO_SMOOTHING: f32 = 0.55;
/// The first kicks of a full-auto spray that ramp up to full strength, from this share of it.
const SUPPRESSION_SHOTS: usize = 4;
const SUPPRESSION_FACTOR: f32 = 0.75;

/// A weapon's recoil keys in `scripts/weapons.vdata`, each angle and magnitude per mode (normal, secondary).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RecoilKeys {
    /// `m_nRecoilSeed`: seeds the kicks' variances.
    pub seed: i32,
    /// `m_bIsFullAuto`: kicks drift from one to the next, and the first few ramp up.
    pub full_auto: bool,
    /// `m_flRecoilAngle`: the kick's direction, degrees (0 straight up, positive to the right).
    pub angle: [f32; 2],
    /// `m_flRecoilAngleVariance`: how far it varies either way.
    pub angle_variance: [f32; 2],
    /// `m_flRecoilMagnitude`: the kick's strength.
    pub magnitude: [f32; 2],
    /// `m_flRecoilMagnitudeVariance`: how far it varies either way.
    pub magnitude_variance: [f32; 2],
}

/// A gun's recoil table: the kick (angle in degrees, magnitude) of each shot of a spray, per mode, from its keys: the
/// gun's recoil seed drives the variances; a full-auto gun's kicks drift from one to the next and its first few ramp
/// up to full strength.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoilPattern {
    table: Vec<(f32, f32)>,
}

impl RecoilPattern {
    /// The table of `keys`.
    pub fn new(keys: &RecoilKeys) -> Self {
        let mut table = Vec::with_capacity(2 * PATTERN_LENGTH);
        for mode in 0..2 {
            let mut random = UniformRandom::new(keys.seed);
            let (mut angle, mut magnitude) = (0.0f32, 0.0f32);
            for shot in 0..PATTERN_LENGTH {
                let new_angle = keys.angle[mode]
                    + random.float(-keys.angle_variance[mode], keys.angle_variance[mode]);
                let new_magnitude = keys.magnitude[mode]
                    + random.float(
                        -keys.magnitude_variance[mode],
                        keys.magnitude_variance[mode],
                    );
                if keys.full_auto && shot > 0 {
                    angle += (new_angle - angle) * FULL_AUTO_SMOOTHING;
                    magnitude += (new_magnitude - magnitude) * FULL_AUTO_SMOOTHING;
                } else {
                    angle = new_angle;
                    magnitude = new_magnitude;
                }
                if keys.full_auto && shot < SUPPRESSION_SHOTS {
                    let t = shot as f32 / SUPPRESSION_SHOTS as f32;
                    magnitude *= SUPPRESSION_FACTOR + (1.0 - SUPPRESSION_FACTOR) * t;
                }
                table.push((angle, magnitude));
            }
        }
        Self { table }
    }

    /// The kick of shot `shot` of a spray (its recoil index) in `mode` (0 normal, 1 secondary).
    pub fn kick(&self, mode: usize, shot: usize) -> (f32, f32) {
        self.table[mode.min(1) * PATTERN_LENGTH + shot % PATTERN_LENGTH]
    }
}

/// A player's aim punch: (pitch, yaw) degrees, negative pitch up as Source's angles, and the velocity driving it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AimPunch {
    /// The punch.
    pub angle: [f32; 2],
    /// Its velocity (degrees per second).
    pub velocity: [f32; 2],
}

impl AimPunch {
    /// A shot's kick: `angle` degrees from straight up (positive to the right) and `magnitude`.
    pub fn kick(&mut self, angle: f32, magnitude: f32) {
        let (sin, cos) = angle.to_radians().sin_cos();
        self.velocity[0] -= cos * magnitude;
        self.velocity[1] -= sin * magnitude;
    }

    /// Moves on by `dt` seconds.
    pub fn advance(&mut self, dt: f32) {
        let keep = (-VELOCITY_DECAY * dt).exp();
        self.velocity = self.velocity.map(|v| v * keep);
        let punch = self.angle.map(|a| a * (-EXPONENTIAL_DECAY * dt).exp());
        let length = punch[0].hypot(punch[1]);
        let linear = LINEAR_DECAY * dt;
        let shrunk = if length > linear {
            punch.map(|a| a * (1.0 - linear / length))
        } else {
            [0.0; 2]
        };
        self.angle = std::array::from_fn(|k| shrunk[k] + self.velocity[k] * dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kick_throws_the_punch_up_and_it_settles_back() {
        let mut punch = AimPunch::default();
        punch.kick(0.0, 30.0);
        let mut highest = 0.0f32;
        for _ in 0..64 {
            punch.advance(1.0 / 64.0);
            highest = highest.min(punch.angle[0]);
        }
        // Up (negative pitch), straight: no yaw.
        assert!(highest < -0.2, "{highest}");
        assert!(punch.angle[1].abs() < 1e-6);
        // Back to nothing within two seconds.
        for _ in 0..64 {
            punch.advance(1.0 / 64.0);
        }
        assert!(punch.angle[0].abs() < 0.05, "{:?}", punch.angle);
    }

    #[test]
    fn full_auto_kicks_ramp_up_and_drift() {
        let keys = RecoilKeys {
            seed: 223,
            full_auto: true,
            angle: [0.0; 2],
            angle_variance: [70.0; 2],
            magnitude: [30.0; 2],
            magnitude_variance: [0.0; 2],
        };
        let pattern = RecoilPattern::new(&keys);
        // The first kick at 0.75 of its strength, the next ones drifting up to full; the index wraps.
        assert!(
            (pattern.kick(0, 0).1 - 22.5).abs() < 1e-4,
            "{:?}",
            pattern.kick(0, 0)
        );
        assert!(pattern.kick(0, 4).1 > pattern.kick(0, 1).1 && pattern.kick(0, 4).1 < 30.0);
        assert!((pattern.kick(0, 20).1 - 30.0).abs() < 1e-3);
        assert_eq!(pattern.kick(0, 3), pattern.kick(0, 3 + PATTERN_LENGTH));
        // The angles vary within the variance, each drifting from the last.
        assert!((0..PATTERN_LENGTH).all(|s| pattern.kick(0, s).0.abs() <= 70.0));
    }
}
