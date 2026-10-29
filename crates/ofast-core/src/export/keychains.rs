//! Keychains on weapons (M10): the charm a weapon carries, hung where CS2 hangs it (`docs/research/cs2-skins.md`,
//! "Keychains").
//!
//! # How CS2 hangs them
//!
//! - **The charm**: `items_game.txt` → `keychain_definitions` → `pedestal_display_model`, a small model of three
//!   bones (`joint1` the ring at the origin, `joint2`, `joint3` the charm) hanging down (−z, about two inches) at
//!   rest; CS2 swings it as cloth (`cloth_sleep_enabled`). The community charms share a blank model and name the
//!   material CS2 draws it with (`keychain_material`); a highlight reel (a tournament souvenir charm) has no model
//!   and shows its `base` definition's ([`Charm`], [`keychain_models`]).
//! - **Where**: the item's keychain offset (attributes 300 – 302) is a point in the weapon model's space (FalleN's
//!   USP-S: (9.74, 0.55, 3.96), an inch from the model's `keychain` attachment, on the slide's side). Without one,
//!   the model's `keychain` attachment (`keychain_legacy` on the CS:GO-era meshes). The model's `KeychainMarkup`
//!   (its keyvalues) lists the areas a keychain may be placed on, each on a bone (`slide`, `silencer`, `clip`…): the
//!   charm follows the bone of the area nearest its point.
//!
//! # As exported
//!
//! The charm's meshes join the weapon's (a merged part, `gltf::MergedModel`), rigid on the weapon bone it follows,
//! moved to its point and hanging as at rest: along the weapon's −z, not swung by gravity or motion.

use std::collections::{BTreeMap, HashMap};

use crate::content::Content;
use crate::export::gltf::load_model;
use crate::export::loadout::Finish;
use crate::export::paint::Paints;
use crate::items::section_values;
use crate::kv3::Value;
use crate::model::{Affine, Model, affine, compose};

/// A keychain definition's charm: its model, and the material CS2 draws it with instead of the model's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charm {
    /// `pedestal_display_model`.
    pub model: String,
    /// `keychain_material`: the community charms share blank models (`workshop_blanks/kc_missinglink_default`, a
    /// grey template) and differ only by it. CS2 draws the whole charm with it (the blanks have one draw call).
    pub material: Option<String>,
}

impl Charm {
    /// The key of the charm's loaded model in [`load_charms`]' map.
    fn key(&self) -> String {
        match &self.material {
            Some(material) => format!("{}|{material}", self.model),
            None => self.model.clone(),
        }
    }
}

/// The charms of `items_game.txt`'s text, by keychain definition id. A highlight-reel keychain (the tournament
/// souvenir charms: `highlight_reel` and `base`, no model of its own) is drawn as its `base` definition (named by
/// `name`).
pub fn keychain_models(items_game: &str) -> BTreeMap<u32, Charm> {
    let field = |wanted| section_values(items_game, "keychain_definitions", wanted);
    let (models, materials, bases) = (
        field("pedestal_display_model"),
        field("keychain_material"),
        field("base"),
    );
    let names = field("name");
    let by_name: HashMap<&str, u32> = names.iter().map(|(&id, n)| (n.as_str(), id)).collect();
    let own = |id: u32| {
        models.get(&id).map(|model| Charm {
            model: model.clone(),
            material: materials.get(&id).cloned(),
        })
    };
    models
        .keys()
        .chain(bases.keys())
        .filter_map(|&id| {
            let charm = own(id).or_else(|| own(*by_name.get(bases.get(&id)?.as_str())?))?;
            Some((id, charm))
        })
        .collect()
}

/// Where a charm hangs on a weapon model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hang {
    /// The weapon bone it follows (index in the model's skeleton).
    pub bone: usize,
    /// The point it hangs from, in the model's space (inches, Source axes).
    pub point: [f32; 3],
}

/// An area of a weapon's `KeychainMarkup`: the bone it is on and its four corners in model space.
#[derive(Debug, Clone, PartialEq)]
struct Area {
    legacy: bool,
    bone: String,
    corners: Vec<[f32; 3]>,
}

impl Area {
    /// How far `p` is from the box around the area's corners.
    fn distance(&self, p: [f32; 3]) -> f32 {
        let mut d2 = 0.0;
        for i in 0..3 {
            let lo = self.corners.iter().map(|c| c[i]).fold(f32::MAX, f32::min);
            let hi = self.corners.iter().map(|c| c[i]).fold(f32::MIN, f32::max);
            let out = (lo - p[i]).max(p[i] - hi).max(0.0);
            d2 += out * out;
        }
        d2.sqrt()
    }
}

/// The keychain areas in a model's keyvalues text (none without `KeychainMarkup`).
fn areas(key_values: &str) -> Vec<Area> {
    if !key_values.contains("KeychainMarkup") {
        return Vec::new();
    }
    let Ok(file) = crate::kv3::text::parse(key_values) else {
        return Vec::new();
    };
    file.root
        .get("KeychainMarkup")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let corners = e.get("Corners").and_then(Value::as_array)?;
            let floats: Vec<f32> = corners
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            Some(Area {
                legacy: e.get("LegacyModel").and_then(Value::as_bool) == Some(true),
                bone: e.get("BoneName").and_then(Value::as_str)?.to_owned(),
                corners: floats.as_chunks::<3>().0.to_vec(),
            })
        })
        .collect()
}

/// Where a charm hangs on `weapon` with mesh groups `mesh_groups` shown: at `offset` (the item's point) or the
/// model's `keychain` attachment, on the bone of the nearest `KeychainMarkup` area (else the attachment's bone).
/// `None` when the model has neither a point nor the attachment.
pub fn hang(weapon: &Model, mesh_groups: u64, offset: Option<[f32; 3]>) -> Option<Hang> {
    let legacy = weapon
        .meshes_in_groups(mesh_groups)
        .any(|m| m.name.contains("legacy"));
    let world = weapon.skeleton.world_transforms();
    let attachment = weapon
        .attachment(if legacy {
            "keychain_legacy"
        } else {
            "keychain"
        })
        .or_else(|| weapon.attachment("keychain"))
        .and_then(|a| {
            let influence = a.influences.first()?;
            let bone = weapon.skeleton.find(&influence.bone)?;
            let m = compose(&world[bone], &affine(influence.rotation, influence.offset));
            Some((bone, [m[0][3], m[1][3], m[2][3]]))
        });
    let point = offset.or(attachment.map(|(_, p)| p))?;
    let nearest = areas(&weapon.key_values_text)
        .into_iter()
        .filter(|a| a.legacy == legacy)
        .min_by(|a, b| a.distance(point).total_cmp(&b.distance(point)))
        .and_then(|a| weapon.skeleton.find(&a.bone));
    let bone = nearest.or(attachment.map(|(b, _)| b)).unwrap_or(0);
    Some(Hang { bone, point })
}

/// Loads the charm models the `finishes` name into `charms` (by path); one that cannot be read is left out, with a
/// warning.
pub(crate) fn load_charms(
    content: &Content,
    paints: &Paints,
    finishes: impl IntoIterator<Item = Option<Finish>>,
    charms: &mut BTreeMap<String, Model>,
    warnings: &mut Vec<String>,
) {
    for keychain in finishes.into_iter().flatten().filter_map(|f| f.keychain) {
        let Some(charm) = paints.keychain(keychain.id) else {
            continue;
        };
        let key = charm.key();
        if charms.contains_key(&key) {
            continue;
        }
        match load_model(content, &charm.model) {
            Ok(mut model) => {
                if let Some(material) = &charm.material {
                    for draw in model.meshes.iter_mut().flat_map(|m| &mut m.draw_calls) {
                        draw.material.clone_from(material);
                    }
                }
                charms.insert(key, model);
            }
            Err(e) => warnings.push(format!("keychain {}: {e}: not shown", charm.model)),
        }
    }
}

/// The charm a weapon with `finish` carries, from `charms` ([`load_charms`]), and where it hangs on `weapon` with
/// mesh groups `mesh_groups` shown.
pub(crate) fn charm<'m>(
    paints: &Paints,
    charms: &'m BTreeMap<String, Model>,
    weapon: &Model,
    mesh_groups: u64,
    finish: Option<Finish>,
) -> Option<(&'m Model, Hang)> {
    let keychain = finish?.keychain?;
    let model = charms.get(&paints.keychain(keychain.id)?.key())?;
    Some((model, hang(weapon, mesh_groups, keychain.offset())?))
}

/// The charm's space → the weapon model's space: moved to the point it hangs from.
pub(crate) fn at_point(hang: &Hang) -> Affine {
    affine([0.0, 0.0, 0.0, 1.0], hang.point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_areas_read_and_measure() {
        let text = r#"<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:vmdlkeys4:version{9f5c1002-f19f-470c-bd42-c3c26c063d33} -->
{
	KeychainMarkup =
	[
		{
			LegacyModel = true
			BoneName = "slide"
			Corners = [ 0.0, 0.5, 3.0, 4.0, 0.5, 3.0, 0.0, 0.5, 4.0, 4.0, 0.5, 4.0 ]
		},
		{
			BoneName = "silencer"
			Corners = [ 10.0, 0.7, 3.8, 18.0, 0.7, 3.8, 10.0, 0.7, 4.3, 18.0, 0.7, 4.3 ]
		},
	]
}"#;
        let areas = areas(text);
        assert_eq!(areas.len(), 2);
        assert!(areas[0].legacy && !areas[1].legacy);
        assert_eq!(areas[1].bone, "silencer");
        assert_eq!(areas[1].corners.len(), 4);
        // Inside the silencer's box: no distance; a point an inch past its end: one.
        assert_eq!(areas[1].distance([12.0, 0.7, 4.0]), 0.0);
        assert!((areas[1].distance([19.0, 0.7, 4.0]) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn definitions_carry_their_material_and_reels_their_base() {
        let text = r#""items_game"
{
	"keychain_definitions"
	{
		"50"
		{
			"name"		"kc_missinglink_pencil"
			"pedestal_display_model"		"weapons/keychains/workshop_blanks/kc_missinglink_default.vmdl"
			"keychain_material"		"weapons/keychains/missinglink_community_01/kc_missinglink_pencil.vmat"
		}
	}
	"keychain_definitions"
	{
		"84"
		{
			"name"		"kc_cologne2026"
			"pedestal_display_model"		"weapons/keychains/cologne2026/kc_cologne2026.vmdl"
		}
		"85"
		{
			"name"		"kc_cologne2026_st1ultimatenukejumpkill"
			"base"		"kc_cologne2026"
			"highlight_reel"		"cologne2026_st1ultimatenukejumpkill"
		}
		"86"
		{
			"name"		"kc_unknown_reel"
			"base"		"kc_missing"
		}
	}
}"#;
        let charms = keychain_models(text);
        assert_eq!(charms.len(), 3, "the reel with an unknown base is left out");
        assert_eq!(
            charms[&50].material.as_deref(),
            Some("weapons/keychains/missinglink_community_01/kc_missinglink_pencil.vmat")
        );
        assert_eq!(charms[&85], charms[&84]);
        assert_eq!(charms[&84].material, None);
        assert_ne!(charms[&50].key(), charms[&50].model);
    }
}
