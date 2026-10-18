//! Map entities read from entity lumps (`*.vents_c`).
//!
//! Each entry of `m_entityKeyValues` holds its properties either as KV3 (`keyValues3Data.values`, used by current
//! CS2 maps — every de_mirage entity) or as a legacy binary blob (`m_keyValuesData`, hashed keys). OFAST reads the
//! KV3 form; the legacy form is recognized and skipped for now (see `docs/formats/map.md`).
//!
//! Values are typed loosely in practice: `origin` can be a float array or a string like `"5208.0 -968.0 44.5"`,
//! and many numbers are stored as strings. The accessors here accept both.

use std::collections::BTreeMap;

use crate::kv3::Value;

/// One property value of an entity.
pub type EntityValue = Value;

/// An entity: lower-cased property names → values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entity {
    properties: BTreeMap<String, EntityValue>,
}

impl Entity {
    /// Builds an entity from one `m_entityKeyValues[]` entry; `None` when it only has the legacy binary form.
    pub(super) fn from_lump_entry(entry: &Value) -> Option<Self> {
        let values = entry
            .get("keyValues3Data")
            .or_else(|| entry.get("m_keyValues3Data"))?
            .get("values")?
            .as_object()?;
        Some(Self {
            properties: values
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                .collect(),
        })
    }

    /// An entity from (key, value) pairs, for tests elsewhere in the crate.
    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: &[(&str, Value)]) -> Self {
        Self {
            properties: pairs
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                .collect(),
        }
    }

    /// Raw property value.
    pub fn get(&self, key: &str) -> Option<&EntityValue> {
        self.properties.get(&key.to_ascii_lowercase())
    }

    /// All properties.
    pub fn properties(&self) -> impl Iterator<Item = (&str, &EntityValue)> {
        self.properties.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The `classname` property.
    pub fn class_name(&self) -> Option<&str> {
        self.get("classname").and_then(Value::as_str)
    }

    /// A string property.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// A numeric property (also accepts numbers stored as strings).
    pub fn number(&self, key: &str) -> Option<f32> {
        let value = self.get(key)?;
        match value.unflagged() {
            Value::String(s) => s.trim().parse().ok(),
            other => other.as_f64().map(|f| f as f32),
        }
    }

    /// A yes/no property, stored as a boolean, a number or a `"0"`/`"1"` string depending on the map's age.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key)?.unflagged() {
            Value::Bool(b) => Some(*b),
            _ => self.number(key).map(|n| n != 0.0),
        }
    }

    /// A 3-component property (`origin`, `angles`, `color`, …) stored as an array or a space-separated string.
    /// Colours may have a 4th component; only the first 3 are returned.
    pub fn vector3(&self, key: &str) -> Option<[f32; 3]> {
        let value = self.get(key)?;
        let parts: Vec<f32> = match value.unflagged() {
            Value::String(s) => s
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect(),
            other => {
                let mut out = Vec::new();
                super::numbers(other, &mut out);
                out
            }
        };
        (parts.len() >= 3).then(|| [parts[0], parts[1], parts[2]])
    }

    /// `origin` (Source inches), or the world origin.
    pub fn origin(&self) -> [f32; 3] {
        self.vector3("origin").unwrap_or([0.0; 3])
    }

    /// `angles` as (pitch, yaw, roll) degrees, or zero.
    pub fn angles(&self) -> [f32; 3] {
        self.vector3("angles").unwrap_or([0.0; 3])
    }

    /// `scales`, or 1.
    pub fn scales(&self) -> [f32; 3] {
        self.vector3("scales").unwrap_or([1.0; 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv3::Object;

    fn entity(pairs: &[(&str, Value)]) -> Entity {
        let values: Object = pairs.iter().map(|(k, v)| (*k, v.clone())).collect();
        let kv3: Object = [
            ("version", Value::Int(1)),
            ("values", Value::Object(values)),
        ]
        .into_iter()
        .collect();
        let entry: Object = [("keyValues3Data", Value::Object(kv3))]
            .into_iter()
            .collect();
        Entity::from_lump_entry(&Value::Object(entry)).unwrap()
    }

    #[test]
    fn reads_arrays_strings_and_case_insensitive_keys() {
        let e = entity(&[
            ("classname", Value::String("light_environment".into())),
            ("Origin", Value::String("5208.0 -968.0 44.5".into())),
            (
                "angles",
                Value::Array(vec![
                    Value::Float(60.0),
                    Value::Float(318.0),
                    Value::Float(0.0),
                ]),
            ),
            ("brightness", Value::String("3.0".into())),
        ]);
        assert_eq!(e.class_name(), Some("light_environment"));
        assert_eq!(e.origin(), [5208.0, -968.0, 44.5]);
        assert_eq!(e.angles(), [60.0, 318.0, 0.0]);
        assert_eq!(e.number("brightness"), Some(3.0));
        assert_eq!(e.scales(), [1.0; 3]);
    }
}
