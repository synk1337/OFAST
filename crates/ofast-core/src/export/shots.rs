//! A round's gun shots for Unreal (M6): where each bullet went and what it met, and where it left the barrel.
//!
//! The demo records each shot's aim and random seed, not its bullets' paths ([`crate::demo::shots`]); CS2's
//! clients trace the bullets themselves to draw tracers, impacts and bullet holes. OFAST does the same: each bullet's
//! direction from the seed ([`crate::demo::shots::spread_offset`]: on the de_nuke pro demo 82 of the 83 bullets a
//! pane of glass shows lie on it, the other being another bullet's), traced from the shooter's eyes through the
//! map's collision as bullets see it ([`crate::physics::CollisionAttribute::stops_bullets`]: not grates, not glass) up
//! to the weapon's range ([`WeaponShots::range`]). A bullet the demo shows meeting a player
//! ([`crate::demo::shots::PlayerHit`]: the blood's effect dispatch) ends on that player; a wall it went through first
//! (CS2's penetration) is its `wall`. Not traced (yet): bullets going on through walls the demo does not show them
//! passing, and through players, and props and doors (they are not in the world's collision).
//!
//! Written to the agents manifest as `shots` (it needs the map's collision and the baked agents):
//!
//! | Key | What |
//! |---|---|
//! | `tick`, `controller` | the shot's tick and shooter |
//! | `item`, `weapon`, `mode`, `silenced` | the weapon's item definition index and name (`weapon_ak47`); the fire mode (1: secondary); whether the weapon's silencer is on (its secondary mode) |
//! | `tracer` | the weapon's tracer system's stem when this shot draws one ([`draws_tracer`]), else `null` |
//! | `eye` | where the bullets start: the shooter's eyes (Source axes, inches) |
//! | `muzzle`, `muzzle_direction` | where they left the barrel as the exported agent shows it (third person), and along the barrel; `null` when the agent shows no gun in hand then |
//! | `view_muzzle`, `view_muzzle_direction` | the same as the shooter's viewmodel shows it (first person), in the viewmodel rig's model space: Source axes (x forward, y left, z up), inches (in Unreal the viewmodel's component space: the camera, before the player's viewmodel offsets and the left hand's mirroring); `null` without a viewmodel then |
//! | `bullets` | per bullet (shotguns fire several): `end` (where it stopped), `on` (`"world"`, `"player"` or `"air"`: nothing within range), `normal` (of the surface met, facing the shooter; `null` on a player or in the air), `surface` (the world surface's property name, `null` when unknown), `impact` (the surface's impact system, [`Impact::effect`]) and `decal` (its bullet-hole decal group); on a player, `wall`: the wall it went through first (`end`, `normal`, `surface`, `impact`, `decal`), or `null` |

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::{Value as Json, json};

use super::gltf::write_png;
use super::players::Muzzle;
use crate::content::Content;
use crate::demo::players::RoundTrack;
use crate::demo::shots::{PlayerHit, Shot, bullet_direction, hit_of, spread_offset};
use crate::items::item_names;
use crate::kv3::Value;
use crate::physics::PhysicsData;
use crate::physics::ragdoll::WorldCollision;
use crate::resource::{BlockKind, Resource};
use crate::texture::Texture;

/// A weapon's shots as `scripts/weapons.vdata` describes them (each entry inherits what it lacks from its `_base`).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponShots {
    /// `m_nNumBullets`: bullets per shot (the Nova 9, the XM1014 6, the MAG-7 and Sawed-Off 8, every other gun 1).
    pub bullets: u32,
    /// `m_nTracerFrequency`: one shot in this many draws a tracer, in the normal and the secondary mode (0: none; a
    /// single value is both). The M4A1-S and the USP-S: none with the silencer on (their secondary mode).
    pub tracer_frequency: [u32; 2],
    /// `m_szTracerParticle`'s file stem: `weapon_tracers_assrifle`, `_rifle` (the snipers), `_pistol`, `_shot`,
    /// `_smg`…
    pub tracer: Option<String>,
    /// `m_eSilencerType` is not `WEAPONSILENCER_NONE`: the secondary mode is the silencer on.
    pub silencer: bool,
    /// `m_flRange`: how far its bullets go (units; rifles and snipers 8192, the Glock 4096, the MAG-7 1400). In
    /// Source's bullets (CS:GO), the trace ends there and a wall further away is not hit.
    pub range: f32,
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
}

/// The `DATA` block of a compiled resource.
fn data_root(content: &Content, path: &str) -> Result<Value, String> {
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
        let path = "scripts/surfaceproperties_impact_effects.txt";
        match content
            .read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                crate::kv3::text::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| e.to_string())
            }) {
            Ok(file) => data.impacts = impacts_of(&file.root),
            Err(e) => warnings.push(format!("{path}: {e}: impacts are concrete's")),
        }
        data
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

/// The player hits the demo sent, by tick (a shot's hits are among its tick's: [`hit_of`]).
fn hits_by_tick(hits: &[PlayerHit]) -> HashMap<u32, Vec<&PlayerHit>> {
    let mut out: HashMap<u32, Vec<&PlayerHit>> = HashMap::new();
    for hit in hits {
        out.entry(hit.tick).or_default().push(hit);
    }
    out
}

/// The hits of `shot` among `by_tick` ([`hits_by_tick`]).
fn hits_of<'a>(shot: &Shot, by_tick: &HashMap<u32, Vec<&'a PlayerHit>>) -> Vec<&'a PlayerHit> {
    by_tick
        .get(&shot.tick)
        .map(|hits| hits.iter().copied().filter(|h| hit_of(shot, h)).collect())
        .unwrap_or_default()
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

/// Whether a shot draws a tracer: one in `frequency` does ([`WeaponShots::tracer_frequency`] for its mode), counted
/// on the magazine: the shots that leave a multiple of `frequency` bullets in it (an inference: CS2's counter is
/// not in its files; this one is the same for every viewer and spaces tracers as the frequency says).
pub fn draws_tracer(frequency: u32, bullets_left: u32) -> bool {
    frequency > 0 && bullets_left.is_multiple_of(frequency)
}

/// The shots of `track` as the manifest's `shots` (module docs), and the bullet holes' decal groups they use (for
/// [`export_decals`]). `collision` is the map's collision as bullets see it (`None`: every bullet that met no player
/// ends in the air) and `physics` the world physics it was built from (the surfaces), `muzzles` and `view_muzzles`
/// the barrels the agents' and the viewmodels' exports found, `data` CS2's weapons and surfaces.
pub fn shots_json(
    track: &RoundTrack,
    collision: Option<&WorldCollision>,
    physics: Option<&PhysicsData>,
    muzzles: &[Muzzle],
    view_muzzles: &[Muzzle],
    data: &ShotData,
) -> (Vec<Json>, BTreeSet<String>) {
    // The world's surface property hashes, by index (RayHit::surface).
    let surfaces: &[u32] = physics.map_or(&[], |p| p.surfaces.as_slice());
    let by_tick = hits_by_tick(&track.player_hits);
    let mut decals = BTreeSet::new();
    let shots = track
        .shots
        .iter()
        .map(|(controller, shot)| {
            let hits = hits_of(shot, &by_tick);
            let weapon = data.weapon(shot.item);
            let bullets: Vec<Json> = (0..weapon.map_or(1, |w| w.bullets.max(1)))
                .map(|bullet| {
                    let json = bullet_json(shot, bullet, &hits, collision, surfaces, data);
                    for group in [&json["decal"], &json["wall"]["decal"]]
                        .into_iter()
                        .filter_map(Json::as_str)
                    {
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
                .filter(|w| draws_tracer(w.tracer_frequency[mode], shot.bullets_left))
                .and_then(|w| w.tracer.clone());
            json!({
                "tick": shot.tick,
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
                "bullets": bullets,
            })
        })
        .collect();
    (shots, decals)
}

/// The bullet holes the shots before the round left (`track.earlier_shots`): CS2's clients keep them from round to
/// round (a hole on de_nuke's A site in round 5 is an AWP bullet's from round 3, tick 19909, where OFAST's trace ends
/// within 5 units of it). As the manifest's `earlier_holes`: `tick`, `end`, `normal` and `decal` of every bullet
/// that met the world where its surface keeps a hole; and the decal groups they use. Arguments as [`shots_json`].
pub fn earlier_holes_json(
    track: &RoundTrack,
    collision: Option<&WorldCollision>,
    physics: Option<&PhysicsData>,
    data: &ShotData,
) -> (Vec<Json>, BTreeSet<String>) {
    let surfaces: &[u32] = physics.map_or(&[], |p| p.surfaces.as_slice());
    let by_tick = hits_by_tick(&track.earlier_hits);
    let mut decals = BTreeSet::new();
    let mut holes = Vec::new();
    for shot in &track.earlier_shots {
        let hits = hits_of(shot, &by_tick);
        for bullet in 0..data.weapon(shot.item).map_or(1, |w| w.bullets.max(1)) {
            let json = bullet_json(shot, bullet, &hits, collision, surfaces, data);
            // Where it met the world: its end, or the wall it went through on its way to a player.
            for at in [&json, &json["wall"]] {
                let Some(group) = at["decal"].as_str() else {
                    continue;
                };
                decals.insert(group.to_owned());
                holes.push(json!({
                    "tick": shot.tick,
                    "end": at["end"],
                    "normal": at["normal"],
                    "decal": group,
                }));
            }
        }
    }
    (holes, decals)
}

/// One bullet of `shot` (module docs): on the player hit whose point lies on its line (a wall `collision` puts
/// before it is the bullet's `wall`: it went through), else where `collision` stops it within the weapon's range
/// (with the surface met: `surfaces`, the world's surface property hashes by index), else in the air at that range.
fn bullet_json(
    shot: &Shot,
    bullet: u32,
    hits: &[&PlayerHit],
    collision: Option<&WorldCollision>,
    surfaces: &[u32],
    data: &ShotData,
) -> Json {
    let direction = bullet_direction(
        shot.angles,
        spread_offset(shot.seed, bullet, shot.inaccuracy, shot.spread),
    );
    let range = range_of(shot, data);
    let along = |point: [f32; 3]| -> (f32, f32) {
        let v: [f32; 3] = std::array::from_fn(|k| point[k] - shot.eye[k]);
        let t = v[0] * direction[0] + v[1] * direction[1] + v[2] * direction[2];
        let off: [f32; 3] = std::array::from_fn(|k| v[k] - direction[k] * t);
        (
            t,
            (off[0] * off[0] + off[1] * off[1] + off[2] * off[2]).sqrt(),
        )
    };
    let world = collision.and_then(|c| c.ray_hit(shot.eye, direction, range));
    let on_player = hits
        .iter()
        .map(|h| (h, along(h.point)))
        .filter(|(_, (t, off))| *t > 0.0 && *off < ON_THE_LINE)
        .min_by(|a, b| a.1.0.total_cmp(&b.1.0));
    // What a bullet met on the world: where, the surface's normal, its name, impact and bullet hole.
    let on_world = |w: &crate::physics::ragdoll::RayHit| {
        let end: [f32; 3] = std::array::from_fn(|k| shot.eye[k] + direction[k] * w.distance);
        let (surface, impact) = match surfaces.get(w.surface as usize) {
            Some(&hash) => data.impact(hash),
            None => (None, data.impacts.get("default")),
        };
        json!({
            "end": rounded(end),
            "normal": unit(w.normal),
            "surface": surface,
            "impact": impact.map(|i| i.effect.as_str()),
            "decal": impact.and_then(|i| i.decal.as_deref()),
        })
    };
    if let Some((hit, (t, _))) = on_player {
        // The demo says it met the player: a wall before them it went through (CS2's penetration).
        let wall = world.filter(|w| w.distance < t - 1.0).map(|w| on_world(&w));
        return json!({
            "end": rounded(hit.point),
            "on": "player",
            "normal": null,
            "surface": null,
            "impact": null,
            "decal": null,
            "wall": wall,
        });
    }
    match world {
        Some(w) => {
            let mut out = on_world(&w);
            out["on"] = json!("world");
            out
        }
        None => {
            let end: [f32; 3] = std::array::from_fn(|k| shot.eye[k] + direction[k] * range);
            json!({
                "end": rounded(end),
                "on": "air",
                "normal": null,
                "surface": null,
                "impact": null,
                "decal": null,
            })
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
        // One in three, on the magazine's count; none at frequency 0.
        let drawn: Vec<u32> = (0..10).filter(|&left| draws_tracer(3, left)).collect();
        assert_eq!(drawn, vec![0, 3, 6, 9]);
        assert!((0..10).all(|left| !draws_tracer(0, left)));
        assert!((0..10).all(|left| draws_tracer(1, left)));
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
            // The second shot met a player 200 inches out, before the wall; the fourth one 600 out, through it.
            player_hits: vec![
                PlayerHit {
                    tick: 20,
                    start: [0.0, 50.0, 0.0],
                    point: [200.0, 50.0, 0.0],
                },
                PlayerHit {
                    tick: 40,
                    start: [0.0, 0.0, 0.0],
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
        };
        let muzzles = [Muzzle {
            controller: 2,
            tick: 10,
            position: [30.0, -5.0, -5.0],
            direction: [1.0, 0.0, 0.0],
        }];
        let view_muzzles = [Muzzle {
            controller: 2,
            tick: 10,
            position: [28.0, -6.0, -4.0],
            direction: [0.0, 0.999_999_9, 0.0],
        }];
        let (shots, decals) = shots_json(
            &track,
            Some(&collision),
            None,
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
        // On the player, before the wall; 26 left: no tracer; no muzzle known.
        let hit = &shots[1]["bullets"][0];
        assert_eq!(
            (hit["on"].as_str(), hit["end"].clone()),
            (Some("player"), json!([200.0, 50.0, 0.0]))
        );
        assert!(shots[1]["tracer"].is_null() && shots[1]["muzzle"].is_null());
        assert!(shots[1]["view_muzzle"].is_null());
        // The silenced M4A1-S: silenced, and no tracer in that mode.
        assert_eq!(shots[2]["silenced"], true);
        assert!(shots[2]["tracer"].is_null());
        // Through the wall into the player behind it: blood there, the wall its `wall`.
        let through = &shots[3]["bullets"][0];
        assert_eq!(through["on"], "player");
        assert_eq!(through["end"], json!([600.0, 0.0, 0.0]));
        assert_eq!(through["wall"]["end"], json!([500.0, 0.0, 0.0]));
        assert_eq!(through["wall"]["decal"], "Impact.Concrete");
        // The Nova: 9 pellets, each to the wall.
        assert_eq!(shots[4]["bullets"].as_array().map(Vec::len), Some(9));
        assert_eq!(decals, BTreeSet::from(["Impact.Concrete".to_owned()]));
        // Without collision every bullet that met no player flies on to its weapon's range.
        let (open, _) = shots_json(&track, None, None, &[], &[], &data);
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
}
