//! Where the weapons a player carries sit on the agent, frame by frame (M4.5).
//!
//! # How CS2 places a player's weapons
//!
//! - **In hand.** A weapon's world model (`weapons/models/<w>/weapon_<w>.vmdl`) has a root bone `weapon`. CS2 puts
//!   it on the agent's `weapon` attachment, which is the `wpn` bone with no offset: the bone CS2's clips and aim
//!   node move ([`crate::anim::aim`]). Evidence (`examples/weapon_grip.rs`): each weapon's right-hand guide bone
//!   `ag1_hand_r` sits at one place relative to `weapon` per class — every rifle at (−3.41, 0.19, −4.51), every
//!   pistol at (−2.80, 0, −3.05) — near where the clips put the hand relative to `wpn` (the rest is the wrist-to-grip
//!   distance), while the models' origins are anywhere (the M4A4's is 22 inches from its `weapon` bone).
//! - **Holstered.** A weapon carried but not in hand hangs on one of the agent's attachments, chosen by its kind
//!   ([`holster`]): rifles on the back (`primary`, on `jiggle_primary`), SMGs on `primary_smg`, pistols on the right
//!   thigh, knives on the left, grenades on the vest (`grenade0` – `grenade4`), the C4 on the back. The weapon's own
//!   `weapon_holster_center` attachment goes onto it. CS2 draws them: rifles slung, pistols in thigh holsters
//!   (`test-data/shots/agents/cs2_run`).
//!
//! # How OFAST bakes it
//!
//! Unreal only plays what the export computed (decision 013), so weapons are bones too: each weapon look a group of
//! players carries gets a bone of its own on the agent's skeleton ([`Slot`], a child of the root), with the weapon's
//! meshes rigidly skinned to it (`super::gltf::export_character`). Every frame, [`slot_target`] puts that bone on
//! `wpn`, on the holster, or the bone is scaled to 0 while the player does not carry the weapon ([`hide_changes`],
//! [`key_places`]: hidden frames stay where the weapon shows, so it never flies or sweeps through the body).
//! Scrubbing, reverse play and Movie Render Queue then show the same weapons in the same places, like the body.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

use crate::demo::players::CarriedWeapon;
use crate::demo::skins::Skin;
use crate::model::{Affine, Model, Skeleton, compose, invert_rigid};

/// A weapon model as carried: its path (without `_c`), its mesh groups (`m_MeshGroupMask`) and its skin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WeaponLook {
    /// `weapons/models/ak47/weapon_rif_ak47.vmdl`, …
    pub model: String,
    /// Bit per mesh group of the model (guns: 1 the current model, 2 the CS:GO-era one).
    pub mesh_groups: u64,
    /// Its paint, when it has one (M10): two players' AKs with different skins are two looks.
    pub finish: Option<Finish>,
}

/// What a weapon's paint looks like: paint kit, pattern seed and wear (the demo's attributes 6, 7, 8,
/// [`crate::demo::skins`]), and its stickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Finish {
    /// Paint kit (0: none, the weapon's own finish under its stickers).
    pub kit: u32,
    /// Pattern seed.
    pub seed: u32,
    /// Wear's bits (`f32::to_bits`: compared exactly).
    pub wear: u32,
    /// The stickers by slot (the weapon shader draws [`STICKER_SLOTS`]).
    pub stickers: [Option<StickerFinish>; STICKER_SLOTS],
    /// The keychain hanging from it.
    pub keychain: Option<KeychainFinish>,
    /// Its name tag, as a hash (the text: [`Finish::name_tag`]; a hash keeps the finish `Copy`).
    pub name_tag: Option<u64>,
}

/// The name tags seen in this process by their hash ([`Finish::name_tag`]).
static NAME_TAGS: LazyLock<Mutex<HashMap<u64, String>>> = LazyLock::new(Mutex::default);

/// `text`'s hash, remembered with it.
fn intern_name(text: &str) -> u64 {
    let hash = blake3::hash(text.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&hash.as_bytes()[..8]);
    let id = u64::from_le_bytes(first);
    NAME_TAGS
        .lock()
        .expect("lock")
        .entry(id)
        .or_insert_with(|| text.to_owned());
    id
}

/// A keychain as a weapon's item sets it (`demo::skins::Keychain`), its point as bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeychainFinish {
    /// Keychain definition (`items_game.txt` → `keychain_definitions`).
    pub id: u32,
    /// The point it hangs from in the weapon model's space; `None`: the model's `keychain` attachment.
    pub offset: Option<[u32; 3]>,
}

impl KeychainFinish {
    /// The point it hangs from, inches.
    pub fn offset(&self) -> Option<[f32; 3]> {
        self.offset.map(|o| o.map(f32::from_bits))
    }
}

/// The sticker slots a weapon's material draws (`csgo_weapon.vfx`: 0 – 4).
pub const STICKER_SLOTS: usize = crate::export::stickers::SLOTS;

/// A sticker as a weapon's item sets it (`demo::skins::Sticker`), floats as bits to compare them exactly; `None`:
/// the slot's own value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StickerFinish {
    /// Sticker kit (`items_game.txt` → `sticker_kits`).
    pub id: u32,
    /// Scraped, 0 – 1.
    pub wear: Option<u32>,
    /// Size.
    pub scale: Option<u32>,
    /// Degrees.
    pub rotation: Option<u32>,
    /// CS2's free placement.
    pub offset: Option<[u32; 2]>,
}

impl Finish {
    /// A paint without stickers (gloves).
    pub fn painted(kit: u32, seed: u32, wear: f32) -> Self {
        Self {
            kit,
            seed,
            wear: wear.to_bits(),
            stickers: [None; STICKER_SLOTS],
            keychain: None,
            name_tag: None,
        }
    }

    /// The name tag's text.
    pub fn name_tag(&self) -> Option<String> {
        let id = self.name_tag?;
        NAME_TAGS.lock().expect("lock").get(&id).cloned()
    }

    /// The finish of a weapon's skin; `None` without a paint kit, a sticker, a keychain or a name tag. Stickers in a
    /// slot the shader does not draw are left out.
    pub fn of(skin: &Skin) -> Option<Self> {
        let mut stickers = [None; STICKER_SLOTS];
        for s in &skin.stickers {
            if let Some(slot) = stickers.get_mut(s.slot as usize) {
                *slot = Some(StickerFinish {
                    id: s.id,
                    wear: s.wear.map(f32::to_bits),
                    scale: s.scale.map(f32::to_bits),
                    rotation: s.rotation.map(f32::to_bits),
                    offset: s.offset.map(|o| o.map(f32::to_bits)),
                });
            }
        }
        let keychain = skin.keychain.as_ref().map(|k| KeychainFinish {
            id: k.id,
            offset: k.offset.map(|o| o.map(f32::to_bits)),
        });
        let name_tag = skin
            .name_tag
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(intern_name);
        let decorated =
            stickers.iter().any(Option::is_some) || keychain.is_some() || name_tag.is_some();
        (skin.paint_kit != 0 || decorated).then_some(Self {
            kit: skin.paint_kit,
            seed: skin.seed,
            wear: skin.wear.to_bits(),
            stickers,
            keychain,
            name_tag,
        })
    }

    /// The wear, 0 – 1.
    pub fn wear(&self) -> f32 {
        f32::from_bits(self.wear)
    }
}

/// One weapon bone on an agent: it carries the `instance`-th weapon of look `look` a player has at once (a second
/// flashbang is instance 1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot {
    /// The weapon model shown.
    pub look: WeaponLook,
    /// Which of the player's weapons of that look.
    pub instance: usize,
}

/// Where a slot's weapon is at one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Not carried (or nowhere to show it): the bone is scaled to 0.
    Hidden,
    /// In hand: the weapon's `weapon` bone on the agent's `weapon` attachment.
    Hand,
    /// Holstered on the agent attachment of that name.
    Holster(&'static str),
}

/// The agent attachment a carried weapon hangs on while not in hand, from its name and its animation category
/// (`weapon_category_*`, as the main graph gives it: [`crate::anim::weapon::WeaponClips`]); `instance` is which of
/// the player's weapons of that kind it is (two flashbangs take `grenade0` and `grenade1`). `None` for what CS2
/// shows nowhere (molotovs and incendiaries, the Healthshot).
///
/// Primary weapons follow `items_game.txt`'s `addon_location` (`primary_rifle`, `_sniper`, `_shotgun`, `_mg` all on
/// the agent's `primary` attachment, `primary_smg` on its own); grenades follow the order of the agent's grenade
/// attachments, as in CS:GO (flashbangs, HE, smoke, decoy).
pub fn holster(name: &str, category: &str, instance: usize) -> Option<&'static str> {
    match (category, name) {
        (_, "weapon_c4") => Some("c4"),
        (_, "weapon_taser") => Some("eholster"),
        (
            "weapon_category_rifle"
            | "weapon_category_sniper"
            | "weapon_category_shotgun"
            | "weapon_category_machinegun",
            _,
        ) => Some("primary"),
        ("weapon_category_smg", _) => Some("primary_smg"),
        ("weapon_category_pistol", _) => Some("pistol"),
        ("weapon_category_knife", _) => Some("knife"),
        (_, "weapon_flashbang") => Some(if instance == 0 {
            "grenade0"
        } else {
            "grenade1"
        }),
        (_, "weapon_hegrenade") => Some("grenade2"),
        (_, "weapon_smokegrenade") => Some("grenade3"),
        (_, "weapon_decoy" | "weapon_tagrenade") => Some("grenade4"),
        _ => None,
    }
}

/// Each weapon a player carries at one tick, as a slot and a place. `active` is the entity in hand; `look` gives a
/// weapon's model (`None`: unknown model, skipped); `kind` gives an item's name and category (`None`: not shown
/// holstered). A weapon carried `count` times (two flashbangs are one entity) gives that many slots: the first one
/// in hand if the entity is, the others holstered in turn (`grenade0`, then `grenade1`).
pub fn carried_places<'a>(
    weapons: &[CarriedWeapon],
    active: Option<u32>,
    look: impl Fn(&CarriedWeapon) -> Option<WeaponLook>,
    kind: impl Fn(u32) -> Option<(&'a str, &'a str)>,
) -> Vec<(Slot, Place)> {
    // Per look: slots given out so far, and holstered copies so far.
    let mut seen: BTreeMap<WeaponLook, (usize, usize)> = BTreeMap::new();
    let mut out = Vec::new();
    for w in weapons {
        let Some(look) = look(w) else { continue };
        for copy in 0..w.count.max(1) {
            let (slots, holstered) = seen.entry(look.clone()).or_default();
            let instance = *slots;
            *slots += 1;
            let place = if copy == 0 && Some(w.entity) == active {
                Place::Hand
            } else {
                let index = *holstered;
                *holstered += 1;
                kind(w.item)
                    .and_then(|(name, category)| holster(name, category, index))
                    .map_or(Place::Hidden, Place::Holster)
            };
            out.push((
                Slot {
                    look: look.clone(),
                    instance,
                },
                place,
            ));
        }
    }
    out
}

/// One slot's places over a round's frames, with the weapon hidden on the last frame before it moves between two
/// shown places and on the first frame after: keyed animation interpolates between frames, and a weapon shown while
/// its bone travels from the thigh to the hand would sweep through the body.
pub fn hide_changes(places: &[Place]) -> Vec<Place> {
    let mut shown = places.to_vec();
    for (k, pair) in places.windows(2).enumerate() {
        if pair[0] != Place::Hidden && pair[1] != Place::Hidden && pair[0] != pair[1] {
            shown[k] = Place::Hidden;
            shown[k + 1] = Place::Hidden;
        }
    }
    shown
}

/// For each frame, the place to key a slot's bone at and whether the weapon shows there. A hidden frame keeps the
/// place of its neighbour that shows the weapon — the frame before if it does, else the frame after, else the last
/// place shown before, else the next one, else the hand — so the weapon shrinks and grows where it is (Unreal
/// blends the scale between keys) and only moves while hidden.
pub fn key_places(shown: &[Place]) -> Vec<(Place, bool)> {
    let visible = |p: &&Place| **p != Place::Hidden;
    let mut next_shown = vec![Place::Hidden; shown.len()];
    let mut upcoming = Place::Hidden;
    for k in (0..shown.len()).rev() {
        if shown[k] != Place::Hidden {
            upcoming = shown[k];
        }
        next_shown[k] = upcoming;
    }
    let mut last = Place::Hidden;
    (0..shown.len())
        .map(|k| {
            if shown[k] != Place::Hidden {
                last = shown[k];
                return (shown[k], true);
            }
            let before = k.checked_sub(1).map_or(Place::Hidden, |j| shown[j]);
            let after = shown.get(k + 1).copied().unwrap_or(Place::Hidden);
            let place = [before, after, last, next_shown[k]]
                .iter()
                .find(visible)
                .copied()
                .unwrap_or(Place::Hand);
            (place, false)
        })
        .collect()
}

/// A weapon model's frames: its `weapon` bone in model space (where the agent's `wpn` goes), and its
/// `weapon_holster_center`, `muzzle_flash` and `shell_eject` attachments relative to that bone. `None` for a model
/// without a `weapon` bone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponFrames {
    /// The `weapon` bone at bind pose, in the weapon model's space.
    pub weapon_bone: Affine,
    /// `weapon_holster_center` in the `weapon` bone's frame (`None`: the model has none; it is not shown holstered).
    pub holster_center: Option<Affine>,
    /// `muzzle_flash` in the `weapon` bone's frame: where a shot leaves the barrel, its x axis along the barrel
    /// (`None`: knives, grenades, the C4).
    pub muzzle: Option<Affine>,
    /// `shell_eject` in the `weapon` bone's frame: the ejection port, the frame a fired case is thrown in
    /// ([`super::brass`]; `None`: guns that keep their cases, the revolver, and what is not a gun).
    pub shell_eject: Option<Affine>,
}

impl WeaponFrames {
    /// Reads them from a weapon model.
    pub fn of(model: &Model) -> Option<Self> {
        let world = model.skeleton.world_transforms();
        let weapon_bone = world[model.skeleton.find("weapon")?];
        let on_weapon_bone = |name: &str| {
            model
                .attachment(name)
                .and_then(|a| a.on_bone(&model.skeleton))
                .map(|(bone, offset)| {
                    compose(&invert_rigid(&weapon_bone), &compose(&world[bone], &offset))
                })
        };
        Some(Self {
            weapon_bone,
            holster_center: on_weapon_bone("weapon_holster_center"),
            muzzle: on_weapon_bone("muzzle_flash"),
            shell_eject: on_weapon_bone("shell_eject"),
        })
    }
}

/// An agent's attachments that weapons go on: `weapon` (the hand) and the holsters, each as (bone, offset).
#[derive(Debug, Clone, Default)]
pub struct AgentMounts {
    /// The `weapon` attachment (the `wpn` bone on every CS2 agent).
    pub hand: Option<(usize, Affine)>,
    /// The holster attachments [`holster`] names, those the agent has.
    pub holsters: BTreeMap<&'static str, (usize, Affine)>,
}

/// Every attachment name [`holster`] can return.
const HOLSTERS: [&str; 11] = [
    "primary",
    "primary_smg",
    "pistol",
    "knife",
    "c4",
    "eholster",
    "grenade0",
    "grenade1",
    "grenade2",
    "grenade3",
    "grenade4",
];

impl AgentMounts {
    /// Reads them from an agent model.
    pub fn of(model: &Model) -> Self {
        let on_bone = |name: &str| {
            model
                .attachment(name)
                .and_then(|a| a.on_bone(&model.skeleton))
        };
        Self {
            hand: on_bone("weapon"),
            holsters: HOLSTERS
                .iter()
                .filter_map(|&name| Some((name, on_bone(name)?)))
                .collect(),
        }
    }
}

/// Where a slot's weapon bone goes in the agent's model space for `place`, given the agent's posed bones `world`
/// (model space, as [`crate::anim::Retargeter::world`] gives them). `None` = hidden.
pub fn slot_target(
    world: &[Affine],
    mounts: &AgentMounts,
    frames: &WeaponFrames,
    place: Place,
) -> Option<Affine> {
    match place {
        Place::Hidden => None,
        Place::Hand => {
            let (bone, offset) = mounts.hand?;
            Some(compose(world.get(bone)?, &offset))
        }
        Place::Holster(name) => {
            // weapon bone · holster centre = agent bone · attachment offset.
            let (bone, offset) = mounts.holsters.get(name)?;
            let mount = compose(world.get(*bone)?, offset);
            Some(compose(&mount, &invert_rigid(&frames.holster_center?)))
        }
    }
}

/// The agent's skeleton with one bone per slot appended, children of bone 0 (the root), named
/// `ofast_weapon_<n>`, at the agent's bind-pose `weapon` attachment (where in-hand weapons go).
pub fn skeleton_with_slots(agent: &Model, mounts: &AgentMounts, slots: usize) -> Skeleton {
    let mut skeleton = agent.skeleton.clone();
    let world = agent.skeleton.world_transforms();
    let hand = mounts
        .hand
        .map(|(bone, offset)| compose(&world[bone], &offset))
        .unwrap_or(crate::model::affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]));
    let local = compose(&invert_rigid(&world[0]), &hand);
    for n in 0..slots {
        skeleton.bones.push(crate::model::Bone {
            name: format!("ofast_weapon_{n}"),
            parent: Some(0),
            position: [local[0][3], local[1][3], local[2][3]],
            rotation: crate::anim::matrix_to_quat(&local),
            flags: 0,
        });
    }
    skeleton
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::affine;

    fn weapon(entity: u32, item: u32, model: &str) -> CarriedWeapon {
        CarriedWeapon {
            entity,
            item,
            model: crate::resource::resource_id(model),
            mesh_groups: 1,
            count: 1,
        }
    }

    #[test]
    fn holsters_by_kind() {
        assert_eq!(
            holster("weapon_ak47", "weapon_category_rifle", 0),
            Some("primary")
        );
        assert_eq!(
            holster("weapon_awp", "weapon_category_sniper", 0),
            Some("primary")
        );
        assert_eq!(
            holster("weapon_mp9", "weapon_category_smg", 0),
            Some("primary_smg")
        );
        assert_eq!(
            holster("weapon_glock", "weapon_category_pistol", 0),
            Some("pistol")
        );
        assert_eq!(
            holster("weapon_knife_t", "weapon_category_knife", 0),
            Some("knife")
        );
        assert_eq!(
            holster("weapon_flashbang", "weapon_category_grenade", 0),
            Some("grenade0")
        );
        assert_eq!(
            holster("weapon_flashbang", "weapon_category_grenade", 1),
            Some("grenade1")
        );
        assert_eq!(
            holster("weapon_c4", "weapon_category_equipment", 0),
            Some("c4")
        );
        assert_eq!(
            holster("weapon_molotov", "weapon_category_grenade", 0),
            None
        );
    }

    #[test]
    fn carried_weapons_get_slots_and_places() {
        let flash = "weapons/models/grenade/flashbang/weapon_flashbang.vmdl";
        let ak = "weapons/models/ak47/weapon_rif_ak47.vmdl";
        let weapons = [weapon(5, 7, ak), weapon(6, 43, flash), weapon(7, 43, flash)];
        let names = [
            (7, ("weapon_ak47", "weapon_category_rifle")),
            (43, ("weapon_flashbang", "weapon_category_grenade")),
        ];
        let places = carried_places(
            &weapons,
            Some(6),
            |w| {
                Some(WeaponLook {
                    model: if w.item == 7 { ak } else { flash }.into(),
                    mesh_groups: w.mesh_groups,
                    finish: None,
                })
            },
            |item| names.iter().find(|(i, _)| *i == item).map(|(_, k)| *k),
        );
        let summary: Vec<(usize, Place)> = places.iter().map(|(s, p)| (s.instance, *p)).collect();
        // The AK on the back, the first flashbang in hand, the second on the first grenade attachment.
        assert_eq!(
            summary,
            [
                (0, Place::Holster("primary")),
                (0, Place::Hand),
                (1, Place::Holster("grenade0"))
            ]
        );
    }

    /// Two flashbangs are one entity carried twice: in hand, the second goes on the first grenade attachment;
    /// holstered, they take `grenade0` and `grenade1`.
    #[test]
    fn a_second_flashbang_is_a_second_slot() {
        let flash = "weapons/models/grenade/flashbang/weapon_flashbang.vmdl";
        let weapons = [CarriedWeapon {
            count: 2,
            ..weapon(6, 43, flash)
        }];
        let look = |w: &CarriedWeapon| {
            Some(WeaponLook {
                model: flash.into(),
                mesh_groups: w.mesh_groups,
                finish: None,
            })
        };
        let kind = |_| Some(("weapon_flashbang", "weapon_category_grenade"));
        let places = |active| -> Vec<(usize, Place)> {
            carried_places(&weapons, active, look, kind)
                .into_iter()
                .map(|(s, p)| (s.instance, p))
                .collect()
        };
        assert_eq!(
            places(Some(6)),
            [(0, Place::Hand), (1, Place::Holster("grenade0"))]
        );
        assert_eq!(
            places(None),
            [
                (0, Place::Holster("grenade0")),
                (1, Place::Holster("grenade1"))
            ]
        );
    }

    #[test]
    fn hidden_frames_stay_where_the_weapon_shows() {
        use Place::{Hand, Hidden, Holster};
        let back = Holster("primary");
        // On the back, drawn, then dropped (not carried).
        let places = [back, back, Hand, Hand, Hidden, Hidden];
        let shown = hide_changes(&places);
        assert_eq!(shown, [back, Hidden, Hidden, Hand, Hidden, Hidden]);
        let keyed = key_places(&shown);
        assert_eq!(
            keyed,
            [
                (back, true),
                (back, false), // shrinks on the back
                (Hand, false), // moves while hidden, grows in the hand
                (Hand, true),
                (Hand, false), // shrinks in the hand
                (Hand, false),
            ]
        );
        // Bought later in the round: hidden frames before take the first place shown.
        let keyed = key_places(&[Hidden, Hidden, back]);
        assert_eq!(keyed[0], (back, false));
        // Never carried: keyed at the hand.
        assert_eq!(key_places(&[Hidden]), [(Hand, false)]);
    }

    #[test]
    fn holstered_weapons_put_their_centre_on_the_mount() {
        let mounts = AgentMounts {
            hand: Some((0, affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]))),
            holsters: [(
                "primary",
                (1, affine([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0])),
            )]
            .into(),
        };
        let frames = WeaponFrames {
            weapon_bone: affine([0.0, 0.0, 0.0, 1.0], [0.0; 3]),
            holster_center: Some(affine([0.0, 0.0, 0.0, 1.0], [2.0, 0.0, 0.0])),
            muzzle: None,
            shell_eject: None,
        };
        let world = [
            affine([0.0, 0.0, 0.0, 1.0], [5.0, 0.0, 0.0]),
            affine([0.0, 0.0, 0.0, 1.0], [0.0, 10.0, 0.0]),
        ];
        let hand = slot_target(&world, &mounts, &frames, Place::Hand).unwrap();
        assert_eq!([hand[0][3], hand[1][3], hand[2][3]], [5.0, 0.0, 0.0]);
        // Mount at (1, 10, 0); the weapon's centre is 2 in front of its bone, so the bone sits at (−1, 10, 0).
        let back = slot_target(&world, &mounts, &frames, Place::Holster("primary")).unwrap();
        assert_eq!([back[0][3], back[1][3], back[2][3]], [-1.0, 10.0, 0.0]);
        assert!(slot_target(&world, &mounts, &frames, Place::Holster("knife")).is_none());
        assert!(slot_target(&world, &mounts, &frames, Place::Hidden).is_none());
    }
}
