//! Game events: deaths, round ends, bomb plants, …
//!
//! CS2 still sends most gameplay events the Source 1 way ("legacy game events"). Early in the demo a descriptor
//! list (`GE_Source1LegacyGameEventList`, message 205) names every event type and the names and types of its keys,
//! once. Each event then (`GE_Source1LegacyGameEvent`, message 207) only carries its event id and the key values
//! in descriptor order. The parser decodes them into [`GameEvent`]s with named keys.
//!
//! Players in events: `userid`, `attacker` and `assister` are **player slots** — the player controller's entity
//! index minus one (`65535` or `-1` = nobody, e.g. the world or fall damage). The `*_pawn` keys hold the pawn's
//! entity handle. Checked against demoparser2 on a real match: every `player_death` names the same victim and
//! attacker (see `tests/cs2_demo.rs`).
//!
//! Knowledge sources (studied, not copied): demoparser2 and demoinfocs-golang (both MIT) — see `CREDITS.md`.

use std::collections::HashMap;

use super::proto::{CMsgSource1LegacyGameEvent, CMsgSource1LegacyGameEventList};

/// One key's value. Valve's key types: 1 string, 2 float, 3 long, 4 short, 5 byte, 6 bool, 7 uint64, 8 a long
/// holding an entity handle, 9 a short holding a player slot.
#[derive(Debug, Clone, PartialEq)]
pub enum EventValue {
    /// Text (weapon names, …).
    String(String),
    /// A float (distances, …).
    Float(f32),
    /// Any integer type (long, short, byte, entity handle, player slot).
    Int(i64),
    /// A flag.
    Bool(bool),
    /// A 64-bit id (Steam ids).
    U64(u64),
}

/// A decoded game event.
#[derive(Debug, Clone, PartialEq)]
pub struct GameEvent {
    /// Tick of the frame that carried it.
    pub tick: u32,
    /// Event name, e.g. `player_death`.
    pub name: String,
    /// Key values in descriptor order.
    pub keys: Vec<(String, EventValue)>,
}

impl GameEvent {
    /// The value of a key.
    pub fn get(&self, key: &str) -> Option<&EventValue> {
        self.keys.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// An integer key (also accepts bools as 0 / 1).
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            EventValue::Int(v) => Some(*v),
            EventValue::Bool(b) => Some(i64::from(*b)),
            EventValue::U64(v) => i64::try_from(*v).ok(),
            _ => None,
        }
    }

    /// A text key.
    pub fn string(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            EventValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// A flag key (also accepts integers: non-zero = true).
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            EventValue::Bool(b) => Some(*b),
            EventValue::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    /// The world position in its `x`, `y`, `z` keys (a grenade's detonation, `hegrenade_detonate` …), inches; `None`
    /// when one of them is missing or not a number.
    pub fn position(&self) -> Option<[f32; 3]> {
        let float = |key: &str| match self.get(key)? {
            EventValue::Float(v) => Some(*v),
            EventValue::Int(v) => Some(*v as f32),
            _ => None,
        };
        Some([float("x")?, float("y")?, float("z")?])
    }

    /// The player controller entity index a player-slot key (`userid`, `attacker`, `assister`) points at, or
    /// `None` for nobody.
    pub fn controller(&self, key: &str) -> Option<u32> {
        let slot = self.int(key)?;
        // 65535 (the short's "none") or negative: no player.
        (0..64).contains(&slot).then(|| slot as u32 + 1)
    }
}

/// Event names and key names by event id, from the descriptor list.
#[derive(Debug, Default)]
pub(crate) struct EventDescriptors {
    by_id: HashMap<i32, (String, Vec<String>)>,
}

impl EventDescriptors {
    /// Replaces the descriptors with a new list.
    pub(crate) fn load(&mut self, list: &CMsgSource1LegacyGameEventList) {
        self.by_id = list
            .descriptors
            .iter()
            .map(|d| {
                (
                    d.eventid(),
                    (
                        d.name().to_owned(),
                        d.keys.iter().map(|k| k.name().to_owned()).collect(),
                    ),
                )
            })
            .collect();
    }

    /// Names an event's keys. Returns `None` for an event id the list does not have.
    pub(crate) fn decode(
        &self,
        tick: u32,
        event: &CMsgSource1LegacyGameEvent,
    ) -> Option<GameEvent> {
        let (name, key_names) = self.by_id.get(&event.eventid())?;
        let keys = key_names
            .iter()
            .zip(&event.keys)
            .map(|(name, key)| (name.clone(), value_of(key)))
            .collect();
        Some(GameEvent {
            tick,
            name: name.clone(),
            keys,
        })
    }
}

/// A key's value, read from the field its type uses.
fn value_of(key: &super::proto::c_msg_source1_legacy_game_event::KeyT) -> EventValue {
    match key.r#type() {
        1 => EventValue::String(key.val_string().to_owned()),
        2 => EventValue::Float(key.val_float()),
        3 | 8 => EventValue::Int(i64::from(key.val_long())),
        4 | 9 => EventValue::Int(i64::from(key.val_short())),
        5 => EventValue::Int(i64::from(key.val_byte())),
        6 => EventValue::Bool(key.val_bool()),
        7 => EventValue::U64(key.val_uint64()),
        // Unknown type: whichever field is set.
        _ => key
            .val_string
            .clone()
            .map(EventValue::String)
            .or(key.val_float.map(EventValue::Float))
            .or(key.val_bool.map(EventValue::Bool))
            .or(key.val_uint64.map(EventValue::U64))
            .unwrap_or(EventValue::Int(i64::from(
                key.val_long.or(key.val_short).or(key.val_byte).unwrap_or(0),
            ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::proto::c_msg_source1_legacy_game_event::KeyT;
    use crate::demo::proto::c_msg_source1_legacy_game_event_list::{DescriptorT, KeyT as KeyName};

    #[test]
    fn keys_are_named_in_descriptor_order() {
        let mut descriptors = EventDescriptors::default();
        let key = |name: &str, kind: i32| KeyName {
            r#type: Some(kind),
            name: Some(name.into()),
        };
        descriptors.load(&CMsgSource1LegacyGameEventList {
            descriptors: vec![DescriptorT {
                eventid: Some(42),
                name: Some("player_death".into()),
                keys: vec![key("userid", 9), key("weapon", 1), key("headshot", 6)],
            }],
        });
        let event = CMsgSource1LegacyGameEvent {
            eventid: Some(42),
            keys: vec![
                KeyT {
                    r#type: Some(9),
                    val_short: Some(3),
                    ..Default::default()
                },
                KeyT {
                    r#type: Some(1),
                    val_string: Some("ak47".into()),
                    ..Default::default()
                },
                KeyT {
                    r#type: Some(6),
                    val_bool: Some(true),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let decoded = descriptors.decode(100, &event).expect("known event");
        assert_eq!(decoded.name, "player_death");
        assert_eq!(decoded.controller("userid"), Some(4));
        assert_eq!(decoded.string("weapon"), Some("ak47"));
        assert_eq!(decoded.bool("headshot"), Some(true));
        assert!(
            descriptors
                .decode(100, &CMsgSource1LegacyGameEvent::default())
                .is_none()
        );
    }

    #[test]
    fn slot_65535_is_nobody() {
        let event = GameEvent {
            tick: 0,
            name: "player_death".into(),
            keys: vec![("attacker".into(), EventValue::Int(65535))],
        };
        assert_eq!(event.controller("attacker"), None);
    }
}
