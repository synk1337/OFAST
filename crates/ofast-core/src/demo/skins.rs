//! A weapon's finish as a demo carries it (M10): paint kit, pattern seed, wear, StatTrak, name tag, stickers and
//! keychain.
//!
//! # Where it is in the demo
//!
//! Every weapon entity networks its econ item: `m_AttributeManager.m_Item` holds the item definition index, the owner's
//! account, the name tag (`m_szCustomName`) and a list of attributes, `m_NetworkedDynamicAttributes.m_Attributes`:
//! each a `CEconItemAttribute` with its definition index (`m_iAttributeDefinitionIndex`) and value
//! (`m_iRawValue32`, 32 bits). The weapon's `m_nFallbackPaintKit` / `Seed` / `Wear` stay 0 in tournament demos (checked
//! on Aurora vs FURIA, 2026-10-27): the attributes are where the skin is.
//!
//! # What the attributes mean
//!
//! `items_game.txt` → `attributes` names each definition index and says how its 32 bits are stored
//! (`stored_as_integer`): `"1"` an integer in the bits, `"0"` a float, `"float_floor_to_integer"` a float holding a
//! whole number. The ones a finish needs:
//!
//! | Index | Name | Stored |
//! |---|---|---|
//! | 6 | set item texture prefab (the paint kit) | float, whole |
//! | 7 | set item texture seed (the pattern seed) | float, whole |
//! | 8 | set item texture wear (0 factory new – 1 battle-scarred) | float |
//! | 80 | kill eater (the StatTrak count) | integer |
//! | 113 + 4n, 114 + 4n, 115 + 4n, 116 + 4n | sticker slot n (0 – 5) id, wear, scale, rotation | integer, floats |
//! | 278 + 2n, 279 + 2n | sticker slot n offset x, y (CS2's free placement) | floats |
//! | 290 + n | sticker slot n schema | integer |
//! | 299, 300 – 302, 306 | keychain slot 0 id, offset x / y / z, seed | integer, floats, integer |
//!
//! A sticker sends only what differs from its slot's defaults: Aurora vs FURIA's AKs carry ids and some offsets, no
//! wear, scale or rotation.

/// A weapon's finish: what [`skin_from_attributes`] reads from its econ attributes. The default is a weapon as it
/// comes (no paint kit, nothing on it).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Skin {
    /// The weapon's item definition index (7 = AK-47), to tell a weapon entity's skin from the next one's when the
    /// entity index is reused.
    pub item: u32,
    /// Paint kit (`items_game.txt` → `paint_kits`): 0 for none, 180 = Fire Serpent.
    pub paint_kit: u32,
    /// Pattern seed, 0 – 1000: where the pattern, wear and grunge textures sit on the weapon.
    pub seed: u32,
    /// Wear, 0 (factory new) to 1.
    pub wear: f32,
    /// StatTrak kill count, when the weapon has one.
    pub stattrak: Option<u32>,
    /// The name tag, when the weapon has one.
    pub name_tag: Option<String>,
    /// Stickers, by slot.
    pub stickers: Vec<Sticker>,
    /// The keychain (charm), when it has one.
    pub keychain: Option<Keychain>,
}

/// A sticker on a weapon. A value the attributes do not send is `None`: the slot's own default.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sticker {
    /// Slot, 0 – 5.
    pub slot: u32,
    /// Sticker kit (`items_game.txt` → `sticker_kits`).
    pub id: u32,
    /// Scraped, 0 (new) – 1.
    pub wear: Option<f32>,
    /// Scale.
    pub scale: Option<f32>,
    /// Rotation, degrees.
    pub rotation: Option<f32>,
    /// CS2's free placement: offset along the weapon, x and y.
    pub offset: Option<[f32; 2]>,
    /// The sticker's schema (attribute `sticker slot n schema`).
    pub schema: Option<u32>,
}

/// A keychain hanging from a weapon.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Keychain {
    /// Keychain definition (`items_game.txt` → `keychain_definitions`).
    pub id: u32,
    /// Where it hangs, x, y, z.
    pub offset: Option<[f32; 3]>,
    /// Its pattern seed.
    pub seed: Option<u32>,
}

/// Paint kit.
const PAINT_KIT: u32 = 6;
/// Pattern seed.
const SEED: u32 = 7;
/// Wear.
const WEAR: u32 = 8;
/// StatTrak count.
const KILL_EATER: u32 = 80;
/// Sticker slot 0's id; slot n's id, wear, scale and rotation are `STICKER_ID + 4n` + 0 – 3.
const STICKER_ID: u32 = 113;
/// Sticker slot 0's offset x; slot n's x and y are `STICKER_OFFSET + 2n` + 0 – 1.
const STICKER_OFFSET: u32 = 278;
/// Sticker slot 0's schema; slot n's is `STICKER_SCHEMA + n`.
const STICKER_SCHEMA: u32 = 290;
/// Sticker slots.
const STICKER_SLOTS: u32 = 6;
/// Keychain slot 0's id, offset x / y / z, seed.
const KEYCHAIN_ID: u32 = 299;
const KEYCHAIN_OFFSET: u32 = 300;
const KEYCHAIN_SEED: u32 = 306;

/// A weapon's finish from its econ attributes (definition index, the value's 32 raw bits) and its name tag (empty
/// for none). See the [module docs](self) for what each index means and how its bits are stored.
pub fn skin_from_attributes(item: u32, attributes: &[(u32, u32)], name_tag: &str) -> Skin {
    let raw = |index: u32| {
        attributes
            .iter()
            .find(|(i, _)| *i == index)
            .map(|&(_, bits)| bits)
    };
    let float = |index: u32| raw(index).map(f32::from_bits);
    // A float holding a whole number ("float_floor_to_integer").
    let whole = |index: u32| float(index).map(|f| f.max(0.0).floor() as u32);
    let stickers = (0..STICKER_SLOTS)
        .filter_map(|slot| {
            let base = STICKER_ID + 4 * slot;
            let id = raw(base).filter(|&id| id != 0)?;
            let offset = STICKER_OFFSET + 2 * slot;
            Some(Sticker {
                slot,
                id,
                wear: float(base + 1),
                scale: float(base + 2),
                rotation: float(base + 3),
                offset: match (float(offset), float(offset + 1)) {
                    (None, None) => None,
                    (x, y) => Some([x.unwrap_or(0.0), y.unwrap_or(0.0)]),
                },
                schema: raw(STICKER_SCHEMA + slot),
            })
        })
        .collect();
    let keychain = raw(KEYCHAIN_ID).filter(|&id| id != 0).map(|id| {
        let axes = [0, 1, 2].map(|k| float(KEYCHAIN_OFFSET + k));
        Keychain {
            id,
            offset: axes
                .iter()
                .any(Option::is_some)
                .then(|| axes.map(|a| a.unwrap_or(0.0))),
            seed: raw(KEYCHAIN_SEED),
        }
    });
    Skin {
        item,
        paint_kit: whole(PAINT_KIT).unwrap_or(0),
        seed: whole(SEED).unwrap_or(0),
        wear: float(WEAR).unwrap_or(0.0),
        stattrak: raw(KILL_EATER),
        name_tag: (!name_tag.is_empty()).then(|| name_tag.to_owned()),
        stickers,
        keychain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(index: u32, value: f32) -> (u32, u32) {
        (index, value.to_bits())
    }

    #[test]
    fn reads_a_painted_weapon_as_the_demo_sends_it() {
        // AK #227 of Aurora vs FURIA, round 17: Fire Serpent, seed 245, wear 0.143, three stickers, a name tag.
        let attributes = [
            f(6, 180.0),
            f(7, 245.0),
            f(8, 0.1433621),
            (117, 2811),
            (121, 2805),
            (125, 37),
        ];
        let skin = skin_from_attributes(7, &attributes, "VICTORIA SIEMPRE");
        assert_eq!(
            (skin.item, skin.paint_kit, skin.seed),
            (7, 180, 245),
            "{skin:?}"
        );
        assert!((skin.wear - 0.1433621).abs() < 1e-7);
        assert_eq!(skin.name_tag.as_deref(), Some("VICTORIA SIEMPRE"));
        assert_eq!(skin.stattrak, None);
        let slots: Vec<(u32, u32)> = skin.stickers.iter().map(|s| (s.slot, s.id)).collect();
        assert_eq!(slots, [(1, 2811), (2, 2805), (3, 37)]);
        assert_eq!(skin.stickers[0].offset, None);
        assert_eq!(skin.keychain, None);
    }

    #[test]
    fn a_fractional_seed_is_floored_and_placements_are_read() {
        let attributes = [
            f(6, 724.0),
            f(7, 242.30241),
            f(8, 0.27),
            (80, 1534),
            (113, 5000),
            f(114, 0.5),
            f(278, 0.04020655),
            f(279, -0.003944248),
            (290, 1337),
            (299, 31),
            f(300, 1.5),
            (306, 7777),
        ];
        let skin = skin_from_attributes(7, &attributes, "");
        assert_eq!(
            (skin.paint_kit, skin.seed, skin.stattrak),
            (724, 242, Some(1534))
        );
        assert_eq!(skin.name_tag, None);
        let sticker = &skin.stickers[0];
        assert_eq!(
            (sticker.slot, sticker.id, sticker.wear),
            (0, 5000, Some(0.5))
        );
        assert_eq!(sticker.offset, Some([0.04020655, -0.003944248]));
        assert_eq!(
            (sticker.scale, sticker.rotation, sticker.schema),
            (None, None, Some(1337))
        );
        let keychain = skin.keychain.expect("keychain");
        assert_eq!(
            (keychain.id, keychain.offset, keychain.seed),
            (31, Some([1.5, 0.0, 0.0]), Some(7777))
        );
    }

    #[test]
    fn a_plain_weapon_has_the_default_finish() {
        assert_eq!(
            skin_from_attributes(1, &[], ""),
            Skin {
                item: 1,
                ..Skin::default()
            }
        );
    }
}
