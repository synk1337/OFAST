//! Gun shots: CS2's fire-bullets message, one per shot, decoded; and each bullet's direction rebuilt from the shot's
//! random seed, as CS2's clients rebuild it.
//!
//! A demo records no bullet impacts on the world. For every shot the server sends each client a fire-bullets
//! message (packet message 452, Valve's `CMsgTEFireBullets`) with where the shooter's eyes were, where they aimed,
//! the weapon and a random seed; every client then traces the bullets itself to draw the tracers, impacts and
//! bullet holes. Only hits on players arrive as effects (message 400, an effect dispatch: the blood). So OFAST does
//! what the clients do: [`decode_fire_bullets`], then [`bullet_direction`] per bullet.
//!
//! OFAST bundles no definition of this message; its fields were read from the demos ([`FireBullets`]) and checked
//! against the shots' `weapon_fire` events (same ticks, same shooters) and against glass the bullets broke (message
//! 423: the exact point a bullet met the pane, on its path). The spread rule is Valve's: a uniform random stream
//! seeded per bullet ([`UniformRandom`]), drawing a random offset within the inaccuracy and another within the
//! spread ([`spread_offset`]); the panes fix the seeding and the order of the draws exactly.

use prost::Message;

use super::stringtable::StringTable;

/// Packet message id of CS2's fire-bullets message (`GE_FireBulletsId`).
pub const FIRE_BULLETS: i32 = 452;
/// Packet message id of an effect dispatch (`TE_EffectDispatchId`): among others, a bullet's hit on a player.
pub const EFFECT_DISPATCH: i32 = 400;

/// A vector as Valve's messages send it (`CMsgVector`): three floats.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct MsgVector {
    /// x.
    #[prost(float, optional, tag = "1")]
    pub x: Option<f32>,
    /// y.
    #[prost(float, optional, tag = "2")]
    pub y: Option<f32>,
    /// z.
    #[prost(float, optional, tag = "3")]
    pub z: Option<f32>,
}

impl MsgVector {
    /// As an array (a missing component is 0, protobuf's default).
    pub fn to_array(self) -> [f32; 3] {
        [
            self.x.unwrap_or(0.0),
            self.y.unwrap_or(0.0),
            self.z.unwrap_or(0.0),
        ]
    }
}

/// CS2's fire-bullets message, the fields OFAST reads (the others: 10 the sound type, 12 a sound effect, 15 the
/// attack type).
#[derive(Clone, PartialEq, Message)]
pub struct FireBullets {
    /// The shooter's eyes (feet + view offset), where the bullets start.
    #[prost(message, optional, tag = "1")]
    pub origin: Option<MsgVector>,
    /// Pitch, yaw, roll in degrees: where the bullets go before the spread (the aim punch of recoil included; a
    /// bullet that broke a pane of glass lies within the shot's inaccuracy of this direction).
    #[prost(message, optional, tag = "2")]
    pub angles: Option<MsgVector>,
    /// The weapon's entity handle.
    #[prost(uint32, optional, tag = "3")]
    pub weapon: Option<u32>,
    /// The fire mode: 1 for the secondary (a silenced USP-S or M4A1-S, a burst Glock or FAMAS, the R8's fan).
    #[prost(uint32, optional, tag = "4")]
    pub mode: Option<u32>,
    /// The random seed of the shot's spread.
    #[prost(uint32, optional, tag = "5")]
    pub seed: Option<u32>,
    /// The shooter's pawn handle.
    #[prost(fixed32, optional, tag = "6")]
    pub player: Option<u32>,
    /// The weapon's inaccuracy at the shot (radians of offset at most: moving, jumping, firing fast).
    #[prost(float, optional, tag = "7")]
    pub inaccuracy: Option<f32>,
    /// The recoil index (shots into the spray, fractional).
    #[prost(float, optional, tag = "8")]
    pub recoil_index: Option<f32>,
    /// The weapon's own spread (radians of offset at most).
    #[prost(float, optional, tag = "9")]
    pub spread: Option<f32>,
    /// The weapon's item definition index (7 AK-47, 4 Glock-18, 61 USP-S…: `items_game.txt`).
    #[prost(uint32, optional, tag = "11")]
    pub item_def_index: Option<u32>,
    /// The shooter's feet.
    #[prost(message, optional, tag = "13")]
    pub feet: Option<MsgVector>,
    /// The bullets in the magazine as the shot is fired.
    #[prost(uint32, optional, tag = "14")]
    pub bullets_left: Option<u32>,
    /// When the shot was fired (local demos only; [`ShotTiming`]).
    #[prost(message, optional, tag = "16")]
    pub timing: Option<ShotTiming>,
    /// The server tick the shot was taken on.
    #[prost(uint32, optional, tag = "19")]
    pub server_tick: Option<u32>,
}

/// When a shot was fired, the fields OFAST reads of the fire-bullets message's field 16 (the same as the
/// `bullet_damage` event's `attack_tick_count` … keys; the others: 1 the aim punch, 6 / 7 the inaccuracy from moving
/// and from being in the air, 8 a type).
#[derive(Clone, PartialEq, Message)]
pub struct ShotTiming {
    /// The game tick of the attack.
    #[prost(int32, optional, tag = "2")]
    pub attack_tick: Option<i32>,
    /// The fraction of the tick interval at which the player fired.
    #[prost(float, optional, tag = "3")]
    pub attack_fraction: Option<f32>,
    /// The tick the shooter's screen showed then (interpolated a few ticks behind).
    #[prost(int32, optional, tag = "4")]
    pub render_tick: Option<i32>,
    /// Its fraction.
    #[prost(float, optional, tag = "5")]
    pub render_fraction: Option<f32>,
}

/// One shot, from its fire-bullets message.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    /// Demo tick of the frame that carried it.
    pub tick: u32,
    /// The shooter's pawn entity index (the low 14 bits of the handle).
    pub pawn: u32,
    /// The weapon's item definition index.
    pub item: u32,
    /// The fire mode (1: secondary).
    pub mode: u32,
    /// Where the bullets start: the shooter's eyes.
    pub eye: [f32; 3],
    /// Pitch, yaw, roll (degrees) before the spread.
    pub angles: [f32; 3],
    /// The spread's random seed.
    pub seed: u32,
    /// Inaccuracy (radians).
    pub inaccuracy: f32,
    /// Spread (radians).
    pub spread: f32,
    /// Recoil index.
    pub recoil_index: f32,
    /// The bullets in the magazine as the shot is fired (30 on a full AK-47's first shot).
    pub bullets_left: u32,
    /// The server tick the shot was taken on (game ticks: the demo's ticks plus an offset, [`shot_times`]).
    pub server_tick: Option<u32>,
    /// The attack's game tick and the fraction of its tick interval at which the player fired (CS2's sub-tick
    /// input); `None` in tournament demos, which leave it out.
    pub attack: Option<(u32, f32)>,
}

/// An effect dispatch's effect, the fields OFAST reads (`CMsgEffectData`; 3 a normal, 4 angles, 5 the entity, 10 a
/// surface, 12 a damage type, 14 a hitbox…).
#[derive(Clone, PartialEq, Message)]
pub struct EffectData {
    /// Where the effect happens: for a bullet, where it met the player.
    #[prost(message, optional, tag = "1")]
    pub origin: Option<MsgVector>,
    /// Where it comes from: for a bullet, the shooter's eyes (the shot's [`Shot::eye`], to the last bit), or where it
    /// came out of the wall or player it went through before.
    #[prost(message, optional, tag = "2")]
    pub start: Option<MsgVector>,
    /// `magnitude`: CS2's blood ([`BLOOD_EFFECT`]) sends the hit's damage here (module docs of `export::shots`).
    #[prost(float, optional, tag = "8")]
    pub magnitude: Option<f32>,
    /// `effectindex`: a [`PARTICLE_EFFECT`]'s particle system, the [`crate::resource::resource_id`] of its path.
    #[prost(fixed64, optional, tag = "11")]
    pub effect_index: Option<u64>,
    /// `effectname`: which effect, as its entry in the demo's [`EFFECT_TABLE`].
    #[prost(uint32, optional, tag = "18")]
    pub effect_name: Option<u32>,
}

/// The string table naming the effects of effect dispatches ([`EffectData::effect_name`] is an entry's index). In
/// CS2's demos of 2026 (three checked, 2026-10-30): 4 `csblood`, 7 `ParticleEffect`, 10 `Impact` (a bullet's hit on a
/// player is dispatched as both `Impact` and `csblood`), 15 `BloodImpact`…
pub const EFFECT_TABLE: &str = "EffectDispatch";
/// The effect CS2 draws a bullet's blood with: `particles/blood_impact/blood_impact_high`, `_med`, `_low` or
/// `_friendly` by the damage it carries ([`EffectData::magnitude`]).
pub const BLOOD_EFFECT: &str = "csblood";
/// The effect playing a particle system named by its id ([`EffectData::effect_index`]): on a bullet's line, the
/// sparks of a headshot a helmet took (`particles/impact_fx/impact_helmet_headshot`), dispatched at the hit's point.
pub const PARTICLE_EFFECT: &str = "ParticleEffect";

/// An effect dispatch message (`CMsgTEEffectDispatch`).
#[derive(Clone, PartialEq, Message)]
pub struct EffectDispatch {
    /// The effect.
    #[prost(message, optional, tag = "1")]
    pub effect: Option<EffectData>,
}

/// A point a bullet met a player at, from an effect dispatch (the demo's only record of where bullets ended).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerHit {
    /// Demo tick of the frame that carried it (the shot's).
    pub tick: u32,
    /// Where that stretch of the bullet's flight started: the shooter's eyes ([`Shot::eye`] of the shot whose bullet
    /// it was, [`hit_of`]), or, past a wall or a player it went through, where it came out of them (on its line).
    pub start: [f32; 3],
    /// Where the bullet met the player.
    pub point: [f32; 3],
    /// The damage a [`BLOOD_EFFECT`] dispatch carries (CS2 picks the blood's size by it); `None` for other effects.
    pub blood: Option<f32>,
    /// A [`PARTICLE_EFFECT`] dispatch's particle system ([`EffectData::effect_index`]); `None` for other effects.
    pub particle: Option<u64>,
}

/// Decodes an effect dispatch (`body`) read at `tick` as a possible bullet hit; `None` without both ends. Other
/// effects (a knife's hit, a grenade's) decode too: [`PlayerHit::start`] lies on no bullet's line for them, and the
/// impacts dispatched on a bullet's line start where they end. `effects` is the demo's [`EFFECT_TABLE`], which names
/// the blood and particle effects ([`PlayerHit::blood`], [`PlayerHit::particle`]; without it, neither is known).
pub fn decode_player_hit(
    tick: u32,
    body: &[u8],
    effects: Option<&StringTable>,
) -> Option<PlayerHit> {
    let effect = EffectDispatch::decode(body).ok()?.effect?;
    let name = effect
        .effect_name
        .and_then(|n| effects?.entries.get(n as usize))
        .map(|e| e.key.as_str());
    Some(PlayerHit {
        tick,
        start: effect.start?.to_array(),
        point: effect.origin?.to_array(),
        blood: (name == Some(BLOOD_EFFECT)).then(|| effect.magnitude.unwrap_or(0.0)),
        particle: effect
            .effect_index
            .filter(|_| name == Some(PARTICLE_EFFECT)),
    })
}

/// Whether `hit` is a hit of a bullet of `shot` from its eyes: same tick, starting there (a hit after a wall or a
/// player starts further along the bullet's line: `export::shots` matches those by the line).
pub fn hit_of(shot: &Shot, hit: &PlayerHit) -> bool {
    hit.tick == shot.tick && hit.start == shot.eye
}

/// Decodes a fire-bullets message (`body`) read at `tick`; `None` when it is not one or lacks the shooter (or names
/// none: an invalid handle) or the aim.
pub fn decode_fire_bullets(tick: u32, body: &[u8]) -> Option<Shot> {
    let m = FireBullets::decode(body).ok()?;
    let attack = m.timing.as_ref().and_then(|t| {
        Some((
            u32::try_from(t.attack_tick?).ok()?,
            t.attack_fraction.unwrap_or(0.0),
        ))
    });
    Some(Shot {
        tick,
        pawn: super::entity::handle_to_index(u64::from(m.player?))?,
        item: m.item_def_index.unwrap_or(0),
        mode: m.mode.unwrap_or(0),
        eye: m.origin?.to_array(),
        angles: m.angles?.to_array(),
        seed: m.seed.unwrap_or(0),
        inaccuracy: m.inaccuracy.unwrap_or(0.0),
        spread: m.spread.unwrap_or(0.0),
        recoil_index: m.recoil_index.unwrap_or(0.0),
        bullets_left: m.bullets_left.unwrap_or(0),
        server_tick: m.server_tick,
        attack,
    })
}

/// When each of `shots` was fired, in demo ticks to a fraction (the order of `shots`; [`Shot::server_tick`],
/// [`Shot::attack`]). A tick's commands are the input of the tick interval before it, and CS2 fires a shot at the
/// moment within it the player clicked: tick − 1 + the attack's fraction, on the server tick turned into demo
/// ticks by the offset most of the shots' messages have (a few arrive a tick early: on the local match demo 78 of
/// 2361). Without the attack's fraction (tournament demos) the shot is at the start of its tick interval; without the
/// server tick, at its message's tick.
pub fn shot_times<'a>(shots: impl IntoIterator<Item = &'a Shot> + Clone) -> Vec<f64> {
    let mut offsets: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    for shot in shots.clone() {
        if let Some(server) = shot.server_tick {
            *offsets
                .entry(i64::from(server) - i64::from(shot.tick))
                .or_default() += 1;
        }
    }
    let offset = offsets
        .into_iter()
        .max_by_key(|&(offset, n)| (n, -offset))
        .map(|(offset, _)| offset);
    shots
        .into_iter()
        .map(|shot| {
            let (tick, fraction) = match (shot.attack, shot.server_tick, offset) {
                (Some((attack, fraction)), _, Some(offset)) => {
                    (i64::from(attack) - offset, fraction)
                }
                (None, Some(server), Some(offset)) => (i64::from(server) - offset, 0.0),
                _ => return f64::from(shot.tick),
            };
            // In f64: f32 keeps only a 64th of a tick at demo ticks past 131 072.
            tick as f64 - 1.0 + f64::from(fraction)
        })
        .collect()
}

/// Valve's uniform random stream (`CUniformRandomStream`): Park and Miller's minimal standard generator with a
/// Bays–Durham shuffle of 32 entries, Numerical Recipes' `ran1` (Press, Teukolsky, Vetterling, Flannery, "Numerical
/// Recipes in C", 2nd ed., 7.1).
#[derive(Debug, Clone)]
pub struct UniformRandom {
    idum: i32,
    iy: i32,
    iv: [i32; 32],
}

const IA: i32 = 16807;
const IM: i32 = 2_147_483_647;
const IQ: i32 = 127_773;
const IR: i32 = 2836;
const NTAB: i32 = 32;
const NDIV: i32 = 1 + (IM - 1) / NTAB;
/// The largest float [`UniformRandom::float`] returns before scaling: just below 1.
const RNMX: f32 = 1.0 - 1.2e-7;

impl UniformRandom {
    /// A stream seeded with `seed` (Valve's `SetSeed`: the generator starts from `-|seed|`).
    pub fn new(seed: i32) -> Self {
        Self {
            idum: if seed < 0 { seed } else { seed.wrapping_neg() },
            iy: 0,
            iv: [0; 32],
        }
    }

    /// One step of the Park–Miller generator (Schrage's method: no overflow).
    fn step(&mut self) {
        let k = self.idum / IQ;
        self.idum = IA.wrapping_mul(self.idum - k * IQ) - IR.wrapping_mul(k);
        if self.idum < 0 {
            self.idum += IM;
        }
    }

    /// The next number, 1 … 2³¹ − 2 (Valve's `GenerateRandomNumber`).
    pub fn number(&mut self) -> i32 {
        if self.idum <= 0 || self.iy == 0 {
            // (Re)start: warm up 8 steps, then fill the shuffle table.
            self.idum = if self.idum.wrapping_neg() < 1 {
                1
            } else {
                self.idum.wrapping_neg()
            };
            for j in (0..NTAB + 8).rev() {
                self.step();
                if j < NTAB {
                    self.iv[j as usize] = self.idum;
                }
            }
            self.iy = self.iv[0];
        }
        self.step();
        let j = (self.iy / NDIV).rem_euclid(NTAB) as usize;
        self.iy = self.iv[j];
        self.iv[j] = self.idum;
        self.iy
    }

    /// A float in `low … high` (Valve's `RandomFloat`).
    pub fn float(&mut self, low: f32, high: f32) -> f32 {
        let unit = ((1.0 / f64::from(IM)) * f64::from(self.number())) as f32;
        unit.min(RNMX) * (high - low) + low
    }
}

/// The spread of bullet `bullet` (0 for the first; shotguns fire several) of a shot: the offset `(right, up)` to
/// add to the unit forward vector, as tangents. The stream is seeded with the shot's seed + 1 + the bullet and
/// gives, in order, a fraction of the inaccuracy, its angle, the spread's angle and a fraction of the spread; the
/// two offsets add up.
///
/// Checked on the bullets that broke glass (`examples/shot_spread.rs`): on the de_nuke pro demo 82 of 83 panes lie
/// on the bullet's path to 0.001 milliradian (the other is another bullet's), on the local match demo all 17 (other
/// seed offsets or draw orders miss by 1 – 100 milliradians). The pistols, the AK-47, the M4A1-S and the MP9 among
/// them. Shotguns follow another rule, not found yet (their first pellet too; `examples/shot_pellets.rs` tries 12 built
/// on their fixed `m_nSpreadSeed`, on the demos' 4 shotgun shots with hits): this one stands in for it. Not checked:
/// the R8 Revolver's and the Negev's own rules.
pub fn spread_offset(seed: u32, bullet: u32, inaccuracy: f32, spread: f32) -> [f32; 2] {
    let mut random = UniformRandom::new(seed.wrapping_add(1).wrapping_add(bullet) as i32);
    let r1 = random.float(0.0, 1.0);
    let a1 = random.float(0.0, std::f32::consts::TAU);
    let a2 = random.float(0.0, std::f32::consts::TAU);
    let r2 = random.float(0.0, 1.0);
    let (i, s) = (r1 * inaccuracy, r2 * spread);
    [a1.cos() * i + a2.cos() * s, a1.sin() * i + a2.sin() * s]
}

/// Source's forward, right and up vectors of pitch / yaw / roll in degrees (`AngleVectors`): x forward at yaw 0,
/// positive pitch looking down, right = forward × up turned.
pub fn angle_vectors(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    [forward, right, up]
}

/// The unit direction of a bullet aimed at `angles` with the spread `offset` ([`spread_offset`]).
pub fn bullet_direction(angles: [f32; 3], offset: [f32; 2]) -> [f32; 3] {
    let [f, r, u] = angle_vectors(angles);
    let d: [f32; 3] = std::array::from_fn(|k| f[k] + offset[0] * r[k] + offset[1] * u[k]);
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    d.map(|c| c / length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::stringtable::StringTableEntry;

    #[test]
    fn effect_dispatches_carry_the_blood_damage_and_particle_ids() {
        let table = StringTable {
            name: EFFECT_TABLE.into(),
            entries: ["error", "csblood", "ParticleEffect"]
                .into_iter()
                .map(|key| StringTableEntry {
                    key: key.into(),
                    value: None,
                })
                .collect(),
            ..Default::default()
        };
        let v = |x: f32| {
            Some(MsgVector {
                x: Some(x),
                y: Some(0.0),
                z: Some(0.0),
            })
        };
        let dispatch = |name: u32, magnitude: Option<f32>, index: Option<u64>| {
            EffectDispatch {
                effect: Some(EffectData {
                    origin: v(100.0),
                    start: v(0.0),
                    magnitude,
                    effect_index: index,
                    effect_name: Some(name),
                }),
            }
            .encode_to_vec()
        };
        let blood = decode_player_hit(7, &dispatch(1, Some(27.0), None), Some(&table)).unwrap();
        assert_eq!((blood.blood, blood.particle), (Some(27.0), None));
        assert_eq!(blood.point, [100.0, 0.0, 0.0]);
        // A zero damage is not sent (protobuf's default): friendly blood, damage 0.
        let friendly = decode_player_hit(7, &dispatch(1, None, None), Some(&table)).unwrap();
        assert_eq!(friendly.blood, Some(0.0));
        let sparks = decode_player_hit(7, &dispatch(2, None, Some(42)), Some(&table)).unwrap();
        assert_eq!((sparks.blood, sparks.particle), (None, Some(42)));
        // Another effect, or no table to name it: neither.
        let other = decode_player_hit(7, &dispatch(0, Some(27.0), Some(42)), Some(&table)).unwrap();
        assert_eq!((other.blood, other.particle), (None, None));
        let unnamed = decode_player_hit(7, &dispatch(1, Some(27.0), None), None).unwrap();
        assert_eq!(unnamed.blood, None);
    }

    #[test]
    fn uniform_random_is_park_miller_with_a_shuffle() {
        // Park–Miller from seed 1 goes 16807, 282475249, 1622650073…; the shuffle table hands them out in another
        // order, but every number is one of the generator's and the stream repeats exactly for a seed.
        let mut a = UniformRandom::new(1);
        let mut b = UniformRandom::new(1);
        let first: Vec<i32> = (0..50).map(|_| a.number()).collect();
        assert_eq!(first, (0..50).map(|_| b.number()).collect::<Vec<_>>());
        assert!(first.iter().all(|&n| (1..IM).contains(&n)));
        let mut c = UniformRandom::new(2);
        assert_ne!(first[0], c.number());
        let f = UniformRandom::new(7).float(0.0, 1.0);
        assert!((0.0..1.0).contains(&f));
    }

    #[test]
    fn spread_matches_bullets_that_broke_glass() {
        // Shots of the aurora-vs-furia de_nuke demo (build 10772) and where their bullets met a pane of glass, as
        // (right, up) tangents from the shot's aim (`examples/shot_spread.rs`): seed, inaccuracy, spread, offset.
        let panes = [
            // tick 6699, USP-S (silenced).
            (
                3_914_992_991_u32,
                0.019_213_805_f32,
                0.0015_f32,
                [-0.011_327_583_f32, -0.004_823_915],
            ),
            // tick 23107, AK-47 deep in a spray.
            (
                3_182_748_996,
                0.194_325_82,
                0.0006,
                [-0.038_105_26, -0.041_591_3],
            ),
            // tick 139023, M4A1-S (silenced).
            (113_867_783, 0.159_233_1, 0.0005, [0.051_223_505, 0.069_765]),
        ];
        for (seed, inaccuracy, spread, seen) in panes {
            let offset = spread_offset(seed, 0, inaccuracy, spread);
            let error = (offset[0] - seen[0]).hypot(offset[1] - seen[1]);
            assert!(error < 2e-6, "seed {seed}: {offset:?} vs {seen:?}");
        }
    }

    #[test]
    fn angle_vectors_follow_source_axes() {
        let [f, r, u] = angle_vectors([0.0, 90.0, 0.0]);
        assert!((f[1] - 1.0).abs() < 1e-6 && f[0].abs() < 1e-6);
        // Looking along +y, right is +x.
        assert!((r[0] - 1.0).abs() < 1e-6);
        assert!((u[2] - 1.0).abs() < 1e-6);
        let [f, _, _] = angle_vectors([30.0, 0.0, 0.0]);
        assert!(f[2] < 0.0, "positive pitch looks down");
    }

    #[test]
    fn fire_bullets_decodes() {
        let message = FireBullets {
            origin: Some(MsgVector {
                x: Some(1.0),
                y: Some(2.0),
                z: Some(3.0),
            }),
            angles: Some(MsgVector {
                x: Some(-10.0),
                y: Some(45.0),
                z: None,
            }),
            weapon: Some(15_581_290),
            mode: None,
            seed: Some(42),
            player: Some(12_960_107),
            inaccuracy: Some(0.0056),
            recoil_index: Some(0.5),
            spread: Some(0.002),
            item_def_index: Some(7),
            feet: None,
            bullets_left: Some(29),
            timing: Some(ShotTiming {
                attack_tick: Some(2502),
                attack_fraction: Some(0.25),
                render_tick: Some(2496),
                render_fraction: Some(0.5),
            }),
            server_tick: Some(2502),
        };
        let shot = decode_fire_bullets(100, &message.encode_to_vec()).expect("decodes");
        assert_eq!(shot.pawn, 363);
        assert_eq!(shot.eye, [1.0, 2.0, 3.0]);
        assert_eq!(shot.angles, [-10.0, 45.0, 0.0]);
        assert_eq!(
            (shot.item, shot.mode, shot.seed, shot.bullets_left),
            (7, 0, 42, 29)
        );
        assert_eq!(
            (shot.server_tick, shot.attack),
            (Some(2502), Some((2502, 0.25)))
        );
    }

    #[test]
    fn shots_are_timed_within_their_tick() {
        let shot = |tick: u32, server: Option<u32>, attack: Option<(u32, f32)>| Shot {
            tick,
            pawn: 1,
            item: 7,
            mode: 0,
            eye: [0.0; 3],
            angles: [0.0; 3],
            seed: 0,
            inaccuracy: 0.0,
            spread: 0.0,
            recoil_index: 0.0,
            bullets_left: 0,
            server_tick: server,
            attack,
        };
        let shots = [
            // Offset 2400 twice; a message a tick early once.
            shot(100, Some(2500), Some((2500, 0.25))),
            shot(110, Some(2510), None),
            shot(119, Some(2520), Some((2520, 0.5))),
            // Neither: its message's tick.
            shot(130, None, None),
        ];
        assert_eq!(shot_times(&shots), vec![99.25, 109.0, 119.5, 130.0]);
    }
}
