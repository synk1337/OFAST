//! A clip skeleton (`.vnmskel_c`, KV3 in the `DATA` block): the bone order of every clip made for it.
//!
//! | Key | Value |
//! |---|---|
//! | `m_boneIDs` | bone names (`root_motion`, `pelvis`, …, `wpnPivot`, `attachWorld`, `camera`) |
//! | `m_parentIndices` | parent bone index, −1 for the root |
//! | `m_parentSpaceReferencePose` | per bone, `[tx, ty, tz, scale, qx, qy, qz, qw]` relative to the parent |
//! | `m_modelSpaceReferencePose` | the same in model space |
//! | `m_maskDefinitions` | named bone weight masks (UpperBody, Flinch, …) used by the graph |
//!
//! `worldmodel.vnmskel` (every third-person player clip): 75 bones; `root_motion` at the identity, `pelvis` at
//! (−2.85, 0, 42.82) inches with rotation (0.5, 0.5, 0.5, −0.5), the same model-space pose as an agent model's pelvis.

use super::Transform;
use crate::kv3::Value;

/// A clip skeleton.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NmSkeleton {
    /// Bone names, in track order.
    pub bones: Vec<String>,
    /// Parent of each bone (`None` for a root).
    pub parents: Vec<Option<usize>>,
    /// Reference pose relative to the parent.
    pub reference: Vec<Transform>,
}

impl NmSkeleton {
    /// Reads a `.vnmskel` `DATA` block.
    pub fn parse(root: &Value) -> Self {
        let bones: Vec<String> = root
            .get("m_boneIDs")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_owned())
            .collect();
        let parents_raw: Vec<i64> = root
            .get("m_parentIndices")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|v| v.as_i64().unwrap_or(-1))
            .collect();
        let reference_raw = root
            .get("m_parentSpaceReferencePose")
            .and_then(Value::as_array)
            .unwrap_or_default();
        let parents = (0..bones.len())
            .map(|i| {
                let p = parents_raw.get(i).copied().unwrap_or(-1);
                // A parent after the bone would break the parents-first order every pose chain relies on.
                (p >= 0 && (p as usize) < i).then_some(p as usize)
            })
            .collect();
        let reference = (0..bones.len())
            .map(|i| {
                let mut values = Vec::new();
                if let Some(v) = reference_raw.get(i) {
                    crate::map::numbers(v, &mut values);
                }
                if values.len() >= 8 {
                    Transform::from_floats(&values)
                } else {
                    Transform::IDENTITY
                }
            })
            .collect();
        Self {
            bones,
            parents,
            reference,
        }
    }

    /// Index of the bone named `name` (exact, then ignoring case).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.bones
            .iter()
            .position(|b| b == name)
            .or_else(|| self.bones.iter().position(|b| b.eq_ignore_ascii_case(name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floats(values: &[f32]) -> Value {
        Value::Array(values.iter().map(|&v| Value::Float(f64::from(v))).collect())
    }

    #[test]
    fn reads_names_parents_and_reference_pose() {
        let root = Value::Object(
            [
                (
                    "m_boneIDs",
                    Value::Array(vec![
                        Value::String("root_motion".into()),
                        Value::String("pelvis".into()),
                    ]),
                ),
                (
                    "m_parentIndices",
                    Value::Array(vec![Value::Int(-1), Value::Int(0)]),
                ),
                (
                    "m_parentSpaceReferencePose",
                    Value::Array(vec![
                        floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
                        floats(&[-2.85, 0.0, 42.82, 1.0, 0.5, 0.5, 0.5, -0.5]),
                    ]),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let s = NmSkeleton::parse(&root);
        assert_eq!(s.bones, ["root_motion", "pelvis"]);
        assert_eq!(s.parents, [None, Some(0)]);
        assert_eq!(s.reference[1].translation, [-2.85, 0.0, 42.82]);
        assert_eq!(s.reference[1].rotation, [0.5, 0.5, 0.5, -0.5]);
        assert_eq!(s.find("PELVIS"), Some(1));
    }
}
