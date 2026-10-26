//! A compressed animation clip (`.vnmclip_c`, KV3 `CNmClip` in the `DATA` block).
//!
//! # Layout
//!
//! | Key | Value |
//! |---|---|
//! | `m_skeleton` | the `.vnmskel` whose bone order the tracks follow |
//! | `m_nNumFrames`, `m_flDuration` | frames and seconds: moving player clips are 30 fps, `duration = (frames − 1) / 30`; a still clip can store 2 frames over its whole length (`idle_rifle`: 0.73 s) |
//! | `m_trackCompressionSettings` | one entry per skeleton bone: translation ranges per axis, scale range, constant rotation, which parts are static |
//! | `m_compressedPoseData` | little-endian `u16` stream, every frame's dynamic values |
//! | `m_compressedPoseOffsets` | where each frame starts in that stream, in `u16` units |
//! | `m_bIsAdditive` | the poses are deltas over another clip (per-weapon idles, shoot, flinch) |
//! | `m_secondaryAnimations` | nested clips for other skeletons (a weapon's magazine and bolt during reload) |
//! | `m_events` | `CNmIDEvent`s: `m_ID` (`WPN_IK_ACTION_ENDING`, `KnifeSwingBack`…) over `m_flStartTime` + `m_flDuration`, both **fractions of the clip** (`{ m_flValue }`; `reload_ak`'s IK event runs 0.849 + 0.151 of its 2.43 s) |
//! | `m_syncTrack`, `m_rootMotion` | foot sync markers, root motion (the pose's `root_motion` bone stays at the origin) |
//!
//! # Decoding a frame
//!
//! For each track, in bone order, start from the static values (constant rotation, translation = the ranges'
//! starts, scale = the scale range's start), then read from the frame's position in the stream:
//!
//! - **rotation** (unless static), 3 × `u16`: `a = d0 & 0x7FFF`, `b = d1 & 0x7FFF`, `c = d2`, each mapped to
//!   `n × (√2 / 32767) − 1/√2`; the missing (largest) component is `√(1 − a² − b² − c²)`, placed at index
//!   `((d0 >> 14) & 2) | (d1 >> 15)` (0 = x … 3 = w), the other slots taking a, b, c in order ("smallest three");
//! - **translation** (unless static), 3 × `u16`: `start + u / 65535 × length` per axis;
//! - **scale** (unless static), 1 × `u16`, the same way.

use std::cell::RefCell;

use super::{AnimError, Transform};
use crate::kv3::Value;

thread_local! {
    /// While tracing ([`trace_start`]): per frame, the address of every clip sampled.
    static TRACE: RefCell<Option<Vec<Vec<usize>>>> = const { RefCell::new(None) };
}

/// Starts recording, on this thread, which clips [`Clip::sample`] samples in each frame begun with [`trace_frame`]:
/// for tools that check which clips a bake played (`examples/recipe_table.rs`). Off, it costs a check per sample.
pub fn trace_start() {
    TRACE.with(|t| *t.borrow_mut() = Some(Vec::new()));
}

/// Starts a new frame of the trace (does nothing when not tracing). The bake calls it before each frame it renders.
pub fn trace_frame() {
    TRACE.with(|t| {
        if let Some(frames) = t.borrow_mut().as_mut() {
            frames.push(Vec::new());
        }
    });
}

/// Stops tracing and returns, per frame begun with [`trace_frame`], the addresses of the clips
/// sampled: compare them with `std::ptr::from_ref(clip) as usize` of the clips a tool knows by path.
pub fn trace_end() -> Vec<Vec<usize>> {
    TRACE.with(|t| t.borrow_mut().take()).unwrap_or_default()
}

/// How one track (bone) is stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSettings {
    /// `(start, length)` of the quantised translation, per axis.
    pub translation_range: [(f32, f32); 3],
    /// `(start, length)` of the quantised scale.
    pub scale_range: (f32, f32),
    /// Rotation when it is static.
    pub constant_rotation: [f32; 4],
    /// The rotation never changes (no data in the stream).
    pub rotation_static: bool,
    /// The translation never changes.
    pub translation_static: bool,
    /// The scale never changes.
    pub scale_static: bool,
}

/// A decoded-on-demand animation clip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clip {
    /// Path of the `.vnmskel` the tracks follow (as stored, without `_c`).
    pub skeleton: String,
    /// Number of frames.
    pub frames: usize,
    /// Length in seconds.
    pub duration: f32,
    /// The poses are deltas over another clip.
    pub additive: bool,
    /// One per skeleton bone.
    pub tracks: Vec<TrackSettings>,
    /// The compressed pose stream.
    pub data: Vec<u16>,
    /// Start of each frame in `data`.
    pub offsets: Vec<usize>,
    /// Nested clips for other skeletons (weapon parts).
    pub secondary: Vec<Clip>,
    /// ID events (`m_events`), in their stored order.
    pub events: Vec<ClipEvent>,
}

/// An ID event of a clip: `id` is on from `start` to `start + duration`, as fractions of the clip's length.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipEvent {
    /// The event's ID (`WPN_IK_ACTION_ENDING`).
    pub id: String,
    /// Start, as a fraction of the clip (0 – 1).
    pub start: f32,
    /// Length, as a fraction of the clip.
    pub duration: f32,
}

impl Clip {
    /// Reads a clip from its `DATA` block (or a nested `m_secondaryAnimations` entry).
    pub fn parse(root: &Value) -> Result<Self, AnimError> {
        let number = |key: &str| root.get(key).and_then(Value::as_f64);
        let bytes: &[u8] = match root.get("m_compressedPoseData").map(Value::unflagged) {
            Some(Value::Blob(bytes)) => bytes,
            _ => &[],
        };
        let data = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        let offsets = root
            .get("m_compressedPoseOffsets")
            .and_then(Value::as_array)
            .ok_or(AnimError::MissingKey("m_compressedPoseOffsets"))?
            .iter()
            .map(|v| v.as_i64().unwrap_or(0).max(0) as usize)
            .collect();
        let tracks = root
            .get("m_trackCompressionSettings")
            .and_then(Value::as_array)
            .ok_or(AnimError::MissingKey("m_trackCompressionSettings"))?
            .iter()
            .map(track_settings)
            .collect();
        let secondary = root
            .get("m_secondaryAnimations")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(Clip::parse)
            .collect::<Result<_, _>>()?;
        let events = root
            .get("m_events")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|event| {
                let fraction = |key: &str| {
                    event
                        .get(key)
                        .and_then(|v| v.get("m_flValue").or(Some(v)))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0) as f32
                };
                Some(ClipEvent {
                    id: event.get("m_ID")?.as_str()?.to_owned(),
                    start: fraction("m_flStartTime"),
                    duration: fraction("m_flDuration"),
                })
            })
            .collect();
        Ok(Self {
            skeleton: root
                .get("m_skeleton")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            frames: number("m_nNumFrames").unwrap_or(0.0).max(0.0) as usize,
            duration: number("m_flDuration").unwrap_or(0.0) as f32,
            additive: matches!(
                root.get("m_bIsAdditive").map(Value::unflagged),
                Some(Value::Bool(true))
            ),
            tracks,
            data,
            offsets,
            secondary,
            events,
        })
    }

    /// The span of event `id` in seconds of the clip, `(start, end)`, if the clip has it.
    pub fn event_span(&self, id: &str) -> Option<(f32, f32)> {
        self.events.iter().find(|e| e.id == id).map(|e| {
            (
                e.start * self.duration,
                (e.start + e.duration) * self.duration,
            )
        })
    }

    /// Whether the poses are deltas over another clip (see the module docs of [`crate::anim`]).
    pub fn is_additive(&self) -> bool {
        self.additive
    }

    /// Frames per second: `(frames − 1) / duration` (30 on moving player clips; 1.4 on `idle_rifle`, 2 frames over
    /// 0.73 s).
    pub fn fps(&self) -> f32 {
        if self.frames > 1 && self.duration > 0.0 {
            (self.frames - 1) as f32 / self.duration
        } else {
            30.0
        }
    }

    /// The pose of frame `frame` (clamped to the last frame), one transform per track.
    pub fn decode_frame(&self, frame: usize) -> Result<Vec<Transform>, AnimError> {
        let frame = frame.min(self.frames.saturating_sub(1));
        let mut at = self.offsets.get(frame).copied().unwrap_or(0);
        let mut take = |count: usize| -> Result<&[u16], AnimError> {
            let values = self.data.get(at..at + count).ok_or(AnimError::Truncated {
                frame,
                at,
                len: self.data.len(),
            })?;
            at += count;
            Ok(values)
        };
        let mut pose = Vec::with_capacity(self.tracks.len());
        for track in &self.tracks {
            let starts = track.translation_range.map(|(start, _)| start);
            let mut bone = Transform {
                translation: starts,
                rotation: track.constant_rotation,
                scale: track.scale_range.0,
            };
            if !track.rotation_static {
                let d = take(3)?;
                bone.rotation = decode_quaternion([d[0], d[1], d[2]]);
            }
            if !track.translation_static {
                let d = take(3)?;
                bone.translation = std::array::from_fn(|k| {
                    let (start, length) = track.translation_range[k];
                    decode_unorm(d[k], start, length)
                });
            }
            if !track.scale_static {
                let d = take(1)?;
                bone.scale = decode_unorm(d[0], track.scale_range.0, track.scale_range.1);
            }
            pose.push(bone);
        }
        Ok(pose)
    }

    /// The pose at `time` seconds, interpolated between the two nearest frames. Past the end it holds the last
    /// frame, or wraps when `looping` (the last frame of a looping clip equals its first).
    pub fn sample(&self, time: f32, looping: bool) -> Result<Vec<Transform>, AnimError> {
        TRACE.with(|t| {
            if let Some(frame) = t.borrow_mut().as_mut().and_then(|f| f.last_mut()) {
                frame.push(std::ptr::from_ref(self) as usize);
            }
        });
        let last = self.frames.saturating_sub(1);
        if last == 0 {
            return self.decode_frame(0);
        }
        let mut t = time.max(0.0);
        if looping && self.duration > 0.0 {
            t %= self.duration;
        }
        let position = (t * self.fps()).min(last as f32);
        let first = position.floor() as usize;
        let fraction = position - first as f32;
        let a = self.decode_frame(first)?;
        if fraction <= f32::EPSILON || first >= last {
            return Ok(a);
        }
        let b = self.decode_frame(first + 1)?;
        Ok(a.iter().zip(&b).map(|(x, y)| x.lerp(y, fraction)).collect())
    }
}

fn track_settings(entry: &Value) -> TrackSettings {
    let range = |key: &str| -> (f32, f32) {
        let r = entry.get(key);
        let f = |k: &str| {
            r.and_then(|r| r.get(k))
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32
        };
        (f("m_flRangeStart"), f("m_flRangeLength"))
    };
    let flag = |key: &str| {
        matches!(
            entry.get(key).map(Value::unflagged),
            Some(Value::Bool(true))
        )
    };
    let mut rotation = Vec::new();
    if let Some(v) = entry.get("m_constantRotation") {
        crate::map::numbers(v, &mut rotation);
    }
    let constant_rotation = if rotation.len() >= 4 {
        [rotation[0], rotation[1], rotation[2], rotation[3]]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    TrackSettings {
        translation_range: [
            range("m_translationRangeX"),
            range("m_translationRangeY"),
            range("m_translationRangeZ"),
        ],
        scale_range: range("m_scaleRange"),
        constant_rotation,
        rotation_static: flag("m_bIsRotationStatic"),
        translation_static: flag("m_bIsTranslationStatic"),
        scale_static: flag("m_bIsScaleStatic"),
    }
}

/// `start + value / 65535 × length`.
fn decode_unorm(value: u16, start: f32, length: f32) -> f32 {
    start + f32::from(value) / f32::from(u16::MAX) * length
}

/// CS2's "smallest three" quaternion in three `u16` (see the module docs).
pub(crate) fn decode_quaternion(d: [u16; 3]) -> [f32; 4] {
    let min = -std::f32::consts::FRAC_1_SQRT_2;
    let step = std::f32::consts::SQRT_2 / 32767.0;
    let a = f32::from(d[0] & 0x7FFF) * step + min;
    let b = f32::from(d[1] & 0x7FFF) * step + min;
    let c = f32::from(d[2]) * step + min;
    let largest = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
    match ((d[0] >> 14) & 2) | (d[1] >> 15) {
        0 => [largest, a, b, c],
        1 => [a, largest, b, c],
        2 => [a, b, largest, c],
        _ => [a, b, c, largest],
    }
}

/// The inverse of [`decode_quaternion`], for tests: the largest component is dropped (made positive by flipping
/// the whole quaternion), the others quantised to 15 bits.
#[cfg(test)]
pub(crate) fn encode_quaternion(q: [f32; 4]) -> [u16; 3] {
    let largest = (0..4)
        .max_by(|&i, &j| q[i].abs().total_cmp(&q[j].abs()))
        .unwrap_or(3);
    let sign = if q[largest] < 0.0 { -1.0 } else { 1.0 };
    let rest: Vec<f32> = (0..4)
        .filter(|&k| k != largest)
        .map(|k| q[k] * sign)
        .collect();
    let min = -std::f32::consts::FRAC_1_SQRT_2;
    let step = std::f32::consts::SQRT_2 / 32767.0;
    let quantise = |v: f32| (((v - min) / step).round().clamp(0.0, 32767.0)) as u16;
    let [a, b, c] = [quantise(rest[0]), quantise(rest[1]), quantise(rest[2])];
    let index = largest as u16;
    [a | ((index & 2) << 14), b | ((index & 1) << 15), c]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::normalize_quat;

    fn close(a: &[f32], b: &[f32], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < tolerance)
    }

    #[test]
    fn smallest_three_quaternions_round_trip() {
        for q in [
            [0.5, 0.5, 0.5, -0.5],
            normalize_quat([0.1, -0.7, 0.2, 0.3]),
            normalize_quat([0.9, 0.1, 0.0, 0.2]),
            [0.0, 0.0, 0.0, 1.0],
            normalize_quat([-0.3, 0.2, 0.8, -0.1]),
        ] {
            let decoded = decode_quaternion(encode_quaternion(q));
            // q and −q are the same rotation.
            let dot: f32 = (0..4).map(|k| q[k] * decoded[k]).sum();
            assert!(dot.abs() > 0.9999, "{q:?} → {decoded:?}");
        }
    }

    /// A two-track, two-frame clip built like CS2 stores it: track 0 fully static, track 1 with dynamic rotation,
    /// translation and scale.
    fn sample_clip() -> Clip {
        let still = TrackSettings {
            translation_range: [(0.0, 0.1); 3],
            scale_range: (1.0, 0.1),
            constant_rotation: [0.0, 0.0, 0.0, 1.0],
            rotation_static: true,
            translation_static: true,
            scale_static: true,
        };
        let moving = TrackSettings {
            translation_range: [(-4.0, 2.0), (0.0, 10.0), (40.0, 4.0)],
            scale_range: (0.5, 1.0),
            constant_rotation: [0.0, 0.0, 0.0, 1.0],
            rotation_static: false,
            translation_static: false,
            scale_static: false,
        };
        let frame = |q: [f32; 4], t: [u16; 3], s: u16| {
            let r = encode_quaternion(q);
            vec![r[0], r[1], r[2], t[0], t[1], t[2], s]
        };
        let mut data = frame([0.5, 0.5, 0.5, -0.5], [0, 65535, 32768], 32768);
        data.extend(frame([0.0, 0.0, 0.0, 1.0], [65535, 0, 0], 0));
        Clip {
            skeleton: "animation/skeletons/characters/worldmodel.vnmskel".into(),
            frames: 2,
            duration: 1.0 / 30.0,
            additive: false,
            tracks: vec![still, moving],
            data,
            offsets: vec![0, 7],
            secondary: Vec::new(),
            events: vec![ClipEvent {
                id: "WPN_IK_ACTION_ENDING".into(),
                start: 0.5,
                duration: 0.5,
            }],
        }
    }

    #[test]
    fn frames_decode_static_and_dynamic_tracks() {
        let clip = sample_clip();
        assert!((clip.fps() - 30.0).abs() < 1e-3);
        // Event fractions turn into seconds of the clip.
        let (start, end) = clip.event_span("WPN_IK_ACTION_ENDING").unwrap();
        assert!((start - 0.5 / 30.0).abs() < 1e-6 && (end - 1.0 / 30.0).abs() < 1e-6);
        assert!(clip.event_span("KnifeSwingBack").is_none());
        let f0 = clip.decode_frame(0).unwrap();
        assert_eq!(f0[0], Transform::IDENTITY);
        assert!(
            close(&f0[1].translation, &[-4.0, 10.0, 42.0], 1e-3),
            "{:?}",
            f0[1]
        );
        assert!((f0[1].scale - 1.0).abs() < 1e-3);
        let dot: f32 = (0..4)
            .map(|k| f0[1].rotation[k] * [0.5, 0.5, 0.5, -0.5][k])
            .sum();
        assert!(dot.abs() > 0.9999);
        let f1 = clip.decode_frame(1).unwrap();
        assert!(close(&f1[1].translation, &[-2.0, 0.0, 40.0], 1e-3));
        // Past the end: the last frame.
        assert_eq!(clip.decode_frame(5).unwrap(), f1);
    }

    #[test]
    fn sampling_interpolates_between_frames() {
        let clip = sample_clip();
        let half = clip.sample(0.5 / 30.0, false).unwrap();
        assert!(
            close(&half[1].translation, &[-3.0, 5.0, 41.0], 1e-3),
            "{:?}",
            half[1]
        );
        // Looping wraps; holding clamps.
        assert!(close(
            &clip.sample(clip.duration + 0.5 / 30.0, true).unwrap()[1].translation,
            &half[1].translation,
            1e-3
        ));
        assert_eq!(
            clip.sample(10.0, false).unwrap(),
            clip.decode_frame(1).unwrap()
        );
    }

    #[test]
    fn truncated_data_is_an_error() {
        let mut clip = sample_clip();
        clip.data.truncate(9);
        assert!(clip.decode_frame(1).is_err());
    }
}
