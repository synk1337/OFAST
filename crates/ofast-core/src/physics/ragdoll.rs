//! A ragdoll: an agent's rigid parts and conical joints (its `PHYS` block), simulated against the world's
//! collision (a map's `world_physics`).
//!
//! The solver is extended position-based dynamics (Müller, Macklin, Chentanez, Jeschke, Kim, "Detailed Rigid Body
//! Simulation with Extended Position Based Dynamics", 2020), with [`SUBSTEPS`] substeps per frame and one
//! iteration each:
//!
//! 1. every body moves under gravity ([`GRAVITY`], CS2's `sv_gravity`) with its velocity;
//! 2. positions are corrected: each joint brings its two frames' origins together and keeps the child's twist axis
//!    (the frames' z, [`TWIST_AXIS`]) within the swing cone (`m_SwingLimit`'s maximum; a joint without a swing
//!    limit is a hinge: the knees) and its twist within `m_TwistLimit`; each
//!    contact pushes a body out of the world (or out of another part not joined to it) along the contact normal,
//!    and static friction holds a contact that would slide less than its push allows;
//! 3. velocities are taken back from the moves, then every contact (with the world or between two parts) loses its
//!    speed along the normal (no bounce) and sliding ones lose speed to dynamic friction, and the parts' damping
//!    applies (`m_flLinearDamping`,
//!    `m_flAngularDamping`, as exponential decay rates, plus their drag: [`DRAG_AS_DAMPING`]).
//!
//! Before the first step the joints and contacts are solved for positions only ([`SETTLE_PASSES`]): the last living
//! pose may break a joint's limits or dip into the world, and fixing that within a substep would turn into speed.
//! Two parts that overlap in that pose (an arm across the chest) do not collide until they have come apart.
//!
//! Bodies: mass from the part (`m_flMass`), centre of mass and inertia from its shapes (capsules as solid
//! cylinders with rounded ends, spheres, hulls as their bounding boxes) times `m_flInertiaScale` (7 on CS2's
//! agents: limbs turn slowly). Contacts: each capsule is sampled as spheres along its segment, tested against the
//! world's solid hulls (the signed distance to a convex hull is taken as the largest distance to one of its face
//! planes: exact over faces, a little generous near edges) and mesh triangles (the closest point), found through a
//! uniform grid ([`WorldCollision`]; "solid": the solid world, grates and glass, not the clips that stop only
//! players or grenades).
//!
//! What it does not do (first version): joint friction (`m_flFriction`), collision with props that move and with
//! other bodies, surface materials (one friction for every surface).

use std::collections::HashMap;

use super::{PhysicsData, ShapeKind};
use crate::model::{Affine, affine, compose, normalize_quat, quat_matrix, quat_mul};

/// Gravity, inches per second²: CS2's `sv_gravity` (800).
pub const GRAVITY: f32 = 800.0;
/// Substeps per [`Ragdoll::step`].
pub const SUBSTEPS: usize = 8;
/// Static friction between a body and the world.
pub const STATIC_FRICTION: f32 = 0.9;
/// Dynamic (sliding) friction between a body and the world.
pub const DYNAMIC_FRICTION: f32 = 0.7;
/// A ragdoll counts as still (and stops being simulated) once no point of any part has drifted more than this
/// (inches) over [`REST_FRAMES`] frames: half an inch (13 mm) in half a second. A body that has come down keeps a
/// creep of a few tenths of an inch per second (the substeps' linearised pushes, one iteration each; 16 substeps
/// made it worse); freezing it then is what engines' sleeping does. A speed threshold did not do: a body on a
/// ramp whose arms shimmered at 2 – 7 in/s without going anywhere stayed above 5 in/s for 19 s. On the 46 deaths of
/// the test demos' first three rounds, bodies lie still after 2.3 – 10.5 s.
pub const REST_DRIFT: f32 = 0.5;
/// Frames [`REST_DRIFT`] is measured over: half a second at the exports' 64 frames per second.
pub const REST_FRAMES: usize = 32;
/// Edge of the world collision grid's cells, inches.
const CELL: f32 = 96.0;
/// A shape covering more grid cells than this (a whole floor of a large map is about 2,000) is kept out of the grid
/// and tested by every query instead: no shape, however large (or broken), fills memory with cells.
const MAX_CELLS: u64 = 4096;
/// Positions-only passes over the joints and contacts before the first step: the last living pose can break the
/// ragdoll's limits (a crouch beyond the hip's range) or overlap the world (a foot inside a stair), and the solver
/// would turn each such correction into speed (1 inch over a 1/512 s substep is 500 in/s).
pub const SETTLE_PASSES: usize = 16;
/// How much of a part's drag (`m_flLinearDrag`, `m_flAngularDrag`: 1 on CS2's agents) counts as extra damping (an
/// exponential decay rate per second, like `m_flLinearDamping`). Rubikon's drag formula is not public. Checked on
/// one body: KSCERATO, shot running at 248 in/s on de_nuke (Aurora vs FURIA, round 5, tick 34415), lies in CS2
/// about 2.4 – 2.8 m from where he died; without drag OFAST's body went 3.7 m (most of it in the air, as it tips
/// over), with drag as damping 2.8 m, and 2.5 m once friction and contact speeds were right.
pub const DRAG_AS_DAMPING: f32 = 1.0;
/// The joint frames' twist axis (0 = x, 1 = y, 2 = z). On CS2's agents it is z: in CS2's own animation poses the
/// knees (twist-only joints, 0° … 110°) bend about the frames' z with 0.0° of swing, the hips' and spine's bends are
/// a twist about z within their ranges (−73° … 36°, −5° … 33°), and the signs match; about x or y the knees' bend
/// reads as swing with no twist (`examples/ragdoll_limits.rs`).
pub const TWIST_AXIS: usize = 2;

type V3 = [f32; 3];
type Q = [f32; 4];
type M3 = [[f32; 3]; 3];

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
fn normalized(a: V3) -> Option<V3> {
    let l = length(a);
    (l > 1e-9).then(|| scale(a, 1.0 / l))
}
fn mat_vec(m: &M3, v: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}
fn transpose(m: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}
fn mat_mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}
fn rotate(q: Q, v: V3) -> V3 {
    mat_vec(&quat_matrix(q), v)
}
/// The inverse of a symmetric positive 3 × 3 matrix (zero when singular).
fn invert(m: &M3) -> M3 {
    let c: M3 = [
        [
            m[1][1] * m[2][2] - m[1][2] * m[2][1],
            m[0][2] * m[2][1] - m[0][1] * m[2][2],
            m[0][1] * m[1][2] - m[0][2] * m[1][1],
        ],
        [
            m[1][2] * m[2][0] - m[1][0] * m[2][2],
            m[0][0] * m[2][2] - m[0][2] * m[2][0],
            m[0][2] * m[1][0] - m[0][0] * m[1][2],
        ],
        [
            m[1][0] * m[2][1] - m[1][1] * m[2][0],
            m[0][1] * m[2][0] - m[0][0] * m[2][1],
            m[0][0] * m[1][1] - m[0][1] * m[1][0],
        ],
    ];
    let det = m[0][0] * c[0][0] + m[0][1] * c[1][0] + m[0][2] * c[2][0];
    if det.abs() < 1e-12 {
        return [[0.0; 3]; 3];
    }
    c.map(|row| row.map(|x| x / det))
}
/// `q` turned by the small rotation vector `theta` (radians): `q + ½ [θ, 0] ⊗ q`, normalised.
fn spin(q: Q, theta: V3) -> Q {
    let d = quat_mul([theta[0], theta[1], theta[2], 0.0], q);
    normalize_quat(std::array::from_fn(|k| q[k] + 0.5 * d[k]))
}
fn rotation_matrix_to_quat(m: &M3) -> Q {
    let a: Affine = std::array::from_fn(|i| [m[i][0], m[i][1], m[i][2], 0.0]);
    crate::anim::matrix_to_quat(&a)
}

/// The world's collision for falling bodies: the hulls and mesh triangles of a `world_physics` model that stop them,
/// in world space, indexed by a uniform grid.
#[derive(Debug, Clone, Default)]
pub struct WorldCollision {
    hulls: Vec<WorldHull>,
    triangles: Vec<WorldTriangle>,
    grid: HashMap<[i32; 3], Vec<Item>>,
    /// Shapes too large for the grid ([`MAX_CELLS`]), in every query.
    large: Vec<Item>,
}

#[derive(Debug, Clone)]
struct WorldHull {
    planes: Vec<(V3, f32)>,
    min: V3,
    max: V3,
    /// The physics shape it came from ([`RayHit::shape`]).
    shape: u32,
    /// Its surface property ([`RayHit::surface`]).
    surface: u32,
}

#[derive(Debug, Clone)]
struct WorldTriangle {
    corners: [V3; 3],
    min: V3,
    max: V3,
    /// The physics shape it came from ([`RayHit::shape`]).
    shape: u32,
    /// Its surface property: its own in a mesh with one per triangle, else its shape's ([`RayHit::surface`]).
    surface: u32,
}

/// Where a ray met the world ([`WorldCollision::ray_hit`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Distance along the ray (inches).
    pub distance: f32,
    /// The surface's unit normal, facing back at the ray.
    pub normal: V3,
    /// The physics shape met: its index among all the shapes of the world's parts, in order (part by part)
    /// ([`WorldCollision::from_physics_where`]); [`NO_SHAPE`] for shapes added by hand.
    pub shape: u32,
    /// The surface property met: an index into the world's [`super::PhysicsData::surfaces`] (a mesh triangle's own,
    /// else its shape's); [`NO_SHAPE`] for shapes added by hand.
    pub surface: u32,
}

/// [`RayHit::shape`] of a hull or triangle added by hand ([`WorldCollision::add_hull`], `add_triangle`, `add_box`).
pub const NO_SHAPE: u32 = u32::MAX;

/// `true` when a sphere at `center` with `radius` cannot reach the box `min`–`max`.
fn outside(center: V3, radius: f32, min: V3, max: V3) -> bool {
    (0..3).any(|k| center[k] < min[k] - radius || center[k] > max[k] + radius)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    Hull(u32),
    Triangle(u32),
}

fn cell_of(p: V3) -> [i32; 3] {
    p.map(|c| (c / CELL).floor() as i32)
}

/// The number of grid cells from `lo` to `hi` (inclusive; 0 when empty).
fn cells_between(lo: [i32; 3], hi: [i32; 3]) -> u64 {
    (0..3)
        .map(|k| (i64::from(hi[k]) - i64::from(lo[k]) + 1).max(0) as u64)
        .fold(1u64, u64::saturating_mul)
}

impl WorldCollision {
    /// The shapes of a world's physics (`maps/<map>/world_physics.vmdl`) that stop a falling body
    /// ([`super::CollisionAttribute::stops_bodies`]: solid world, grates, glass; not the player, NPC and grenade
    /// clips).
    pub fn from_physics(world: &PhysicsData) -> Self {
        Self::from_physics_where(world, super::CollisionAttribute::stops_bodies)
    }

    /// The shapes of a world's physics whose collision attribute `keep` accepts (bullets:
    /// [`super::CollisionAttribute::stops_bullets`]). A shape without an attribute is left out.
    pub fn from_physics_where(
        world: &PhysicsData,
        keep: impl Fn(&super::CollisionAttribute) -> bool,
    ) -> Self {
        let mut out = Self::default();
        for (index, shape) in world.parts.iter().flat_map(|p| &p.shapes).enumerate() {
            if !world.attribute(shape).is_some_and(&keep) {
                continue;
            }
            let index = index as u32;
            let surface = shape.surface as u32;
            match &shape.kind {
                ShapeKind::Hull {
                    planes, min, max, ..
                } if !planes.is_empty() => {
                    out.push_hull(planes.clone(), *min, *max, index, surface)
                }
                ShapeKind::Mesh {
                    vertices,
                    triangles,
                    materials,
                    ..
                } => {
                    for (k, t) in triangles.iter().enumerate() {
                        let own = materials.get(k).copied().unwrap_or(surface);
                        out.push_triangle(t.map(|i| vertices[i as usize]), index, own);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Adds a convex hull (outward planes: inside when `normal · p − offset < 0` for all).
    pub fn add_hull(&mut self, planes: Vec<(V3, f32)>, min: V3, max: V3) {
        self.push_hull(planes, min, max, NO_SHAPE, NO_SHAPE);
    }

    fn push_hull(&mut self, planes: Vec<(V3, f32)>, min: V3, max: V3, shape: u32, surface: u32) {
        let id = self.hulls.len() as u32;
        self.hulls.push(WorldHull {
            planes,
            min,
            max,
            shape,
            surface,
        });
        self.insert(Item::Hull(id), min, max);
    }

    /// Adds a triangle. One without area (two corners on the same point, or all three in a line) is left out: its
    /// edges belong to its neighbours, and its closest point would divide by zero.
    pub fn add_triangle(&mut self, corners: [V3; 3]) {
        self.push_triangle(corners, NO_SHAPE, NO_SHAPE);
    }

    fn push_triangle(&mut self, corners: [V3; 3], shape: u32, surface: u32) {
        let [a, b, c] = corners;
        let area = length(cross(sub(b, a), sub(c, a)));
        if area.is_nan() || area <= 1e-6 {
            return;
        }
        let id = self.triangles.len() as u32;
        let min = std::array::from_fn(|k| corners.iter().map(|c| c[k]).fold(f32::MAX, f32::min));
        let max = std::array::from_fn(|k| corners.iter().map(|c| c[k]).fold(f32::MIN, f32::max));
        self.triangles.push(WorldTriangle {
            corners,
            min,
            max,
            shape,
            surface,
        });
        self.insert(Item::Triangle(id), min, max);
    }

    /// An axis-aligned box, solid (six planes): handy for tests and simple floors.
    pub fn add_box(&mut self, min: V3, max: V3) {
        let planes = vec![
            ([1.0, 0.0, 0.0], max[0]),
            ([-1.0, 0.0, 0.0], -min[0]),
            ([0.0, 1.0, 0.0], max[1]),
            ([0.0, -1.0, 0.0], -min[1]),
            ([0.0, 0.0, 1.0], max[2]),
            ([0.0, 0.0, -1.0], -min[2]),
        ];
        self.add_hull(planes, min, max);
    }

    fn insert(&mut self, item: Item, min: V3, max: V3) {
        if !min.iter().chain(&max).all(|c| c.is_finite()) {
            // Broken bounds: nothing can be near it.
            return;
        }
        let (lo, hi) = (cell_of(min), cell_of(max));
        if cells_between(lo, hi) > MAX_CELLS {
            self.large.push(item);
            return;
        }
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    self.grid.entry([x, y, z]).or_default().push(item);
                }
            }
        }
    }

    /// Number of hulls and triangles.
    pub fn counts(&self) -> (usize, usize) {
        (self.hulls.len(), self.triangles.len())
    }

    /// The deepest overlap of a sphere with the world: (depth, outward normal), as the solver sees it.
    pub fn sphere_contact(&self, center: V3, radius: f32) -> Option<(f32, V3)> {
        let mut items = Vec::new();
        self.query(
            sub(center, [radius; 3]),
            add(center, [radius; 3]),
            &mut items,
        );
        items
            .into_iter()
            .filter_map(|item| self.contact(item, center, radius))
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Where a ray from `from` along the unit `direction` first meets the world within `reach` inches (a bullet's
    /// path). The grid is walked cell by cell along the ray (Amanatides and Woo, "A Fast Voxel Traversal Algorithm
    /// for Ray Tracing", 1987), each shape tested once: a hull by clipping the ray against its planes, a triangle
    /// both ways round (Möller and Trumbore, "Fast, Minimum Storage Ray/Triangle Intersection", 1997). A ray that
    /// starts inside a hull is not stopped by it.
    pub fn ray_hit(&self, from: V3, direction: V3, reach: f32) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        let mut tested = std::collections::HashSet::new();
        let mut test = |item: Item, best: &mut Option<RayHit>| {
            if tested.insert(item)
                && let Some(hit) = self.ray_item(item, from, direction, reach)
                && best.is_none_or(|b| hit.distance < b.distance)
            {
                *best = Some(hit);
            }
        };
        for &item in &self.large {
            test(item, &mut best);
        }
        let mut cell = cell_of(from);
        let step: [i32; 3] = direction.map(|d| {
            if d > 0.0 {
                1
            } else if d < 0.0 {
                -1
            } else {
                0
            }
        });
        // Distance along the ray to the next cell boundary on each axis, and between boundaries.
        let mut next: V3 = std::array::from_fn(|k| match step[k] {
            1 => ((cell[k] + 1) as f32 * CELL - from[k]) / direction[k],
            -1 => (cell[k] as f32 * CELL - from[k]) / direction[k],
            _ => f32::INFINITY,
        });
        let delta: V3 = direction.map(|d| {
            if d == 0.0 {
                f32::INFINITY
            } else {
                CELL / d.abs()
            }
        });
        // A ray across the whole map passes a few hundred cells; the bound only guards against a broken direction.
        for _ in 0..100_000 {
            if let Some(items) = self.grid.get(&cell) {
                for &item in items {
                    test(item, &mut best);
                }
            }
            let axis = (0..3)
                .min_by(|&a, &b| next[a].total_cmp(&next[b]))
                .unwrap_or(0);
            let leave = next[axis];
            // Every hit nearer than where the ray leaves this cell lies in a cell already walked.
            if best.is_some_and(|b| b.distance <= leave) || leave > reach || !leave.is_finite() {
                break;
            }
            cell[axis] += step[axis];
            next[axis] += delta[axis];
        }
        best
    }

    /// Where a ray meets one shape ([`WorldCollision::ray_hit`]).
    fn ray_item(&self, item: Item, from: V3, direction: V3, reach: f32) -> Option<RayHit> {
        match item {
            Item::Hull(h) => {
                let hull = &self.hulls[h as usize];
                let (mut enter, mut leave, mut normal) = (0.0f32, reach, None);
                for &(n, offset) in &hull.planes {
                    let towards = dot(n, direction);
                    // Positive outside the plane.
                    let outside = dot(n, from) - offset;
                    if towards.abs() < 1e-9 {
                        if outside > 0.0 {
                            return None;
                        }
                        continue;
                    }
                    let t = -outside / towards;
                    if towards < 0.0 {
                        if t > enter {
                            enter = t;
                            normal = Some(n);
                        }
                    } else if t < leave {
                        leave = t;
                    }
                    if enter > leave {
                        return None;
                    }
                }
                // No entering plane ahead: the ray starts inside the hull (or misses it).
                normal.map(|normal| RayHit {
                    distance: enter,
                    normal,
                    shape: hull.shape,
                    surface: hull.surface,
                })
            }
            Item::Triangle(t) => {
                let triangle = &self.triangles[t as usize];
                let [a, b, c] = triangle.corners;
                let (e1, e2) = (sub(b, a), sub(c, a));
                let p = cross(direction, e2);
                let det = dot(e1, p);
                if det.abs() < 1e-9 {
                    return None;
                }
                let s = sub(from, a);
                let u = dot(s, p) / det;
                let q = cross(s, e1);
                let v = dot(direction, q) / det;
                let distance = dot(e2, q) / det;
                if u < 0.0 || v < 0.0 || u + v > 1.0 || distance <= 1e-4 || distance > reach {
                    return None;
                }
                let n = normalized(cross(e1, e2))?;
                Some(RayHit {
                    distance,
                    normal: if dot(n, direction) > 0.0 {
                        scale(n, -1.0)
                    } else {
                        n
                    },
                    shape: triangle.shape,
                    surface: triangle.surface,
                })
            }
        }
    }

    /// The shapes whose cells meet the box `min`–`max` (and the shapes too large for the grid), without repeats.
    fn query(&self, min: V3, max: V3, out: &mut Vec<Item>) {
        out.clear();
        out.extend(&self.large);
        let (lo, hi) = (cell_of(min), cell_of(max));
        if cells_between(lo, hi) > self.grid.len() as u64 {
            // A box larger than the grid itself (a body flung very fast): walk the filled cells instead.
            let inside = |c: &[i32; 3]| (0..3).all(|k| lo[k] <= c[k] && c[k] <= hi[k]);
            for (_, items) in self.grid.iter().filter(|(c, _)| inside(c)) {
                out.extend(items);
            }
        } else {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        if let Some(items) = self.grid.get(&[x, y, z]) {
                            out.extend(items);
                        }
                    }
                }
            }
        }
        out.sort_unstable_by_key(|i| match i {
            Item::Hull(h) => (0, *h),
            Item::Triangle(t) => (1, *t),
        });
        out.dedup();
    }

    /// Where a sphere at `center` with `radius` overlaps `item`: (depth, outward normal).
    fn contact(&self, item: Item, center: V3, radius: f32) -> Option<(f32, V3)> {
        match item {
            Item::Hull(h) => {
                let hull = &self.hulls[h as usize];
                if outside(center, radius, hull.min, hull.max) {
                    return None;
                }
                let (distance, normal) = hull
                    .planes
                    .iter()
                    .map(|&(n, d)| (dot(n, center) - d, n))
                    .fold(
                        (f32::MIN, [0.0, 0.0, 1.0]),
                        |a, b| if b.0 > a.0 { b } else { a },
                    );
                (distance < radius).then_some((radius - distance, normal))
            }
            Item::Triangle(t) => {
                let triangle = &self.triangles[t as usize];
                if outside(center, radius, triangle.min, triangle.max) {
                    return None;
                }
                let [a, b, c] = triangle.corners;
                let p = closest_on_triangle(center, a, b, c);
                let offset = sub(center, p);
                let distance = length(offset);
                if distance >= radius {
                    return None;
                }
                let normal = normalized(offset)
                    .or_else(|| normalized(cross(sub(b, a), sub(c, a))))
                    .unwrap_or([0.0, 0.0, 1.0]);
                Some((radius - distance, normal))
            }
        }
    }
}

/// The point of triangle `a b c` nearest `p` (Ericson, "Real-Time Collision Detection", 5.1.5).
fn closest_on_triangle(p: V3, a: V3, b: V3, c: V3) -> V3 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

/// The closest points of segments `p1 q1` and `p2 q2` (Ericson 5.1.9): their parameters along each.
fn closest_segments(p1: V3, q1: V3, p2: V3, q2: V3) -> (f32, f32) {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    if a <= 1e-9 && e <= 1e-9 {
        return (0.0, 0.0);
    }
    if a <= 1e-9 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = dot(d1, r);
    if e <= 1e-9 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = dot(d1, d2);
    let denom = a * e - b * b;
    let mut s = if denom != 0.0 {
        ((b * f - c * e) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (s, t)
}

/// One rigid part in simulation. Positions are of its centre of mass; the part's own frame is the centre of mass
/// frame moved by `-com` (same rotation).
#[derive(Debug, Clone)]
struct Body {
    x: V3,
    q: Q,
    v: V3,
    w: V3,
    x_prev: V3,
    q_prev: Q,
    inverse_mass: f32,
    /// Inverse inertia in the body's frame.
    inverse_inertia: M3,
    /// Centre of mass in the part's frame.
    com: V3,
    /// Collision spheres (centre in the centre-of-mass frame, radius).
    spheres: Vec<(V3, f32)>,
    /// Capsules for collisions between parts (ends in the centre-of-mass frame, radius).
    capsules: Vec<(V3, V3, f32)>,
    linear_damping: f32,
    angular_damping: f32,
    /// Farthest a collision sphere reaches from the centre of mass (for the rest test).
    reach: f32,
}

impl Body {
    fn inverse_inertia_world(&self) -> M3 {
        let r = quat_matrix(self.q);
        mat_mul(&mat_mul(&r, &self.inverse_inertia), &transpose(&r))
    }

    /// How hard the body resists a push along `n` at `r` (from the centre of mass, world).
    fn weight(&self, r: V3, n: V3) -> f32 {
        let rn = cross(r, n);
        self.inverse_mass + dot(rn, mat_vec(&self.inverse_inertia_world(), rn))
    }

    /// Applies the positional impulse `p` at `r` (from the centre of mass).
    fn push(&mut self, p: V3, r: V3) {
        self.x = add(self.x, scale(p, self.inverse_mass));
        let theta = mat_vec(&self.inverse_inertia_world(), cross(r, p));
        self.q = spin(self.q, theta);
    }

    /// The velocity of the point at `r` (from the centre of mass, world).
    fn point_velocity(&self, r: V3) -> V3 {
        add(self.v, cross(self.w, r))
    }

    /// Applies the impulse `p` (mass × inches per second) at `r` (from the centre of mass) to the velocities.
    fn impulse(&mut self, p: V3, r: V3) {
        self.v = add(self.v, scale(p, self.inverse_mass));
        self.w = add(self.w, mat_vec(&self.inverse_inertia_world(), cross(r, p)));
    }

    /// Applies the angular impulse `p` (axis × amount).
    fn twist(&mut self, p: V3) {
        let theta = mat_vec(&self.inverse_inertia_world(), p);
        self.q = spin(self.q, theta);
    }

    /// The part's frame (world).
    fn frame(&self) -> Affine {
        let origin = sub(self.x, rotate(self.q, self.com));
        affine(self.q, origin)
    }

    /// A point of the centre-of-mass frame, in the world.
    fn point(&self, local: V3) -> V3 {
        add(self.x, rotate(self.q, local))
    }
}

/// A joint in simulation, its frames moved into each body's centre-of-mass frame.
#[derive(Debug, Clone)]
struct JointState {
    body1: usize,
    body2: usize,
    /// Joint origin in each body's centre-of-mass frame.
    anchor1: V3,
    anchor2: V3,
    /// Joint frame rotation in each body.
    rotation1: M3,
    rotation2: M3,
    /// Swing cone half-angle (0 when the joint has none: locked).
    swing: f32,
    twist: Option<(f32, f32)>,
}

/// A contact found in this substep, for the velocity pass.
#[derive(Debug, Clone, Copy)]
struct Contact {
    body: usize,
    /// From the centre of mass to the contact point, world.
    r: V3,
    normal: V3,
    /// Push applied along the normal this substep.
    lambda: f32,
}

/// A contact between two parts found in this substep, for the velocity pass.
#[derive(Debug, Clone, Copy)]
struct PairContact {
    a: usize,
    b: usize,
    /// From `a` towards `b`.
    normal: V3,
    /// From each centre of mass to its contact point, world.
    r1: V3,
    r2: V3,
    /// Push applied along the normal this substep.
    lambda: f32,
}

/// Two parts that collide with each other (not joined).
#[derive(Debug, Clone, Copy)]
struct Pair {
    a: usize,
    b: usize,
    /// `false` while the two still overlap as they started (an arm across the chest in the living pose): pushing
    /// them apart would fling the arm. They collide once they have come apart.
    active: bool,
}

/// A ragdoll in simulation.
#[derive(Debug, Clone)]
pub struct Ragdoll {
    bodies: Vec<Body>,
    joints: Vec<JointState>,
    pairs: Vec<Pair>,
    /// The positions-only passes ([`SETTLE_PASSES`]) are done.
    settled: bool,
    /// Frames since `anchor` with no part drifting more than [`REST_DRIFT`] from it.
    still_frames: usize,
    /// Each part's centre of mass and rotation when the current still stretch began.
    anchor: Vec<(V3, Q)>,
    /// The fastest point of any part after the last step, inches per second.
    speed: f32,
    candidates: Vec<Item>,
}

impl Ragdoll {
    /// The ragdoll of `data` (an agent's `PHYS` block), its parts at the world `frames` (one per part; a missing
    /// one at the origin) and moving with `velocities`: per part, how its frame moves (the frame origin's velocity,
    /// inches per second, and the frame's spin, radians per second).
    pub fn new(data: &PhysicsData, frames: &[Affine], velocities: &[(V3, V3)]) -> Self {
        let bodies: Vec<Body> = data
            .parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let (com, inertia, volume_spheres, capsules) =
                    shape_properties(&part.shapes, part.mass);
                let inertia = inertia.map(|row| row.map(|c| c * part.inertia_scale.max(1e-3)));
                let frame = frames
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]));
                let q = rotation_matrix_to_quat(&std::array::from_fn(|r| {
                    [frame[r][0], frame[r][1], frame[r][2]]
                }));
                let origin = [frame[0][3], frame[1][3], frame[2][3]];
                let x = add(origin, rotate(q, com));
                // The centre of mass moves with the origin plus the spin about it.
                let (v_origin, w) = velocities.get(i).copied().unwrap_or_default();
                let v = add(v_origin, cross(w, rotate(q, com)));
                let reach = volume_spheres
                    .iter()
                    .map(|(c, r)| length(*c) + r)
                    .fold(0.0, f32::max);
                Body {
                    x,
                    q,
                    v,
                    w,
                    x_prev: x,
                    q_prev: q,
                    inverse_mass: if part.mass > 0.0 {
                        1.0 / part.mass
                    } else {
                        0.0
                    },
                    inverse_inertia: invert(&inertia),
                    com,
                    spheres: volume_spheres,
                    capsules,
                    linear_damping: part.linear_damping + DRAG_AS_DAMPING * part.linear_drag,
                    angular_damping: part.angular_damping + DRAG_AS_DAMPING * part.angular_drag,
                    reach,
                }
            })
            .collect();
        let rotation = |m: &Affine| -> M3 { std::array::from_fn(|r| [m[r][0], m[r][1], m[r][2]]) };
        let joints: Vec<JointState> = data
            .joints
            .iter()
            .map(|j| JointState {
                body1: j.body1,
                body2: j.body2,
                anchor1: sub(
                    [j.frame1[0][3], j.frame1[1][3], j.frame1[2][3]],
                    bodies[j.body1].com,
                ),
                anchor2: sub(
                    [j.frame2[0][3], j.frame2[1][3], j.frame2[2][3]],
                    bodies[j.body2].com,
                ),
                rotation1: rotation(&j.frame1),
                rotation2: rotation(&j.frame2),
                swing: j.swing.map_or(0.0, |(_, max)| max),
                twist: j.twist,
            })
            .collect();
        let joined = |a: usize, b: usize| {
            data.joints.iter().any(|j| {
                !j.collide && ((j.body1, j.body2) == (a, b) || (j.body1, j.body2) == (b, a))
            })
        };
        let mut pairs = Vec::new();
        for a in 0..bodies.len() {
            for b in a + 1..bodies.len() {
                if !joined(a, b) {
                    let active = overlaps(&bodies[a], &bodies[b]).is_empty();
                    pairs.push(Pair { a, b, active });
                }
            }
        }
        Self {
            bodies,
            joints,
            pairs,
            settled: false,
            still_frames: 0,
            anchor: Vec::new(),
            speed: f32::MAX,
            candidates: Vec::new(),
        }
    }

    /// Number of parts.
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// `true` without parts.
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Adds the impulse `impulse` (mass × inches per second) at the world point `point` to part `part`.
    pub fn apply_impulse(&mut self, part: usize, point: V3, impulse: V3) {
        let Some(body) = self.bodies.get_mut(part) else {
            return;
        };
        let r = sub(point, body.x);
        body.impulse(impulse, r);
    }

    /// The parts' frames (world), in part order.
    pub fn frames(&self) -> Vec<Affine> {
        self.bodies.iter().map(Body::frame).collect()
    }

    /// The parts' centre-of-mass velocities, inches per second.
    pub fn velocities(&self) -> Vec<V3> {
        self.bodies.iter().map(|b| b.v).collect()
    }

    /// Still: for [`REST_FRAMES`] frames, no point of any part has drifted more than [`REST_DRIFT`].
    pub fn at_rest(&self) -> bool {
        self.still_frames >= REST_FRAMES
    }

    /// The fastest point of any part after the last step (centre of mass speed plus spin times reach), inches per
    /// second.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Advances the simulation by `dt` seconds (one frame) against `world`.
    pub fn step(&mut self, dt: f32, world: &WorldCollision) {
        if self.bodies.is_empty() || dt <= 0.0 {
            return;
        }
        // The world shapes near the ragdoll for this frame.
        let margin = 24.0 + self.bodies.iter().map(|b| length(b.v)).fold(0.0, f32::max) * dt;
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for b in &self.bodies {
            for k in 0..3 {
                lo[k] = lo[k].min(b.x[k] - b.reach - margin);
                hi[k] = hi[k].max(b.x[k] + b.reach + margin);
            }
        }
        let mut candidates = std::mem::take(&mut self.candidates);
        world.query(lo, hi, &mut candidates);

        let mut contacts: Vec<Contact> = Vec::new();
        let mut pair_contacts: Vec<PairContact> = Vec::new();
        if !self.settled {
            // Into the limits and out of the world without gaining speed: the moves change positions only.
            for _ in 0..SETTLE_PASSES {
                self.solve_joints();
                self.solve_world(world, &candidates, &mut contacts, false);
                self.solve_self(&mut pair_contacts);
            }
            contacts.clear();
            pair_contacts.clear();
            self.settled = true;
        }
        let h = dt / SUBSTEPS as f32;
        for _ in 0..SUBSTEPS {
            for b in &mut self.bodies {
                b.x_prev = b.x;
                b.q_prev = b.q;
                if b.inverse_mass > 0.0 {
                    b.v[2] -= GRAVITY * h;
                }
                b.x = add(b.x, scale(b.v, h));
                b.q = spin(b.q, scale(b.w, h));
            }
            self.solve_joints();
            contacts.clear();
            pair_contacts.clear();
            self.solve_world(world, &candidates, &mut contacts, true);
            self.solve_self(&mut pair_contacts);
            for b in &mut self.bodies {
                b.v = scale(sub(b.x, b.x_prev), 1.0 / h);
                let dq = quat_mul(b.q, [-b.q_prev[0], -b.q_prev[1], -b.q_prev[2], b.q_prev[3]]);
                let sign = if dq[3] >= 0.0 { 2.0 } else { -2.0 };
                b.w = scale([dq[0], dq[1], dq[2]], sign / h);
            }
            for c in &contacts {
                self.contact_velocity(c, h);
            }
            for c in &pair_contacts {
                self.pair_velocity(c, h);
            }
            for b in &mut self.bodies {
                b.v = scale(b.v, (-b.linear_damping * h).exp());
                b.w = scale(b.w, (-b.angular_damping * h).exp());
            }
        }
        self.candidates = candidates;
        let fastest = self
            .bodies
            .iter()
            .map(|b| length(b.v) + length(b.w) * b.reach)
            .fold(0.0, f32::max);
        self.speed = fastest;
        // How far the farthest point of any part is from where it was when the still stretch began: its centre of
        // mass's move plus its turn times its reach.
        let drift = self
            .bodies
            .iter()
            .zip(&self.anchor)
            .map(|(b, (x, q))| {
                let d = quat_mul(b.q, [-q[0], -q[1], -q[2], q[3]]);
                let angle = 2.0 * length([d[0], d[1], d[2]]).atan2(d[3].abs());
                length(sub(b.x, *x)) + angle * b.reach
            })
            .fold(0.0, f32::max);
        if self.anchor.len() == self.bodies.len() && drift <= REST_DRIFT {
            self.still_frames += 1;
        } else {
            self.anchor = self.bodies.iter().map(|b| (b.x, b.q)).collect();
            self.still_frames = 0;
        }
    }

    fn solve_joints(&mut self) {
        for j in 0..self.joints.len() {
            let joint = self.joints[j].clone();
            let (i1, i2) = (joint.body1, joint.body2);
            // The two frames' origins meet.
            let r1 = rotate(self.bodies[i1].q, joint.anchor1);
            let r2 = rotate(self.bodies[i2].q, joint.anchor2);
            let gap = sub(add(self.bodies[i2].x, r2), add(self.bodies[i1].x, r1));
            if let Some(n) = normalized(gap) {
                let c = length(gap);
                let w = self.bodies[i1].weight(r1, n) + self.bodies[i2].weight(r2, n);
                if w > 0.0 {
                    let p = scale(n, c / w);
                    self.bodies[i1].push(p, r1);
                    self.bodies[i2].push(scale(p, -1.0), r2);
                }
            }
            // Each frame's twist axis and the axis after it (z and x), world.
            let axes = |b: &Body, m: &M3| {
                let r = mat_mul(&quat_matrix(b.q), m);
                let column = |k: usize| [r[0][k], r[1][k], r[2][k]];
                (column(TWIST_AXIS), column((TWIST_AXIS + 1) % 3))
            };
            // Swing: the child's twist axis within the cone around the parent's (locked when the joint has no
            // swing limit: a hinge, like the knees).
            {
                let max = joint.swing;
                let (a, _) = axes(&self.bodies[i1], &joint.rotation1);
                let (b, _) = axes(&self.bodies[i2], &joint.rotation2);
                let angle = length(cross(a, b)).atan2(dot(a, b));
                if angle > max
                    && let Some(n) = normalized(cross(a, b))
                {
                    self.rotate_pair(i1, i2, n, angle - max);
                }
            }
            // Twist: the turn about the (mean) twist axis between the two frames' next axes, within the range.
            if let Some((min, max)) = joint.twist {
                let (a, ya) = axes(&self.bodies[i1], &joint.rotation1);
                let (b, yb) = axes(&self.bodies[i2], &joint.rotation2);
                if let Some(n) = normalized(add(a, b)) {
                    let project = |y: V3| normalized(sub(y, scale(n, dot(n, y))));
                    if let (Some(y1), Some(y2)) = (project(ya), project(yb)) {
                        let angle = dot(n, cross(y1, y2)).atan2(dot(y1, y2));
                        let excess = if angle < min {
                            angle - min
                        } else if angle > max {
                            angle - max
                        } else {
                            0.0
                        };
                        if excess != 0.0 {
                            self.rotate_pair(i1, i2, n, excess);
                        }
                    }
                }
            }
        }
    }

    /// Turns body `i1` by `angle` about `n` and body `i2` back, shared by their inertia (bringing `i2`'s axis
    /// towards `i1`'s by `angle`).
    fn rotate_pair(&mut self, i1: usize, i2: usize, n: V3, angle: f32) {
        let w1 = dot(n, mat_vec(&self.bodies[i1].inverse_inertia_world(), n));
        let w2 = dot(n, mat_vec(&self.bodies[i2].inverse_inertia_world(), n));
        if w1 + w2 <= 0.0 {
            return;
        }
        let p = scale(n, angle / (w1 + w2));
        self.bodies[i1].twist(p);
        self.bodies[i2].twist(scale(p, -1.0));
    }

    /// Pushes the parts out of the world; with `friction`, static friction holds the contacts that would slide less
    /// than their push allows (measured from the substep's start, so not in the settle passes).
    fn solve_world(
        &mut self,
        world: &WorldCollision,
        candidates: &[Item],
        contacts: &mut Vec<Contact>,
        friction: bool,
    ) {
        for i in 0..self.bodies.len() {
            if self.bodies[i].inverse_mass == 0.0 {
                continue;
            }
            for s in 0..self.bodies[i].spheres.len() {
                let (local, radius) = self.bodies[i].spheres[s];
                for &item in candidates {
                    let center = self.bodies[i].point(local);
                    let Some((depth, normal)) = world
                        .contact(item, center, radius)
                        .filter(|(depth, _)| depth.is_finite())
                    else {
                        continue;
                    };
                    let body = &mut self.bodies[i];
                    let r = sub(sub(center, scale(normal, radius)), body.x);
                    let w = body.weight(r, normal);
                    if w <= 0.0 {
                        continue;
                    }
                    let lambda = depth / w;
                    body.push(scale(normal, lambda), r);
                    if !friction {
                        continue;
                    }
                    // Static friction: undo the contact point's slide when the push would hold it.
                    let r_local = rotate([-body.q[0], -body.q[1], -body.q[2], body.q[3]], r);
                    let now = add(body.x, rotate(body.q, r_local));
                    let before = add(body.x_prev, rotate(body.q_prev, r_local));
                    let moved = sub(now, before);
                    let slide = sub(moved, scale(normal, dot(normal, moved)));
                    let slide_length = length(slide);
                    if slide_length > 0.0 && slide_length < STATIC_FRICTION * depth {
                        let n = scale(slide, 1.0 / slide_length);
                        let r_now = sub(now, body.x);
                        let w = body.weight(r_now, n);
                        if w > 0.0 {
                            body.push(scale(n, -slide_length / w), r_now);
                        }
                    }
                    contacts.push(Contact {
                        body: i,
                        r,
                        normal,
                        lambda,
                    });
                }
            }
        }
    }

    fn solve_self(&mut self, contacts: &mut Vec<PairContact>) {
        for k in 0..self.pairs.len() {
            let Pair { a, b, active } = self.pairs[k];
            let overlaps = overlaps(&self.bodies[a], &self.bodies[b]);
            if !active {
                // Still overlapping as they started: left alone until they part.
                self.pairs[k].active = overlaps.is_empty();
                continue;
            }
            for (normal, depth, r1, r2) in overlaps {
                let w = self.bodies[a].weight(r1, normal) + self.bodies[b].weight(r2, normal);
                if w <= 0.0 {
                    continue;
                }
                let lambda = depth / w;
                let p = scale(normal, lambda);
                self.bodies[a].push(scale(p, -1.0), r1);
                self.bodies[b].push(p, r2);
                contacts.push(PairContact {
                    a,
                    b,
                    normal,
                    r1,
                    r2,
                    lambda,
                });
            }
        }
    }

    /// Velocity pass for a contact between two parts, as [`Self::contact_velocity`] for the world: no speed
    /// between them along the normal, and their sliding on each other slowed by dynamic friction. Without it, each
    /// push apart was taken back as a speed apart and the joints pulled the parts together again: a body on the
    /// floor kept an arm moving at a few inches per second for seconds.
    fn pair_velocity(&mut self, c: &PairContact, h: f32) {
        let (a, b) = (&self.bodies[c.a], &self.bodies[c.b]);
        let relative = |a: &Body, b: &Body| sub(b.point_velocity(c.r2), a.point_velocity(c.r1));
        let along = |a: &Body, b: &Body, d: V3| a.weight(c.r1, d) + b.weight(c.r2, d);
        let vn = dot(c.normal, relative(a, b));
        let w = along(a, b, c.normal);
        if w > 0.0 {
            let p = scale(c.normal, -vn / w);
            self.bodies[c.a].impulse(scale(p, -1.0), c.r1);
            self.bodies[c.b].impulse(p, c.r2);
        }
        let (a, b) = (&self.bodies[c.a], &self.bodies[c.b]);
        let v = relative(a, b);
        let vt = sub(v, scale(c.normal, dot(c.normal, v)));
        let vt_length = length(vt);
        if vt_length > 1e-6 {
            let t = scale(vt, 1.0 / vt_length);
            let w = along(a, b, t);
            if w > 0.0 {
                let p = scale(t, -(vt_length / w).min(DYNAMIC_FRICTION * c.lambda / h));
                self.bodies[c.a].impulse(scale(p, -1.0), c.r1);
                self.bodies[c.b].impulse(p, c.r2);
            }
        }
    }

    /// Velocity pass for a world contact: no bounce (no speed along the normal), and sliding slowed by dynamic
    /// friction. The friction impulse is at most [`DYNAMIC_FRICTION`] times the contact's push over the substep
    /// (`λ / h`, mass × inches per second), so a heavy part slows by about μ·g·h per substep, as on a real floor.
    /// (Müller et al.'s equation 30 compares that impulse with a speed directly, which makes a 35-pound pelvis
    /// 35 times too sticky.)
    fn contact_velocity(&mut self, c: &Contact, h: f32) {
        let body = &mut self.bodies[c.body];
        // The normal speed goes to 0 both ways (Müller et al.'s equation 34 without restitution): the push out of
        // the world this substep was taken back as a speed away from it (depth / h), which kept resting bodies
        // hopping at a few inches per second.
        let vn = dot(c.normal, body.point_velocity(c.r));
        let w = body.weight(c.r, c.normal);
        if w > 0.0 {
            body.impulse(scale(c.normal, -vn / w), c.r);
        }
        let v = body.point_velocity(c.r);
        let vt = sub(v, scale(c.normal, dot(c.normal, v)));
        let vt_length = length(vt);
        if vt_length > 1e-6 {
            let t = scale(vt, 1.0 / vt_length);
            let w = body.weight(c.r, t);
            if w > 0.0 {
                // The impulse that stops the slide, or the most friction allows.
                let p = (vt_length / w).min(DYNAMIC_FRICTION * c.lambda / h);
                body.impulse(scale(t, -p), c.r);
            }
        }
    }
}

/// Where two parts' capsules overlap: for each overlapping pair of capsules, the normal from `a` to `b`, the depth
/// and the points of contact from each centre of mass (world).
fn overlaps(a: &Body, b: &Body) -> Vec<(V3, f32, V3, V3)> {
    let mut out = Vec::new();
    for &(pa, qa, ra) in &a.capsules {
        for &(pb, qb, rb) in &b.capsules {
            let (p1, q1) = (a.point(pa), a.point(qa));
            let (p2, q2) = (b.point(pb), b.point(qb));
            let (s, t) = closest_segments(p1, q1, p2, q2);
            let c1 = add(p1, scale(sub(q1, p1), s));
            let c2 = add(p2, scale(sub(q2, p2), t));
            let between = sub(c2, c1);
            let distance = length(between);
            if distance >= ra + rb || distance < 1e-6 {
                continue;
            }
            let n = scale(between, 1.0 / distance);
            let r1 = sub(add(c1, scale(n, ra)), a.x);
            let r2 = sub(sub(c2, scale(n, rb)), b.x);
            out.push((n, ra + rb - distance, r1, r2));
        }
    }
    out
}

/// Centre of mass, inertia (about it, part frame), collision spheres and capsules (centre-of-mass frame) of a
/// part's shapes, for a total `mass`.
#[allow(clippy::type_complexity)]
fn shape_properties(
    shapes: &[super::Shape],
    mass: f32,
) -> (V3, M3, Vec<(V3, f32)>, Vec<(V3, V3, f32)>) {
    // Each shape's volume, centre and inertia per unit mass about its own centre.
    let mut pieces: Vec<(f32, V3, M3)> = Vec::new();
    let mut spheres: Vec<(V3, f32)> = Vec::new();
    let mut capsules: Vec<(V3, V3, f32)> = Vec::new();
    for shape in shapes {
        match &shape.kind {
            ShapeKind::Sphere { center, radius } => {
                let i = 0.4 * radius * radius;
                pieces.push((
                    4.0 / 3.0 * std::f32::consts::PI * radius.powi(3),
                    *center,
                    [[i, 0.0, 0.0], [0.0, i, 0.0], [0.0, 0.0, i]],
                ));
                spheres.push((*center, *radius));
                capsules.push((*center, *center, *radius));
            }
            ShapeKind::Capsule { a, b, radius } => {
                let axis = sub(*b, *a);
                let l = length(axis);
                let u = normalized(axis).unwrap_or([1.0, 0.0, 0.0]);
                let r = *radius;
                // A solid cylinder with rounded ends: the ends count as 3/4 of their radius in length.
                let h = l + 1.5 * r;
                let (along, across) = (0.5 * r * r, (3.0 * r * r + h * h) / 12.0);
                let inertia: M3 = std::array::from_fn(|i| {
                    std::array::from_fn(|j| {
                        let identity = if i == j { 1.0 } else { 0.0 };
                        across * (identity - u[i] * u[j]) + along * u[i] * u[j]
                    })
                });
                let volume = std::f32::consts::PI * r * r * (l + 4.0 / 3.0 * r);
                pieces.push((volume, scale(add(*a, *b), 0.5), inertia));
                // Spheres along the segment, no further apart than the radius (at most 64: a broken capsule of
                // radius 0 would otherwise ask for endless spheres).
                let count = if r > 0.0 {
                    ((l / r).ceil() as usize).clamp(1, 64)
                } else {
                    1
                };
                for k in 0..=count {
                    spheres.push((add(*a, scale(axis, k as f32 / count as f32)), r));
                }
                capsules.push((*a, *b, r));
            }
            ShapeKind::Hull { min, max, .. } => {
                let size = sub(*max, *min);
                let inertia = [
                    [(size[1].powi(2) + size[2].powi(2)) / 12.0, 0.0, 0.0],
                    [0.0, (size[0].powi(2) + size[2].powi(2)) / 12.0, 0.0],
                    [0.0, 0.0, (size[0].powi(2) + size[1].powi(2)) / 12.0],
                ];
                let center = scale(add(*min, *max), 0.5);
                pieces.push((size[0] * size[1] * size[2], center, inertia));
                let radius = 0.5 * size.iter().copied().fold(f32::MAX, f32::min);
                spheres.push((center, radius));
                capsules.push((center, center, radius));
            }
            ShapeKind::Mesh { .. } => {}
        }
    }
    let volume: f32 = pieces.iter().map(|p| p.0).sum();
    if volume <= 0.0 {
        return (
            [0.0; 3],
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            spheres,
            capsules,
        );
    }
    let com = scale(
        pieces.iter().fold([0.0; 3], |s, p| add(s, scale(p.1, p.0))),
        1.0 / volume,
    );
    let mut inertia = [[0.0f32; 3]; 3];
    for (v, center, own) in &pieces {
        let m = mass * v / volume;
        let d = sub(*center, com);
        for i in 0..3 {
            for j in 0..3 {
                let identity = if i == j { 1.0 } else { 0.0 };
                // Parallel axis: the shape's own inertia plus its mass at its offset from the centre of mass.
                inertia[i][j] += m * (own[i][j] + dot(d, d) * identity - d[i] * d[j]);
            }
        }
    }
    let shift = |p: V3| sub(p, com);
    (
        com,
        inertia,
        spheres.into_iter().map(|(c, r)| (shift(c), r)).collect(),
        capsules
            .into_iter()
            .map(|(a, b, r)| (shift(a), shift(b), r))
            .collect(),
    )
}

/// A joint's swing (the angle between the two frames' twist axes, [`TWIST_AXIS`]: z) and twist (the next axes', x,
/// turn about the mean twist axis), radians, for parts at the world frames `frame1` and `frame2`: what [`Ragdoll`]
/// keeps within the joint's limits.
pub fn joint_angles(joint: &super::Joint, frame1: &Affine, frame2: &Affine) -> (f32, f32) {
    joint_angles_about(joint, frame1, frame2, TWIST_AXIS)
}

/// [`joint_angles`] with the twist axis `twist_axis` of the joint frames (0 = x, 1 = y, 2 = z): the swing between
/// the two frames' twist axes, the twist of the next axis about their mean.
pub fn joint_angles_about(
    joint: &super::Joint,
    frame1: &Affine,
    frame2: &Affine,
    twist_axis: usize,
) -> (f32, f32) {
    let world1 = compose(frame1, &joint.frame1);
    let world2 = compose(frame2, &joint.frame2);
    let axis = |m: &Affine, k: usize| [m[0][k], m[1][k], m[2][k]];
    let (a, b) = (axis(&world1, twist_axis), axis(&world2, twist_axis));
    let swing = length(cross(a, b)).atan2(dot(a, b));
    let next = (twist_axis + 1) % 3;
    let twist = normalized(add(a, b)).map_or(0.0, |n| {
        let project = |y: V3| normalized(sub(y, scale(n, dot(n, y))));
        match (project(axis(&world1, next)), project(axis(&world2, next))) {
            (Some(y1), Some(y2)) => dot(n, cross(y1, y2)).atan2(dot(y1, y2)),
            _ => 0.0,
        }
    });
    (swing, twist)
}

#[cfg(test)]
mod tests {
    use super::super::{CollisionAttribute, Joint, Part, Shape};
    use super::*;

    fn capsule_part(mass: f32, a: V3, b: V3, radius: f32) -> Part {
        Part {
            mass,
            inertia_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            linear_drag: 0.0,
            angular_drag: 0.0,
            shapes: vec![Shape {
                attribute: 0,
                surface: 0,
                kind: ShapeKind::Capsule { a, b, radius },
            }],
        }
    }

    fn floor() -> WorldCollision {
        let mut world = WorldCollision::default();
        world.add_box([-500.0, -500.0, -100.0], [500.0, 500.0, 0.0]);
        world
    }

    fn identity_at(p: V3) -> Affine {
        affine([0.0, 0.0, 0.0, 1.0], p)
    }

    #[test]
    fn a_capsule_falls_and_rests_on_the_floor() {
        let data = PhysicsData {
            parts: vec![capsule_part(10.0, [0.0; 3], [20.0, 0.0, 0.0], 4.0)],
            ..Default::default()
        };
        let mut ragdoll = Ragdoll::new(&data, &[identity_at([0.0, 0.0, 50.0])], &[]);
        let world = floor();
        let mut frames = 0;
        while !ragdoll.at_rest() && frames < 64 * 5 {
            ragdoll.step(1.0 / 64.0, &world);
            frames += 1;
        }
        assert!(ragdoll.at_rest(), "still after {frames} frames");
        let frame = ragdoll.frames()[0];
        // Lying on the floor: its axis 4 inches up (the radius), not sunk in.
        assert!((frame[2][3] - 4.0).abs() < 0.5, "height {}", frame[2][3]);
        assert!(frame[0][0].abs() > 0.99, "still lying along x");
    }

    #[test]
    fn a_push_moves_the_body_its_way() {
        let data = PhysicsData {
            parts: vec![capsule_part(10.0, [0.0; 3], [20.0, 0.0, 0.0], 4.0)],
            ..Default::default()
        };
        let mut ragdoll = Ragdoll::new(&data, &[identity_at([0.0, 0.0, 4.0])], &[]);
        ragdoll.apply_impulse(0, [10.0, 0.0, 4.0], [0.0, 1000.0, 0.0]);
        ragdoll.step(1.0 / 64.0, &floor());
        assert!(
            ragdoll.velocities()[0][1] > 50.0,
            "{:?}",
            ragdoll.velocities()
        );
    }

    /// Two capsules joined end to end: a thigh hanging from a pelvis held still.
    fn chain(swing: Option<(f32, f32)>, twist: Option<(f32, f32)>) -> PhysicsData {
        let mut pelvis = capsule_part(0.0, [-5.0, 0.0, 0.0], [5.0, 0.0, 0.0], 4.0);
        pelvis.mass = 0.0;
        PhysicsData {
            parts: vec![pelvis, capsule_part(10.0, [0.0; 3], [20.0, 0.0, 0.0], 3.0)],
            joints: vec![Joint {
                kind: 4,
                body1: 0,
                body2: 1,
                frame1: identity_at([5.0, 0.0, 0.0]),
                frame2: identity_at([0.0; 3]),
                swing,
                twist,
                friction: 0.0,
                collide: false,
            }],
            attributes: vec![CollisionAttribute::default()],
            ..Default::default()
        }
    }

    #[test]
    fn joints_hold_and_keep_their_swing() {
        let data = chain(Some((0.0, 0.5)), None);
        // The thigh starts along +x from the pelvis's end; gravity pulls it down, the cone stops it at 0.5 rad.
        let frames = [
            identity_at([0.0, 0.0, 100.0]),
            identity_at([5.0, 0.0, 100.0]),
        ];
        let mut ragdoll = Ragdoll::new(&data, &frames, &[]);
        let empty = WorldCollision::default();
        for _ in 0..128 {
            ragdoll.step(1.0 / 64.0, &empty);
        }
        let now = ragdoll.frames();
        let (swing, _) = joint_angles(&data.joints[0], &now[0], &now[1]);
        assert!(swing < 0.5 + 0.02, "swing {swing}");
        assert!(swing > 0.4, "it did swing down: {swing}");
        // The joint's two origins still meet.
        let a = compose(&now[0], &data.joints[0].frame1);
        let b = compose(&now[1], &data.joints[0].frame2);
        let gap = length(sub(
            [a[0][3], a[1][3], a[2][3]],
            [b[0][3], b[1][3], b[2][3]],
        ));
        assert!(gap < 0.05, "gap {gap}");
    }

    #[test]
    fn twist_stays_within_its_range() {
        // No swing limit: a hinge about the frames' z (the twist axis), spun about it.
        let data = chain(None, Some((-0.3, 0.3)));
        let frames = [
            identity_at([0.0, 0.0, 100.0]),
            identity_at([5.0, 0.0, 100.0]),
        ];
        let mut ragdoll = Ragdoll::new(
            &data,
            &frames,
            &[([0.0; 3], [0.0; 3]), ([0.0; 3], [0.0, 0.0, 20.0])],
        );
        let empty = WorldCollision::default();
        let (mut worst, mut worst_swing) = (0.0f32, 0.0f32);
        for _ in 0..64 {
            ragdoll.step(1.0 / 64.0, &empty);
            let now = ragdoll.frames();
            let (swing, twist) = joint_angles(&data.joints[0], &now[0], &now[1]);
            worst = worst.max(twist.abs());
            worst_swing = worst_swing.max(swing);
        }
        assert!(worst < 0.3 + 0.05, "twist reached {worst}");
        assert!(worst > 0.25, "it was spun to its limit: {worst}");
        // Gravity pulls the thigh down, but a hinge does not swing.
        assert!(worst_swing < 0.05, "swing {worst_swing}");
    }

    #[test]
    fn the_same_start_gives_the_same_fall() {
        let data = chain(Some((0.0, 1.0)), Some((-0.5, 0.5)));
        let frames = [identity_at([0.0, 0.0, 30.0]), identity_at([5.0, 0.0, 30.0])];
        let run = || {
            let mut ragdoll = Ragdoll::new(&data, &frames, &[]);
            ragdoll.apply_impulse(1, [15.0, 0.0, 30.0], [0.0, 500.0, 300.0]);
            for _ in 0..90 {
                ragdoll.step(1.0 / 64.0, &floor());
            }
            ragdoll.frames()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_body_starting_in_the_floor_does_not_jump_out() {
        // 2 inches into the floor (a foot inside a stair): the settle passes lift it without giving it speed.
        let data = PhysicsData {
            parts: vec![capsule_part(10.0, [0.0; 3], [20.0, 0.0, 0.0], 4.0)],
            ..Default::default()
        };
        let mut ragdoll = Ragdoll::new(&data, &[identity_at([0.0, 0.0, 2.0])], &[]);
        let world = floor();
        ragdoll.step(1.0 / 64.0, &world);
        // Without the settle passes: 2 inches in one 1/512 s substep, about 1000 in/s up.
        assert!(ragdoll.speed() < 20.0, "moves at {} in/s", ragdoll.speed());
        // Pushed out sphere by sphere, one end comes out higher; gravity lays it down again.
        for _ in 0..64 {
            ragdoll.step(1.0 / 64.0, &world);
        }
        let frame = ragdoll.frames()[0];
        assert!((frame[2][3] - 4.0).abs() < 0.2, "{frame:?}");
        assert!(frame[0][0] > 0.999, "lies flat: {frame:?}");
    }

    #[test]
    fn a_sliding_body_stops_as_friction_says() {
        // Lying along x, pushed along x at 200 in/s: friction μ·g slows it, d = v² / (2 μ g) ≈ 36 inches.
        let data = PhysicsData {
            parts: vec![capsule_part(30.0, [0.0; 3], [20.0, 0.0, 0.0], 4.0)],
            ..Default::default()
        };
        let mut ragdoll = Ragdoll::new(
            &data,
            &[identity_at([0.0, 0.0, 4.0])],
            &[([200.0, 0.0, 0.0], [0.0; 3])],
        );
        let world = floor();
        for _ in 0..128 {
            ragdoll.step(1.0 / 64.0, &world);
        }
        let expected = 200.0f32.powi(2) / (2.0 * DYNAMIC_FRICTION * GRAVITY);
        let slid = ragdoll.frames()[0][0][3];
        assert!(
            (slid - expected).abs() < 0.25 * expected,
            "slid {slid}, expected about {expected}"
        );
    }

    #[test]
    fn start_velocities_are_the_frames() {
        // A part spinning about its own origin (a limb about its joint), without gravity's pull yet: the origin
        // stays, the far end moves.
        let data = PhysicsData {
            parts: vec![capsule_part(10.0, [0.0; 3], [20.0, 0.0, 0.0], 4.0)],
            ..Default::default()
        };
        let ragdoll = Ragdoll::new(
            &data,
            &[identity_at([0.0, 0.0, 100.0])],
            &[([0.0; 3], [0.0, 0.0, 10.0])],
        );
        // Centre of mass 10 inches out: ω × r = 100 in/s along y.
        let v = ragdoll.velocities()[0];
        assert!((v[1] - 100.0).abs() < 1e-3, "{v:?}");
    }

    #[test]
    fn broken_shapes_stay_harmless() {
        let mut world = WorldCollision::default();
        // A triangle with two corners on one point is left out (its closest point would be 0 / 0).
        world.add_triangle([[0.0; 3], [0.0; 3], [10.0, 0.0, 0.0]]);
        // A shape with endless bounds is not indexed; a huge one is tested by every query, not stored per cell.
        world.add_box([0.0; 3], [f32::INFINITY, 1.0, 1.0]);
        world.add_box([-1e6, -1e6, -1e6], [1e6, 1e6, 0.0]);
        assert_eq!(world.counts(), (2, 0));
        assert!(world.grid.is_empty());
        let (depth, _) = world
            .sphere_contact([0.0, 0.0, 1.0], 4.0)
            .expect("on the floor");
        assert!((depth - 3.0).abs() < 1e-3);
        // A capsule of radius 0 gets a bounded number of spheres.
        let data = PhysicsData {
            parts: vec![capsule_part(10.0, [0.0; 3], [20.0, 0.0, 0.0], 0.0)],
            ..Default::default()
        };
        assert!(
            Ragdoll::new(&data, &[identity_at([0.0; 3])], &[]).bodies[0]
                .spheres
                .len()
                <= 65
        );
    }

    #[test]
    fn rays_stop_at_the_nearest_surface() {
        let mut world = WorldCollision::default();
        // A wall 2000 inches away (many cells along the ray) and a nearer one behind which it hides.
        world.add_box([2000.0, -50.0, -50.0], [2010.0, 50.0, 50.0]);
        world.add_box([500.0, -50.0, -50.0], [510.0, 50.0, 50.0]);
        // A floor triangle, crossed from above.
        world.add_triangle([
            [-100.0, -100.0, -20.0],
            [100.0, -100.0, -20.0],
            [0.0, 100.0, -20.0],
        ]);
        let hit = world
            .ray_hit([0.0; 3], [1.0, 0.0, 0.0], 8192.0)
            .expect("hits");
        assert!((hit.distance - 500.0).abs() < 1e-3, "{hit:?}");
        assert_eq!(hit.normal, [-1.0, 0.0, 0.0]);
        assert_eq!(hit.shape, NO_SHAPE);
        // Past the near wall (starting inside it does not stop the ray), the far one.
        let hit = world
            .ray_hit([505.0, 0.0, 0.0], [1.0, 0.0, 0.0], 8192.0)
            .expect("hits");
        assert!((hit.distance - 1495.0).abs() < 1e-3, "{hit:?}");
        // Out of reach: nothing.
        assert!(world.ray_hit([0.0; 3], [1.0, 0.0, 0.0], 400.0).is_none());
        // Down onto the triangle, its normal turned to face the ray; backwards, nothing.
        let down = world
            .ray_hit([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 100.0)
            .expect("floor");
        assert!((down.distance - 20.0).abs() < 1e-4 && down.normal == [0.0, 0.0, 1.0]);
        let up = world
            .ray_hit([0.0, 0.0, -40.0], [0.0, 0.0, 1.0], 100.0)
            .expect("floor");
        assert!((up.distance - 20.0).abs() < 1e-4 && up.normal == [0.0, 0.0, -1.0]);
        assert!(world.ray_hit([0.0; 3], [-1.0, 0.0, 0.0], 8192.0).is_none());
        // A slanted ray crossing cell corners still finds the far wall.
        let d = normalized([1.0, 0.01, 0.005]).expect("direction");
        let hit = world
            .ray_hit([600.0, 0.0, 0.0], d, 8192.0)
            .expect("far wall");
        assert!((hit.distance * d[0] - 1400.0).abs() < 1e-2, "{hit:?}");
    }

    #[test]
    fn rays_name_the_surface_they_meet() {
        // A world mesh of two floor triangles, its own surface 1, its triangles' 2 and 3.
        let mesh = Shape {
            attribute: 0,
            surface: 1,
            kind: ShapeKind::Mesh {
                vertices: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [0.0, 10.0, 0.0],
                    [10.0, 10.0, 0.0],
                ],
                triangles: vec![[0, 1, 2], [1, 3, 2]],
                materials: vec![2, 3],
                min: [0.0; 3],
                max: [10.0, 10.0, 0.0],
            },
        };
        let world = PhysicsData {
            parts: vec![Part {
                mass: 0.0,
                inertia_scale: 1.0,
                linear_damping: 0.0,
                angular_damping: 0.0,
                linear_drag: 0.0,
                angular_drag: 0.0,
                shapes: vec![mesh],
            }],
            attributes: vec![super::super::CollisionAttribute {
                group: "Default".into(),
                interact_as: vec![],
            }],
            surfaces: vec![10, 11, 12, 13],
            ..Default::default()
        };
        let collision = WorldCollision::from_physics_where(&world, |a| a.stops_bullets());
        let down = [0.0, 0.0, -1.0];
        let first = collision
            .ray_hit([2.0, 2.0, 5.0], down, 100.0)
            .expect("first triangle");
        let second = collision
            .ray_hit([8.0, 8.0, 5.0], down, 100.0)
            .expect("second triangle");
        assert_eq!((first.shape, first.surface), (0, 2));
        assert_eq!((second.shape, second.surface), (0, 3));
    }

    #[test]
    fn bullets_go_through_grates_and_glass() {
        let solid = super::super::CollisionAttribute {
            group: "Default".into(),
            interact_as: vec![],
        };
        let grate = super::super::CollisionAttribute {
            group: "ConditionallySolid".into(),
            interact_as: vec!["passbullets".into()],
        };
        let glass = super::super::CollisionAttribute {
            group: "Default".into(),
            interact_as: vec!["window".into()],
        };
        assert!(solid.stops_bullets());
        assert!(grate.stops_bodies() && !grate.stops_bullets());
        assert!(glass.stops_bodies() && !glass.stops_bullets());
    }

    #[test]
    fn triangles_and_segments_closest_points() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]);
        let inside = closest_on_triangle([2.0, 2.0, 5.0], a, b, c);
        assert!(length(sub(inside, [2.0, 2.0, 0.0])) < 1e-5, "{inside:?}");
        assert_eq!(closest_on_triangle([-5.0, -5.0, 0.0], a, b, c), a);
        let (s, t) = closest_segments(
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [5.0, -5.0, 3.0],
            [5.0, 5.0, 3.0],
        );
        assert!((s - 0.5).abs() < 1e-6 && (t - 0.5).abs() < 1e-6);
        // A mesh floor stops a body like a hull floor.
        let mut world = WorldCollision::default();
        world.add_triangle([
            [-500.0, -500.0, 0.0],
            [500.0, -500.0, 0.0],
            [0.0, 500.0, 0.0],
        ]);
        let (depth, normal) = world
            .contact(Item::Triangle(0), [0.0, 0.0, 2.0], 4.0)
            .expect("touches");
        assert!((depth - 2.0).abs() < 1e-5 && (normal[2] - 1.0).abs() < 1e-5);
    }
}
