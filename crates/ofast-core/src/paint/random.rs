//! The random numbers a paint kit's seed gives (`docs/research/cs2-skins.md`): a uniform random stream, Park and
//! Miller's minimal standard generator with a Bays–Durham shuffle (Numerical Recipes' `ran1`), as Valve's Source
//! engine has it and as CS2's composite materials are taken to roll their variables with
//! (`COMP_MAT_PROPERTY_MUTATOR_RANDOM_ROLL_INPUT_VARIABLES`).
//!
//! The state is a 31-bit integer advanced by `state × 16807 mod (2³¹ − 1)` (Schrage's method keeps it in 32 bits),
//! with a table of 32 earlier values that each output is drawn from by the previous output's top bits. Seeding
//! negates the seed and forgets the table; the first draw fills it after 8 discarded steps. A float is the output
//! scaled to (0, 1), clamped just under 1, then to the range asked for. Knowledge from the generator's description
//! in Numerical Recipes (2nd ed., §7.1) and the MIT-licensed CS:GO pattern calculators that use it
//! (github.com/chescos/csgo-fade-percentage-calculator, github.com/Step7750/UniformRandom); to be checked against
//! CS2's own patterns.

/// The multiplier.
const IA: i32 = 16807;
/// The modulus, 2³¹ − 1.
const IM: i32 = 2_147_483_647;
/// `IM / IA` and `IM % IA`, for Schrage's method.
const IQ: i32 = 127_773;
const IR: i32 = 2836;
/// The shuffle table's size.
const NTAB: usize = 32;
/// How many outputs share a table slot.
const NDIV: i32 = 1 + (IM - 1) / NTAB as i32;
/// The largest float drawn, just under 1.
const RNMX: f64 = 1.0 - 1.2e-7;

/// A uniform random stream: the same seed gives the same numbers.
#[derive(Debug, Clone)]
pub struct UniformRandom {
    state: i32,
    last: i32,
    table: [i32; NTAB],
}

impl UniformRandom {
    /// A stream seeded with `seed`.
    pub fn new(seed: i32) -> Self {
        Self {
            // A positive seed is negated: a state ≤ 0 asks for the table to be filled.
            state: if seed < 0 { seed } else { seed.wrapping_neg() },
            last: 0,
            table: [0; NTAB],
        }
    }

    /// One step of the generator (Schrage's method: no overflow in 32 bits).
    fn step(state: i32) -> i32 {
        let k = state / IQ;
        let next = IA * (state - k * IQ) - IR * k;
        if next < 0 { next + IM } else { next }
    }

    /// The next integer, 1 to 2³¹ − 2.
    pub fn next_int(&mut self) -> i32 {
        if self.state <= 0 || self.last == 0 {
            // Fill the table: 8 steps thrown away, then 32 kept (the last one in slot 0).
            self.state = if -self.state < 1 { 1 } else { -self.state };
            for j in (0..NTAB + 8).rev() {
                self.state = Self::step(self.state);
                if j < NTAB {
                    self.table[j] = self.state;
                }
            }
            self.last = self.table[0];
        }
        self.state = Self::step(self.state);
        let j = (self.last / NDIV) as usize;
        self.last = self.table[j];
        self.table[j] = self.state;
        self.last
    }

    /// The next float from `low` (included) towards `high`.
    pub fn float(&mut self, low: f32, high: f32) -> f32 {
        let unit = (f64::from(self.next_int()) / f64::from(IM)).min(RNMX) as f32;
        unit * (high - low) + low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let draws = |seed| {
            let mut r = UniformRandom::new(seed);
            [r.float(0.0, 1.0), r.float(0.0, 1.0), r.float(-55.0, -65.0)]
        };
        assert_eq!(draws(245), draws(245));
        assert_ne!(draws(245), draws(246));
        let [a, b, rotation] = draws(661);
        assert!((0.0..1.0).contains(&a) && (0.0..1.0).contains(&b));
        // A range written backwards (Fade's −55 to −65) is drawn backwards.
        assert!((-65.0..=-55.0).contains(&rotation), "{rotation}");
    }

    #[test]
    fn schrage_steps_match_the_plain_product() {
        for state in [1, 2, 12345, 1_000_000_000, IM - 1] {
            let plain = (i64::from(state) * i64::from(IA) % i64::from(IM)) as i32;
            assert_eq!(UniformRandom::step(state), plain);
        }
    }

    #[test]
    fn integers_stay_within_the_modulus() {
        let mut r = UniformRandom::new(0);
        for _ in 0..10_000 {
            let n = r.next_int();
            assert!((1..IM).contains(&n), "{n}");
        }
    }
}
