//! A round's gun shots for Unreal (M6): where each bullet went and what it met, and where it left the barrel.
//!
//! The demo records each shot's aim and random seed, not its bullets' paths ([`crate::demo::shots`]); CS2's
//! clients trace the bullets themselves to draw tracers, impacts and bullet holes. OFAST does the same: each bullet's
//! direction from the seed ([`crate::demo::shots::spread_offset`]: on the de_nuke pro demo 82 of the 83 bullets a
//! pane of glass shows lie on it, the other being another bullet's), traced from the shooter's eyes through the
//! map's collision as bullets see it ([`crate::physics::CollisionAttribute::stops_bullets`]: not grates, not glass) up
//! to the weapon's range ([`WeaponShots::range`]).
//!
//! **Penetration.** A bullet goes through walls ([`walls_on`]: where its line goes into the world's solids and comes
//! out) and players, each costing it damage ([`penetrate`]: CS:GO's rule, which CS2 keeps, with each surface's
//! `bulletPenetrationDistanceModifier` from `scripts/surfaceproperties_game.txt`, [`ShotData::through`]), until one
//! stops it (less than 1 damage left, a wall thicker than [`MAX_THROUGH`], or [`MAX_PENETRATIONS`] gone through).
//! The demo's hits on players ([`crate::demo::shots::PlayerHit`]: the blood's effect dispatch) are the truth along
//! the way: a hit after a wall starts where the bullet came out of it, so a wall before a hit is gone through
//! whatever the rule says, a wall traced between a hit's start and its point is not there for CS2, and a later hit
//! starting just past a player is where the bullet came out of them (else where its line leaves their hitboxes as
//! the exported agent shows them, [`Body`], else [`PLAYER_DEPTH`] on). Checked
//! (`examples/shot_penetration.rs`, 13 demos of 6 maps): 456 of the 545 hits after a wall start within half an inch
//! of where the trace comes out of one; the damage CS2 recorded for 289 of the 314 non-lethal hits through walls is
//! the rule's exactly, 6 more within 1 (10 of the others are on one de_inferno demo's plaster walls, where that
//! build of CS2 took plaster's distance modifier as 0.5, not the current 0.7). The rotating doors are walls too, as
//! the demo has them standing ([`Doors`]: their own models, not in the world's collision), without bullet holes (a
//! door moves); no hit of the 13 demos has a door on its line before it, so the demos neither confirm nor refute
//! them. The map's other model entities are not walls: with its `func_brush`, `prop_dynamic` and `func_breakable`
//! models placed as the entity lump has them, 33 of the 40 bullets that crossed a visible one still hit a player
//! straight from the eyes, and the damages through walls came out worse (40 off, against 19); the disabled ones
//! (other game modes' blockers and props: `[PR#]retake.bsite`, de_nuke's `[PR#]props.2v2` silo doors, not
//! networked) far worse (184 off). CS2 networks most of its `prop_dynamic` props without the `solid` interaction
//! layer (de_nuke: 7 of 9 interact as window, block sound and touch all only). What the other 81 hits after
//! something came out of is not known yet (the other 8 of the 89 start away from the traced way out).
//!
//! Written to the agents manifest as `shots` (it needs the map's collision and the baked agents):
//!
//! | Key | What |
//! |---|---|
//! | `tick`, `controller` | the shot's tick (its message's) and shooter |
//! | `time` | when it was fired, demo ticks to a thousandth ([`crate::demo::shots::shot_times`]: CS2 fires between ticks) |
//! | `item`, `weapon`, `mode`, `silenced` | the weapon's item definition index and name (`weapon_ak47`); the fire mode (1: secondary); whether the weapon's silencer is on (its secondary mode) |
//! | `tracer` | the weapon's tracer system's stem when this shot draws one ([`draws_tracer`]), else `null` |
//! | `eye` | where the bullets start: the shooter's eyes (Source axes, inches) |
//! | `muzzle`, `muzzle_direction` | where they left the barrel as the exported agent shows it (third person), and along the barrel; `null` when the agent shows no gun in hand then |
//! | `view_muzzle`, `view_muzzle_direction` | the same as the shooter's viewmodel shows it (first person), in the viewmodel rig's model space: Source axes (x forward, y left, z up), inches (in Unreal the viewmodel's component space: the camera, before the player's viewmodel offsets and the left hand's mirroring); `null` without a viewmodel then |
//! | `view_tracer` | where the shooter's own view shows the tracer leave, in the world: the viewmodel's muzzle on the shot tick's camera, moved to the world point seen at the same place on the screen ([`view_tracer_start`]); `null` without a viewmodel |
//! | `bullets` | per bullet (shotguns fire several): `end` (where it stopped), `on` (`"world"`, `"player"` or `"air"`: nothing within range), `normal` (of the surface met, facing the shooter; `null` on a player or in the air), `surface` (the world surface's property name, `null` when unknown), `impact` (the surface's impact system, [`Impact::effect`]), `decal` (its bullet-hole decal group) and, on the world, `thickness` (of the wall it stopped in, `null` when the trace finds no way out of it); `through`: what it went through before, in order: each wall's way in and way out (`on` `"world"`, `end`, `normal`: facing back at the bullet going in, along it coming out, `surface`, `impact`, `decal`, `thickness`; `door` true on a door's side, which takes no bullet hole) and each player (`on` `"player"`, `end`: where it met them, `victim`: the player hit, their controller, from the posed hitboxes nearest the hit, [`body_hit`]; `null` when none is near); a bullet ending on a player has `victim` too |

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::{Value as Json, json};

use super::gltf::write_png;
use super::players::Muzzle;
use crate::content::Content;
use crate::demo::players::{RoundTrack, ViewmodelSettings};
use crate::demo::recoil::RecoilKeys;
use crate::demo::shots::{PlayerHit, Shot, bullet_direction, shot_times, spread_offset};
use crate::items::item_names;
use crate::kv3::Value;
use crate::physics::PhysicsData;
use crate::physics::ragdoll::{Crossing, WorldCollision};
use crate::resource::{BlockKind, Resource};
use crate::texture::Texture;

/// A weapon's shots as `scripts/weapons.vdata` describes them (each entry inherits what it lacks from its `_base`).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponShots {
    /// `m_nNumBullets`: bullets per shot (the Nova 9, the XM1014 6, the MAG-7 and Sawed-Off 8, every other gun 1).
    pub bullets: u32,
    /// `m_nTracerFrequency`, in the normal and the secondary mode (a single value is both): 0 draws no tracer (the
    /// M4A1-S and the USP-S with the silencer on, their secondary mode); any other value one on every shot, as CS2
    /// draws them ([`draws_tracer`]), not one in that many.
    pub tracer_frequency: [u32; 2],
    /// `m_szTracerParticle`'s file stem: `weapon_tracers_assrifle`, `_rifle` (the snipers), `_pistol`, `_shot`,
    /// `_smg`…
    pub tracer: Option<String>,
    /// `m_eSilencerType` is not `WEAPONSILENCER_NONE`: the secondary mode is the silencer on.
    pub silencer: bool,
    /// `m_flRange`: how far its bullets go (units; rifles and snipers 8192, the Glock 4096, the MAG-7 1400). In
    /// Source's bullets (CS:GO), the trace ends there and a wall further away is not hit.
    pub range: f32,
    /// `m_nDamage`: a bullet's damage as it leaves the barrel (the AK-47 36, the AWP 115, the Glock 30).
    pub damage: f32,
    /// `m_flPenetration`: its power through walls (1 the pistols, SMGs and shotguns; 2 the rifles, machine guns and
    /// Desert Eagle; 2.5 the snipers).
    pub penetration: f32,
    /// `m_flRangeModifier`: the share of its damage a bullet keeps per 500 units it flies (the AK-47 0.98, the MAG-7
    /// 0.45).
    pub range_modifier: f32,
    /// `m_flHeadshotMultiplier` (4 on most guns).
    pub headshot_multiplier: f32,
    /// `m_flArmorRatio`: how much of a hit on armour still goes to health (halved: the AK-47's 1.55 lets 77.5 %).
    pub armor_ratio: f32,
    /// `m_szWorldModel`: the gun's model (`weapons/models/ak47/weapon_rif_ak47.vmdl`; CS2's viewmodels draw it
    /// too), whose shoot sequence throws its cases ([`super::brass`]).
    pub model: Option<String>,
    /// Its recoil keys ([`crate::demo::recoil::RecoilPattern`]).
    pub recoil: RecoilKeys,
}

/// What a bullet does to a surface, from `scripts/surfaceproperties_impact_effects.txt`.
#[derive(Debug, Clone, PartialEq)]
pub struct Impact {
    /// The impact's particle system, its file stem: `impact_concrete`, `impact_metal`, `impact_wood`…
    pub effect: String,
    /// The bullet hole's decal group (`scripts/decalgroups.vdata`): `Impact.Concrete`, `Impact.Metal`…; `None` for
    /// surfaces that keep none (grass, water).
    pub decal: Option<String>,
}

/// What CS2's files say about shots ([`ShotData::load`]).
#[derive(Debug, Clone, Default)]
pub struct ShotData {
    /// Per weapon name (`weapon_ak47`).
    pub weapons: HashMap<String, WeaponShots>,
    /// Item definition index → weapon name (`scripts/items/items_game.txt`).
    pub items: HashMap<u32, String>,
    /// Surface property name hash ([`crate::physics::surface_hash`]) → its name and its base's hash (0: none), from
    /// `surfaceproperties/surfaceproperties.vsurf` (`surfacePropertyName`, `m_nameHash`, `m_baseNameHash`).
    pub surfaces: HashMap<u32, (String, u32)>,
    /// Lower-cased surface name → its impact.
    pub impacts: HashMap<String, Impact>,
    /// Lower-cased surface name → what bullets going through it lose, as `scripts/surfaceproperties_game.txt` sets
    /// it (a surface sets only what differs from its base: [`ShotData::through`]).
    pub game: HashMap<String, SurfaceGame>,
}

/// A surface's game properties for bullets (`scripts/surfaceproperties_game.txt`); `None` where the file leaves it to
/// the surface's base.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceGame {
    /// `bulletPenetrationDistanceModifier`: how easily bullets go through (concrete's `default` 0.5, `solidmetal`
    /// 0.27, `Wood` 0.9, `metalgrate` 0.95).
    pub distance: Option<f32>,
    /// `bulletPenetrationDamageModifier` (`default` 0.5, `solidmetal` 0.3, `Wood` 0.6).
    pub damage: Option<f32>,
    /// `gamematerial`: the surface's material letter (`C` concrete, `M` metal, `W` wood, `G` grate…).
    pub material: Option<String>,
}

/// [`SurfaceGame`] with every value found: what a bullet loses going through a surface ([`ShotData::through`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Through {
    /// [`SurfaceGame::distance`].
    pub distance: f32,
    /// [`SurfaceGame::damage`].
    pub damage: f32,
    /// [`SurfaceGame::material`] (concrete's `C` when no surface up the chain sets one).
    pub material: String,
}

/// The `DATA` block of a compiled resource.
pub(super) fn data_root(content: &Content, path: &str) -> Result<Value, String> {
    let bytes = content
        .read_resource(path)
        .map_err(|e| format!("{path}: {e}"))?;
    let resource = Resource::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    resource
        .kv3(BlockKind::DATA)
        .map(|file| file.root)
        .map_err(|e| format!("{path}: {e}"))
}

/// A path's file stem: `particles/impact_fx/impact_concrete.vpcf` → `impact_concrete`.
fn stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file).to_owned()
}

impl ShotData {
    /// Reads `scripts/weapons.vdata`, `surfaceproperties/surfaceproperties.vsurf` and
    /// `scripts/surfaceproperties_impact_effects.txt`, and the items' names from `items_text` (`items_game.txt`). A
    /// file that cannot be read leaves its part empty, with a warning.
    pub fn load(content: &Content, items_text: Option<&str>, warnings: &mut Vec<String>) -> Self {
        let mut data = ShotData {
            items: items_text.map(item_names).unwrap_or_default(),
            ..Self::default()
        };
        match data_root(content, "scripts/weapons.vdata") {
            Ok(root) => data.weapons = weapons_of(&root),
            Err(e) => warnings.push(format!(
                "{e}: every gun fires one bullet and draws no tracer"
            )),
        }
        match data_root(content, "surfaceproperties/surfaceproperties.vsurf") {
            Ok(root) => data.surfaces = surfaces_of(&root),
            Err(e) => warnings.push(format!("{e}: impacts are concrete's")),
        }
        let text_root = |path: &str| {
            content
                .read(path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    crate::kv3::text::parse(&String::from_utf8_lossy(&bytes))
                        .map_err(|e| e.to_string())
                })
                .map(|file| file.root)
                .map_err(|e| format!("{path}: {e}"))
        };
        match text_root("scripts/surfaceproperties_impact_effects.txt") {
            Ok(root) => data.impacts = impacts_of(&root),
            Err(e) => warnings.push(format!("{e}: impacts are concrete's")),
        }
        match text_root("scripts/surfaceproperties_game.txt") {
            Ok(root) => data.game = games_of(&root),
            Err(e) => warnings.push(format!("{e}: every wall stops bullets as concrete does")),
        }
        data
    }

    /// What a bullet loses going through the surface named by `hash`: each value the nearest up its base chain sets
    /// (`metal` sets its distance modifier, 0.4, and takes its damage modifier and material from `solidmetal`), else
    /// `default`'s, else concrete's as CS2 sets it (0.5, 0.5, `C`).
    pub fn through(&self, hash: u32) -> Through {
        let (mut distance, mut damage, mut material) = (None, None, None);
        let mut at = hash;
        let mut chain = Vec::new();
        // The chains are a few long; the bound only guards against a loop.
        for _ in 0..16 {
            let Some((surface, base)) = self.surfaces.get(&at) else {
                break;
            };
            chain.push(surface.to_ascii_lowercase());
            if *base == 0 {
                break;
            }
            at = *base;
        }
        chain.push("default".to_owned());
        for game in chain.iter().filter_map(|name| self.game.get(name)) {
            distance = distance.or(game.distance);
            damage = damage.or(game.damage);
            material = material.or_else(|| game.material.clone());
        }
        Through {
            distance: distance.unwrap_or(0.5),
            damage: damage.unwrap_or(0.5),
            material: material.unwrap_or_else(|| "C".to_owned()),
        }
    }

    /// The shots of the weapon with item definition index `item`.
    pub fn weapon(&self, item: u32) -> Option<&WeaponShots> {
        self.weapons.get(self.items.get(&item)?)
    }

    /// The surface named by `hash` and the impact a bullet makes on it: the surface's name, and the impact of the
    /// nearest surface up its base chain that has one (`metal` → `solidmetal`, `sand` → `dirt`, `brick` →
    /// `concrete`…), else `default`'s (concrete).
    pub fn impact(&self, hash: u32) -> (Option<&str>, Option<&Impact>) {
        let name = self.surfaces.get(&hash).map(|(n, _)| n.as_str());
        let mut at = hash;
        // The chains are a few long; the bound only guards against a loop.
        for _ in 0..16 {
            let Some((surface, base)) = self.surfaces.get(&at) else {
                break;
            };
            if let Some(impact) = self.impacts.get(&surface.to_ascii_lowercase()) {
                return (name, Some(impact));
            }
            if *base == 0 {
                break;
            }
            at = *base;
        }
        (name, self.impacts.get("default"))
    }
}

/// [`ShotData::weapons`] from `weapons.vdata`'s root: every `weapon_*` entry, its fields looked up through its
/// `_base` entries.
fn weapons_of(root: &Value) -> HashMap<String, WeaponShots> {
    let Some(entries) = root.as_object() else {
        return HashMap::new();
    };
    let field = |name: &str, key: &str| -> Option<&Value> {
        let mut at = entries.get(name)?;
        for _ in 0..16 {
            if let Some(value) = at.get(key) {
                return Some(value);
            }
            at = entries.get(at.get("_base")?.as_str()?)?;
        }
        None
    };
    let count = |v: &Value| v.as_i64().and_then(|n| u32::try_from(n).ok());
    entries
        .iter()
        .filter(|(name, _)| name.starts_with("weapon_"))
        .map(|(name, _)| {
            // A number field (an integer or a float), else `default`.
            let number = |key: &str, default: f32| {
                field(name, key)
                    .and_then(Value::as_f64)
                    .map_or(default, |n| n as f32)
            };
            // A pair of numbers per mode (a single number is both).
            let pair = |key: &str| -> [f32; 2] {
                let value = field(name, key);
                let at = |i: usize| {
                    value
                        .and_then(|v| match v.as_array() {
                            Some(modes) => modes.get(i).or(modes.first()).and_then(Value::as_f64),
                            None => v.as_f64(),
                        })
                        .unwrap_or(0.0) as f32
                };
                [at(0), at(1)]
            };
            let tracer_frequency = match field(name, "m_nTracerFrequency") {
                Some(v) => match v.as_array() {
                    Some(modes) => {
                        let first = modes.first().and_then(count).unwrap_or(0);
                        [first, modes.get(1).and_then(count).unwrap_or(first)]
                    }
                    None => [count(v).unwrap_or(0); 2],
                },
                None => [0, 0],
            };
            let shots = WeaponShots {
                bullets: field(name, "m_nNumBullets").and_then(count).unwrap_or(1),
                tracer_frequency,
                tracer: field(name, "m_szTracerParticle")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                    .map(stem),
                silencer: field(name, "m_eSilencerType")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s != "WEAPONSILENCER_NONE"),
                range: field(name, "m_flRange")
                    .and_then(Value::as_f64)
                    .map_or(REACH, |r| r as f32),
                damage: number("m_nDamage", 0.0),
                penetration: number("m_flPenetration", 1.0),
                range_modifier: number("m_flRangeModifier", 1.0),
                headshot_multiplier: number("m_flHeadshotMultiplier", 4.0),
                armor_ratio: number("m_flArmorRatio", 1.0),
                model: field(name, "m_szWorldModel")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned),
                recoil: RecoilKeys {
                    seed: field(name, "m_nRecoilSeed")
                        .and_then(Value::as_i64)
                        .unwrap_or(0) as i32,
                    full_auto: field(name, "m_bIsFullAuto")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    angle: pair("m_flRecoilAngle"),
                    angle_variance: pair("m_flRecoilAngleVariance"),
                    magnitude: pair("m_flRecoilMagnitude"),
                    magnitude_variance: pair("m_flRecoilMagnitudeVariance"),
                },
            };
            (name.to_owned(), shots)
        })
        .collect()
}

/// [`ShotData::surfaces`] from `surfaceproperties.vsurf`'s root (`SurfacePropertiesList`).
fn surfaces_of(root: &Value) -> HashMap<u32, (String, u32)> {
    let hash = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_i64)
            .and_then(|h| u32::try_from(h).ok())
    };
    root.get("SurfacePropertiesList")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let name = s.get("surfacePropertyName")?.as_str()?.to_owned();
            Some((
                hash(s, "m_nameHash")?,
                (name, hash(s, "m_baseNameHash").unwrap_or(0)),
            ))
        })
        .collect()
}

/// [`ShotData::impacts`] from `surfaceproperties_impact_effects.txt`'s root (`SurfacePropertiesList`).
fn impacts_of(root: &Value) -> HashMap<String, Impact> {
    root.get("SurfacePropertiesList")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let name = s.get("surfacePropertyName")?.as_str()?.to_ascii_lowercase();
            let impact = Impact {
                effect: stem(s.get("effect")?.as_str()?),
                decal: s
                    .get("impactDecalName")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            Some((name, impact))
        })
        .collect()
}

/// [`ShotData::game`] from `surfaceproperties_game.txt`'s root (`SurfacePropertiesList`).
fn games_of(root: &Value) -> HashMap<String, SurfaceGame> {
    let number = |s: &Value, key: &str| s.get(key).and_then(Value::as_f64).map(|v| v as f32);
    root.get("SurfacePropertiesList")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let name = s.get("surfacePropertyName")?.as_str()?.to_ascii_lowercase();
            let game = SurfaceGame {
                distance: number(s, "bulletPenetrationDistanceModifier"),
                damage: number(s, "bulletPenetrationDamageModifier"),
                material: s
                    .get("gamematerial")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            Some((name, game))
        })
        .collect()
}

/// Writes the bullet holes of decal groups `groups` (`scripts/decalgroups.vdata`): each option's colour texture
/// (`g_tColor`) and height map (`g_tHeight`: the hole's depth, white at the surface) as `decal_<texture stem>.png`
/// in `out_dir`. Returns the manifest's `decals`: per group, its options' `file`, `height_file` (`null` without a
/// height map), `width` and `height` (`DecalWorldWidth`, `DecalWorldHeight`: units) and `probability`. An option
/// that cannot be read is left out with a warning.
pub fn export_decals(
    content: &Content,
    groups: &BTreeSet<String>,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Vec<Json>> {
    let mut out = BTreeMap::new();
    if groups.is_empty() {
        return out;
    }
    let root = match data_root(content, "scripts/decalgroups.vdata") {
        Ok(root) => root,
        Err(e) => {
            warnings.push(format!("{e}: no bullet holes"));
            return out;
        }
    };
    for group in groups {
        let options = root
            .get(group)
            .and_then(|g| g.get("m_vecOptions"))
            .and_then(Value::as_array)
            .unwrap_or_default();
        let mut written = Vec::new();
        for option in options {
            let Some(material) = option.get("m_hMaterial").and_then(Value::as_str) else {
                continue;
            };
            match export_decal(content, material, out_dir) {
                Ok(mut entry) => {
                    entry["probability"] = json!(
                        option
                            .get("m_flProbability")
                            .and_then(Value::as_f64)
                            .unwrap_or(1.0)
                    );
                    written.push(entry);
                }
                Err(e) => warnings.push(format!("bullet hole {material}: {e}")),
            }
        }
        out.insert(group.clone(), written);
    }
    out
}

/// One bullet hole material's colour texture and size ([`export_decals`]).
fn export_decal(content: &Content, material: &str, out_dir: &Path) -> Result<Json, String> {
    let root = data_root(content, material)?;
    let texture_param = |name: &str| {
        root.get("m_textureParams")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .find(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
            .and_then(|p| p.get("m_pValue"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let attribute = |name: &str| {
        root.get("m_floatAttributes")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .find(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
            .and_then(|p| p.get("m_flValue"))
            .and_then(Value::as_f64)
    };
    let write = |texture_path: &str| -> Result<String, String> {
        let bytes = content
            .read_resource(texture_path)
            .map_err(|e| format!("{texture_path}: {e}"))?;
        let resource = Resource::parse(&bytes).map_err(|e| format!("{texture_path}: {e}"))?;
        let texture = Texture::parse(&resource).map_err(|e| format!("{texture_path}: {e}"))?;
        let image = texture
            .decode_rgba8(&bytes)
            .map_err(|e| format!("{texture_path}: {e}"))?;
        let file = format!("decal_{}.png", stem(texture_path));
        write_png(&out_dir.join(&file), &image).map_err(|e| e.to_string())?;
        Ok(file)
    };
    let colour = texture_param("g_tColor").ok_or("no colour texture (g_tColor)")?;
    // The hole's depth (CS2 draws it with parallax and occlusion: its holes read black where the colour texture is
    // grey concrete dust); optional.
    let depth = texture_param("g_tHeight").and_then(|path| write(&path).ok());
    Ok(json!({
        "material": material,
        "file": write(&colour)?,
        "height_file": depth,
        "width": attribute("DecalWorldWidth").unwrap_or(6.0),
        "height": attribute("DecalWorldHeight").unwrap_or(6.0),
    }))
}

/// How far a bullet of a weapon whose range is not known is traced (inches): the longest range of CS2's guns
/// (`m_flRange` 8192 on rifles and snipers).
pub const REACH: f32 = 8192.0;

/// A solid a bullet's line goes through ([`walls_on`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Wall {
    /// Where the line goes in (inches from where the bullet started).
    pub distance: f32,
    /// How far it goes through (inches); infinite when it does not come out within the line's reach.
    pub thickness: f32,
    /// The surface it goes in by, facing back at the bullet; and its surface property's hash
    /// ([`crate::physics::surface_hash`]; 0 unknown).
    pub enter_normal: [f32; 3],
    /// [`Wall::enter_normal`]'s surface property hash.
    pub enter_surface: u32,
    /// The surface it comes out by, facing along the bullet ([`Wall::thickness`] finite).
    pub exit_normal: [f32; 3],
    /// [`Wall::exit_normal`]'s surface property hash.
    pub exit_surface: u32,
    /// What the bullet loses going in ([`ShotData::through`]).
    pub enter: Through,
    /// What it loses coming out.
    pub exit: Through,
    /// The way in is a door's ([`Doors`]): it moves, so a bullet hole (a decal fixed in the world) would hang in the
    /// air once it swings: the bullet makes its impact there but no hole.
    pub door_in: bool,
    /// The way out is a door's.
    pub door_out: bool,
}

/// [`Crossing::shape`] of a door's surface ([`Doors::crossings`]).
const DOOR_SHAPE: u32 = u32::MAX - 1;

/// Two solids whose surfaces are closer than this (inches) along a bullet's line are one wall to it (a wall built of
/// two hulls side by side).
const TOUCHING: f32 = 0.1;

/// The solids of `collision` the line from `from` along the unit `direction` goes through within `reach`
/// ([`WorldCollision::ray_crossings`]): a wall begins where the line goes into a solid while outside every solid and
/// ends where it comes out of the last one it is in (solids that overlap or touch are one wall). A way out met
/// outside every solid (the eyes inside one, a one-sided surface seen from behind: 0 – 0.7 % of bullets from the
/// eyes on the demos checked) is passed through; a way in never matched by a way out (an open mesh) makes a wall
/// with no end, which stops the bullet. `surfaces`: the world's surface property hashes by index
/// ([`PhysicsData::surfaces`]).
pub fn walls_on(
    collision: &WorldCollision,
    surfaces: &[u32],
    data: &ShotData,
    from: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> Vec<Wall> {
    let hash = |index: u32| surfaces.get(index as usize).copied().unwrap_or(0);
    let crossings = collision
        .ray_crossings(from, direction, reach)
        .into_iter()
        .map(|c| (c, hash(c.surface)))
        .collect();
    walls_through(crossings, data)
}

/// The walls `crossings` make ([`walls_on`]): each crossing of a line with a surface, with its surface property's
/// hash (0: unknown), from the world's collision and others (the doors, [`Doors`]); sorted here.
pub fn walls_through(mut crossings: Vec<(Crossing, u32)>, data: &ShotData) -> Vec<Wall> {
    crossings.sort_by(|a, b| a.0.distance.total_cmp(&b.0.distance));
    let mut walls: Vec<Wall> = Vec::new();
    let mut depth = 0usize;
    // Every crossing counts, even two at one point facing the same way: a line through an edge two triangles of a
    // face share crosses both, but merging those (one crossing) matched CS2 worse on the 13 demos checked (450 hits
    // after a wall starting at a traced way out, against 456; 284 exact damages through walls, against 289): CS2's
    // world meshes also hold separate solids' faces at one place.
    for (crossing, surface) in crossings {
        if crossing.entering {
            depth += 1;
            if depth > 1 {
                continue;
            }
            match walls.last_mut() {
                // Into a solid touching the last one's way out: the same wall.
                Some(last) if crossing.distance - (last.distance + last.thickness) < TOUCHING => {
                    last.thickness = f32::INFINITY;
                }
                _ => {
                    walls.push(Wall {
                        distance: crossing.distance,
                        thickness: f32::INFINITY,
                        enter_normal: crossing.normal,
                        enter_surface: surface,
                        exit_normal: [0.0; 3],
                        exit_surface: 0,
                        enter: data.through(surface),
                        exit: data.through(surface),
                        door_in: crossing.shape == DOOR_SHAPE,
                        door_out: false,
                    });
                }
            }
        } else if depth > 0 {
            depth -= 1;
            if depth == 0
                && let Some(wall) = walls.last_mut()
            {
                wall.thickness = crossing.distance - wall.distance;
                wall.exit_normal = crossing.normal;
                wall.exit_surface = surface;
                wall.exit = data.through(surface);
                wall.door_out = crossing.shape == DOOR_SHAPE;
            }
        }
    }
    // A line through an edge where a solid's faces meet goes in and out at one point: no wall (on de_anubis CS2's
    // damage shows a bullet there lost nothing).
    walls.retain(|w| w.thickness > GRAZE);
    walls
}

/// A wall thinner than this (inches) is a line grazing an edge ([`walls_on`]).
const GRAZE: f32 = 1e-3;

/// What a bullet's line meets ([`bullet_json`]): the world's collision (`None`: nothing), its surface property hashes
/// by index, the doors as `track` has them standing, and the players' bodies on the ticks of their hits (sorted by
/// tick).
struct Obstacles<'a> {
    world: Option<&'a WorldCollision>,
    surfaces: &'a [u32],
    doors: Option<(&'a Doors, &'a RoundTrack)>,
    bodies: &'a [Body],
}

/// How far (inches) a hit's point may lie from a body's hitbox surface for the hit to be on that body ([`body_hit`]),
/// and how far ahead of the hit along the bullet's line its stretch through the body may start ([`way_through`]).
pub const BODY_MATCH: f32 = 4.0;

impl Obstacles<'_> {
    /// The walls on the line from `from` along `direction` within `reach` on `tick` ([`walls_through`]).
    fn walls(
        &self,
        tick: u32,
        from: [f32; 3],
        direction: [f32; 3],
        reach: f32,
        data: &ShotData,
    ) -> Vec<Wall> {
        let mut crossings: Vec<(Crossing, u32)> = self
            .world
            .map(|c| c.ray_crossings(from, direction, reach))
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                (
                    c,
                    self.surfaces.get(c.surface as usize).copied().unwrap_or(0),
                )
            })
            .collect();
        if let Some((doors, track)) = self.doors {
            crossings.extend(doors.crossings(track, tick, from, direction, reach));
        }
        walls_through(crossings, data)
    }
}

/// The body a bullet met at `point` on `tick`: of `bodies` (sorted by tick) posed on that tick, not the `shooter`'s,
/// the one with a hitbox surface nearest the point, within [`BODY_MATCH`].
pub fn body_hit(
    bodies: &[Body],
    tick: u32,
    point: [f32; 3],
    shooter: Option<u32>,
) -> Option<&Body> {
    let first = bodies.partition_point(|b| b.tick < tick);
    bodies[first..]
        .iter()
        .take_while(|b| b.tick == tick)
        .filter(|b| Some(b.controller) != shooter)
        .map(|b| (b, b.surface_distance(point)))
        .filter(|(_, d)| *d <= BODY_MATCH)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

/// The stretch of the line from `from` along `direction` inside `body` that a bullet meeting it `hit` inches along
/// went through ([`Body::spans`]): the one holding the hit, else the nearest that goes in ahead of it within
/// [`BODY_MATCH`]; `None` when none does (a stretch that ends before the hit is not it).
pub fn way_through(
    body: &Body,
    from: [f32; 3],
    direction: [f32; 3],
    hit: f32,
) -> Option<(f32, f32)> {
    body.spans(from, direction)
        .into_iter()
        .filter(|s| s.1 > hit && s.0 - hit <= BODY_MATCH)
        .min_by(|a, b| a.0.max(hit).total_cmp(&b.0.max(hit)))
}

/// The round's rotating doors as bullets meet them (CS2's `prop_door_rotating`: models of their own, not in the
/// world's collision): each door model's collision in its own space, posed on each tick as the demo has the door
/// ([`Doors::crossings`]; de_nuke's door model has one hull and no bones, so its collision is in the model's space as
/// the world's is; a door's scale is not read: CS2's maps leave them unscaled). On the 13 demos checked no hit after a
/// wall came out of a door (the doors change no count of `examples/shot_penetration.rs`, `OFAST_DOORS=1`), but a
/// closed door in a bullet's way is in it.
#[derive(Debug, Clone, Default)]
pub struct Doors {
    /// Per door model (resource id): its collision as bullets see it, and its surface property hashes by index.
    models: HashMap<u64, (WorldCollision, Vec<u32>)>,
}

impl Doors {
    /// Reads the collision (`PHYS`) of every door model the doors of `track` have, found by resource id in `paths`; a
    /// model that cannot be read is left out with a warning (its door stops no bullet).
    pub fn load(
        content: &Content,
        track: &RoundTrack,
        paths: &HashMap<u64, String>,
        warnings: &mut Vec<String>,
    ) -> Self {
        let ids: BTreeSet<u64> = track
            .samples
            .iter()
            .flat_map(|s| &s.doors)
            .map(|d| d.model)
            .collect();
        let mut models = HashMap::new();
        for id in ids {
            let Some(path) = paths.get(&id) else {
                warnings.push(format!(
                    "door model {id:016x} is not a known model: bullets pass it"
                ));
                continue;
            };
            let physics = content
                .read_resource(path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    let resource = Resource::parse(&bytes).map_err(|e| e.to_string())?;
                    PhysicsData::from_resource(&resource).map_err(|e| e.to_string())
                });
            match physics {
                Ok(Some(physics)) => {
                    let collision =
                        WorldCollision::from_physics_where(&physics, |a| a.stops_bullets());
                    if collision.counts() == (0, 0) {
                        warnings.push(format!(
                            "door {path}: no collision stops bullets: bullets pass it"
                        ));
                    }
                    models.insert(id, (collision, physics.surfaces.clone()));
                }
                Ok(None) => warnings.push(format!("door {path}: no collision: bullets pass it")),
                Err(e) => warnings.push(format!("door {path}: {e}: bullets pass it")),
            }
        }
        Doors { models }
    }

    /// Per door model (resource id): how many hulls and triangles of its collision stop bullets.
    pub fn counts(&self) -> BTreeMap<u64, (usize, usize)> {
        self.models
            .iter()
            .map(|(&id, (collision, _))| (id, collision.counts()))
            .collect()
    }

    /// Where a line from `from` along the unit `direction` crosses the doors within `reach`, as they stand on the
    /// first sample of `track` at or after `tick`: each crossing, in the world, with its surface property's hash.
    pub fn crossings(
        &self,
        track: &RoundTrack,
        tick: u32,
        from: [f32; 3],
        direction: [f32; 3],
        reach: f32,
    ) -> Vec<(Crossing, u32)> {
        let Some(sample) = track
            .samples
            .get(track.samples.partition_point(|s| s.tick < tick))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for door in &sample.doors {
            let Some((collision, surfaces)) = self.models.get(&door.model) else {
                continue;
            };
            // The door's axes in the world (Source's angles: x forward, y left, z up), the line in its space.
            let [forward, right, up] = crate::demo::shots::angle_vectors(door.angles);
            let axes = [forward, right.map(|c| -c), up];
            let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            let offset: [f32; 3] = std::array::from_fn(|k| from[k] - door.position[k]);
            let local_from = axes.map(|axis| dot(offset, axis));
            let local_direction = axes.map(|axis| dot(direction, axis));
            for mut crossing in collision.ray_crossings(local_from, local_direction, reach) {
                let n = crossing.normal;
                crossing.normal = std::array::from_fn(|k| (0..3).map(|i| axes[i][k] * n[i]).sum());
                crossing.shape = DOOR_SHAPE;
                let hash = surfaces
                    .get(crossing.surface as usize)
                    .copied()
                    .unwrap_or(0);
                out.push((crossing, hash));
            }
        }
        out
    }
}

/// The points within `radius` of the segment `a`–`b`, in the world (a hitbox, [`Body`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    /// One end of the segment.
    pub a: [f32; 3],
    /// The other end.
    pub b: [f32; 3],
    /// The radius (inches).
    pub radius: f32,
}

/// A player's hitboxes on one tick as the exported agent shows them ([`super::players`]: the agent model's
/// `cstrike` set, [`crate::model::Hitbox`], on its posed bones): what a bullet goes through when it goes through
/// the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The player's controller.
    pub controller: u32,
    /// The tick.
    pub tick: u32,
    /// The hitboxes, in the world.
    pub hitboxes: Vec<Capsule>,
}

impl Body {
    /// The stretches of the line from `from` along the unit `direction` that lie inside the body: each hitbox's
    /// ([`capsule_span`]), those that overlap or touch merged, nearest first.
    pub fn spans(&self, from: [f32; 3], direction: [f32; 3]) -> Vec<(f32, f32)> {
        let mut spans: Vec<(f32, f32)> = self
            .hitboxes
            .iter()
            .filter_map(|c| capsule_span(c, from, direction))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for (start, end) in spans {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        merged
    }

    /// How far `point` lies from the nearest hitbox's surface (inches, inside or out); infinite without hitboxes.
    pub fn surface_distance(&self, point: [f32; 3]) -> f32 {
        self.hitboxes
            .iter()
            .map(|c| (segment_distance(point, c.a, c.b) - c.radius).abs())
            .fold(f32::INFINITY, f32::min)
    }
}

/// How well `bodies` (the players' hitboxes on the ticks of their hits, sorted by tick) fit the demo's hits on
/// `track`, as one line, choosing bodies as the bullets do ([`body_hit`], [`way_through`]): for each hit, how far its
/// point lies from the body's hitbox surface, and along its line how far the body's way in lies from the point; where
/// a later hit on the same line starts just past a player (where the bullet came out of them), how far the body's
/// way out lies from that start, against [`PLAYER_DEPTH`]'s.
pub fn hitbox_check(track: &RoundTrack, bodies: &[Body]) -> String {
    let (mut surfaces, mut ins, mut outs, mut depths) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let hits: Vec<&PlayerHit> = track
        .player_hits
        .iter()
        .filter(|h| h.start != h.point)
        .collect();
    for hit in &hits {
        let v: [f32; 3] = std::array::from_fn(|k| hit.point[k] - hit.start[k]);
        let at = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if at <= 0.0 {
            continue;
        }
        let direction = v.map(|c| c / at);
        let Some(body) = body_hit(bodies, hit.tick, hit.point, None) else {
            continue;
        };
        surfaces.push(body.surface_distance(hit.point));
        let Some((into, out)) = way_through(body, hit.start, direction, at) else {
            continue;
        };
        ins.push((into - at).abs());
        // A later hit on this line starting just past this one: where the bullet came out.
        let next = hits
            .iter()
            .filter(|h| h.tick == hit.tick)
            .filter_map(|h| {
                let w: [f32; 3] = std::array::from_fn(|k| h.start[k] - hit.start[k]);
                let t: f32 = (0..3).map(|k| w[k] * direction[k]).sum();
                let off = (0..3)
                    .map(|k| (w[k] - direction[k] * t).powi(2))
                    .sum::<f32>();
                (off.sqrt() < ON_THE_LINE && t > at && t < at + MAX_BODY).then_some(t)
            })
            .min_by(f32::total_cmp);
        if let Some(next) = next {
            outs.push((out - next).abs());
            depths.push((at + PLAYER_DEPTH - next).abs());
        }
    }
    let summary = |mut offs: Vec<f32>| {
        offs.sort_by(f32::total_cmp);
        let within = |d: f32| offs.iter().filter(|&&o| o <= d).count();
        format!(
            "{} within 1 in, {} within 3, median {:.1} in",
            within(1.0),
            within(3.0),
            offs.get(offs.len() / 2).copied().unwrap_or(f32::NAN)
        )
    };
    format!(
        "hitboxes: {} of {} hits on a posed body; the point against its surface: {}; its way in against the point \
         ({}): {}; {} bodies a bullet went through: its way out against the next hit's start: {} (8 inches deep: {})",
        surfaces.len(),
        hits.len(),
        summary(surfaces),
        ins.len(),
        summary(ins),
        outs.len(),
        summary(outs),
        summary(depths),
    )
}

/// The distance from `p` to the segment `a`–`b`.
fn segment_distance(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
    let ap: [f32; 3] = std::array::from_fn(|k| p[k] - a[k]);
    let length2: f32 = ab.iter().map(|c| c * c).sum();
    let t = if length2 > 0.0 {
        ((0..3).map(|k| ap[k] * ab[k]).sum::<f32>() / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (0..3)
        .map(|k| (ap[k] - ab[k] * t).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Where the line from `from` along the unit `direction` is inside `capsule`: the distances it goes in and comes out
/// at (either may be behind `from`); `None` when it misses. A capsule is convex, so the line's stretch inside it is
/// the union of its stretches in the two end spheres and the cylinder between them.
pub fn capsule_span(capsule: &Capsule, from: [f32; 3], direction: [f32; 3]) -> Option<(f32, f32)> {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let sub = |a: [f32; 3], b: [f32; 3]| -> [f32; 3] { std::array::from_fn(|k| a[k] - b[k]) };
    let r2 = capsule.radius * capsule.radius;
    let sphere = |center: [f32; 3]| {
        let v = sub(from, center);
        let b = dot(v, direction);
        let disc = b * b - (dot(v, v) - r2);
        (disc >= 0.0).then(|| (-b - disc.sqrt(), -b + disc.sqrt()))
    };
    let axis = sub(capsule.b, capsule.a);
    let length = dot(axis, axis).sqrt();
    let cylinder = (length > 1e-6)
        .then(|| {
            let u = axis.map(|c| c / length);
            let w = sub(from, capsule.a);
            let (wu, du) = (dot(w, u), dot(direction, u));
            let w_side = sub(w, u.map(|c| c * wu));
            let d_side = sub(direction, u.map(|c| c * du));
            let (qa, qb, qc) = (
                dot(d_side, d_side),
                2.0 * dot(w_side, d_side),
                dot(w_side, w_side) - r2,
            );
            // Within the infinite cylinder…
            let round = if qa < 1e-9 {
                (qc <= 0.0).then_some((f32::NEG_INFINITY, f32::INFINITY))
            } else {
                let disc = qb * qb - 4.0 * qa * qc;
                (disc >= 0.0).then(|| {
                    let s = disc.sqrt();
                    ((-qb - s) / (2.0 * qa), (-qb + s) / (2.0 * qa))
                })
            }?;
            // …and between its two end planes.
            let slab = if du.abs() < 1e-9 {
                (0.0..=length)
                    .contains(&wu)
                    .then_some((f32::NEG_INFINITY, f32::INFINITY))
            } else {
                let (t0, t1) = (-wu / du, (length - wu) / du);
                Some((t0.min(t1), t0.max(t1)))
            }?;
            let span = (round.0.max(slab.0), round.1.min(slab.1));
            (span.0 <= span.1).then_some(span)
        })
        .flatten();
    [sphere(capsule.a), sphere(capsule.b), cylinder]
        .into_iter()
        .flatten()
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
}

/// The damage a bullet of `weapon` still does `distance` inches from where it started, past `walls` (those before
/// `distance`); `None` when it stops in one ([`penetrate`]). It loses its range falloff on the way
/// ([`WeaponShots::range_modifier`] per 500 inches flown in the open: the inches inside walls cost by the wall's
/// rule only, which is what fits CS2's damage).
pub fn damage_at(weapon: &WeaponShots, walls: &[Wall], distance: f32) -> Option<f32> {
    let mut damage = weapon.damage;
    let mut at = 0.0;
    for wall in walls.iter().filter(|w| w.distance < distance) {
        damage *= weapon.range_modifier.powf((wall.distance - at) / 500.0);
        damage = penetrate(weapon, wall, damage)?;
        at = wall.distance + wall.thickness;
    }
    Some(damage * weapon.range_modifier.powf((distance - at).max(0.0) / 500.0))
}

/// The damage a bullet with `damage` left keeps after going through `wall`; `None` when it stops in it (less than 1
/// left, or the wall thicker than [`MAX_THROUGH`]). CS:GO's rule as its players have written it down, which CS2
/// still follows: with `m` the mean of the wall's two sides' distance modifiers ([`Through::distance`]; 3 when both
/// are wood or cardboard, `gamematerial` `W`, `U`; 2 both plastic, `L`), the bullet loses 16 % of its damage, `3.75 / penetration × 3 / m`
/// ([`WeaponShots::penetration`]) and `thickness² / (24 m)`. A player is a wall of flesh ([`PLAYER_DEPTH`]).
/// Checked on CS2's own record of the hits (`examples/shot_penetration.rs`: a hit after a wall starts where the
/// bullet came out of it, the damage is in `player_hurt`; the module docs give the counts). Through a player, flesh
/// (0.9) and the body's depth: a Glock bullet 8.1 inches through a head did the 18 damage the rule gives to the head
/// behind it (local match demo, tick 2825).
pub fn penetrate(weapon: &WeaponShots, wall: &Wall, damage: f32) -> Option<f32> {
    if !wall.thickness.is_finite() || wall.thickness > MAX_THROUGH {
        return None;
    }
    // The same material on both sides: wood and cardboard 3, plastic 2 (CS:GO's; wood checked on CS2's damage).
    let modifier = match (
        wall.enter.material.as_str(),
        wall.enter.material == wall.exit.material,
    ) {
        ("W" | "U", true) => 3.0,
        ("L", true) => 2.0,
        _ => (wall.enter.distance + wall.exit.distance) / 2.0,
    };
    let per_modifier = if modifier > 0.0 { 1.0 / modifier } else { 0.0 };
    let lost = damage * 0.16
        + (3.0 / weapon.penetration * 1.25).max(0.0) * per_modifier * 3.0
        + per_modifier * wall.thickness * wall.thickness / 24.0;
    let left = damage - lost.max(0.0);
    (left >= 1.0).then_some(left)
}

/// The thickest wall a bullet can go through (inches): CS:GO's search for a way out of a solid stops 90 units in.
pub const MAX_THROUGH: f32 = 90.0;

/// How many walls and players a bullet goes through at most (CS:GO's four; the demos checked never show more than
/// two: the damage runs out first).
pub const MAX_PENETRATIONS: usize = 4;

/// How far a bullet goes through a player's body (inches) when neither the demo shows where it came out nor the
/// player's posed hitboxes do ([`Body`]): about the middle of the bodies the checked demos show a bullet through (a
/// later hit on its line starts where it came out: 1.7 – 14.8 in over 42 bodies, an arm to a chest).
pub const PLAYER_DEPTH: f32 = 8.0;

/// How far past a player's hit a later hit's start may lie to be where the bullet came out of that player (inches;
/// [`PLAYER_DEPTH`]).
const MAX_BODY: f32 = 20.0;

/// The damage a hit of `damage` on hit group `hitgroup` (`player_hurt`'s: 1 head, 2 chest, 3 stomach, 4 / 5 arms,
/// 6 / 7 legs) does to a player with `armor`, and to the armour: the hit group's multiplier (the weapon's
/// [`WeaponShots::headshot_multiplier`], the stomach 1.25, the legs 0.75), then armour (not on the legs, nor on the
/// head without a `helmet`) lets [`WeaponShots::armor_ratio`] / 2 through and takes half the rest.
pub fn to_health(
    weapon: &WeaponShots,
    damage: f32,
    hitgroup: u32,
    armor: f32,
    helmet: bool,
) -> (f32, f32) {
    let damage = damage
        * match hitgroup {
            1 => weapon.headshot_multiplier,
            3 => 1.25,
            6 | 7 => 0.75,
            _ => 1.0,
        };
    let armoured = armor > 0.0 && !matches!(hitgroup, 6 | 7) && (hitgroup != 1 || helmet);
    if !armoured {
        return (damage.floor(), 0.0);
    }
    let health = damage * weapon.armor_ratio * 0.5;
    let taken = (damage - health) * 0.5;
    if taken > armor {
        return ((damage - armor * 2.0).floor(), armor.floor());
    }
    (health.floor(), taken.floor())
}

/// The player hits the demo sent, by tick (a shot's hits are among its tick's: [`hits_of`]). The other effects
/// dispatched there, which start where they end, are left out.
fn hits_by_tick(hits: &[PlayerHit]) -> HashMap<u32, Vec<&PlayerHit>> {
    let mut out: HashMap<u32, Vec<&PlayerHit>> = HashMap::new();
    for hit in hits.iter().filter(|h| h.start != h.point) {
        out.entry(hit.tick).or_default().push(hit);
    }
    out
}

/// The hits that may be `shot`'s among `by_tick` ([`hits_by_tick`]): its tick's. A hit starts at the shooter's eyes
/// ([`crate::demo::shots::hit_of`]) or, after a wall or a player, where the bullet came out of it; [`bullet_json`] keeps a bullet's own,
/// those whose start and point both lie on its line.
fn hits_of<'a>(shot: &Shot, by_tick: &HashMap<u32, Vec<&'a PlayerHit>>) -> Vec<&'a PlayerHit> {
    by_tick.get(&shot.tick).cloned().unwrap_or_default()
}

/// How far `shot`'s bullets are traced: its weapon's range ([`WeaponShots::range`]), else [`REACH`].
fn range_of(shot: &Shot, data: &ShotData) -> f32 {
    data.weapon(shot.item)
        .map(|w| w.range)
        .filter(|r| *r > 0.0)
        .unwrap_or(REACH)
}

/// How near (inches) a bullet's line must pass a player hit the demo sent to be that hit's bullet: the effect lies
/// on the line of the bullet OFAST rebuilds (to a thousandth of an inch on all 302 hits of the de_nuke pro demo and
/// 423 of the local match demo's 424; `examples/shot_traces.rs`), so only rounding is allowed for.
const ON_THE_LINE: f32 = 0.5;

/// Whether a shot draws a tracer: every shot of a weapon whose tracer frequency for its mode is not 0
/// ([`WeaponShots::tracer_frequency`]: 0 for the silenced M4A1-S and USP-S, the MP5-SD, and what is not a gun).
/// CS2 does not space them as the frequency (3 on the AK-47, the M4s, most SMGs) says: on the Aurora vs FURIA de_nuke
/// demo, YEKINDAR's view showed a tracer leave his AK on each of four shots in a row (round 5, ticks 37683 – 37702),
/// and soulfly's view one from molodoy's AK on shots that left 28, 26 and 25 rounds in the magazine (40813 – 40833),
/// where one in three counted on the magazine would draw none.
pub fn draws_tracer(frequency: u32) -> bool {
    frequency > 0
}

/// Where a shooter sees their own tracer leave (Source axes, inches, in the world): the viewmodel's muzzle
/// `view_muzzle` (the viewmodel rig's space: x forward, y left, z up) placed on the camera at `eye` looking along
/// `pitch` and `yaw` as the player's `settings` place the viewmodel (mirrored for the left hand, moved by the
/// offsets), then moved to the world point that shows at the same place on the screen: the viewmodel is drawn at its
/// own field of view, so its sideways and up offsets are scaled by tan(world FOV / 2) / tan(viewmodel FOV / 2), both
/// at 4:3 (CS2's world FOV 90). This is what Source's `FormatViewModelAttachment` does for first-person effects
/// (Source SDK 2013), and what Unreal's first-person rendering shows (its field of view on the viewmodel, the camera's
/// on the world).
pub fn view_tracer_start(
    view_muzzle: [f32; 3],
    settings: &ViewmodelSettings,
    eye: [f32; 3],
    pitch: f32,
    yaw: f32,
) -> [f32; 3] {
    let [right_offset, forward_offset, up_offset] = settings.offset;
    // The left hand mirrors the viewmodel, its sideways offset with it (import_demo.py's key_viewmodel_hand).
    let side = if settings.left_handed { -1.0 } else { 1.0 };
    let squash = 1.0 / (settings.fov.to_radians() * 0.5).tan().max(1e-3);
    let forward = view_muzzle[0] + forward_offset;
    let left = side * (view_muzzle[1] - right_offset) * squash;
    let up = (view_muzzle[2] + up_offset) * squash;
    let [ahead, right, upward] = crate::demo::shots::angle_vectors([pitch, yaw, 0.0]);
    std::array::from_fn(|k| eye[k] + ahead[k] * forward - right[k] * left + upward[k] * up)
}

/// The shots of `track` as the manifest's `shots` (module docs), and the bullet holes' decal groups they use (for
/// [`export_decals`]). `collision` is the map's collision as bullets see it (`None`: every bullet that met no player
/// ends in the air) and `physics` the world physics it was built from (the surfaces), `doors` the round's doors'
/// collision ([`Doors`]), `bodies` the players' hitboxes on the ticks of their hits (sorted by tick), `muzzles` and
/// `view_muzzles` the barrels the agents' and the viewmodels' exports found, `data` CS2's weapons and surfaces.
#[allow(clippy::too_many_arguments)]
pub fn shots_json(
    track: &RoundTrack,
    collision: Option<&WorldCollision>,
    physics: Option<&PhysicsData>,
    doors: Option<&Doors>,
    bodies: &[Body],
    muzzles: &[Muzzle],
    view_muzzles: &[Muzzle],
    data: &ShotData,
) -> (Vec<Json>, BTreeSet<String>) {
    let obstacles = Obstacles {
        world: collision,
        // The world's surface property hashes, by index (RayHit::surface).
        surfaces: physics.map_or(&[], |p| p.surfaces.as_slice()),
        doors: doors.map(|d| (d, track)),
        bodies,
    };
    let by_tick = hits_by_tick(&track.player_hits);
    // The offset from more shots than the round's (a round with few shots); the round's come first.
    let times = shot_times(
        track
            .shots
            .iter()
            .map(|(_, s)| s)
            .chain(&track.earlier_shots),
    );
    let mut decals = BTreeSet::new();
    let shots = track
        .shots
        .iter()
        .zip(times)
        .map(|((controller, shot), time)| {
            let hits = hits_of(shot, &by_tick);
            let weapon = data.weapon(shot.item);
            let bullets: Vec<Json> = (0..weapon.map_or(1, |w| w.bullets.max(1)))
                .map(|bullet| {
                    let json =
                        bullet_json(shot, bullet, Some(*controller), &hits, &obstacles, data);
                    for group in world_hits(&json).filter_map(|at| at["decal"].as_str()) {
                        decals.insert(group.to_owned());
                    }
                    json
                })
                .collect();
            let of_shot = |m: &&Muzzle| m.controller == *controller && m.tick == shot.tick;
            let muzzle = muzzles.iter().find(of_shot);
            let view_muzzle = view_muzzles.iter().find(of_shot);
            let mode = (shot.mode as usize).min(1);
            let tracer = weapon
                .filter(|w| draws_tracer(w.tracer_frequency[mode]))
                .and_then(|w| w.tracer.clone());
            // Where the shooter's own view shows the tracer leave: their viewmodel's muzzle, from the camera of
            // the shot's tick (the one Unreal's POV camera has there).
            let view_tracer = view_muzzle.and_then(|m| {
                let at = track.samples.partition_point(|s| s.tick < shot.tick);
                let player = track
                    .samples
                    .get(at)?
                    .players
                    .iter()
                    .find(|p| p.controller == *controller)?;
                let eye = std::array::from_fn(|k| player.position[k] + player.view_offset[k]);
                Some(view_tracer_start(
                    m.position,
                    &player.viewmodel,
                    eye,
                    player.pitch,
                    player.yaw,
                ))
            });
            json!({
                "tick": shot.tick,
                "time": (time * 1000.0).round() / 1000.0,
                "controller": controller,
                "item": shot.item,
                "weapon": data.items.get(&shot.item),
                "mode": shot.mode,
                "silenced": weapon.is_some_and(|w| w.silencer) && shot.mode == 1,
                "tracer": tracer,
                "eye": rounded(shot.eye),
                "muzzle": muzzle.map(|m| rounded(m.position)),
                "muzzle_direction": muzzle.map(|m| unit(m.direction)),
                "view_muzzle": view_muzzle.map(|m| rounded(m.position)),
                "view_muzzle_direction": view_muzzle.map(|m| unit(m.direction)),
                "view_tracer": view_tracer.map(rounded),
                "bullets": bullets,
            })
        })
        .collect();
    (shots, decals)
}

/// The bullet holes the shots before the round left (`track.earlier_shots`): CS2's clients keep them from round to
/// round (a hole on de_nuke's A site in round 5 is an AWP bullet's from round 3, tick 19909, where OFAST's trace ends
/// within 5 units of it). As the manifest's `earlier_holes`: `tick`, `end`, `normal` and `decal` of every place a
/// bullet met the world where its surface keeps a hole (both sides of a wall it went through); and the decal groups
/// they use. Arguments as [`shots_json`].
pub fn earlier_holes_json(
    track: &RoundTrack,
    collision: Option<&WorldCollision>,
    physics: Option<&PhysicsData>,
    data: &ShotData,
) -> (Vec<Json>, BTreeSet<String>) {
    // No doors: the earlier rounds' doors are not in this round's samples (a hole there would be on a door anyway).
    let obstacles = Obstacles {
        world: collision,
        surfaces: physics.map_or(&[], |p| p.surfaces.as_slice()),
        doors: None,
        bodies: &[],
    };
    let by_tick = hits_by_tick(&track.earlier_hits);
    let mut decals = BTreeSet::new();
    let mut holes = Vec::new();
    for shot in &track.earlier_shots {
        let hits = hits_of(shot, &by_tick);
        for bullet in 0..data.weapon(shot.item).map_or(1, |w| w.bullets.max(1)) {
            let json = bullet_json(shot, bullet, None, &hits, &obstacles, data);
            // Where it met the world: the walls it went through, both sides, and its end.
            for at in world_hits(&json) {
                let Some(group) = at["decal"].as_str() else {
                    continue;
                };
                decals.insert(group.to_owned());
                holes.push(json!({
                    "tick": shot.tick,
                    "end": at["end"],
                    "normal": at["normal"],
                    "decal": group,
                    "thickness": at["thickness"],
                }));
            }
        }
    }
    (holes, decals)
}

/// Where a bullet's JSON ([`bullet_json`]) met the world: each wall's way in and out it went through, and where it
/// stopped when that is the world.
fn world_hits(bullet: &Json) -> impl Iterator<Item = &Json> {
    bullet["through"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(std::iter::once(bullet))
        .filter(|at| at["on"] == "world")
}

/// One bullet of `shot` (module docs; `shooter` its shooter's controller, `None` for the earlier rounds' shots),
/// walked from the eyes along its line within the weapon's range: through the walls `obstacles` has on it (the
/// world's, [`walls_on`], and the doors') and the players among `hits` (the demo's, whose start and point both lie on
/// its line), each costing it damage ([`penetrate`]), until one stops it, or into the air. The demo's hits are the
/// truth: a wall or player before one it shows is gone through (the rule giving less than 1 damage left, a door in
/// the way OFAST cannot see), a wall traced between a hit's start and its point is not there for CS2, and a later hit
/// starting just past a player is where the bullet came out of them; else it comes out of the player's hitboxes
/// where `obstacles`' bodies have them ([`body_hit`], [`way_through`]), which also name the player hit (`victim`).
fn bullet_json(
    shot: &Shot,
    bullet: u32,
    shooter: Option<u32>,
    hits: &[&PlayerHit],
    obstacles: &Obstacles<'_>,
    data: &ShotData,
) -> Json {
    let direction = bullet_direction(
        shot.angles,
        spread_offset(shot.seed, bullet, shot.inaccuracy, shot.spread),
    );
    let range = range_of(shot, data);
    let weapon = data.weapon(shot.item);
    let along = |point: [f32; 3]| -> (f32, f32) {
        let v: [f32; 3] = std::array::from_fn(|k| point[k] - shot.eye[k]);
        let t = v[0] * direction[0] + v[1] * direction[1] + v[2] * direction[2];
        let off: [f32; 3] = std::array::from_fn(|k| v[k] - direction[k] * t);
        (
            t,
            (off[0] * off[0] + off[1] * off[1] + off[2] * off[2]).sqrt(),
        )
    };
    let point = |t: f32| -> [f32; 3] { std::array::from_fn(|k| shot.eye[k] + direction[k] * t) };
    // This bullet's hits: (the point's distance along the line, the start's), nearest first; one dispatched twice is
    // one.
    let mut players: Vec<(f32, f32)> = hits
        .iter()
        .filter_map(|h| {
            let ((start, start_off), (at, off)) = (along(h.start), along(h.point));
            (start_off < ON_THE_LINE && off < ON_THE_LINE && start > -ON_THE_LINE && at > start)
                .then_some((at, start))
        })
        .collect();
    players.sort_by(|a, b| a.0.total_cmp(&b.0));
    players.dedup_by(|later, kept| later.0 - kept.0 < ON_THE_LINE);
    let shown = players.last().map_or(0.0, |p| p.0);
    let walls: Vec<Wall> = obstacles
        .walls(shot.tick, shot.eye, direction, range, data)
        .into_iter()
        .filter(|w| {
            !players
                .iter()
                .any(|&(at, start)| w.distance > start + ON_THE_LINE && w.distance < at)
        })
        .collect();
    // What a bullet met on the world: where, the surface's normal, its name, impact and bullet hole.
    let on_world = |t: f32, normal: [f32; 3], hash: u32| {
        let (surface, impact) = data.impact(hash);
        json!({
            "on": "world",
            "end": rounded(point(t)),
            "normal": unit(normal),
            "surface": surface,
            "impact": impact.map(|i| i.effect.as_str()),
            "decal": impact.and_then(|i| i.decal.as_deref()),
        })
    };
    let stopped = |mut end: Json, through: Vec<Json>| {
        for key in ["normal", "surface", "impact", "decal"] {
            if end.get(key).is_none() {
                end[key] = Json::Null;
            }
        }
        end["through"] = Json::Array(through);
        end
    };
    let flesh = data.through(crate::physics::surface_hash("flesh"));
    let falloff = |damage: f32, from: f32, to: f32| {
        weapon.map_or(damage, |w| {
            damage * w.range_modifier.powf((to - from).max(0.0) / 500.0)
        })
    };
    let mut through = Vec::new();
    let mut damage = weapon.map_or(0.0, |w| w.damage);
    let (mut at, mut gone) = (0.0f32, 0usize);
    let (mut next_wall, mut next_player) = (0, 0);
    loop {
        let player = players.get(next_player);
        match walls.get(next_wall) {
            Some(wall) if player.is_none_or(|p| wall.distance < p.0) => {
                next_wall += 1;
                damage = falloff(damage, at, wall.distance);
                // How thick the wall is (`null`: no way out found), for a bullet hole not to show through it.
                let thickness = if wall.thickness.is_finite() {
                    json!((f64::from(wall.thickness) * 100.0).round() / 100.0)
                } else {
                    Json::Null
                };
                let mut entry = on_world(wall.distance, wall.enter_normal, wall.enter_surface);
                entry["thickness"] = thickness.clone();
                // On a door: no hole (it would stay where the door was).
                let no_hole = |item: &mut Json, door: bool| {
                    if door {
                        item["decal"] = Json::Null;
                        item["door"] = json!(true);
                    }
                };
                no_hole(&mut entry, wall.door_in);
                let left = weapon
                    .filter(|_| gone < MAX_PENETRATIONS)
                    .and_then(|w| penetrate(w, wall, damage));
                if left.is_none() && shown <= wall.distance {
                    return stopped(entry, through);
                }
                damage = left.unwrap_or(1.0);
                through.push(entry);
                at = wall.distance;
                if wall.thickness.is_finite() {
                    at += wall.thickness;
                    let mut exit = on_world(at, wall.exit_normal, wall.exit_surface);
                    exit["thickness"] = thickness;
                    no_hole(&mut exit, wall.door_out);
                    through.push(exit);
                }
                gone += 1;
            }
            _ => {
                let Some(&(hit, _)) = player else {
                    let end = json!({ "on": "air", "end": rounded(point(range)) });
                    return stopped(end, through);
                };
                next_player += 1;
                damage = falloff(damage, at, hit);
                // The player hit: the body nearest the hit's point, when the export posed one there.
                let victim = body_hit(obstacles.bodies, shot.tick, point(hit), shooter);
                let end = json!({
                    "on": "player",
                    "end": rounded(point(hit)),
                    "victim": victim.map(|b| b.controller),
                });
                // Where it came out: a later hit's start just past (not past the next wall: that start is the
                // wall's way out), else where the line leaves the player's hitboxes, else a body's depth on; never
                // beyond the next wall's way in, which is the wall's to charge.
                let wall_ahead = walls.get(next_wall).map_or(f32::INFINITY, |w| w.distance);
                let exit = players
                    .iter()
                    .map(|p| p.1)
                    .filter(|&start| start > hit && start < hit + MAX_BODY && start <= wall_ahead)
                    .min_by(f32::total_cmp)
                    .or_else(|| {
                        victim
                            .and_then(|b| way_through(b, shot.eye, direction, hit))
                            .map(|(_, out)| out)
                    })
                    .unwrap_or(hit + PLAYER_DEPTH)
                    .min(wall_ahead)
                    .max(hit);
                let body = Wall {
                    distance: hit,
                    thickness: exit - hit,
                    enter_normal: [0.0; 3],
                    enter_surface: 0,
                    exit_normal: [0.0; 3],
                    exit_surface: 0,
                    enter: flesh.clone(),
                    exit: flesh.clone(),
                    door_in: false,
                    door_out: false,
                };
                let left = weapon
                    .filter(|_| gone < MAX_PENETRATIONS)
                    .and_then(|w| penetrate(w, &body, damage));
                if left.is_none() && shown <= hit {
                    return stopped(end, through);
                }
                damage = left.unwrap_or(1.0);
                through.push(end);
                at = exit;
                gone += 1;
            }
        }
    }
}

/// A unit vector rounded to four decimals.
fn unit(v: [f32; 3]) -> [f64; 3] {
    v.map(|c| (f64::from(c) * 1e4).round() / 1e4)
}

/// A position rounded to a hundredth of an inch.
fn rounded(p: [f32; 3]) -> [f64; 3] {
    p.map(|c| (f64::from(c) * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::surface_hash;

    fn kv3(text: &str) -> Value {
        crate::kv3::text::parse(text).expect("KV3 text parses").root
    }

    fn data() -> ShotData {
        let weapons = kv3(r#"{
                weapon_base = { m_nNumBullets = 1 m_nTracerFrequency = 0 m_eSilencerType = "WEAPONSILENCER_NONE" }
                rifle = { _base = "weapon_base" m_nTracerFrequency = 3
                    m_szTracerParticle = resource_name:"particles/weapons/cs_weapon_fx/weapon_tracers_assrifle.vpcf" }
                weapon_ak47 = { _base = "rifle" }
                weapon_m4a1_silencer = { _base = "rifle" m_nTracerFrequency = [ 3, 0 ]
                    m_eSilencerType = "WEAPONSILENCER_DETACHABLE" }
                weapon_nova = { _base = "weapon_base" m_nNumBullets = 9 m_nTracerFrequency = 1 m_flRange = 3000.0
                    m_szTracerParticle = resource_name:"particles/weapons/cs_weapon_fx/weapon_tracers_shot.vpcf" }
            }"#);
        let surface = |name: &str, base: &str| {
            let base = if base.is_empty() {
                0
            } else {
                surface_hash(base)
            };
            format!(
                r#"{{ surfacePropertyName = "{name}" m_nameHash = {} m_baseNameHash = {base} }}"#,
                surface_hash(name)
            )
        };
        let surfaces = kv3(&format!(
            "{{ SurfacePropertiesList = [ {}, {}, {}, {}, {}, {} ] }}",
            surface("default", ""),
            surface("solidmetal", "default"),
            surface("metal", "solidmetal"),
            surface("Metal_Box", "metal"),
            surface("dirt", "default"),
            surface("sand", "dirt"),
        ));
        let impacts = kv3(r#"{ SurfacePropertiesList = [
                { surfacePropertyName = "default" effect = resource:"particles/impact_fx/impact_concrete.vpcf"
                  impactDecalName = "Impact.Concrete" },
                { surfacePropertyName = "solidmetal" effect = resource:"particles/impact_fx/impact_metal.vpcf"
                  impactDecalName = "Impact.Metal" },
                { surfacePropertyName = "dirt" effect = resource:"particles/impact_fx/impact_dirt.vpcf"
                  impactDecalName = "Impact.Dirt" },
            ] }"#);
        ShotData {
            weapons: weapons_of(&weapons),
            items: [
                (7, "weapon_ak47"),
                (60, "weapon_m4a1_silencer"),
                (35, "weapon_nova"),
            ]
            .into_iter()
            .map(|(i, n)| (i, n.to_owned()))
            .collect(),
            surfaces: surfaces_of(&surfaces),
            impacts: impacts_of(&impacts),
            game: HashMap::new(),
        }
    }

    #[test]
    fn weapons_inherit_from_their_bases() {
        let data = data();
        let ak = data.weapon(7).expect("AK-47");
        assert_eq!(ak.bullets, 1);
        assert_eq!(ak.tracer_frequency, [3, 3]);
        assert_eq!(ak.tracer.as_deref(), Some("weapon_tracers_assrifle"));
        assert!(!ak.silencer);
        let m4 = data.weapon(60).expect("M4A1-S");
        assert_eq!(m4.tracer_frequency, [3, 0]);
        assert!(m4.silencer);
        let nova = data.weapon(35).expect("Nova");
        assert_eq!((nova.bullets, nova.tracer_frequency), (9, [1, 1]));
        assert_eq!(nova.tracer.as_deref(), Some("weapon_tracers_shot"));
        // Entries other than weapons (bases) are not weapons.
        assert!(!data.weapons.contains_key("rifle"));
    }

    #[test]
    fn surfaces_take_the_impact_up_their_base_chain() {
        let data = data();
        let effect = |name: &str| {
            let (surface, impact) = data.impact(surface_hash(name));
            (surface.map(str::to_owned), impact.map(|i| i.effect.clone()))
        };
        assert_eq!(
            effect("Metal_Box"),
            (Some("Metal_Box".into()), Some("impact_metal".into()))
        );
        assert_eq!(
            effect("sand"),
            (Some("sand".into()), Some("impact_dirt".into()))
        );
        // A surface the list does not know: concrete's, with no name.
        assert_eq!(
            effect("unobtainium"),
            (None, Some("impact_concrete".into()))
        );
        assert_eq!(
            data.impact(surface_hash("metal"))
                .1
                .and_then(|i| i.decal.clone()),
            Some("Impact.Metal".into())
        );
    }

    #[test]
    fn tracers_follow_the_frequency() {
        // Every shot, whatever the frequency; none at frequency 0.
        assert!(draws_tracer(3) && draws_tracer(1));
        assert!(!draws_tracer(0));
    }

    fn shot(tick: u32, item: u32, mode: u32, eye: [f32; 3], bullets_left: u32) -> Shot {
        Shot {
            tick,
            pawn: 1,
            item,
            mode,
            eye,
            angles: [0.0, 0.0, 0.0],
            seed: 1,
            inaccuracy: 0.0,
            spread: 0.0,
            recoil_index: 0.0,
            bullets_left,
            server_tick: None,
            attack: None,
        }
    }

    #[test]
    fn bullets_end_on_walls_and_players() {
        let data = data();
        let mut collision = WorldCollision::default();
        // A wall 500 inches ahead (along +x, the shots' aim).
        collision.add_box([500.0, -100.0, -100.0], [510.0, 100.0, 100.0]);
        let track = RoundTrack {
            round: 1,
            start_tick: 0,
            end_tick: 100,
            tick_interval: 1.0 / 64.0,
            map_name: "de_test".into(),
            samples: Vec::new(),
            events: Vec::new(),
            smoke_logs: Vec::new(),
            shots: vec![
                (2, shot(10, 7, 0, [0.0, 0.0, 0.0], 27)),
                (2, shot(20, 7, 0, [0.0, 50.0, 0.0], 26)),
                (3, shot(30, 60, 1, [0.0, 0.0, 0.0], 19)),
                (3, shot(40, 7, 0, [0.0, 0.0, 0.0], 25)),
                (3, shot(50, 35, 0, [0.0, 0.0, 0.0], 7)),
            ],
            // The second shot met a player 200 inches out, before the wall; the fourth one 600 out, through it (its
            // hit starts where it came out of the wall).
            player_hits: vec![
                PlayerHit {
                    tick: 20,
                    start: [0.0, 50.0, 0.0],
                    point: [200.0, 50.0, 0.0],
                },
                PlayerHit {
                    tick: 40,
                    start: [510.0, 0.0, 0.0],
                    point: [600.0, 0.0, 0.0],
                },
            ],
            // Before the round: one shot into the wall, one into a player (no hole).
            earlier_shots: vec![
                shot(1, 7, 0, [0.0, -20.0, 0.0], 10),
                shot(2, 7, 0, [0.0, 20.0, 0.0], 9),
            ],
            earlier_hits: vec![PlayerHit {
                tick: 2,
                start: [0.0, 20.0, 0.0],
                point: [100.0, 20.0, 0.0],
            }],
            server_tick_offset: None,
        };
        let muzzles = [Muzzle {
            controller: 2,
            tick: 10,
            position: [30.0, -5.0, -5.0],
            direction: [1.0, 0.0, 0.0],
            eject: [None; 4],
        }];
        let view_muzzles = [Muzzle {
            controller: 2,
            tick: 10,
            position: [28.0, -6.0, -4.0],
            direction: [0.0, 0.999_999_9, 0.0],
            eject: [None; 4],
        }];
        let (shots, decals) = shots_json(
            &track,
            Some(&collision),
            None,
            None,
            &[],
            &muzzles,
            &view_muzzles,
            &data,
        );
        assert_eq!(shots.len(), 5);
        // On the wall: its normal faces the shooter; no physics data, so the default surface's impact.
        let wall = &shots[0]["bullets"][0];
        assert_eq!(wall["on"], "world");
        assert_eq!(wall["end"], json!([500.0, 0.0, 0.0]));
        assert_eq!(wall["normal"], json!([-1.0, 0.0, 0.0]));
        assert_eq!(wall["impact"], "impact_concrete");
        assert_eq!(wall["decal"], "Impact.Concrete");
        assert_eq!(shots[0]["muzzle"], json!([30.0, -5.0, -5.0]));
        // In first person too, its direction rounded.
        assert_eq!(shots[0]["view_muzzle"], json!([28.0, -6.0, -4.0]));
        assert_eq!(shots[0]["view_muzzle_direction"], json!([0.0, 1.0, 0.0]));
        assert_eq!(shots[0]["tracer"], "weapon_tracers_assrifle");
        assert_eq!(shots[0]["weapon"], "weapon_ak47");
        // On the player, before the wall; a tracer like every shot; no muzzle known.
        let hit = &shots[1]["bullets"][0];
        assert_eq!(
            (hit["on"].as_str(), hit["end"].clone()),
            (Some("player"), json!([200.0, 50.0, 0.0]))
        );
        assert_eq!(shots[1]["tracer"], "weapon_tracers_assrifle");
        assert!(shots[1]["muzzle"].is_null());
        assert!(shots[1]["view_muzzle"].is_null());
        // The silenced M4A1-S: silenced, and no tracer in that mode.
        assert_eq!(shots[2]["silenced"], true);
        assert!(shots[2]["tracer"].is_null());
        // Through the wall into the player behind it (the demo shows it there, though the test's AK does no
        // damage): blood there, the wall's way in and out on the way.
        let through = &shots[3]["bullets"][0];
        assert_eq!(through["on"], "player");
        assert_eq!(through["end"], json!([600.0, 0.0, 0.0]));
        assert_eq!(through["through"][0]["end"], json!([500.0, 0.0, 0.0]));
        assert_eq!(through["through"][0]["normal"], json!([-1.0, 0.0, 0.0]));
        assert_eq!(through["through"][1]["end"], json!([510.0, 0.0, 0.0]));
        assert_eq!(through["through"][1]["normal"], json!([1.0, 0.0, 0.0]));
        assert_eq!(through["through"][1]["decal"], "Impact.Concrete");
        assert!(wall["through"].as_array().is_some_and(Vec::is_empty));
        // The Nova: 9 pellets, each to the wall.
        assert_eq!(shots[4]["bullets"].as_array().map(Vec::len), Some(9));
        assert_eq!(decals, BTreeSet::from(["Impact.Concrete".to_owned()]));
        // Without collision every bullet that met no player flies on to its weapon's range.
        let (open, _) = shots_json(&track, None, None, None, &[], &[], &[], &data);
        assert_eq!(open[0]["bullets"][0]["on"], "air");
        assert_eq!(
            open[0]["bullets"][0]["end"],
            json!([REACH as f64, 0.0, 0.0])
        );
        let nova_end = open[4]["bullets"][0]["end"][0].as_f64().unwrap_or_default();
        assert!((nova_end - 3000.0).abs() < 1.0, "{nova_end}");
        // The earlier shots leave the one hole on the wall.
        let (holes, groups) = earlier_holes_json(&track, Some(&collision), None, &data);
        assert_eq!(holes.len(), 1);
        assert_eq!(holes[0]["tick"], 1);
        assert_eq!(holes[0]["end"], json!([500.0, -20.0, 0.0]));
        assert_eq!(holes[0]["decal"], "Impact.Concrete");
        assert_eq!(groups.len(), 1);
    }

    /// A rifle as CS2's AK-47: 36 damage, penetration 2, 0.98 kept per 500 units.
    fn rifle() -> WeaponShots {
        WeaponShots {
            bullets: 1,
            tracer_frequency: [0, 0],
            tracer: None,
            silencer: false,
            range: REACH,
            damage: 36.0,
            penetration: 2.0,
            range_modifier: 0.98,
            headshot_multiplier: 4.0,
            armor_ratio: 1.55,
            model: None,
            recoil: RecoilKeys::default(),
        }
    }

    fn concrete(distance: f32, thickness: f32) -> Wall {
        let side = Through {
            distance: 0.5,
            damage: 0.5,
            material: "C".into(),
        };
        Wall {
            distance,
            thickness,
            enter_normal: [-1.0, 0.0, 0.0],
            enter_surface: 0,
            exit_normal: [1.0, 0.0, 0.0],
            exit_surface: 0,
            enter: side.clone(),
            exit: side,
            door_in: false,
            door_out: false,
        }
    }

    #[test]
    fn own_tracers_leave_where_the_viewmodel_shows_its_muzzle() {
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3);
        let muzzle = [30.0, -5.0, -3.0];
        let settings = |fov: f32, offset: [f32; 3], left_handed: bool| ViewmodelSettings {
            fov,
            offset,
            left_handed,
        };
        // At the world's own FOV and no offsets: the muzzle as it is, 5 right and 3 down.
        let plain = view_tracer_start(muzzle, &settings(90.0, [0.0; 3], false), [0.0; 3], 0.0, 0.0);
        assert!(near(plain, [30.0, -5.0, -3.0]), "{plain:?}");
        // A narrower viewmodel FOV draws it nearer the middle: the world point is further out (1 / tan 30°).
        let narrow =
            view_tracer_start(muzzle, &settings(60.0, [0.0; 3], false), [0.0; 3], 0.0, 0.0);
        let s = 1.0 / 30f32.to_radians().tan();
        assert!(near(narrow, [30.0, -5.0 * s, -3.0 * s]), "{narrow:?}");
        // Offsets (right, forward, up) move it; the left hand mirrors it and its sideways offset.
        let moved = view_tracer_start(
            muzzle,
            &settings(90.0, [1.0, 2.0, -1.0], false),
            [0.0; 3],
            0.0,
            0.0,
        );
        assert!(near(moved, [32.0, -6.0, -4.0]), "{moved:?}");
        let left = view_tracer_start(
            muzzle,
            &settings(90.0, [1.0, 2.0, -1.0], true),
            [0.0; 3],
            0.0,
            0.0,
        );
        assert!(near(left, [32.0, 6.0, -4.0]), "{left:?}");
        // On the camera: looking along +y from (10, 20, 64), its right is +x.
        let turned = view_tracer_start(
            muzzle,
            &settings(90.0, [0.0; 3], false),
            [10.0, 20.0, 64.0],
            0.0,
            90.0,
        );
        assert!(near(turned, [15.0, 50.0, 61.0]), "{turned:?}");
    }

    #[test]
    fn lines_go_through_capsules_and_bodies() {
        let close = |a: Option<(f32, f32)>, b: (f32, f32)| {
            a.is_some_and(|a| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3)
        };
        // Upright, 10 inches between its ends' centres, 3 round.
        let upright = Capsule {
            a: [0.0, 0.0, 0.0],
            b: [0.0, 0.0, 10.0],
            radius: 3.0,
        };
        // Across its middle: through the cylinder, 6 inches.
        let across = capsule_span(&upright, [-10.0, 0.0, 5.0], [1.0, 0.0, 0.0]);
        assert!(close(across, (7.0, 13.0)), "{across:?}");
        // Down its axis: from the top of the upper sphere to the bottom of the lower one.
        let down = capsule_span(&upright, [0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        assert!(close(down, (7.0, 23.0)), "{down:?}");
        // Across the upper sphere above the segment's end, 2 off its centre: 2·√5 inches.
        let cap = capsule_span(&upright, [-10.0, 0.0, 12.0], [1.0, 0.0, 0.0]);
        let half = 5.0f32.sqrt();
        assert!(close(cap, (10.0 - half, 10.0 + half)), "{cap:?}");
        // Beside it, nothing.
        assert_eq!(
            capsule_span(&upright, [-10.0, 4.0, 5.0], [1.0, 0.0, 0.0]),
            None
        );
        // A body: two capsules that overlap make one stretch, a third apart another.
        let body = Body {
            controller: 1,
            tick: 0,
            hitboxes: vec![
                upright,
                Capsule {
                    a: [4.0, 0.0, 0.0],
                    b: [4.0, 0.0, 10.0],
                    radius: 3.0,
                },
                Capsule {
                    a: [20.0, 0.0, 0.0],
                    b: [20.0, 0.0, 10.0],
                    radius: 2.0,
                },
            ],
        };
        let spans = body.spans([-10.0, 0.0, 5.0], [1.0, 0.0, 0.0]);
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!(close(Some(spans[0]), (7.0, 17.0)) && close(Some(spans[1]), (28.0, 32.0)));
    }

    #[test]
    fn doors_stand_where_the_demo_has_them() {
        use crate::demo::players::{DoorSample, TickSample};
        // A door 4 inches thick and 48 wide in its own space (x 0 – 48 from the hinge, y 0 – 4), swung 90 degrees
        // about its hinge at (100, 0, 0): it then spans x 96 – 100 across the line y = 20 along +x.
        let mut leaf = WorldCollision::default();
        leaf.add_box([0.0, 0.0, 0.0], [48.0, 4.0, 96.0]);
        let doors = Doors {
            models: HashMap::from([(7, (leaf, Vec::new()))]),
        };
        let door = |yaw: f32| DoorSample {
            entity: 1,
            model: 7,
            closed_position: [100.0, 0.0, 0.0],
            closed_angles: [0.0; 3],
            position: [100.0, 0.0, 0.0],
            angles: [0.0, yaw, 0.0],
            state: 0,
        };
        let sample = |tick: u32, yaw: f32| TickSample {
            tick,
            players: Vec::new(),
            dropped: Vec::new(),
            doors: vec![door(yaw)],
            props: Vec::new(),
            grenades: Vec::new(),
            fires: Vec::new(),
        };
        let track = RoundTrack {
            round: 1,
            start_tick: 0,
            end_tick: 100,
            tick_interval: 1.0 / 64.0,
            map_name: "de_test".into(),
            samples: vec![sample(10, 0.0), sample(20, 90.0)],
            events: Vec::new(),
            smoke_logs: Vec::new(),
            shots: Vec::new(),
            player_hits: Vec::new(),
            earlier_shots: Vec::new(),
            earlier_hits: Vec::new(),
            server_tick_offset: None,
        };
        let line =
            |tick: u32| doors.crossings(&track, tick, [0.0, 20.0, 10.0], [1.0, 0.0, 0.0], REACH);
        // Closed (along x from the hinge, y 0 – 4): the line at y = 20 passes it.
        assert!(line(10).is_empty());
        // Swung open to y 0 – 48 (its x the world's y), 4 thick toward −x: in at x 96, out at x 100.
        let open = line(20);
        assert_eq!(open.len(), 2, "{open:?}");
        assert!((open[0].0.distance - 96.0).abs() < 1e-3 && open[0].0.entering);
        // A door's: its walls take no bullet hole.
        assert!(open.iter().all(|(c, _)| c.shape == DOOR_SHAPE));
        assert!(
            walls_through(open.clone(), &data())
                .iter()
                .all(|w| w.door_in && w.door_out)
        );
        assert!(
            (open[0].0.normal[0] + 1.0).abs() < 1e-3,
            "{:?}",
            open[0].0.normal
        );
        assert!((open[1].0.distance - 100.0).abs() < 1e-3 && !open[1].0.entering);
    }

    #[test]
    fn walls_cost_what_cs2_takes() {
        let rifle = rifle();
        // 10 inches of concrete 500 inches out: 36 × 0.98 = 35.28 there, less 16 %, 3.75 / 2 × 3 × 2 and 2 × 10² /
        // 24.
        let wall = concrete(500.0, 10.0);
        let left = penetrate(&rifle, &wall, 35.28).expect("goes through");
        assert!(
            (left - (35.28 * 0.84 - 11.25 - 200.0 / 24.0)).abs() < 1e-4,
            "{left}"
        );
        assert!(penetrate(&rifle, &concrete(500.0, 30.0), 35.28).is_none());
        // Wood on both sides: 3, whatever the surfaces' own modifiers.
        let mut wood = wall.clone();
        wood.enter.material = "W".into();
        wood.exit.material = "W".into();
        let through_wood = penetrate(&rifle, &wood, 35.28).expect("goes through");
        assert!((through_wood - (35.28 * 0.84 - 1.875 - 100.0 / 72.0)).abs() < 1e-4);
        // Range falloff before, through and after the wall.
        let at = damage_at(&rifle, &[wall], 600.0).expect("reaches");
        assert!(
            (at - left * 0.98f32.powf(90.0 / 500.0)).abs() < 1e-4,
            "{at}"
        );
        // A hit's damage: the stomach × 1.25; on armour 77.5 % of it, the armour half the rest.
        assert_eq!(to_health(&rifle, 20.0, 3, 0.0, false), (25.0, 0.0));
        assert_eq!(to_health(&rifle, 20.0, 2, 100.0, true), (15.0, 2.0));
        // Legs have no armour; a head without a helmet neither.
        assert_eq!(to_health(&rifle, 20.0, 6, 100.0, true), (15.0, 0.0));
        assert_eq!(to_health(&rifle, 20.0, 1, 100.0, false), (80.0, 0.0));
    }

    #[test]
    fn bullets_go_through_walls_and_players() {
        let mut data = data();
        data.weapons.insert("weapon_m249".into(), rifle());
        data.items.insert(14, "weapon_m249".into());
        let mut collision = WorldCollision::default();
        collision.add_box([500.0, -100.0, -100.0], [510.0, 100.0, 100.0]);
        collision.add_box([700.0, -100.0, -100.0], [710.0, 100.0, 100.0]);
        // Two boxes side by side are one wall.
        collision.add_box([800.0, -100.0, -100.0], [802.0, 100.0, 100.0]);
        collision.add_box([802.0, -100.0, -100.0], [804.0, 100.0, 100.0]);
        let walls = walls_on(&collision, &[], &data, [0.0; 3], [1.0, 0.0, 0.0], REACH);
        let spans: Vec<_> = walls.iter().map(|w| (w.distance, w.thickness)).collect();
        assert_eq!(spans, vec![(500.0, 10.0), (700.0, 10.0), (800.0, 4.0)]);
        let fire = |eye: [f32; 3], hits: &[PlayerHit]| {
            let hits: Vec<&PlayerHit> = hits.iter().collect();
            bullet_json(
                &shot(10, 14, 0, eye, 50),
                0,
                None,
                &hits,
                &Obstacles {
                    world: Some(&collision),
                    surfaces: &[],
                    doors: None,
                    bodies: &[],
                },
                &data,
            )
        };
        // Through the first wall (10 left), not the second.
        let bullet = fire([0.0; 3], &[]);
        assert_eq!(bullet["on"], "world");
        assert_eq!(bullet["end"], json!([700.0, 0.0, 0.0]));
        let through: Vec<_> = bullet["through"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|t| (t["on"].clone(), t["end"][0].clone()))
            .collect();
        assert_eq!(
            through,
            vec![
                (json!("world"), json!(500.0)),
                (json!("world"), json!(510.0))
            ]
        );
        // Through a player 200 out (8 inches of flesh: 13 left), stopped by the wall.
        let past = PlayerHit {
            tick: 10,
            start: [0.0, 30.0, 0.0],
            point: [200.0, 30.0, 0.0],
        };
        let bullet = fire([0.0, 30.0, 0.0], &[past]);
        assert_eq!(bullet["through"][0]["on"], "player");
        assert_eq!(bullet["end"], json!([500.0, 30.0, 0.0]));
        // A second player the demo shows behind the wall: the bullet got there.
        let behind = PlayerHit {
            tick: 10,
            start: [510.0, 30.0, 0.0],
            point: [600.0, 30.0, 0.0],
        };
        let bullet = fire([0.0, 30.0, 0.0], &[past, behind]);
        assert_eq!(bullet["on"], "player");
        assert_eq!(bullet["end"], json!([600.0, 30.0, 0.0]));
        assert_eq!(bullet["through"].as_array().map(Vec::len), Some(3));
        // A hit that starts at the eyes beyond a traced wall: the wall is not there for CS2.
        let clear = PlayerHit {
            tick: 10,
            start: [0.0, 60.0, 0.0],
            point: [600.0, 60.0, 0.0],
        };
        let bullet = fire([0.0, 60.0, 0.0], &[clear]);
        assert_eq!(bullet["through"][0]["on"], "player");
        assert_eq!(bullet["through"].as_array().map(Vec::len), Some(1));
        // On through the player (13 left) into the next wall.
        assert_eq!(bullet["end"], json!([700.0, 60.0, 0.0]));
        // The first player's hitboxes on the shot's tick, 80 inches of them along the line: the bullet stops in them.
        let thick = [Body {
            controller: 2,
            tick: 10,
            hitboxes: vec![Capsule {
                a: [230.0, 30.0, 0.0],
                b: [250.0, 30.0, 0.0],
                radius: 30.0,
            }],
        }];
        let bullet = bullet_json(
            &shot(10, 14, 0, [0.0, 30.0, 0.0], 50),
            0,
            Some(1),
            &[&past],
            &Obstacles {
                world: Some(&collision),
                surfaces: &[],
                doors: None,
                bodies: &thick,
            },
            &data,
        );
        assert_eq!(bullet["on"], "player");
        assert_eq!(bullet["end"], json!([200.0, 30.0, 0.0]));
        // That body names the player hit; without bodies, nobody is named.
        assert_eq!(bullet["victim"], 2);
        let bullet = fire([0.0, 30.0, 0.0], &[past]);
        assert_eq!(bullet["through"][0]["victim"], Json::Null);
    }
}
