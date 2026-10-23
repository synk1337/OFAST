//! Thrown grenades and the fires they light, per tick (M6).
//!
//! A thrown grenade is a projectile entity of its own, one class per kind: `CSmokeGrenadeProjectile`,
//! `CFlashbangProjectile`, `CHEGrenadeProjectile`, `CMolotovProjectile` (also the incendiary), `CDecoyProjectile`.
//! The server simulates it and sends, on every tick it moves, its origin (the body component's cell and offset, like
//! a pawn's) and rotation (`m_angRotation`: it spins about 22° a tick in the air), with its world model, its thrower
//! (`m_hThrower`, the pawn), its bounces (`m_nBounces`), the throw (`m_vInitialPosition`, `m_vInitialVelocity`)
//! and the time it is set to go off (`m_flDetonateTime`, game seconds; 0 for a smoke, which pops once it rests).
//! A smoke stays an entity while its cloud lasts: `m_nSmokeEffectTickBegin` (a game tick) says when the cloud
//! began, `m_vSmokeDetonationPos` where, `m_vSmokeColor` its tint; it also networks the inputs of the cloud's
//! simulation (`m_VoxelFrameData`, [`voxel_log`](GrenadePaths::voxel_log), decoded by [`super::smoke`]). A
//! flashbang's and an HE grenade's entity can stay in the demo for seconds after it went off (tournament demos; a
//! local demo removes a flashbang a tick before its event): the detonation events say when it is gone. A molotov
//! leaves when it lands and lights a fire.
//!
//! A fire is a `CInferno` entity (`m_nInfernoType` 0 for a molotov, 1 for an incendiary): up to 64 flames, each a
//! world position (`m_firePositions`), the flame it spread from (`m_fireParentPositions`), the surface normal it
//! burns on (`m_BurnNormal`) and whether it burns (`m_bFireIsBurning`); `m_fireCount` flames are in use. It starts
//! at `m_nFireEffectTickBegin` (a game tick) and burns `m_nFireLifetime` seconds (7). On the Aurora vs FURIA de_nuke
//! demo a molotov spread to 16 flames over its first 2 s, a few ticks apart. `docs/formats/demo.md`, "Grenades".

use std::collections::HashMap;

use super::decoder::FieldValue;
use super::entity::{Entity, ServerClass, handle_to_index};
use super::fieldpath::FieldPath;
use super::parser::DemoParser;
use super::players::{CELL_WIDTH, MAX_COORD};
use super::serializer::Serializers;

/// What a thrown grenade is, by its entity class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GrenadeKind {
    /// `CSmokeGrenadeProjectile`.
    Smoke,
    /// `CFlashbangProjectile`.
    Flashbang,
    /// `CHEGrenadeProjectile`.
    HighExplosive,
    /// `CMolotovProjectile`: a molotov or an incendiary grenade (the fire's `m_nInfernoType` tells them apart).
    Molotov,
    /// `CDecoyProjectile`.
    Decoy,
}

impl GrenadeKind {
    /// Every kind, with its entity class.
    pub const CLASSES: [(GrenadeKind, &'static str); 5] = [
        (GrenadeKind::Smoke, "CSmokeGrenadeProjectile"),
        (GrenadeKind::Flashbang, "CFlashbangProjectile"),
        (GrenadeKind::HighExplosive, "CHEGrenadeProjectile"),
        (GrenadeKind::Molotov, "CMolotovProjectile"),
        (GrenadeKind::Decoy, "CDecoyProjectile"),
    ];

    /// The kind's name in OFAST's files (`smoke`, `flashbang`, `he`, `molotov`, `decoy`).
    pub fn name(self) -> &'static str {
        match self {
            GrenadeKind::Smoke => "smoke",
            GrenadeKind::Flashbang => "flashbang",
            GrenadeKind::HighExplosive => "he",
            GrenadeKind::Molotov => "molotov",
            GrenadeKind::Decoy => "decoy",
        }
    }
}

/// A thrown grenade at one tick: in the air, rolling, resting before it goes off, or (a smoke) while its cloud lasts.
#[derive(Debug, Clone, PartialEq)]
pub struct GrenadeSample {
    /// The projectile's entity index.
    pub entity: u32,
    /// The entity's serial number: a later grenade can reuse the slot (`entity`) with another serial.
    pub serial: u32,
    /// What it is.
    pub kind: GrenadeKind,
    /// Its model (resource id).
    pub model: u64,
    /// The thrower's controller entity index (from the pawn `m_hThrower` names), if known.
    pub thrower: Option<u32>,
    /// Origin, world, inches.
    pub position: [f32; 3],
    /// Rotation, degrees: pitch, yaw, roll (`QAngle`).
    pub angles: [f32; 3],
    /// Times it bounced so far.
    pub bounces: u32,
    /// A smoke's cloud, once it began.
    pub smoke: Option<SmokeCloud>,
}

/// A smoke grenade's cloud.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokeCloud {
    /// Demo tick the cloud began (`m_nSmokeEffectTickBegin`, a game tick, converted).
    pub start_tick: u32,
    /// Where it went off (`m_vSmokeDetonationPos`), world, inches.
    pub position: [f32; 3],
    /// Its tint, 0 – 255 per channel (`m_vSmokeColor`: (75, 127, 155) seen).
    pub color: [f32; 3],
    /// How many times its voxels have been sent (`m_nVoxelUpdate`): a new shape when it changes.
    pub voxel_update: i64,
}

/// A smoke's voxel log (`m_VoxelFrameData`) as it grew over the round: a snapshot `(demo tick, bytes)` each time
/// it changed. Decoded by [`super::smoke`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SmokeLog {
    /// The smoke's entity index.
    pub entity: u32,
    /// Its serial number ([`GrenadeSample::serial`]).
    pub serial: u32,
    /// Every new state of the log, oldest first.
    pub snapshots: Vec<(u32, Vec<u8>)>,
}

/// One flame of a fire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flame {
    /// Where it burns, world, inches.
    pub position: [f32; 3],
    /// The surface normal it burns on.
    pub normal: [f32; 3],
    /// It burns (`m_bFireIsBurning`); a flame that went out keeps its place.
    pub burning: bool,
}

/// A fire (`CInferno`) at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct FireSample {
    /// The fire's entity index.
    pub entity: u32,
    /// The entity's serial number ([`GrenadeSample::serial`]).
    pub serial: u32,
    /// From an incendiary grenade (`m_nInfernoType` 1) rather than a molotov (0).
    pub incendiary: bool,
    /// Demo tick it started (`m_nFireEffectTickBegin`, converted), if sent.
    pub start_tick: Option<u32>,
    /// How long it burns, seconds (`m_nFireLifetime`).
    pub lifetime: f32,
    /// Its flames in use (`m_fireCount` of them), in the order they appeared: index k is slot k every tick.
    pub flames: Vec<Flame>,
}

/// Field paths of one projectile class.
#[derive(Debug)]
struct ProjectilePaths {
    kind: GrenadeKind,
    cell: [FieldPath; 3],
    offset: [FieldPath; 3],
    rotation: Option<FieldPath>,
    model: Option<FieldPath>,
    thrower: Option<FieldPath>,
    bounces: Option<FieldPath>,
    smoke_tick: Option<FieldPath>,
    smoke_position: Option<FieldPath>,
    smoke_color: Option<FieldPath>,
    voxel_update: Option<FieldPath>,
    /// The voxel log (`m_VoxelFrameData`, elements one level deeper) and its bytes in use.
    voxel_data: Option<FieldPath>,
    voxel_size: Option<FieldPath>,
}

/// Field paths of the fire class.
#[derive(Debug)]
struct FirePaths {
    class: u32,
    count: FieldPath,
    positions: FieldPath,
    normals: Option<FieldPath>,
    burning: Option<FieldPath>,
    kind: Option<FieldPath>,
    start: Option<FieldPath>,
    lifetime: Option<FieldPath>,
}

/// The field paths of every grenade class the demo has, looked up once by name; a class missing a position field
/// (a CS2 update renamed it) is left out.
#[derive(Debug, Default)]
pub struct GrenadePaths {
    /// By class id.
    projectiles: HashMap<u32, ProjectilePaths>,
    fire: Option<FirePaths>,
}

/// The x, y and z members of a vector field, e.g. `CBodyComponent.m_cellX` / `Y` / `Z`.
fn axes(serializers: &Serializers, class: &ServerClass, prefix: &str) -> Option<[FieldPath; 3]> {
    let field = |axis: &str| serializers.path_of(class.serializer, &format!("{prefix}{axis}"));
    Some([field("X")?, field("Y")?, field("Z")?])
}

impl GrenadePaths {
    /// Finds the paths in the demo's classes.
    pub fn find(serializers: &Serializers, classes: &[Option<ServerClass>]) -> Self {
        let mut out = Self::default();
        for class in classes.iter().flatten() {
            let field = |name: &str| serializers.path_of(class.serializer, name);
            if let Some(&(kind, _)) = GrenadeKind::CLASSES.iter().find(|(_, c)| *c == class.name) {
                let (Some(cell), Some(offset)) = (
                    axes(serializers, class, "CBodyComponent.m_cell"),
                    axes(serializers, class, "CBodyComponent.m_vec"),
                ) else {
                    continue;
                };
                out.projectiles.insert(
                    class.id,
                    ProjectilePaths {
                        kind,
                        cell,
                        offset,
                        rotation: field("CBodyComponent.m_angRotation"),
                        model: field("CBodyComponent.m_skeletonInstance.m_modelState.m_hModel"),
                        thrower: field("m_hThrower"),
                        bounces: field("m_nBounces"),
                        smoke_tick: field("m_nSmokeEffectTickBegin"),
                        smoke_position: field("m_vSmokeDetonationPos"),
                        smoke_color: field("m_vSmokeColor"),
                        voxel_update: field("m_nVoxelUpdate"),
                        voxel_data: field("m_VoxelFrameData"),
                        voxel_size: field("m_nVoxelFrameDataSize"),
                    },
                );
            } else if class.name == "CInferno"
                && let (Some(count), Some(positions)) =
                    (field("m_fireCount"), field("m_firePositions"))
            {
                out.fire = Some(FirePaths {
                    class: class.id,
                    count,
                    positions,
                    normals: field("m_BurnNormal"),
                    burning: field("m_bFireIsBurning"),
                    kind: field("m_nInfernoType"),
                    start: field("m_nFireEffectTickBegin"),
                    lifetime: field("m_nFireLifetime"),
                });
            }
        }
        out
    }

    /// The grenades of this tick, in entity order. `controller_of` maps a pawn's entity index to its controller's.
    /// A projectile whose origin cell was never sent (not placed yet) is left out.
    pub fn grenades(
        &self,
        parser: &DemoParser<'_>,
        controller_of: &HashMap<u32, u32>,
    ) -> Vec<GrenadeSample> {
        let mut out = Vec::new();
        for entity in parser.entities().iter() {
            let Some(paths) = self.projectiles.get(&entity.class_id) else {
                continue;
            };
            let Some(position) = origin(entity, &paths.cell, &paths.offset) else {
                continue;
            };
            let get = |path: &Option<FieldPath>| path.as_ref().and_then(|p| entity.get(p));
            let vector = |path: &Option<FieldPath>| {
                get(path)
                    .and_then(FieldValue::as_vector3)
                    .unwrap_or_default()
            };
            // A game tick of 0 (or below) is "not yet".
            let smoke_start = get(&paths.smoke_tick)
                .and_then(FieldValue::as_i64)
                .filter(|&t| t > 0)
                .and_then(|t| u32::try_from(t).ok())
                .and_then(|t| parser.game_tick_to_demo(t));
            out.push(GrenadeSample {
                entity: entity.index,
                serial: entity.serial,
                kind: paths.kind,
                model: get(&paths.model).and_then(FieldValue::as_u64).unwrap_or(0),
                thrower: get(&paths.thrower)
                    .and_then(FieldValue::as_u64)
                    .and_then(handle_to_index)
                    .and_then(|pawn| controller_of.get(&pawn).copied()),
                position,
                angles: vector(&paths.rotation),
                bounces: get(&paths.bounces)
                    .and_then(FieldValue::as_i64)
                    .and_then(|b| u32::try_from(b).ok())
                    .unwrap_or(0),
                smoke: smoke_start.map(|start_tick| SmokeCloud {
                    start_tick,
                    // Never sent (an old demo, a renamed field): where the grenade lies, not the map's origin.
                    position: get(&paths.smoke_position)
                        .and_then(FieldValue::as_vector3)
                        .unwrap_or(position),
                    color: vector(&paths.smoke_color),
                    voxel_update: get(&paths.voxel_update)
                        .and_then(FieldValue::as_i64)
                        .unwrap_or(0),
                }),
            });
        }
        out
    }

    /// A smoke's voxel log as it stands (`m_VoxelFrameData`, its first `m_nVoxelFrameDataSize` bytes), or `None`
    /// for an entity that is no smoke or sent none. Elements never sent read as 0.
    pub fn voxel_log(&self, parser: &DemoParser<'_>, entity: u32) -> Option<Vec<u8>> {
        let e = parser.entities().get(entity)?;
        let paths = self.projectiles.get(&e.class_id)?;
        let (data, size) = (paths.voxel_data.as_ref()?, paths.voxel_size.as_ref()?);
        let size = e.get(size)?.as_i64()?.clamp(0, 4096) as i32;
        Some(
            (0..size)
                .map(|k| {
                    data.child(k)
                        .and_then(|p| e.get(&p))
                        .and_then(FieldValue::as_u64)
                        .unwrap_or(0) as u8
                })
                .collect(),
        )
    }

    /// The fires of this tick, in entity order.
    pub fn fires(&self, parser: &DemoParser<'_>) -> Vec<FireSample> {
        let Some(paths) = &self.fire else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entity in parser.entities().iter() {
            if entity.class_id != paths.class {
                continue;
            }
            let get = |path: &Option<FieldPath>| path.as_ref().and_then(|p| entity.get(p));
            let element =
                |array: &FieldPath, k: i32| array.child(k).and_then(|p| entity.get(&p).cloned());
            let count = entity
                .get(&paths.count)
                .and_then(FieldValue::as_i64)
                .unwrap_or(0)
                .clamp(0, 64) as i32;
            // Every slot in use, so a flame keeps its index across ticks (the replay matches flames by it); one whose
            // position never came reads as not burning.
            let flames = (0..count)
                .map(|k| {
                    let position = element(&paths.positions, k).and_then(|v| v.as_vector3());
                    let normal = paths
                        .normals
                        .as_ref()
                        .and_then(|a| element(a, k))
                        .and_then(|v| v.as_vector3())
                        .unwrap_or([0.0, 0.0, 1.0]);
                    let burning = paths
                        .burning
                        .as_ref()
                        .and_then(|a| element(a, k))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    Flame {
                        position: position.unwrap_or_default(),
                        normal,
                        burning: burning && position.is_some(),
                    }
                })
                .collect();
            out.push(FireSample {
                entity: entity.index,
                serial: entity.serial,
                incendiary: get(&paths.kind).and_then(FieldValue::as_i64) == Some(1),
                start_tick: get(&paths.start)
                    .and_then(FieldValue::as_i64)
                    .filter(|&t| t > 0)
                    .and_then(|t| u32::try_from(t).ok())
                    .and_then(|t| parser.game_tick_to_demo(t)),
                lifetime: get(&paths.lifetime)
                    .and_then(FieldValue::as_f32)
                    .unwrap_or(0.0),
                flames,
            });
        }
        out
    }
}

/// `cell × 512 − 16384 + offset` per axis; `None` until the cell was sent (an offset never sent is 0). Also the
/// position of weapons and doors ([`super::players`]).
pub(super) fn origin(
    entity: &Entity,
    cell: &[FieldPath; 3],
    offset: &[FieldPath; 3],
) -> Option<[f32; 3]> {
    let mut out = [0.0; 3];
    for (k, value) in out.iter_mut().enumerate() {
        let c = entity.get(&cell[k])?.as_u64()? as f32;
        let o = entity
            .get(&offset[k])
            .and_then(FieldValue::as_f32)
            .unwrap_or(0.0);
        *value = c * CELL_WIDTH - MAX_COORD + o;
    }
    Some(out)
}
