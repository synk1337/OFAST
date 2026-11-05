//! Muzzle flashes (M6): which of CS2's flashes each gun fires, and from where.
//!
//! `scripts/weapons.vdata` names no muzzle flash (its `m_szMuzzleFlashParticle` is empty on every gun, read
//! 2026-11-05). The flash comes from the gun's world model, as its cases do ([`super::brass`]): the sequence the model
//! plays with each shot (`shoot`; `shoot_unsilenced` for the M4A1-S and USP-S with their silencer off;
//! `shoot_secondary` for the Dual Berettas' other pistol) starts a particle system with an event, mostly tagged
//! `muzzleflash` (not the Negev's: OFAST goes by the system's name, as the cases' events carry the tag too):
//! `AE_CL_CREATE_PARTICLE_EFFECT_CFG` naming a control-point configuration of the system (`thirdperson`, `game`,
//! `awp`…, whose first driver puts control point 0 on an attachment), or
//! `AE_CL_CREATE_PARTICLE_EFFECT` naming the attachment itself (the Desert Eagle's). The systems are 15 of
//! `particles/unified_weapon_fx/` (read 2026-11-05): `uweapon_muzflsh_ak47` (the AK-47, the M4A1-S without its
//! silencer), `_riffle` (M4A4, FAMAS), `_riffle_lrg` (Galil AR, SG 553), `_aug`, `weapon_muzzleflash_snip` (AWP,
//! SSG 08), `_snip_ar` (SCAR-20, G3SG1), `uweapon_muzflsh_mach` (M249, Negev), `_shot` (the shotguns),
//! `uweapon_muzzleflash_subm` (the SMGs), `uweapon_muzsilenced_subm` (MP5-SD, USP-S), `uweapon_muzsilenced_rif`
//! (M4A1-S, at its silencer's tip: `muzzle_flash2`), `uweapon_muzzleflash_pist` (the pistols), `_pist_revolver` (R8),
//! `uweapon_muzflsh_deagle`; the Zeus fires none. In the shooter's own view CS2 plays each one's `_fps` sibling on
//! the viewmodel (the AUG's and SG 553's sights an `_fps_ironsight` one): a system with the third-person one as its
//! child, smaller (`tools/unreal/shot_effects.py` draws both).
//!
//! The shots carry the system's stem and draw at the configuration's attachment (`muzzle_flash`, or `muzzle_flash2`:
//! the M4A1-S's silencer tip, one of the Dual Berettas' pistols), [`super::shots`]. Which of the Dual Berettas' two
//! sequences a shot plays is not in the demo: OFAST alternates by the magazine's count
//! ([`super::shots::ShotData::flash_of`]), not checked against CS2.

use std::collections::BTreeMap;

use super::shots::data_root;
use crate::content::Content;
use crate::kv3::Value;
use crate::resource::{BlockKind, Resource};

/// One flash a gun's shot starts (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashEvent {
    /// The particle system's file stem: `uweapon_muzflsh_ak47`.
    pub system: String,
    /// The attachment it plays on: `muzzle_flash` or `muzzle_flash2`.
    pub attachment: String,
}

/// The flashes of one gun, by the sequence that fires them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GunFlashes {
    /// `shoot`: every shot (with the silencer on, for the M4A1-S and USP-S; one of the pistols, for the Dual Berettas:
    /// at `muzzle_flash2`).
    pub fire: Option<FlashEvent>,
    /// `shoot_unsilenced`: the M4A1-S's and USP-S's with their silencer off.
    pub unsilenced: Option<FlashEvent>,
    /// `shoot_secondary`: the Dual Berettas' other pistol (at `muzzle_flash`).
    pub secondary: Option<FlashEvent>,
}

impl GunFlashes {
    /// The flash of a shot: `silencer` whether the gun has one, `silenced` whether it is on, `secondary` the Dual
    /// Berettas' `shoot_secondary`.
    pub fn of_shot(&self, silencer: bool, silenced: bool, secondary: bool) -> Option<&FlashEvent> {
        if silencer && !silenced {
            return self.unsilenced.as_ref().or(self.fire.as_ref());
        }
        if secondary {
            return self.secondary.as_ref().or(self.fire.as_ref());
        }
        self.fire.as_ref()
    }
}

/// A path's file stem: `particles/unified_weapon_fx/uweapon_muzflsh_ak47.vpcf` → `uweapon_muzflsh_ak47`.
fn stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file).to_owned()
}

/// The attachment a particle system's configuration `config` puts control point 0 on
/// (`m_controlPointConfigurations[].m_drivers[]`, a driver without `m_iControlPoint` being point 0); `None` when the
/// system or the configuration cannot be read.
fn config_attachment(content: &Content, system: &str, config: &str) -> Option<String> {
    let root = data_root(content, system).ok()?;
    root.get("m_controlPointConfigurations")?
        .as_array()?
        .iter()
        .find(|c| c.get("m_name").and_then(Value::as_str) == Some(config))?
        .get("m_drivers")?
        .as_array()?
        .iter()
        .find(|d| {
            d.get("m_iControlPoint")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                == 0
        })?
        .get("m_attachmentName")?
        .as_str()
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
}

/// The muzzle flash event of one sequence's events (module docs).
fn flash_of(content: &Content, events: &[Value]) -> Option<FlashEvent> {
    events.iter().find_map(|e| {
        let name = e.get("m_sEventName").and_then(Value::as_str)?;
        let data = e.get("m_EventData")?;
        // A muzzle flash's system (`…muzflsh…`, `…muzzleflash…`, `…muzsilenced…`): the events' `muzzleflash` tag is
        // on the cases' event too, and missing on some flashes (the Negev's).
        let system = data
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| stem(s).contains("muz"))?;
        let attachment = match name {
            "AE_CL_CREATE_PARTICLE_EFFECT_CFG" => {
                let config = data.get("config").and_then(Value::as_str)?;
                // The cases share the tag: their configuration is `shell_eject`.
                if config == "shell_eject" {
                    return None;
                }
                config_attachment(content, system, config)
                    .unwrap_or_else(|| "muzzle_flash".to_owned())
            }
            "AE_CL_CREATE_PARTICLE_EFFECT" => data
                .get("attachment_point")
                .and_then(Value::as_str)
                .unwrap_or("muzzle_flash")
                .to_owned(),
            _ => return None,
        };
        Some(FlashEvent {
            system: stem(system),
            attachment,
        })
    })
}

/// A gun's flashes, from its world model's sequences (module docs); empty when the model cannot be read.
pub fn gun_flashes(content: &Content, model: &str) -> GunFlashes {
    let Ok(bytes) = content.read_resource(model) else {
        return GunFlashes::default();
    };
    let Some(anims) = Resource::parse(&bytes)
        .ok()
        .and_then(|r| r.kv3(BlockKind(*b"ANIM")).ok())
        .map(|f| f.root)
    else {
        return GunFlashes::default();
    };
    let sequence = |wanted: &str| {
        anims
            .get("m_animArray")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .find(|s| s.get("m_name").and_then(Value::as_str) == Some(wanted))
            .and_then(|s| s.get("m_eventArray"))
            .and_then(Value::as_array)
            .and_then(|events| flash_of(content, events))
    };
    GunFlashes {
        fire: sequence("shoot"),
        unsilenced: sequence("shoot_unsilenced"),
        secondary: sequence("shoot_secondary"),
    }
}

/// The flashes of every gun in `models` (weapon name → world model), by weapon name; guns that fire none (the Zeus,
/// the knives, the grenades) are left out.
pub fn load_flashes<'a>(
    content: &Content,
    models: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, GunFlashes> {
    models
        .into_iter()
        .map(|(weapon, model)| (weapon.to_owned(), gun_flashes(content, model)))
        .filter(|(_, flashes)| flashes.fire.is_some() || flashes.unsilenced.is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flashes() -> GunFlashes {
        let event = |system: &str, attachment: &str| {
            Some(FlashEvent {
                system: system.into(),
                attachment: attachment.into(),
            })
        };
        GunFlashes {
            fire: event("uweapon_muzsilenced_rif", "muzzle_flash2"),
            unsilenced: event("uweapon_muzflsh_ak47", "muzzle_flash"),
            secondary: None,
        }
    }

    #[test]
    fn a_silenced_gun_flashes_by_its_silencer() {
        let m4 = flashes();
        assert_eq!(
            m4.of_shot(true, true, false).unwrap().system,
            "uweapon_muzsilenced_rif"
        );
        assert_eq!(
            m4.of_shot(true, false, false).unwrap().system,
            "uweapon_muzflsh_ak47"
        );
        // A gun without a silencer always fires `shoot`; a left pistol without its own sequence too.
        assert_eq!(
            m4.of_shot(false, false, true).unwrap().attachment,
            "muzzle_flash2"
        );
    }
}
