//! A model's **reference pose**: the first frame of the animation embedded in the model (its `ANIM` block, named
//! `ref`), which is how CS2 draws a model nothing animates — props, doors, fans. The mesh's vertices are stored at
//! the skeleton's bind pose (`m_modelSkeleton`); for most models the reference pose is that bind pose, but some turn
//! a bone: de_nuke's metal doors (`metal_door_001_br`) have a root bone at 90° about x in the bind pose and at
//! (0.5, 0.5, 0.5, 0.5) in `ref`, so drawn at bind pose they stand 90° about z from where CS2 shows them (a closed
//! door looks open).
//!
//! # Layout
//!
//! The `ANIM` block lists animations (`m_animArray`), each with frame blocks (`m_pData.m_frameblockArray`: a frame
//! range and the indices of its segments in `m_segmentArray`). The `AGRP` block's decode key
//! (`m_decodeKey.m_dataChannelArray`) names the channels: `Position`, `Angle` and `Scale` bone channels, each with
//! element names (bones) and element indices. A segment (`m_nLocalChannel`, `m_container`) holds one channel's values
//! for some elements; its container is little-endian:
//!
//! | Bytes | What |
//! |---|---|
//! | u16 | decoder: an index into `m_decoderArray` (`CCompressedStaticVector3`, `CCompressedAnimQuaternion`, …) |
//! | u16 | cardinality (values per element) |
//! | u16 | element count n |
//! | u16 | the container's total length |
//! | n × u16 | element indices (the channel's `m_nElementIndexArray` values) |
//! | … | values: once per element for `Static` decoders, else per frame of the block, frame by frame |
//!
//! Values: `…Float` an f32; `…StaticVector3` / `…AnimVector3` three half floats, `…FullVector3` three f32;
//! `…DeltaVector3` three f32 bases then three half deltas per frame; `…StaticQuaternion` / `…AnimQuaternion` 48 bits,
//! `…FullQuaternion` four f32. A 48-bit quaternion is three 16-bit words: the low 15 bits of each, minus 16384, times
//! sin(45°) / 16384, are the three smallest components; the top bits of the first two words say which component was
//! dropped (none set: w; second: x; first: y; both: z) and the third word's the sign of that one, which is
//! √(1 − the others²).
//!
//! Only the reference frame is read (frame 0 of `ref`, else of the first animation that is not a delta). Knowledge from
//! ValveResourceFormat (MIT, `ModelAnimation/` segment decoders), checked on de_nuke's door against its own physics
//! hull; independent implementation.

use super::{Affine, Skeleton, affine, compose, half_to_f32, normalize_quat};
use crate::kv3::Value;
use crate::resource::{BlockKind, Resource};

/// One bone's reference transform relative to its parent, where the reference frame has data for it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RefBone {
    /// Translation from the parent, inches.
    pub position: Option<[f32; 3]>,
    /// Rotation from the parent, `(x, y, z, w)`.
    pub rotation: Option<[f32; 4]>,
}

/// The reference frame's local transform of each skeleton bone that has one (`None` without an embedded animation
/// or when the blocks cannot be read).
pub fn reference_bones(resource: &Resource<'_>, skeleton: &Skeleton) -> Option<Vec<RefBone>> {
    let anim = resource.kv3(BlockKind(*b"ANIM")).ok()?.root;
    let group = resource.kv3(BlockKind(*b"AGRP")).ok()?.root;
    let animations = anim.get("m_animArray")?.as_array()?;
    // `ref`, else the first animation that is a pose of its own (not deltas over another one).
    let delta = |a: &&Value| {
        a.get("m_flags")
            .and_then(|f| f.get("m_bDelta"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let animation = animations
        .iter()
        .find(|a| a.get("m_name").and_then(Value::as_str) == Some("ref"))
        .or_else(|| animations.iter().find(|a| !delta(a)))
        .filter(|a| !delta(a))?;
    let block = animation
        .get("m_pData")?
        .get("m_frameblockArray")?
        .as_array()?
        .iter()
        .find(|b| {
            let start = b.get("m_nStartFrame").and_then(Value::as_i64).unwrap_or(0);
            let end = b.get("m_nEndFrame").and_then(Value::as_i64).unwrap_or(0);
            start <= 0 && 0 <= end
        })?;
    let decoders: Vec<&str> = anim
        .get("m_decoderArray")?
        .as_array()?
        .iter()
        .map(|d| {
            d.get("m_szName")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    let segments = anim.get("m_segmentArray")?.as_array()?;
    let channels = group
        .get("m_decodeKey")?
        .get("m_dataChannelArray")?
        .as_array()?;
    let mut bones = vec![RefBone::default(); skeleton.bones.len()];
    for index in block.get("m_segmentIndexArray")?.as_array()? {
        let Some(segment) = index
            .as_i64()
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| segments.get(i))
        else {
            continue;
        };
        let Some(channel) = segment
            .get("m_nLocalChannel")
            .and_then(Value::as_i64)
            .and_then(|c| usize::try_from(c).ok())
            .and_then(|c| channels.get(c))
        else {
            continue;
        };
        let Some(Value::Blob(container)) = segment.get("m_container").map(Value::unflagged) else {
            continue;
        };
        let Some(values) = decode_segment(container, &decoders) else {
            continue;
        };
        let variable = channel
            .get("m_szVariableName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let names = channel
            .get("m_szElementNameArray")
            .and_then(Value::as_array);
        let indices = channel
            .get("m_nElementIndexArray")
            .and_then(Value::as_array);
        let (Some(names), Some(indices)) = (names, indices) else {
            continue;
        };
        for (element, value) in values {
            // The element index → its name in the channel → the skeleton's bone.
            let Some(bone) = indices
                .iter()
                .position(|i| i.as_i64() == Some(i64::from(element)))
                .and_then(|k| names.get(k))
                .and_then(Value::as_str)
                .and_then(|name| skeleton.find(name))
            else {
                continue;
            };
            match (variable, value) {
                ("Position", Decoded::Vector(v)) => bones[bone].position = Some(v),
                ("Angle", Decoded::Quaternion(q)) => bones[bone].rotation = Some(normalize_quat(q)),
                _ => {}
            }
        }
    }
    Some(bones)
}

/// Each skeleton bone in model space at the reference pose: the reference frame's local transform where it has one,
/// else the bind pose's (`None` without an embedded animation).
pub fn reference_pose(resource: &Resource<'_>, skeleton: &Skeleton) -> Option<Vec<Affine>> {
    let bones = reference_bones(resource, skeleton)?;
    let mut world: Vec<Affine> = Vec::with_capacity(skeleton.bones.len());
    for (bone, reference) in skeleton.bones.iter().zip(&bones) {
        let local = affine(
            reference.rotation.unwrap_or(bone.rotation),
            reference.position.unwrap_or(bone.position),
        );
        world.push(match bone.parent {
            Some(p) => compose(&world[p], &local),
            None => local,
        });
    }
    Some(world)
}

/// A decoded value of a segment.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Decoded {
    Float(f32),
    Vector([f32; 3]),
    Quaternion([f32; 4]),
}

/// The first frame's values of a segment container: (element index, value) per element. `None` for a decoder this
/// reader does not know or a container too short.
fn decode_segment(container: &[u8], decoders: &[&str]) -> Option<Vec<(u16, Decoded)>> {
    let word = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes([
            *container.get(at)?,
            *container.get(at + 1)?,
        ]))
    };
    let decoder = *decoders.get(usize::from(word(0)?))?;
    let count = usize::from(word(4)?);
    let elements: Vec<u16> = (0..count).map(|k| word(8 + 2 * k)).collect::<Option<_>>()?;
    let data = container.get(8 + 2 * count..)?;
    let f32_at = |at: usize| -> Option<f32> {
        Some(f32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let half_at = |at: usize| -> Option<f32> {
        Some(half_to_f32(u16::from_le_bytes(
            data.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    // Frame 0 of the block: per-frame decoders start with it, so element k's value sits at k × size either way.
    let value = |k: usize| -> Option<Decoded> {
        Some(match decoder {
            "CCompressedStaticFloat" | "CCompressedFullFloat" => Decoded::Float(f32_at(4 * k)?),
            "CCompressedStaticVector3" | "CCompressedAnimVector3" => {
                Decoded::Vector([half_at(6 * k)?, half_at(6 * k + 2)?, half_at(6 * k + 4)?])
            }
            "CCompressedStaticFullVector3" | "CCompressedFullVector3" => {
                Decoded::Vector([f32_at(12 * k)?, f32_at(12 * k + 4)?, f32_at(12 * k + 8)?])
            }
            "CCompressedDeltaVector3" => {
                let deltas = 12 * count;
                Decoded::Vector(std::array::from_fn(|i| {
                    f32_at(12 * k + 4 * i).unwrap_or(0.0)
                        + half_at(deltas + 6 * k + 2 * i).unwrap_or(0.0)
                }))
            }
            "CCompressedStaticQuaternion" | "CCompressedAnimQuaternion" => {
                Decoded::Quaternion(quaternion_48(data.get(6 * k..6 * k + 6)?.try_into().ok()?))
            }
            "CCompressedFullQuaternion" => Decoded::Quaternion([
                f32_at(16 * k)?,
                f32_at(16 * k + 4)?,
                f32_at(16 * k + 8)?,
                f32_at(16 * k + 12)?,
            ]),
            _ => return None,
        })
    };
    elements
        .iter()
        .enumerate()
        .map(|(k, &e)| Some((e, value(k)?)))
        .collect()
}

/// A 48-bit compressed quaternion `(x, y, z, w)` (see the module docs).
fn quaternion_48(bytes: [u8; 6]) -> [f32; 4] {
    let words = [0, 2, 4].map(|i| u16::from_le_bytes([bytes[i], bytes[i + 1]]));
    let scale = std::f32::consts::FRAC_1_SQRT_2 / 16384.0;
    let [a, b, c] = words.map(|w| (f32::from(w & 0x7FFF) - 16384.0) * scale);
    let mut largest = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
    if words[2] & 0x8000 != 0 {
        largest = -largest;
    }
    match (words[0] & 0x8000 != 0, words[1] & 0x8000 != 0) {
        (false, false) => [a, b, c, largest],
        (false, true) => [largest, a, b, c],
        (true, false) => [c, largest, a, b],
        (true, true) => [b, c, largest, a],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// de_nuke's `metal_door_001_br`: the reference frame's three segments as stored.
    #[test]
    fn decodes_the_door_reference_frame() {
        let decoders = [
            "CCompressedStaticFloat",
            "CCompressedFullFloat",
            "CCompressedStaticVector3",
            "CCompressedStaticFullVector3",
            "CCompressedAnimVector3",
            "CCompressedDeltaVector3",
            "CCompressedFullVector3",
            "CCompressedStaticQuaternion",
            "CCompressedAnimQuaternion",
        ];
        let position = [2, 0, 3, 0, 1, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let angle = [
            8, 0, 4, 0, 1, 0, 0x10, 0, 1, 0, 0x3F, 0x6D, 0x3F, 0x6D, 0x3F, 0x6D,
        ];
        let scale = [0, 0, 1, 0, 1, 0, 0x0E, 0, 2, 0, 0, 0, 0x80, 0x3F];
        assert_eq!(
            decode_segment(&position, &decoders),
            Some(vec![(0, Decoded::Vector([0.0; 3]))])
        );
        assert_eq!(
            decode_segment(&scale, &decoders),
            Some(vec![(2, Decoded::Float(1.0))])
        );
        let Some(values) = decode_segment(&angle, &decoders) else {
            panic!("angle decodes");
        };
        let (element, Decoded::Quaternion(q)) = values[0] else {
            panic!("a quaternion");
        };
        assert_eq!(element, 1);
        // (0.5, 0.5, 0.5, 0.5): the root of CS2's agents has the same.
        assert!(q.iter().all(|c| (c - 0.5).abs() < 1e-3), "{q:?}");
    }

    #[test]
    fn quaternion_48_names_the_dropped_component() {
        // Three zeros (16384) with each choice of the dropped component, positive and negative.
        let zeros = |first: bool, second: bool, negative: bool| {
            let w = |top: bool| 16384u16 | if top { 0x8000 } else { 0 };
            let [a, b, c] = [w(first), w(second), w(negative)].map(u16::to_le_bytes);
            quaternion_48([a[0], a[1], b[0], b[1], c[0], c[1]])
        };
        assert_eq!(zeros(false, false, false), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(zeros(false, true, false), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(zeros(true, false, false), [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(zeros(true, true, true), [0.0, 0.0, -1.0, 0.0]);
    }
}
