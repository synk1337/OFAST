//! A smoke grenade's cloud: its voxel log decoded (`m_VoxelFrameData`), and OFAST's own simulation of the cloud
//! from it (M6).
//!
//! CS2 does not send a smoke's cloud. It sends the inputs of a deterministic simulation that the server and every
//! client run alike, as an append-only byte log on the smoke's entity, one frame per simulation step
//! (`docs/formats/demo.md`, "Smoke voxels"):
//!
//! ```text
//! frame    u16 number, u16 payload length, payload
//! payload  u8 fading, u8 flags,
//!          if flags & 1: u8 n, n × emitter { u8 x, y, z, level; f32 hold }
//!          if flags & 2: u16 n, n × brick  { u16 index; u64 solid mask }
//!          u8 n, n × 20-byte hole record (not decoded)
//! ```
//!
//! The simulation runs on a 32³ grid of 20-unit voxels whose lowest corner is the detonation point − 320 on each
//! axis. The grid is split into 8³ bricks of 4³ voxels; a brick's index and the bits of its solid mask (one per voxel:
//! world geometry, where smoke cannot go) count in Morton order, x in the lowest bit. The server sends a brick the first
//! time its simulation needs it, so the bricks of a frame are also a check of any simulation that claims to be CS2's:
//! it must need exactly those bricks at exactly that frame. Emitters are where the smoke comes out.
//!
//! What the log's layout is, and how the simulation behaves, comes from public reverse engineering (5stackgg/demo-parser,
//! AGPL-3.0: facts only, no code; cross-checked by CS2-insight-agent and memorin.app) and was checked against our own
//! demos (`examples/smoke_log_probe.rs`): on the Aurora vs FURIA de_nuke demo's round 5 every frame of the 9 smokes
//! reads to its exact length and the frames chain to exactly the bytes in use. The simulation here is OFAST's own,
//! written from the behaviour and fitted to the bricks (`docs/decisions/017-grenades-and-effects.md`).

use super::grenades::SmokeLog;

/// Voxels along each axis of a smoke's grid.
pub const GRID: usize = 32;
/// A voxel's edge, inches (world units).
pub const VOXEL_SIZE: f32 = 20.0;
/// Voxels along each axis of a brick.
pub const BRICK: usize = 4;
/// Bricks along each axis of the grid.
pub const BRICKS: usize = GRID / BRICK;

/// Where the smoke comes out: a voxel the simulation feeds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    /// The voxel, grid coordinates (0 – 31 each).
    pub voxel: [u8; 3],
    /// Its level (5 for every emitter seen).
    pub level: u8,
    /// Its hold value (0 for every emitter seen).
    pub hold: f32,
}

/// A 4³-voxel block of world geometry, as the server traced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brick {
    /// The brick's index in the grid: a Morton code of its brick coordinates (0 – 7 each, x in the lowest bit).
    pub index: u16,
    /// One bit per voxel of the brick, Morton order: set where the voxel is solid.
    pub solid: u64,
}

impl Brick {
    /// The brick's coordinates in bricks (0 – 7 each).
    pub fn coordinates(&self) -> [usize; 3] {
        demorton(self.index as u32, 3)
    }

    /// Whether the voxel at `local` (0 – 3 each, within the brick) is solid.
    pub fn is_solid(&self, local: [usize; 3]) -> bool {
        self.solid >> morton(local) & 1 != 0
    }
}

/// One frame of a smoke's log: the inputs of one simulation step.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VoxelFrame {
    /// The frame's number, counting from 0 at the detonation.
    pub number: u16,
    /// Demo tick of the first log snapshot that holds this frame: when the server had run the step.
    pub tick: u32,
    /// The cloud is fading: it stops growing.
    pub fading: bool,
    /// Emitters added in this frame.
    pub emitters: Vec<Emitter>,
    /// Bricks the simulation needed for the first time in this frame.
    pub bricks: Vec<Brick>,
    /// Hole records (an HE blast or bullets through the smoke), raw: their layout is not known yet.
    pub holes: Vec<[u8; 20]>,
}

/// A log that does not read as the layout says.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmokeLogError {
    /// A frame or one of its records runs past the end of the bytes in use.
    #[error("smoke voxel log: frame {frame} runs past the end of the log")]
    Truncated {
        /// The frame's number (or the frame count so far, when even the number is cut off).
        frame: u16,
    },
    /// A frame's records end before or after its stated length.
    #[error("smoke voxel log: frame {frame} holds {length} bytes but its records take {read}")]
    Length {
        /// The frame's number.
        frame: u16,
        /// Its stated payload length.
        length: usize,
        /// The bytes its records take.
        read: usize,
    },
}

/// A cursor over a frame's payload.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
}

/// Decodes one frame's payload (after its number and length), with the bytes its records took.
fn decode_payload(number: u16, payload: &[u8]) -> Option<(VoxelFrame, usize)> {
    let mut r = Reader {
        bytes: payload,
        at: 0,
    };
    let fading = r.u8()? != 0;
    let flags = r.u8()?;
    let mut frame = VoxelFrame {
        number,
        fading,
        ..VoxelFrame::default()
    };
    if flags & 1 != 0 {
        for _ in 0..r.u8()? {
            let e = r.take(8)?;
            frame.emitters.push(Emitter {
                voxel: [e[0], e[1], e[2]],
                level: e[3],
                hold: f32::from_le_bytes([e[4], e[5], e[6], e[7]]),
            });
        }
    }
    if flags & 2 != 0 {
        for _ in 0..r.u16()? {
            let b = r.take(10)?;
            frame.bricks.push(Brick {
                index: u16::from_le_bytes([b[0], b[1]]),
                solid: u64::from_le_bytes(b[2..10].try_into().ok()?),
            });
        }
    }
    for _ in 0..r.u8()? {
        frame.holes.push(r.take(20)?.try_into().ok()?);
    }
    Some((frame, r.at))
}

/// Decodes a log (`m_VoxelFrameData`'s bytes in use) into its frames, oldest first. Their `tick` is 0: a single
/// snapshot does not say when each frame was made ([`frames`] does).
pub fn decode_log(bytes: &[u8]) -> Result<Vec<VoxelFrame>, SmokeLogError> {
    match decode_prefix(bytes) {
        (frames, None) => Ok(frames),
        (_, Some(error)) => Err(error),
    }
}

/// Decodes a log as far as it reads: the frames before the first one that does not, and that frame's error.
fn decode_prefix(bytes: &[u8]) -> (Vec<VoxelFrame>, Option<SmokeLogError>) {
    let mut out: Vec<VoxelFrame> = Vec::new();
    let mut r = Reader { bytes, at: 0 };
    while r.at < bytes.len() {
        let (Some(number), Some(length)) = (r.u16(), r.u16()) else {
            let frame = out.last().map_or(0, |f| f.number.wrapping_add(1));
            return (out, Some(SmokeLogError::Truncated { frame }));
        };
        let length = length as usize;
        let Some(payload) = r.take(length) else {
            return (out, Some(SmokeLogError::Truncated { frame: number }));
        };
        let Some((frame, read)) = decode_payload(number, payload) else {
            return (out, Some(SmokeLogError::Truncated { frame: number }));
        };
        if read != length {
            let error = SmokeLogError::Length {
                frame: number,
                length,
                read,
            };
            return (out, Some(error));
        }
        out.push(frame);
    }
    (out, None)
}

/// A smoke's frames over the round: every frame of every snapshot of its log once, by number, each timed by the first
/// snapshot that holds it. A snapshot is read as far as it reads: the log only grows, so a frame that does not decode
/// (a record layout OFAST does not know yet) would otherwise lose every frame after it; the frames before it are kept
/// and [`log_error`] tells what stopped the rest.
pub fn frames(log: &SmokeLog) -> Vec<VoxelFrame> {
    let mut out: Vec<VoxelFrame> = Vec::new();
    for (tick, bytes) in &log.snapshots {
        for mut frame in decode_prefix(bytes).0 {
            if out.last().is_none_or(|last| frame.number > last.number) {
                frame.tick = *tick;
                out.push(frame);
            }
        }
    }
    out
}

/// Why a smoke's log does not read to its end (its last snapshot holds every frame), or `None` when it does.
pub fn log_error(log: &SmokeLog) -> Option<SmokeLogError> {
    log.snapshots
        .last()
        .and_then(|(_, bytes)| decode_prefix(bytes).1)
}

/// The simulation's constants. [`Default`] is OFAST's fit to CS2 (`examples/smoke_fit.rs`).
///
/// The weights of a voxel and its six neighbours add up to 1, so the step moves smoke without making or losing any;
/// only the decay takes it away. The cloud then grows until the decay over its voxels matches what the emitters feed:
/// it fills a volume, spreading further along a corridor than in the open, as CS2's smokes do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokeSettings {
    /// Density an emitter's voxel is set to by the first step (the burst).
    pub burst: f32,
    /// Density an emitter adds to its voxel each later step, until the cloud fades.
    pub feed: f32,
    /// The most the feed fills an emitter's voxel to.
    pub cap: f32,
    /// Density every voxel with smoke loses each step.
    pub decay: f32,
    /// Weight of the voxel's own density.
    pub own: f32,
    /// Weight of each of the four side neighbours (x and y).
    pub side: f32,
    /// Weight of the neighbour above (z + 1): more than below, so the cloud sinks.
    pub above: f32,
    /// Weight of the neighbour below (z − 1).
    pub below: f32,
    /// An HE blast's hole ([`Simulation::carve`], every step while it lasts): its radius, voxels.
    pub hole_radius: f32,
    /// The width of the hole's soft wall, voxels.
    pub hole_edge: f32,
    /// Seconds the hole stays at its full radius after the blast.
    pub hole_hold: f32,
    /// Seconds it then takes to shrink to nothing, letting the cloud back in.
    pub hole_close: f32,
}

impl SmokeSettings {
    /// The hole's radius (voxels) `age` seconds after its blast: [`SmokeSettings::hole_radius`] for
    /// [`SmokeSettings::hole_hold`], then shrinking evenly over [`SmokeSettings::hole_close`]; 0 before the blast and
    /// after.
    pub fn hole_radius_at(&self, age: f32) -> f32 {
        if age < 0.0 {
            0.0
        } else if age <= self.hole_hold {
            self.hole_radius
        } else {
            self.hole_radius
                * (1.0 - (age - self.hole_hold) / self.hole_close.max(f32::EPSILON)).max(0.0)
        }
    }
}

impl Default for SmokeSettings {
    /// The best fit to round 5 of the Aurora vs FURIA de_nuke demo (9 smokes, 340 bricks sent after frame 0;
    /// `examples/smoke_fit.rs`, 2026-10-23): 134 bricks needed within 2 frames of CS2, 36 never (its cloud's thinnest
    /// reaches), 15 that CS2 never needed. CS2's own constants are not known; this is a fit, not a copy.
    fn default() -> Self {
        let side = 0.4 / 6.0;
        Self {
            burst: 3000.0,
            feed: 60.0,
            cap: 250.0,
            decay: 0.5,
            own: 0.6,
            side,
            above: side * 1.25,
            below: side * 0.75,
            hole_radius: 12.0,
            hole_edge: 2.0,
            hole_hold: 0.5,
            hole_close: 4.0,
        }
    }
}

/// No neighbour (outside the grid).
const NONE: u16 = u16::MAX;

/// OFAST's simulation of one smoke's cloud on its 32³ grid, stepped frame by frame through the log.
///
/// Each step: the frame's bricks become known; the first step sets every emitter's voxel to the burst; later steps
/// (until the cloud fades) feed the emitters; then every open voxel, in Morton order and in place (a voxel sees the
/// neighbours already updated this step), takes density from its six neighbours and loses the decay. In place, as
/// CS2's simulation is reported to work, the sweep is lopsided: the −x, −y and −z neighbours are already updated
/// when a voxel reads them, so in one step smoke can travel several voxels towards +x, +y and +z but one towards −,
/// and a cloud in the open leans a little that way. The fit was made with this sweep. Solid voxels
/// hold none, and smoke does not flow into them: what a voxel would give a solid neighbour (or one outside the grid)
/// it keeps. A voxel in a brick the server has not sent so far counts as solid: CS2's own simulation had not needed
/// it yet, so OFAST's cloud never gets ahead of CS2's. The step each brick was first needed (a voxel next to it has
/// smoke) is noted ([`Simulation::needed`]) to measure how close the two simulations are.
#[derive(Debug, Clone)]
pub struct Simulation {
    settings: SmokeSettings,
    /// Density per voxel, indexed by the voxel's Morton code (so a brick's voxels are 64 in a row).
    density: Vec<f32>,
    /// Per voxel: its six neighbours' Morton codes (x − 1, x + 1, y − 1, y + 1, z − 1, z + 1), [`NONE`] outside.
    neighbours: Vec<[u16; 6]>,
    /// Per voxel: in a brick sent so far, and not solid.
    open: Vec<bool>,
    /// The emitters' voxels (Morton codes).
    emitters: Vec<u16>,
    /// Per brick: the first step OFAST's simulation needed it.
    needed: Vec<Option<u16>>,
    /// Steps run so far.
    steps: u16,
    /// The cloud fades (no more feed).
    fading: bool,
}

impl Simulation {
    /// An empty grid.
    pub fn new(settings: SmokeSettings) -> Self {
        let voxels = GRID * GRID * GRID;
        let neighbours = (0..voxels as u32)
            .map(|code| {
                let c = demorton(code, 5);
                let mut out = [NONE; 6];
                for axis in 0..3 {
                    for (side, delta) in [(0, -1i32), (1, 1)] {
                        let v = c[axis] as i32 + delta;
                        if (0..GRID as i32).contains(&v) {
                            let mut n = c;
                            n[axis] = v as usize;
                            out[2 * axis + side] = morton(n) as u16;
                        }
                    }
                }
                out
            })
            .collect();
        Self {
            settings,
            density: vec![0.0; voxels],
            neighbours,
            open: vec![false; voxels],
            emitters: Vec::new(),
            needed: vec![None; BRICKS * BRICKS * BRICKS],
            steps: 0,
            fading: false,
        }
    }

    /// Runs the step of `frame` (the frames in order, from frame 0).
    pub fn step(&mut self, frame: &VoxelFrame) {
        let s = self.settings;
        for brick in &frame.bricks {
            let first = brick.index as usize * 64;
            for (bit, open) in self.open.iter_mut().skip(first).take(64).enumerate() {
                *open = brick.solid >> bit & 1 == 0;
            }
        }
        for e in &frame.emitters {
            let voxel = e.voxel.map(|v| v as usize);
            if voxel.iter().all(|&v| v < GRID) {
                self.emitters.push(morton(voxel) as u16);
            }
        }
        self.fading |= frame.fading;
        let first = self.steps == 0;
        if first || !self.fading {
            for &e in &self.emitters {
                if self.open[e as usize] {
                    let d = &mut self.density[e as usize];
                    *d = if first {
                        s.burst
                    } else {
                        (*d + s.feed).min(s.cap.max(*d))
                    };
                }
            }
        }
        // The weight a voxel pulls from each neighbour, and the weight it keeps instead when that neighbour is
        // closed: what the neighbour would have pulled from it (above and below swap), so no smoke is lost.
        let pull = [s.side, s.side, s.side, s.side, s.below, s.above];
        let keep = [s.side, s.side, s.side, s.side, s.above, s.below];
        for code in 0..self.density.len() {
            let here = self.density[code];
            let mut pulled = s.own * here;
            for (k, &n) in self.neighbours[code].iter().enumerate() {
                pulled += if n != NONE && self.open[n as usize] {
                    pull[k] * self.density[n as usize]
                } else {
                    keep[k] * here
                };
            }
            let value = if self.open[code] {
                (pulled - s.decay).max(0.0)
            } else {
                0.0
            };
            // A voxel with smoke needs to know whether each neighbour is solid: their bricks are needed (this fits
            // CS2's bricks better than needing a brick only once smoke would enter it).
            if value > 0.0 {
                for &n in self.neighbours[code].iter().filter(|&&n| n != NONE) {
                    self.needed[n as usize >> 6].get_or_insert(self.steps);
                }
            }
            self.density[code] = value;
        }
        self.steps += 1;
    }

    /// An HE grenade's hole in the cloud: the smoke within `radius` voxels of `centre` (grid coordinates in voxels from
    /// the grid's corner: a voxel's middle is at its index + 0.5) goes, all of it inside `radius − edge` and less of
    /// it towards `radius`, so the hole's wall is not a staircase of voxels. Run after every step while the hole lasts
    /// ([`SmokeSettings::hole_radius_at`]): once, the emitters inside it would fill it again within a step.
    pub fn carve(&mut self, centre: [f32; 3], radius: f32, edge: f32) {
        for (code, d) in self.density.iter_mut().enumerate() {
            if *d == 0.0 {
                continue;
            }
            let v = demorton(code as u32, 5);
            let distance = (0..3)
                .map(|k| (v[k] as f32 + 0.5 - centre[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            *d *= ((distance - (radius - edge)) / edge.max(f32::EPSILON)).clamp(0.0, 1.0);
        }
    }

    /// Density of the voxel at `voxel` (grid coordinates, 0 – 31 each).
    pub fn density(&self, voxel: [usize; 3]) -> f32 {
        self.density[morton(voxel) as usize]
    }

    /// Density per voxel, x fastest, then y, then z: the grid as a 3D texture lays it out.
    pub fn grid(&self) -> Vec<f32> {
        let mut out = vec![0.0; self.density.len()];
        for (code, &d) in self.density.iter().enumerate() {
            let [x, y, z] = demorton(code as u32, 5);
            out[x + GRID * (y + GRID * z)] = d;
        }
        out
    }

    /// [`Simulation::grid`] for drawing: a closed voxel (solid, or in a brick not sent) under an open one takes its
    /// density. A solid voxel is one that touches a surface, so without this the cloud would float a voxel above the
    /// floor (up to 20 units: CS2 draws its smoke down to the floor); the floor hides what lies under its surface.
    /// Only downwards: a wall, door or catwalk thinner than a voxel sits inside one solid voxel, and smoke spread into
    /// it sideways (or up) would show on its far side. A thin floor seen from below still shows the voxel's lower
    /// part.
    pub fn render_grid(&self) -> Vec<f32> {
        let mut out = vec![0.0; self.density.len()];
        for code in 0..self.density.len() {
            // Neighbours are x − 1, x + 1, y − 1, y + 1, z − 1, z + 1: the one above is the last.
            let above = self.neighbours[code][5];
            let d = if self.open[code] {
                self.density[code]
            } else if above != NONE && self.open[above as usize] {
                self.density[above as usize]
            } else {
                0.0
            };
            let [x, y, z] = demorton(code as u32, 5);
            out[x + GRID * (y + GRID * z)] = d;
        }
        out
    }

    /// Per brick (by index): the first step OFAST's simulation needed it, if it did.
    pub fn needed(&self) -> &[Option<u16>] {
        &self.needed
    }
}

/// The Morton code of `c` (each coordinate below 2^10): the bits of x, y and z interleaved, x lowest.
pub fn morton(c: [usize; 3]) -> u32 {
    let mut out = 0u32;
    for bit in 0..10 {
        for (axis, &v) in c.iter().enumerate() {
            out |= ((v >> bit & 1) as u32) << (3 * bit + axis);
        }
    }
    out
}

/// The coordinates of a Morton code of `bits` bits per axis.
pub fn demorton(code: u32, bits: u32) -> [usize; 3] {
    let mut out = [0usize; 3];
    for bit in 0..bits {
        for (axis, v) in out.iter_mut().enumerate() {
            *v |= ((code >> (3 * bit + axis as u32) & 1) as usize) << bit;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame's bytes: number, length, payload.
    fn frame_bytes(number: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = number.to_le_bytes().to_vec();
        out.extend((payload.len() as u16).to_le_bytes());
        out.extend(payload);
        out
    }

    #[test]
    fn morton_codes_interleave_x_lowest() {
        assert_eq!(morton([1, 0, 0]), 1);
        assert_eq!(morton([0, 1, 0]), 2);
        assert_eq!(morton([0, 0, 1]), 4);
        assert_eq!(morton([2, 0, 0]), 8);
        assert_eq!(morton([7, 7, 7]), 511);
        // Brick 116 of round 5's first smoke: x 4, y 2, z 3.
        assert_eq!(demorton(116, 3), [4, 2, 3]);
        for code in 0..4096 {
            assert_eq!(morton(demorton(code, 4)), code);
        }
    }

    #[test]
    fn holes_open_hold_and_close() {
        let s = SmokeSettings::default();
        assert_eq!(s.hole_radius_at(-0.1), 0.0);
        assert_eq!(s.hole_radius_at(0.0), s.hole_radius);
        assert_eq!(s.hole_radius_at(s.hole_hold), s.hole_radius);
        let halfway = s.hole_radius_at(s.hole_hold + s.hole_close / 2.0);
        assert!((halfway - s.hole_radius / 2.0).abs() < 1e-4, "{halfway}");
        assert_eq!(s.hole_radius_at(s.hole_hold + s.hole_close + 0.1), 0.0);

        // A one-emitter cloud in an open grid (a few voxels across), carved 2.5 voxels around the middle of the voxel
        // before the emitter with a 1-voxel wall: nothing left within 1.5, part of it out to 2.5, all of it beyond.
        let bricks = (0..512u16).map(|index| Brick { index, solid: 0 }).collect();
        let mut sim = Simulation::new(s);
        sim.step(&VoxelFrame {
            emitters: vec![Emitter {
                voxel: [16, 16, 16],
                level: 5,
                hold: 0.0,
            }],
            bricks,
            ..VoxelFrame::default()
        });
        for number in 1..60 {
            sim.step(&VoxelFrame {
                number,
                ..VoxelFrame::default()
            });
        }
        let before = [16, 17, 18].map(|x| sim.density([x, 16, 16]));
        assert!(before.iter().all(|&d| d > 0.0), "{before:?}");
        sim.carve([15.5, 16.5, 16.5], 2.5, 1.0);
        assert_eq!(sim.density([15, 16, 16]), 0.0);
        assert_eq!(sim.density([16, 16, 16]), 0.0);
        // Voxel 17's middle is 2 voxels out: half way through the wall, half its smoke kept.
        assert!((sim.density([17, 16, 16]) - before[1] / 2.0).abs() < 1e-3);
        assert_eq!(sim.density([18, 16, 16]), before[2]);
    }

    #[test]
    fn smoke_stays_out_of_solid_voxels_but_is_drawn_onto_the_floor() {
        // An open grid but for a floor (the top layer, local z 3, of the brick under the emitter) and a wall stub of
        // two voxels next to the emitter: (17, 16, 16) and (17, 16, 17), local (1, 0, 0) and (1, 0, 1) of its brick.
        let floor = morton([4, 4, 3]) as u16;
        let layer: u64 = (0..64u32)
            .filter(|&bit| demorton(bit, 2)[2] == 3)
            .map(|bit| 1u64 << bit)
            .sum();
        let wall = morton([4, 4, 4]) as u16;
        let stub = 1u64 << morton([1, 0, 0]) | 1u64 << morton([1, 0, 1]);
        let bricks = (0..512u16)
            .map(|index| Brick {
                index,
                solid: match index {
                    i if i == floor => layer,
                    i if i == wall => stub,
                    _ => 0,
                },
            })
            .collect();
        let mut sim = Simulation::new(SmokeSettings::default());
        sim.step(&VoxelFrame {
            emitters: vec![Emitter {
                voxel: [16, 16, 16],
                level: 5,
                hold: 0.0,
            }],
            bricks,
            ..VoxelFrame::default()
        });
        sim.step(&VoxelFrame {
            number: 1,
            ..VoxelFrame::default()
        });
        let at = |grid: &[f32], [x, y, z]: [usize; 3]| grid[x + GRID * (y + GRID * z)];
        let (grid, render) = (sim.grid(), sim.render_grid());
        // The floor voxel under the emitter holds no smoke, but is drawn with the smoke above it.
        assert_eq!(at(&grid, [16, 16, 15]), 0.0);
        assert!(at(&grid, [16, 16, 16]) > 0.0);
        assert_eq!(at(&render, [16, 16, 15]), at(&grid, [16, 16, 16]));
        // Not sideways: the wall stub's lower voxel, next to the emitter's dense voxel with the stub's upper voxel
        // (solid too) above it, stays empty.
        assert!(at(&grid, [16, 16, 16]) > 0.0);
        assert_eq!(at(&render, [17, 16, 16]), 0.0);
        // Open voxels are drawn as they are.
        assert_eq!(at(&render, [16, 16, 17]), at(&grid, [16, 16, 17]));
        // No smoke flowed through the floor: the voxel under it is empty.
        assert_eq!(at(&grid, [16, 16, 14]), 0.0);
    }

    #[test]
    fn frames_decode_and_chain() {
        // Frame 0: not fading, one emitter and one brick; frame 1: a heartbeat; frame 2: fading, one hole.
        let mut first = vec![0u8, 3, 1, 15, 16, 17, 5];
        first.extend(0.0f32.to_le_bytes());
        first.extend(1u16.to_le_bytes());
        first.extend(116u16.to_le_bytes());
        first.extend((1u64 << 63 | 1).to_le_bytes());
        first.push(0);
        let mut log = frame_bytes(0, &first);
        log.extend(frame_bytes(1, &[0, 0, 0]));
        let mut third = vec![1u8, 0, 1];
        third.extend([9u8; 20]);
        log.extend(frame_bytes(2, &third));

        let frames = decode_log(&log).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames[0].emitters,
            vec![Emitter {
                voxel: [15, 16, 17],
                level: 5,
                hold: 0.0
            }]
        );
        let brick = frames[0].bricks[0];
        assert_eq!(brick.coordinates(), [4, 2, 3]);
        assert!(
            brick.is_solid([0, 0, 0]) && brick.is_solid([3, 3, 3]) && !brick.is_solid([1, 0, 0])
        );
        assert_eq!(
            (frames[1].number, frames[1].fading, frames[1].bricks.len()),
            (1, false, 0)
        );
        assert!(frames[2].fading);
        assert_eq!(frames[2].holes, vec![[9u8; 20]]);

        // Cut short, or a length that does not match the records.
        assert_eq!(
            decode_log(&log[..log.len() - 1]),
            Err(SmokeLogError::Truncated { frame: 2 })
        );
        let long = frame_bytes(0, &[0, 0, 0, 0]);
        assert_eq!(
            decode_log(&long),
            Err(SmokeLogError::Length {
                frame: 0,
                length: 4,
                read: 3
            })
        );

        // Snapshots of the growing log: each frame once, timed by the first snapshot holding it.
        let smoke = SmokeLog {
            entity: 1,
            serial: 1,
            snapshots: vec![
                (100, log[..frame_bytes(0, &first).len()].to_vec()),
                (107, log[..log.len() - 4 - third.len()].to_vec()),
                (114, log.clone()),
            ],
        };
        let timed = super::frames(&smoke);
        let ticks: Vec<(u16, u32)> = timed.iter().map(|f| (f.number, f.tick)).collect();
        assert_eq!(ticks, vec![(0, 100), (1, 107), (2, 114)]);
        assert_eq!(log_error(&smoke), None);

        // A frame that does not read (a hole record longer than OFAST knows) stops the log there: the frames before it
        // are kept, and the error says where it stopped.
        let mut broken = log.clone();
        broken.extend(frame_bytes(3, &[0, 0, 1, 7, 7]));
        let smoke = SmokeLog {
            entity: 1,
            serial: 1,
            snapshots: vec![(100, log.clone()), (107, broken)],
        };
        assert_eq!(super::frames(&smoke).len(), 3);
        assert_eq!(
            log_error(&smoke),
            Some(SmokeLogError::Truncated { frame: 3 })
        );
    }
}
