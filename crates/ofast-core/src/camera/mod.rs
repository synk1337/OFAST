//! Cameras (M7, `docs/decisions/021-cameras.md`): every camera of a round as OFAST builds it, before Unreal keys
//! it.
//!
//! - [`smooth`]: a calmer first-person view, pinned to the original aim where the player shoots.
//! - [`shots`]: cameras from outside the players, made from the round (behind a player, around him, from a fixed point
//!   that sees a kill, from above), kept out of walls with the map's collision.
//! - [`hlae`]: HLAE's camera files (`mirv_campath` XML, `mirv_camio` `.cam`), read and written, and a campath
//!   sampled with HLAE's own interpolation.
//!
//! Everything here is in CS2's units and angles: Source units (inches), x/y/z as the game has them, angles
//! `(pitch, yaw, roll)` in degrees (pitch positive looking down, yaw counter-clockwise from +x), and a field of view as
//! CS2 counts it: horizontal on a 4:3 picture (CS2's default 90; [`fov_at_aspect`] gives the picture's own).
//! `export::cameras` writes them to the round's cameras file (`docs/formats/cameras.md`).

pub mod hlae;
pub mod shots;
pub mod smooth;

/// A point or direction, Source units.
pub type Vec3 = [f64; 3];

/// A unit quaternion `(w, x, y, z)`.
pub type Quat = [f64; 4];

/// CS2's default field of view (horizontal at 4:3).
pub const DEFAULT_FOV: f64 = 90.0;

/// A camera at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraKey {
    /// Demo tick, fractional between ticks.
    pub tick: f64,
    /// Where the camera is.
    pub position: Vec3,
    /// Where it looks: `(pitch, yaw, roll)`, degrees, Source's.
    pub angles: Vec3,
    /// Field of view, degrees, horizontal at 4:3 (CS2's).
    pub fov: f64,
}

pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` scaled to length 1, or zero for a zero vector.
pub(crate) fn unit(a: Vec3) -> Vec3 {
    let l = length(a);
    if l > 0.0 { scale(a, 1.0 / l) } else { [0.0; 3] }
}

/// The unit vector a view with `pitch` and `yaw` (degrees, Source's) looks along.
pub fn forward(pitch: f64, yaw: f64) -> Vec3 {
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sy, cy) = yaw.to_radians().sin_cos();
    [cp * cy, cp * sy, -sp]
}

/// `(pitch, yaw)` in degrees of a view from `from` looking at `to` (Source's; `(0, 0)` when they are the same point).
pub fn look_at(from: Vec3, to: Vec3) -> [f64; 2] {
    let d = sub(to, from);
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if flat == 0.0 && d[2] == 0.0 {
        return [0.0, 0.0];
    }
    [
        (-d[2]).atan2(flat).to_degrees(),
        d[1].atan2(d[0]).to_degrees(),
    ]
}

/// An angle in degrees brought into (−180, 180].
pub fn wrap_degrees(angle: f64) -> f64 {
    let a = angle.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// Angles in degrees made continuous: each moved by whole turns to within 180° of the one before (yaw crossing ±180
/// stays a small turn).
pub fn unwrap_degrees(angles: &mut [f64]) {
    for i in 1..angles.len() {
        let previous = angles[i - 1];
        angles[i] = previous + wrap_degrees(angles[i] - previous);
    }
}

fn quat_mul(a: Quat, b: Quat) -> Quat {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

/// The rotation of Source angles `(pitch, yaw, roll)` (degrees) as a unit quaternion: about z by yaw, about y by pitch,
/// about x by roll, each right-handed, applied roll first (`yaw · pitch · roll`, Source's angle matrix; HLAE's
/// `Quaternion::FromQREulerAngles`).
pub fn angles_to_quat([pitch, yaw, roll]: Vec3) -> Quat {
    let half = |a: f64| (a.to_radians() * 0.5).sin_cos();
    let (sp, cp) = half(pitch);
    let (sy, cy) = half(yaw);
    let (sr, cr) = half(roll);
    quat_mul(
        quat_mul([cy, 0.0, 0.0, sy], [cp, 0.0, sp, 0.0]),
        [cr, sr, 0.0, 0.0],
    )
}

/// The Source angles `(pitch, yaw, roll)` (degrees) of a quaternion ([`angles_to_quat`] the other way; it need not be
/// unit length). Looking straight up or down, where yaw and roll turn about the same axis, roll is 0.
pub fn quat_to_angles(q: Quat) -> Vec3 {
    let n = q.iter().map(|v| v * v).sum::<f64>();
    let s = if n > 0.0 { 2.0 / n } else { 0.0 };
    let [w, x, y, z] = q;
    // The rotation's columns: where it takes x (forward), y (left) and z (up).
    let forward = [
        1.0 - s * (y * y + z * z),
        s * (x * y + w * z),
        s * (x * z - w * y),
    ];
    let left = [
        s * (x * y - w * z),
        1.0 - s * (x * x + z * z),
        s * (y * z + w * x),
    ];
    let up_z = 1.0 - s * (x * x + y * y);
    let flat = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt();
    let pitch = (-forward[2]).atan2(flat).to_degrees();
    if flat > 1e-9 {
        let yaw = forward[1].atan2(forward[0]).to_degrees();
        let roll = left[2].atan2(up_z).to_degrees();
        [pitch, yaw, roll]
    } else {
        // Roll 0: left is then (−sin yaw, cos yaw, 0).
        [pitch, (-left[0]).atan2(left[1]).to_degrees(), 0.0]
    }
}

/// Spherical interpolation from `a` to `b` (unit quaternions), the short way round.
pub fn slerp(a: Quat, b: Quat, t: f64) -> Quat {
    let mut b = b;
    let mut cos = a.iter().zip(&b).map(|(x, y)| x * y).sum::<f64>();
    if cos < 0.0 {
        b = b.map(|v| -v);
        cos = -cos;
    }
    let (wa, wb) = if cos > 1.0 - 1e-12 {
        (1.0 - t, t)
    } else {
        let theta = cos.acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    std::array::from_fn(|k| wa * a[k] + wb * b[k])
}

/// A field of view counted as CS2 counts it (horizontal at 4:3) as the horizontal field of view of a picture of
/// `aspect` (width / height): CS2 keeps the vertical one (90 → 106.26° at 16:9). HLAE's `AlienSwarm_FovScaling`.
pub fn fov_at_aspect(fov: f64, aspect: f64) -> f64 {
    let ratio = aspect / (4.0 / 3.0);
    2.0 * (ratio * (fov.to_radians() * 0.5).tan()).atan().to_degrees()
}

/// [`fov_at_aspect`] the other way: a picture's horizontal field of view as CS2 counts it.
pub fn fov_from_aspect(fov: f64, aspect: f64) -> f64 {
    let ratio = aspect / (4.0 / 3.0);
    2.0 * ((fov.to_radians() * 0.5).tan() / ratio).atan().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn angles_round_trip_through_quaternions() {
        for angles in [
            [0.0, 0.0, 0.0],
            [30.0, 120.0, 0.0],
            [-45.0, -170.0, 10.0],
            [80.0, 45.0, -25.0],
            [12.5, 179.0, 3.0],
        ] {
            let back = quat_to_angles(angles_to_quat(angles));
            for k in 0..3 {
                assert!(
                    close(wrap_degrees(back[k] - angles[k]), 0.0, 1e-9),
                    "{angles:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn quaternion_rotates_forward_like_the_angles() {
        // The rotation's first column is the view's forward vector.
        let angles = [20.0, 70.0, 15.0];
        let q = angles_to_quat(angles);
        let [w, x, y, z] = q;
        let column = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        let expected = forward(20.0, 70.0);
        for k in 0..3 {
            assert!(close(column[k], expected[k], 1e-12));
        }
        // Pitch positive looks down.
        assert!(forward(30.0, 0.0)[2] < 0.0);
    }

    #[test]
    fn look_at_and_forward_agree() {
        let [pitch, yaw] = look_at([0.0, 0.0, 0.0], [10.0, 10.0, -10.0]);
        let f = forward(pitch, yaw);
        let expected = unit([10.0, 10.0, -10.0]);
        for k in 0..3 {
            assert!(close(f[k], expected[k], 1e-12));
        }
    }

    #[test]
    fn unwrapping_keeps_turns_short() {
        let mut yaws = [170.0, -175.0, -160.0, 175.0];
        unwrap_degrees(&mut yaws);
        assert_eq!(yaws, [170.0, 185.0, 200.0, 175.0]);
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-180.0), 180.0);
    }

    #[test]
    fn fields_of_view_scale_as_cs2s() {
        assert!(close(
            fov_at_aspect(90.0, 16.0 / 9.0),
            106.260_204_708_311_96,
            1e-9
        ));
        assert!(close(
            fov_from_aspect(fov_at_aspect(75.0, 2.4), 2.4),
            75.0,
            1e-9
        ));
        assert!(close(fov_at_aspect(90.0, 4.0 / 3.0), 90.0, 1e-9));
    }

    #[test]
    fn slerp_goes_the_short_way() {
        let a = angles_to_quat([0.0, 170.0, 0.0]);
        let b = angles_to_quat([0.0, -170.0, 0.0]);
        let mid = quat_to_angles(slerp(a, b, 0.5));
        assert!(close(wrap_degrees(mid[1] - 180.0), 0.0, 1e-9), "{mid:?}");
    }
}
