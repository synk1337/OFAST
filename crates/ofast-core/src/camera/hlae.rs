//! HLAE's camera files (M7): `mirv_campath` paths (XML) and `mirv_camio` recordings (`.cam`), read and written, so
//! a camera made in CS2 with HLAE comes into Unreal, and one made in Unreal goes back to CS2.
//!
//! Learned from HLAE's source (advancedfx, MIT; `docs/formats/cameras.md`), not copied from it:
//!
//! - **Campath** (`shared/CamPath.cpp`, `CamPath::Save` / `Load`): `<campath>` with optional `positionInterp`,
//!   `rotationInterp`, `fovInterp` (`default`, `linear` / `sLinear`, `cubic` / `sCubic`), `offset` (seconds) and
//!   `hold`; `<points>` of `<p t x y z fov rx ry rz qw qx qy qz selected/>`: `t` the time (CS2's game clock minus the
//!   offset), the position, the field of view as CS2 counts it, roll (`rx`), pitch (`ry`) and yaw (`rz`) in degrees,
//!   and the same rotation as a quaternion, which wins when both are there. HLAE plays it at `curtime − offset`,
//!   between its first and last points only (clamped to them with `hold`).
//! - **Interpolation** (`shared/AfxMath.h`, `.cpp`): `default` is cubic. Position and field of view: a cubic
//!   spline through the points (Numerical Recipes' `spline` / `splint`, its slope 0 at both ends: at rest there), or
//!   linear. Rotation: James McEnnan's quaternion spline (`qspline`, CC0: angular rates solved so they are continuous
//!   at the points, each interval a third-order slew), or spherical linear. HLAE needs 4 points for a cubic path
//!   and plays none with fewer; OFAST falls back to linear and spherical linear there.
//! - **`.cam`** (`shared/CamIO.cpp`): `advancedfx Cam`, `version 1` or `2`, an optional `scaleFov none|alienSwarm`,
//!   a `channels` line, `DATA`, then one line per frame: `time x y z xRotation yRotation zRotation fov` with x roll,
//!   y pitch, z yaw. CS2's HLAE (`AfxHookSource2/main.cpp`) writes `curtime` and the field of view scaled to the
//!   picture (`AlienSwarm_FovScaling`: [`super::fov_at_aspect`]); without a `scaleFov` line a reader scales it back
//!   with its own picture's aspect.
//! - **Time**: CS2's game clock, `curtime` (`CMirvTime::curtime_get`), is the server's tick times the tick interval;
//!   the server's tick runs a constant distance ahead of the demo's ([`Clock`]).

use super::{CameraKey, DEFAULT_FOV, Quat, Vec3, angles_to_quat, quat_to_angles, slerp};

/// Something wrong in an HLAE file.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum HlaeError {
    /// Not a `.cam` file, or a line that does not read.
    #[error("not an HLAE .cam file: {0}")]
    Cam(String),
    /// Not a campath, or a point that does not read.
    #[error("not an HLAE campath: {0}")]
    Campath(String),
}

/// HLAE's clock (CS2's game time, seconds) and the demo's ticks: `time = (tick + server_tick_offset) × tick_interval`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    /// Seconds per tick.
    pub tick_interval: f64,
    /// How far the server's tick runs ahead of the demo's (`RoundTrack::server_tick_offset`).
    pub server_tick_offset: i64,
}

impl Clock {
    /// The demo tick (fractional) of a game time.
    pub fn tick(&self, time: f64) -> f64 {
        time / self.tick_interval - self.server_tick_offset as f64
    }

    /// The game time of a demo tick.
    pub fn time(&self, tick: f64) -> f64 {
        (tick + self.server_tick_offset as f64) * self.tick_interval
    }
}

/// How a `.cam` file's field of view is stored (`scaleFov`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FovScaling {
    /// No `scaleFov` line: the game's own scaling, which in CS2 is `alienSwarm` (what CS2's HLAE writes).
    Default,
    /// `scaleFov none`: as the game counts it (horizontal at 4:3 in CS2).
    None,
    /// `scaleFov alienSwarm`: the picture's horizontal field of view.
    AlienSwarm,
}

/// One frame of a `.cam` file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamFrame {
    /// Game time, seconds.
    pub time: f64,
    /// Where the camera is.
    pub position: Vec3,
    /// `(pitch, yaw, roll)`, degrees (the file's y, z and x rotations).
    pub angles: Vec3,
    /// The field of view as the file stores it ([`FovScaling`]).
    pub fov: f64,
}

/// A `.cam` file.
#[derive(Debug, Clone, PartialEq)]
pub struct CamFile {
    /// 1 or 2.
    pub version: u32,
    /// How [`CamFrame::fov`] is stored.
    pub fov_scaling: FovScaling,
    /// The frames, in the file's order.
    pub frames: Vec<CamFrame>,
}

/// Reads a `.cam` file (module docs).
pub fn read_cam(text: &str) -> Result<CamFile, HlaeError> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    if lines.next().map(str::trim) != Some("advancedfx Cam") {
        return Err(HlaeError::Cam(
            "first line is not \"advancedfx Cam\"".into(),
        ));
    }
    let mut version = 0;
    let mut fov_scaling = FovScaling::Default;
    let mut data = false;
    for line in lines.by_ref() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("version") => {
                version = words
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| HlaeError::Cam(format!("bad version line {line:?}")))?;
            }
            Some("scaleFov") => {
                fov_scaling = if words.next() == Some("alienSwarm") {
                    FovScaling::AlienSwarm
                } else {
                    FovScaling::None
                };
            }
            Some("DATA") => {
                data = true;
                break;
            }
            _ => {}
        }
    }
    if !(1..=2).contains(&version) {
        return Err(HlaeError::Cam(format!(
            "version {version} (1 or 2 expected)"
        )));
    }
    if !data {
        return Err(HlaeError::Cam("no DATA line".into()));
    }
    let mut frames = Vec::new();
    for (number, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let values: Vec<f64> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|e| HlaeError::Cam(format!("frame {number}: {e}")))?;
        let [time, x, y, z, roll, pitch, yaw, fov] = values[..] else {
            return Err(HlaeError::Cam(format!(
                "frame {number}: {} values, 8 expected",
                values.len()
            )));
        };
        frames.push(CamFrame {
            time,
            position: [x, y, z],
            angles: [pitch, yaw, roll],
            fov,
        });
    }
    Ok(CamFile {
        version,
        fov_scaling,
        frames,
    })
}

impl CamFile {
    /// The frames as camera keys on the demo's ticks: the field of view as CS2 counts it, a stored picture's field of
    /// view taken back with `aspect` (width / height of the picture it was recorded at; the file does not say).
    pub fn keys(&self, clock: Clock, aspect: f64) -> Vec<CameraKey> {
        self.frames
            .iter()
            .map(|f| CameraKey {
                tick: clock.tick(f.time),
                position: f.position,
                angles: f.angles,
                fov: match self.fov_scaling {
                    FovScaling::None => f.fov,
                    FovScaling::Default | FovScaling::AlienSwarm => {
                        super::fov_from_aspect(f.fov, aspect)
                    }
                },
            })
            .collect()
    }
}

/// A `.cam` file (version 2, as CS2's HLAE writes it) of camera keys: game times, and each field of view as a picture
/// of `aspect` shows it (HLAE scales it back with the picture it plays on).
pub fn write_cam(keys: &[CameraKey], clock: Clock, aspect: f64) -> String {
    let mut out = String::from(
        "advancedfx Cam\nversion 2\nchannels time xPosition yPosition zPosition xRotation yRotation zRotation fov\nDATA\n",
    );
    for k in keys {
        out.push_str(&format!(
            "{:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}\n",
            clock.time(k.tick),
            k.position[0],
            k.position[1],
            k.position[2],
            k.angles[2],
            k.angles[0],
            k.angles[1],
            super::fov_at_aspect(k.fov, aspect)
        ));
    }
    out
}

/// How a campath moves between its points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interp {
    /// HLAE's default: cubic.
    #[default]
    Default,
    /// Linear (`linear`; for the rotation `sLinear`: spherical).
    Linear,
    /// Cubic (`cubic`; for the rotation `sCubic`).
    Cubic,
}

impl Interp {
    fn cubic(self) -> bool {
        self != Interp::Linear
    }

    fn parse(value: &str) -> Interp {
        match value.to_ascii_lowercase().as_str() {
            "linear" | "slinear" => Interp::Linear,
            "cubic" | "scubic" => Interp::Cubic,
            _ => Interp::Default,
        }
    }

    fn name(self, rotation: bool) -> &'static str {
        match (self, rotation) {
            (Interp::Default, _) => "default",
            (Interp::Linear, false) => "linear",
            (Interp::Cubic, false) => "cubic",
            (Interp::Linear, true) => "sLinear",
            (Interp::Cubic, true) => "sCubic",
        }
    }
}

/// One point of a campath.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CampathPoint {
    /// Seconds: CS2's game clock minus the path's [`Campath::offset`].
    pub time: f64,
    /// Where the camera is.
    pub position: Vec3,
    /// Its rotation (unit quaternion, Source's axes: [`super::angles_to_quat`]).
    pub rotation: Quat,
    /// Field of view as CS2 counts it.
    pub fov: f64,
    /// Selected in HLAE's editor.
    pub selected: bool,
}

/// A `mirv_campath` path.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Campath {
    /// Position interpolation.
    pub position_interp: Interp,
    /// Rotation interpolation.
    pub rotation_interp: Interp,
    /// Field of view interpolation.
    pub fov_interp: Interp,
    /// Added to the points' times (seconds).
    pub offset: f64,
    /// HLAE holds the first and last points before and after the path.
    pub hold: bool,
    /// The points, by time.
    pub points: Vec<CampathPoint>,
}

fn number(value: &str, name: &str) -> Result<f64, HlaeError> {
    value
        .trim()
        .parse()
        .map_err(|_| HlaeError::Campath(format!("{name}={value:?} is not a number")))
}

/// Reads a campath (module docs). Points with the same time: the last one read wins, as in HLAE.
pub fn read_campath(xml: &str) -> Result<Campath, HlaeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut path: Option<Campath> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| HlaeError::Campath(e.to_string()))?;
        let element = match &event {
            Event::Start(e) | Event::Empty(e) => e,
            Event::Eof => break,
            _ => continue,
        };
        let mut attributes = Vec::new();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|e| HlaeError::Campath(e.to_string()))?;
            attributes.push((attribute.key.0.to_owned(), attribute.value.into_owned()));
        }
        let get = |name: &str| {
            attributes
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        match element.name().0 {
            "campath" => {
                path = Some(Campath {
                    position_interp: get("positionInterp").map_or(Interp::Default, Interp::parse),
                    rotation_interp: get("rotationInterp").map_or(Interp::Default, Interp::parse),
                    fov_interp: get("fovInterp").map_or(Interp::Default, Interp::parse),
                    offset: get("offset").map_or(Ok(0.0), |v| number(v, "offset"))?,
                    hold: get("hold").is_some(),
                    points: Vec::new(),
                });
            }
            "p" => {
                let Some(path) = path.as_mut() else {
                    continue;
                };
                let Some(time) = get("t") else { continue };
                let value =
                    |name: &str, default: f64| get(name).map_or(Ok(default), |v| number(v, name));
                let rotation = if let [Some(w), Some(x), Some(y), Some(z)] =
                    ["qw", "qx", "qy", "qz"].map(get)
                {
                    [
                        number(w, "qw")?,
                        number(x, "qx")?,
                        number(y, "qy")?,
                        number(z, "qz")?,
                    ]
                } else {
                    angles_to_quat([value("ry", 0.0)?, value("rz", 0.0)?, value("rx", 0.0)?])
                };
                let point = CampathPoint {
                    time: number(time, "t")?,
                    position: [value("x", 0.0)?, value("y", 0.0)?, value("z", 0.0)?],
                    rotation,
                    fov: value("fov", DEFAULT_FOV)?,
                    selected: get("selected").is_some(),
                };
                match path.points.iter().position(|p| p.time == point.time) {
                    Some(i) => path.points[i] = point,
                    None => path.points.push(point),
                }
            }
            _ => {}
        }
    }
    let mut path = path.ok_or_else(|| HlaeError::Campath("no <campath> element".into()))?;
    path.points.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(path)
}

/// A campath as HLAE saves one (`CamPath::Save`).
pub fn write_campath(path: &Campath) -> String {
    let mut root = String::from("<campath");
    if path.position_interp != Interp::Default {
        root.push_str(&format!(
            " positionInterp=\"{}\"",
            path.position_interp.name(false)
        ));
    }
    if path.rotation_interp != Interp::Default {
        root.push_str(&format!(
            " rotationInterp=\"{}\"",
            path.rotation_interp.name(true)
        ));
    }
    if path.fov_interp != Interp::Default {
        root.push_str(&format!(" fovInterp=\"{}\"", path.fov_interp.name(false)));
    }
    if path.offset != 0.0 {
        root.push_str(&format!(" offset=\"{:.6}\"", path.offset));
    }
    if path.hold {
        root.push_str(" hold=\"\"");
    }
    let mut out = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{root}>\n\t<points>\n");
    out.push_str(
        "\t\t<!--Points are in Quake coordinates, meaning x=forward, y=left, z=up and rotation order is first rx, \
         then ry and lastly rz.\nRotation direction follows the right-hand grip rule.\nrx (roll), ry (pitch), \
         rz(yaw) are the Euler angles in degrees.\nqw, qx, qy, qz are the quaternion values.\nWhen read it is \
         sufficient that either rx, ry, rz OR qw, qx, qy, qz are present.\nIf both are present then qw, qx, qy, qz \
         take precedence.-->\n",
    );
    for p in &path.points {
        let [pitch, yaw, roll] = quat_to_angles(p.rotation);
        out.push_str(&format!(
            "\t\t<p t=\"{:.6}\" x=\"{:.6}\" y=\"{:.6}\" z=\"{:.6}\" fov=\"{:.6}\" rx=\"{roll:.6}\" ry=\"{pitch:.6}\" \
             rz=\"{yaw:.6}\" qw=\"{:.6}\" qx=\"{:.6}\" qy=\"{:.6}\" qz=\"{:.6}\"{}/>\n",
            p.time,
            p.position[0],
            p.position[1],
            p.position[2],
            p.fov,
            p.rotation[0],
            p.rotation[1],
            p.rotation[2],
            p.rotation[3],
            if p.selected { " selected=\"\"" } else { "" }
        ));
    }
    out.push_str("\t</points>\n</campath>\n");
    out
}

impl Campath {
    /// A path through camera keys, at their game times (no offset, HLAE's default interpolation).
    pub fn from_keys(keys: &[CameraKey], clock: Clock) -> Campath {
        Campath {
            points: keys
                .iter()
                .map(|k| CampathPoint {
                    time: clock.time(k.tick),
                    position: k.position,
                    rotation: angles_to_quat(k.angles),
                    fov: k.fov,
                    selected: false,
                })
                .collect(),
            ..Campath::default()
        }
    }

    /// The path sampled on every whole demo tick between its first and last points, and at both of them: the camera
    /// as HLAE moves it. Empty without points.
    pub fn keys(&self, clock: Clock) -> Vec<CameraKey> {
        let Some(sampler) = Sampler::new(self) else {
            return Vec::new();
        };
        let first = clock.tick(self.points[0].time + self.offset);
        let last = clock.tick(self.points[self.points.len() - 1].time + self.offset);
        let mut ticks = vec![first];
        let mut tick = first.floor() + 1.0;
        while tick < last {
            ticks.push(tick);
            tick += 1.0;
        }
        if last > first {
            ticks.push(last);
        }
        ticks
            .into_iter()
            .map(|tick| {
                let (position, rotation, fov) = sampler.eval(clock.time(tick) - self.offset);
                CameraKey {
                    tick,
                    position,
                    angles: quat_to_angles(rotation),
                    fov,
                }
            })
            .collect()
    }
}

/// A campath ready to evaluate at any time ([`Campath::keys`]).
struct Sampler {
    times: Vec<f64>,
    position: [Curve; 3],
    fov: Curve,
    rotation: Rotation,
}

impl Sampler {
    fn new(path: &Campath) -> Option<Sampler> {
        if path.points.is_empty() {
            return None;
        }
        let times: Vec<f64> = path.points.iter().map(|p| p.time).collect();
        let curve = |values: Vec<f64>, interp: Interp| Curve::new(&times, values, interp.cubic());
        Some(Sampler {
            position: std::array::from_fn(|k| {
                curve(
                    path.points.iter().map(|p| p.position[k]).collect(),
                    path.position_interp,
                )
            }),
            fov: curve(path.points.iter().map(|p| p.fov).collect(), path.fov_interp),
            rotation: Rotation::new(
                &times,
                path.points.iter().map(|p| p.rotation).collect(),
                path.rotation_interp.cubic(),
            ),
            times,
        })
    }

    fn eval(&self, time: f64) -> (Vec3, Quat, f64) {
        let t = time.clamp(self.times[0], self.times[self.times.len() - 1]);
        (
            std::array::from_fn(|k| self.position[k].eval(&self.times, t)),
            self.rotation.eval(&self.times, t),
            self.fov.eval(&self.times, t),
        )
    }
}

/// The interval `[x[klo], x[klo + 1]]` holding `t`, by bisection as Numerical Recipes' `splint` finds it.
fn interval(x: &[f64], t: f64) -> usize {
    let (mut lo, mut hi) = (0, x.len() - 1);
    while hi - lo > 1 {
        let k = (hi + lo) / 2;
        if x[k] > t {
            hi = k;
        } else {
            lo = k;
        }
    }
    lo
}

/// One value of a campath over time: a cubic spline at rest at both ends (4 points or more), or linear.
enum Curve {
    Linear(Vec<f64>),
    Spline { y: Vec<f64>, y2: Vec<f64> },
}

impl Curve {
    fn new(x: &[f64], y: Vec<f64>, cubic: bool) -> Curve {
        let n = y.len();
        if !cubic || n < 4 {
            return Curve::Linear(y);
        }
        // Second derivatives of the spline whose first derivative is 0 at both ends (HLAE calls `spline` with
        // `y1Natural` and `ynNatural` false and end slopes 0: the path starts and ends at rest), the tridiagonal
        // system by decomposition.
        let mut y2 = vec![0.0; n];
        let mut u = vec![0.0; n - 1];
        let h0 = x[1] - x[0];
        y2[0] = -0.5;
        u[0] = (3.0 / h0) * ((y[1] - y[0]) / h0);
        for i in 1..n - 1 {
            let sig = (x[i] - x[i - 1]) / (x[i + 1] - x[i - 1]);
            let p = sig * y2[i - 1] + 2.0;
            y2[i] = (sig - 1.0) / p;
            let slope =
                (y[i + 1] - y[i]) / (x[i + 1] - x[i]) - (y[i] - y[i - 1]) / (x[i] - x[i - 1]);
            u[i] = (6.0 * slope / (x[i + 1] - x[i - 1]) - sig * u[i - 1]) / p;
        }
        let hn = x[n - 1] - x[n - 2];
        let (qn, un) = (0.5, (3.0 / hn) * (-(y[n - 1] - y[n - 2]) / hn));
        y2[n - 1] = (un - qn * u[n - 2]) / (qn * y2[n - 2] + 1.0);
        for k in (0..n - 1).rev() {
            y2[k] = y2[k] * y2[k + 1] + u[k];
        }
        Curve::Spline { y, y2 }
    }

    fn eval(&self, x: &[f64], t: f64) -> f64 {
        match self {
            Curve::Linear(y) => {
                if y.len() == 1 {
                    return y[0];
                }
                let i = interval(x, t);
                let h = x[i + 1] - x[i];
                let b = if h > 0.0 { (t - x[i]) / h } else { 0.0 };
                y[i] + (y[i + 1] - y[i]) * b
            }
            Curve::Spline { y, y2 } => {
                let i = interval(x, t);
                let h = x[i + 1] - x[i];
                let a = (x[i + 1] - t) / h;
                let b = (t - x[i]) / h;
                a * y[i]
                    + b * y[i + 1]
                    + ((a * a * a - a) * y2[i] + (b * b * b - b) * y2[i + 1]) * (h * h) / 6.0
            }
        }
    }
}

/// HLAE's threshold for a zero angle (`AFX_MATH_EPS`).
const EPS: f64 = 1e-6;

/// The rotation of a campath over time: McEnnan's quaternion spline (4 points or more; quaternions as `[x, y, z, w]`
/// inside, each turned to the hemisphere of the one before), or spherical linear.
enum Rotation {
    Linear(Vec<Quat>),
    Spline {
        y: Vec<[f64; 4]>,
        h: Vec<f64>,
        dtheta: Vec<f64>,
        e: Vec<Vec3>,
        w: Vec<Vec3>,
    },
}

fn dot3(a: Vec3, b: Vec3) -> f64 {
    super::dot(a, b)
}

/// `a` scaled to unit length (zero stays zero) and its length (`unvec`).
fn unvec(a: Vec3) -> (Vec3, f64) {
    let m = super::length(a);
    if m > 0.0 {
        (super::scale(a, 1.0 / m), m)
    } else {
        ([0.0; 3], m)
    }
}

/// The slew angle (radians) and unit axis from `qi` to `qf` (`[x, y, z, w]`; `getang`).
fn getang(qi: [f64; 4], qf: [f64; 4]) -> (f64, Vec3) {
    let temp = [
        qi[3] * qf[0] - qi[0] * qf[3] - qi[1] * qf[2] + qi[2] * qf[1],
        qi[3] * qf[1] - qi[1] * qf[3] - qi[2] * qf[0] + qi[0] * qf[2],
        qi[3] * qf[2] - qi[2] * qf[3] - qi[0] * qf[1] + qi[1] * qf[0],
    ];
    let ca = qi[0] * qf[0] + qi[1] * qf[1] + qi[2] * qf[2] + qi[3] * qf[3];
    let (e, sa) = unvec(temp);
    (2.0 * sa.atan2(ca), e)
}

/// Between an angular rate and a slew's coefficient vector (`bd`): `to_rate` false gives the coefficients of a
/// rate, true the rate of coefficients.
fn bd(e: Vec3, dtheta: f64, to_rate: bool, x: Vec3) -> Vec3 {
    if dtheta <= EPS {
        return x;
    }
    let (sa, ca) = dtheta.sin_cos();
    let (b1, b2) = if to_rate {
        (sa / dtheta, (ca - 1.0) / dtheta)
    } else {
        (0.5 * dtheta * sa / (1.0 - ca), 0.5 * dtheta)
    };
    let b0 = dot3(x, e);
    let t2 = super::cross(e, x);
    let t1 = super::cross(t2, e);
    std::array::from_fn(|i| b0 * e[i] + b1 * t1[i] + b2 * t2[i])
}

/// The non-linear rate terms of a slew's final acceleration (`rf`).
fn rf(e: Vec3, dtheta: f64, w: Vec3) -> Vec3 {
    if dtheta <= EPS {
        return [0.0; 3];
    }
    let (sa, ca) = dtheta.sin_cos();
    let t2 = super::cross(e, w);
    let t1 = super::cross(t2, e);
    let d = dot3(w, e);
    let mag = dot3(w, w);
    let c1 = 1.0 - ca;
    let r0 = 0.5 * (mag - d * d) * (dtheta - sa) / c1;
    let r1 = d * (dtheta * sa - 2.0 * c1) / (dtheta * c1);
    std::array::from_fn(|i| r0 * e[i] + r1 * t1[i])
}

impl Rotation {
    fn new(x: &[f64], rotations: Vec<Quat>, cubic: bool) -> Rotation {
        let n = rotations.len();
        if !cubic || n < 4 {
            return Rotation::Linear(rotations);
        }
        // [x, y, z, w], the short way from each to the next.
        let mut y: Vec<[f64; 4]> = Vec::with_capacity(n);
        for q in &rotations {
            let mut v = [q[1], q[2], q[3], q[0]];
            if let Some(last) = y.last()
                && v.iter().zip(last).map(|(a, b)| a * b).sum::<f64>() < 0.0
            {
                v = v.map(|c| -c);
            }
            y.push(v);
        }
        let h: Vec<f64> = (0..n - 1).map(|i| x[i + 1] - x[i]).collect();
        let (dtheta, e): (Vec<f64>, Vec<Vec3>) = (0..n - 1).map(|i| getang(y[i], y[i + 1])).unzip();
        // The intermediate angular rates (`rates`, HLAE's 2 iterations at most, tolerance AFX_MATH_EPS), the end
        // rates zero.
        let mut w = vec![[0.0; 3]; n];
        let mut wprev = vec![[0.0; 3]; n];
        let (mut a, mut b, mut c) = (vec![0.0; n - 1], vec![0.0; n - 1], vec![0.0; n - 1]);
        let (wi, wf) = ([0.0; 3], [0.0; 3]);
        let max_iterations = 2;
        let mut iteration = 0;
        loop {
            wprev[1..n - 1].copy_from_slice(&w[1..n - 1]);
            for i in 1..n - 1 {
                a[i] = 2.0 / h[i - 1];
                b[i] = 4.0 / h[i - 1] + 4.0 / h[i];
                c[i] = 2.0 / h[i];
                let t1 = rf(e[i - 1], dtheta[i - 1], wprev[i]);
                for j in 0..3 {
                    w[i][j] = 6.0
                        * (dtheta[i - 1] * e[i - 1][j] / (h[i - 1] * h[i - 1])
                            + dtheta[i] * e[i][j] / (h[i] * h[i]))
                        - t1[j];
                }
            }
            let t1 = bd(e[0], dtheta[0], true, wi);
            let t2 = bd(e[n - 2], dtheta[n - 2], false, wf);
            for j in 0..3 {
                w[1][j] -= a[1] * t1[j];
                w[n - 2][j] -= c[n - 2] * t2[j];
            }
            for i in 1..n - 2 {
                b[i + 1] -= c[i] * a[i + 1] / b[i];
                let t1 = bd(e[i], dtheta[i], true, w[i]);
                for j in 0..3 {
                    w[i + 1][j] -= t1[j] * a[i + 1] / b[i];
                }
            }
            let last = b[n - 2];
            for v in &mut w[n - 2] {
                *v /= last;
            }
            for i in (1..n - 2).rev() {
                let t1 = bd(e[i], dtheta[i], false, w[i + 1]);
                for j in 0..3 {
                    w[i][j] = (w[i][j] - c[i] * t1[j]) / b[i];
                }
            }
            let dw = (1..n - 1)
                .flat_map(|i| (0..3).map(move |j| (i, j)))
                .map(|(i, j)| (w[i][j] - wprev[i][j]).powi(2))
                .sum::<f64>()
                .sqrt();
            let more = iteration < max_iterations && dw > EPS;
            iteration += 1;
            if !more {
                break;
            }
        }
        w[0] = wi;
        w[n - 1] = wf;
        Rotation::Spline { y, h, dtheta, e, w }
    }

    fn eval(&self, x: &[f64], t: f64) -> Quat {
        match self {
            Rotation::Linear(q) => {
                if q.len() == 1 {
                    return q[0];
                }
                let i = interval(x, t);
                let span = x[i + 1] - x[i];
                slerp(
                    q[i],
                    q[i + 1],
                    if span > 0.0 { (t - x[i]) / span } else { 0.0 },
                )
            }
            Rotation::Spline { y, h, dtheta, e, w } => {
                let k = interval(x, t);
                let q = slew3(t - x[k], h[k], dtheta[k], e[k], w[k], w[k + 1], y[k]);
                [q[3], q[0], q[1], q[2]]
            }
        }
    }
}

/// The attitude `t` seconds into a third-order slew of `dt` seconds from `qi` (`[x, y, z, w]`) turning `dtheta` about
/// `e`, starting at the rate `wi` and ending at `wf` (`slew3_init` and `slew3`, the attitude only).
fn slew3(t: f64, dt: f64, dtheta: f64, e: Vec3, wi: Vec3, wf: Vec3, qi: [f64; 4]) -> [f64; 4] {
    if dt <= 0.0 {
        return qi;
    }
    let bvec = if dtheta > EPS {
        let (sa, ca) = dtheta.sin_cos();
        let c1 = 0.5 * sa * dtheta / (1.0 - ca);
        let c2 = 0.5 * dtheta;
        let b0 = dot3(e, wf);
        let v2 = super::cross(e, wf);
        let v1 = super::cross(v2, e);
        std::array::from_fn(|i| b0 * e[i] + c1 * v1[i] + c2 * v2[i])
    } else {
        wf
    };
    let a2: Vec3 = std::array::from_fn(|i| e[i] * dtheta);
    let a0: Vec3 = std::array::from_fn(|i| wi[i] * dt);
    let a1: Vec3 = std::array::from_fn(|i| bvec[i] * dt - 3.0 * a2[i]);
    let x = t / dt;
    let x1 = x - 1.0;
    let th0: Vec3 = std::array::from_fn(|i| ((x * a2[i] + x1 * a1[i]) * x + x1 * x1 * a0[i]) * x);
    let (u, ang) = unvec(th0);
    let (sa, ca) = (0.5 * ang).sin_cos();
    [
        ca * qi[0] + sa * (u[2] * qi[1] - u[1] * qi[2] + u[0] * qi[3]),
        ca * qi[1] + sa * (-u[2] * qi[0] + u[0] * qi[2] + u[1] * qi[3]),
        ca * qi[2] + sa * (u[1] * qi[0] - u[0] * qi[1] + u[2] * qi[3]),
        ca * qi[3] + sa * (-u[0] * qi[0] - u[1] * qi[1] - u[2] * qi[2]),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::wrap_degrees;
    use super::*;

    const CLOCK: Clock = Clock {
        tick_interval: 1.0 / 64.0,
        server_tick_offset: 2403,
    };

    fn key(tick: f64, position: Vec3, angles: Vec3, fov: f64) -> CameraKey {
        CameraKey {
            tick,
            position,
            angles,
            fov,
        }
    }

    fn assert_angles(a: Vec3, b: Vec3, tolerance: f64) {
        for k in 0..3 {
            assert!(
                wrap_degrees(a[k] - b[k]).abs() <= tolerance,
                "{a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn clock_maps_game_time_to_demo_ticks() {
        let time = CLOCK.time(1000.0);
        assert!((time - 3403.0 / 64.0).abs() < 1e-12);
        assert!((CLOCK.tick(time) - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn cam_files_round_trip() {
        let keys = [
            key(100.0, [1.0, 2.0, 3.0], [10.0, 20.0, 5.0], 90.0),
            key(100.5, [4.0, -5.0, 6.5], [-12.0, 179.0, 0.0], 75.0),
        ];
        let text = write_cam(&keys, CLOCK, 16.0 / 9.0);
        assert!(text.starts_with("advancedfx Cam\nversion 2\n"));
        let file = read_cam(&text).unwrap();
        assert_eq!(file.version, 2);
        assert_eq!(file.fov_scaling, FovScaling::Default);
        // Written as the picture's field of view.
        assert!((file.frames[0].fov - 106.260_205).abs() < 1e-5);
        for (a, b) in file.keys(CLOCK, 16.0 / 9.0).iter().zip(&keys) {
            assert!((a.tick - b.tick).abs() < 1e-3);
            assert!((0..3).all(|k| (a.position[k] - b.position[k]).abs() < 1e-5));
            assert_angles(a.angles, b.angles, 1e-5);
            assert!((a.fov - b.fov).abs() < 1e-4);
        }
    }

    #[test]
    fn cam_columns_are_roll_pitch_yaw() {
        let text = "advancedfx Cam\r\nversion 1\r\nscaleFov none\r\nchannels time xPosition yPosition zPosition \
                    xRotation yRotation zRotation fov\r\nDATA\r\n1.5 10 20 30 3 -4 170 90\r\n";
        let file = read_cam(text).unwrap();
        assert_eq!(file.fov_scaling, FovScaling::None);
        assert_eq!(file.frames[0].angles, [-4.0, 170.0, 3.0]);
        assert_eq!(file.keys(CLOCK, 16.0 / 9.0)[0].fov, 90.0);
        assert!(read_cam("advancedfx Cam\nversion 3\nDATA\n").is_err());
        assert!(read_cam("hello").is_err());
    }

    /// A campath as HLAE saves one (its comment, `selected`, quaternions and Euler angles).
    const HLAE_CAMPATH: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<campath rotationInterp="sLinear" offset="1.500000">
	<points>
		<!--Points are in Quake coordinates.-->
		<p t="100.000000" x="0.000000" y="0.000000" z="64.000000" fov="90.000000" rx="0.000000" ry="0.000000" rz="0.000000" qw="1.000000" qx="0.000000" qy="0.000000" qz="0.000000"/>
		<p t="101.000000" x="100.000000" y="0.000000" z="64.000000" fov="80.000000" rx="0.000000" ry="0.000000" rz="90.000000" selected=""/>
	</points>
</campath>
"#;

    #[test]
    fn campaths_read_as_hlae_saves_them() {
        let path = read_campath(HLAE_CAMPATH).unwrap();
        assert_eq!(path.rotation_interp, Interp::Linear);
        assert_eq!(path.position_interp, Interp::Default);
        assert_eq!(path.offset, 1.5);
        assert_eq!(path.points.len(), 2);
        assert!(path.points[1].selected);
        assert_angles(
            quat_to_angles(path.points[1].rotation),
            [0.0, 90.0, 0.0],
            1e-9,
        );
        // Two points: linear. Halfway in time, halfway along and turned 45°.
        let keys = path.keys(CLOCK);
        let start = CLOCK.tick(101.5);
        assert!((keys[0].tick - start).abs() < 1e-9);
        assert_eq!(keys.len(), 1 + 63 + 1);
        let mid = keys
            .iter()
            .find(|k| (k.tick - (start + 32.0)).abs() < 1e-6)
            .unwrap();
        assert!((mid.position[0] - 50.0).abs() < 1e-6);
        assert!((mid.fov - 85.0).abs() < 1e-9);
        assert_angles(mid.angles, [0.0, 45.0, 0.0], 1e-6);
        // And back out, as HLAE would read it.
        let again = read_campath(&write_campath(&path)).unwrap();
        assert_eq!(again.points.len(), 2);
        assert_eq!(again.offset, 1.5);
        assert_eq!(again.rotation_interp, Interp::Linear);
        assert!((again.points[1].fov - 80.0).abs() < 1e-9);
    }

    #[test]
    fn cubic_paths_go_through_their_points_smoothly() {
        let keys: Vec<CameraKey> = (0..6)
            .map(|i| {
                let i = f64::from(i);
                key(
                    1000.0 + i * 64.0,
                    [i * 100.0, (i * 0.7).sin() * 80.0, 64.0 + i * 5.0],
                    [i * 4.0 - 10.0, i * 50.0 - 120.0, 0.0],
                    90.0 - i * 3.0,
                )
            })
            .collect();
        let path = Campath::from_keys(&keys, CLOCK);
        let sampled = path.keys(CLOCK);
        // Through every point.
        for k in &keys {
            let s = sampled
                .iter()
                .find(|s| (s.tick - k.tick).abs() < 1e-6)
                .unwrap();
            assert!(
                (0..3).all(|j| (s.position[j] - k.position[j]).abs() < 1e-6),
                "{s:?} {k:?}"
            );
            assert_angles(s.angles, k.angles, 1e-6);
            assert!((s.fov - k.fov).abs() < 1e-6);
        }
        // No jumps: neighbouring ticks turn by little (the points are 50° of yaw a second apart).
        for pair in sampled.windows(2) {
            let turn = wrap_degrees(pair[1].angles[1] - pair[0].angles[1]).abs();
            assert!(turn < 2.5, "{turn} between {:?} and {:?}", pair[0], pair[1]);
        }
        // A cubic spline: at an inner point its slope is continuous (equal difference quotients on each side).
        let at = |tick: f64| {
            sampled
                .iter()
                .find(|s| (s.tick - tick).abs() < 1e-6)
                .unwrap()
                .position[0]
        };
        let left = at(1128.0) - at(1127.0);
        let right = at(1129.0) - at(1128.0);
        assert!((left - right).abs() < 0.05, "{left} {right}");
        // At rest at both ends, as HLAE's (its spline's slope is 0 there): the first tick moves far less.
        let first = at(1001.0) - at(1000.0);
        let last = at(1320.0) - at(1319.0);
        assert!(
            first.abs() < right.abs() / 10.0 && last.abs() < right.abs() / 10.0,
            "{first} {last} {right}"
        );
    }
}
