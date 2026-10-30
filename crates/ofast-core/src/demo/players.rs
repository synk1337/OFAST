//! Per-tick player positions and eye angles for one round, and the round's game events (deaths, round end, bomb…).
//!
//! Players are two linked entities:
//!
//! - a **controller** (`CCSPlayerController`, one per connected player, entity index = player slot + 1) with the
//!   name (`m_iszPlayerName`), Steam id (`m_steamID`), team (`m_iTeamNum`) and a handle to the pawn
//!   (`m_hPlayerPawn`);
//! - a **pawn** (`CCSPlayerPawn`, the body in the world) with the position, the view offset and the eye angles.
//!
//! Only the Terrorists and Counter-Terrorists are sampled: tournament demos also list spectators (the observer and
//! casters, team 1), whose pawn is never alive and stays at the world origin.
//!
//! The position is split into a coarse grid cell and an offset inside it, both on the pawn's `CBodyComponent`:
//! `x = m_cellX × 512 − 16384 + m_vecX` (cells are 512 units wide and the world spans ±16384 units), same for y and
//! z. That is the feet. The first-person camera sits `m_vecViewOffset` above them (63.96875 units standing,
//! 45.96875 crouched, smoothed over steps, so it changes from tick to tick), and CS2 aims it along `m_angEyeAngles`
//! = (pitch, yaw, roll) in degrees. Both were checked against the camera CS2 itself shows when spectating a player.
//!
//! Rounds come from the game rules entity (`CCSGameRulesProxy` → `m_pGameRules`). A round **starts** when the
//! freeze time begins (`m_bFreezePeriod` turns on) outside warmup and before the match has ended (`m_gamePhase` =
//! 5). It lasts until the tick before the next round starts, the match ends, or the demo ends — so it includes the
//! few seconds after the round is decided. Its number is `m_totalRoundsPlayed + 1` at the start. See
//! `docs/formats/demo.md`.
//!
//! What drives a player's animation ([`RigInputs`]: velocity, ground, crouch, walking, the weapon in hand and its
//! animation state, jumps and landings) comes from the pawn, its movement and weapon services, and the active
//! weapon's entity. Those fields are looked up leniently: if a CS2 update renames one, it reads as its default and
//! the positions keep working.
//!
//! Weapons nobody carries ([`DroppedWeapon`]: a dead player's gun, a weapon dropped or thrown down) have no owner
//! (`m_hOwnerEntity` and the body component's `m_hParent` are the invalid handle). The server simulates them as
//! physics objects and sends their origin (cell and offset, like a pawn's) and rotation (`m_angRotation`: pitch,
//! yaw, roll) on every tick they move; carried weapons send zeros (they follow their owner's hand).

use std::collections::{BTreeMap, HashMap};

use super::decoder::FieldValue;
use super::entity::{Entity, ServerClass, handle_to_index};
use super::events::GameEvent;
use super::fieldpath::FieldPath;
use super::grenades::{FireSample, GrenadePaths, GrenadeSample, SmokeLog, origin};
use super::parser::{DemoError, DemoParser, RawMessage};
use super::serializer::Serializers;
use super::shots::{self, PlayerHit, Shot};
use super::skins::{Skin, skin_from_attributes};

/// Width of one position grid cell, in world units.
pub const CELL_WIDTH: f32 = 512.0;
/// Half the size of the world: cell coordinates are offset by this so they are never negative.
pub const MAX_COORD: f32 = 16384.0;
/// `m_iTeamNum` of the Terrorists.
pub const TERRORISTS: u32 = 2;
/// `m_iTeamNum` of the Counter-Terrorists.
pub const COUNTER_TERRORISTS: u32 = 3;

/// One player at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSample {
    /// Controller entity index (player slot + 1).
    pub controller: u32,
    /// Player name.
    pub name: String,
    /// 64-bit Steam id (0 for bots).
    pub steam_id: u64,
    /// Team number: 2 = Terrorists, 3 = Counter-Terrorists. Spectators (team 1) and unassigned players (0) are left
    /// out of the samples.
    pub team: u32,
    /// World position of the pawn's origin (feet), in Source units (1 unit = 1 inch).
    pub position: [f32; 3],
    /// From the feet to the first-person camera: the pawn's networked `m_vecViewOffset`, in world units.
    ///
    /// z is the eye height. It is not a constant: 63.96875 standing (not 64), 45.96875 fully crouched, values in
    /// between while crouching or standing up, other values in special poses (37.96875 was seen crouched with the C4
    /// in hand), and when the feet go up or down a step it moves the other way for a few ticks so the camera glides.
    /// It comes in steps of 1/64, at most 64. x and y are networked too but were 0 in every sample of the checked
    /// match; they are kept because the engine adds the whole vector (so does demoinfocs-golang's `PositionEyes`),
    /// so [`eye_position`](Self::eye_position) stays right even if a CS2 update starts using them.
    pub view_offset: [f32; 3],
    /// Looking up/down in degrees (negative = up).
    pub pitch: f32,
    /// Looking left/right in degrees (-180..180, 0 = +x axis, counter-clockwise).
    pub yaw: f32,
    /// The pawn is alive (`m_lifeState` = 0).
    pub alive: bool,
    /// What the player's animation follows: movement, crouch, weapon.
    pub rig: RigInputs,
    /// The agent model, its mesh groups and the gloves.
    pub appearance: Appearance,
    /// Every weapon the player carries, the one in hand included (`m_pWeaponServices.m_hMyWeapons`), in the order
    /// the game lists them.
    pub weapons: Vec<CarriedWeapon>,
    /// What killed the player, while dead (`None` while alive): the pawn's ragdoll damage fields, set on the death
    /// tick and kept until the next death.
    pub death: Option<Death>,
    /// How the player set up their first-person view (field of view, offsets, hand).
    pub viewmodel: ViewmodelSettings,
}

/// A player's own first-person view settings: CS2's `viewmodel_fov`, `viewmodel_offset_x` / `_y` / `_z` and
/// `cl_righthand`, which the pawn networks (`m_flViewmodelFOV`, `m_flViewmodelOffsetX` / `Y` / `Z`,
/// `m_bLeftHanded`) so that spectators see the viewmodel as the player does (`docs/formats/demo.md`, "First-person
/// view"). Pawns that send none read as CS2's defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewmodelSettings {
    /// The viewmodel's field of view, degrees, horizontal at 4:3 like CS2's world FOV (CS2 allows 60 – 68).
    pub fov: f32,
    /// How far the viewmodel is moved from where its clips put it, inches, in view space: x to the right, y
    /// forward, z up (CS2 allows −2 … 2.5 for x, −2 … 2 for y and z).
    pub offset: [f32; 3],
    /// Drawn in the left hand (mirrored).
    pub left_handed: bool,
}

impl Default for ViewmodelSettings {
    /// CS2's defaults: FOV 60, offsets (1, 1, −1), right-handed.
    fn default() -> Self {
        Self {
            fov: 60.0,
            offset: [1.0, 1.0, -1.0],
            left_handed: false,
        }
    }
}

/// What killed a player: what CS2's client starts the ragdoll from (it simulates the ragdoll itself; demos carry no
/// ragdoll pose). Read from the pawn on the death tick (`docs/formats/demo.md`, "Deaths").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Death {
    /// The bone hit (`m_nRagdollDamageBone`), an index into the agent's skeleton: 7 = `head_0` for a headshot.
    pub bone: Option<u32>,
    /// The force of the killing hit (`m_vRagdollDamageForce`), world, along the shot: magnitudes of about 20000 –
    /// 30000 for rifle bullets.
    pub force: [f32; 3],
    /// Where it hit (`m_vRagdollDamagePosition`), world.
    pub position: [f32; 3],
    /// A headshot (`m_bRagdollDamageHeadshot`).
    pub headshot: bool,
    /// The weapon's name (`m_szRagdollDamageWeaponName`: `m4a1_silencer`, `ak47`…).
    pub weapon: String,
}

/// What a player looks like. Models are named by resource id ([`crate::resource::resource_id`] of the `.vmdl`
/// path), as entities send them; 0 = not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Appearance {
    /// The agent (`CBodyComponent` → `m_skeletonInstance.m_modelState.m_hModel`), e.g. the id of
    /// `agents/models/ctm_sas/ctm_sas.vmdl`.
    pub model: u64,
    /// The model's mesh groups shown, one bit per group (`m_MeshGroupMask`). On agents: the defuse kit hidden or
    /// shown, the third-person body with or without its own gloves (`docs/formats/demo.md`, "Models").
    pub mesh_groups: u64,
    /// Item definition index of the gloves (`m_EconGloves.m_iItemDefinitionIndex`); 0 = the agent's own.
    pub gloves: u32,
    /// The gloves' finish (`m_EconGloves`' attributes, as a weapon's: [`super::skins`]): paint kit, pattern seed and
    /// the wear's bits; kit 0 = none.
    pub glove_finish: [u32; 3],
}

/// A weapon a player carries: in hand, holstered or slung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarriedWeapon {
    /// The weapon's entity index; [`ActiveWeapon::entity`] names the one in hand.
    pub entity: u32,
    /// Item definition index (7 = AK-47, …).
    pub item: u32,
    /// The weapon's world model (resource id): a knife skin's own model, for example.
    pub model: u64,
    /// The world model's mesh groups shown: on guns bit 0 is the current model, bit 1 the CS:GO-era one that older
    /// skins are painted for.
    pub mesh_groups: u64,
    /// How many of it the player has: 1, except flashbangs, of which a player can carry two in one entity (the
    /// count is the pawn's `m_pWeaponServices.m_iAmmo[FLASHBANG_AMMO]`).
    pub count: u32,
}

/// A weapon lying in the world at one tick, owned by nobody: dropped by a dying player (the gun in hand and a
/// grenade), dropped or thrown down by a living one, or left from earlier in the round. The server moves it as a
/// physics object; a player picking it up ends it (it is carried again).
#[derive(Debug, Clone, PartialEq)]
pub struct DroppedWeapon {
    /// The weapon's entity index.
    pub entity: u32,
    /// Item definition index (7 = AK-47, …).
    pub item: u32,
    /// The weapon entity's class (`CAK47`, `CHEGrenade`, `CC4`, …).
    pub class: String,
    /// The world model (resource id), as when it was carried.
    pub model: u64,
    /// The world model's mesh groups ([`CarriedWeapon::mesh_groups`]).
    pub mesh_groups: u64,
    /// The model's origin, world, inches.
    pub position: [f32; 3],
    /// The model's rotation, degrees: pitch, yaw, roll (Source's `QAngle`: roll about x first, then pitch about y,
    /// then yaw about z).
    pub angles: [f32; 3],
}

/// A rotating door (`CPropDoorRotating`, the map's `prop_door_rotating`) at one tick. The server swings it about its
/// origin (its hinge) and sends the rotation on every tick it moves; the entity is created again every round.
#[derive(Debug, Clone, PartialEq)]
pub struct DoorSample {
    /// The door's entity index (it changes from round to round).
    pub entity: u32,
    /// The door's model (resource id).
    pub model: u64,
    /// Where the door's origin is when closed (`m_closedPosition`): the map entity's origin, what names it
    /// across rounds.
    pub closed_position: [f32; 3],
    /// Its rotation when closed (`m_closedAngles`), degrees: pitch, yaw, roll (the map entity's `angles`).
    pub closed_angles: [f32; 3],
    /// The origin now (body component's cell and offset; a rotating door keeps it).
    pub position: [f32; 3],
    /// The rotation now, degrees: pitch, yaw, roll (`CBodyComponent.m_angRotation`).
    pub angles: [f32; 3],
    /// `m_eDoorState`: 0 closed, 1 opening, 2 open, 3 closing (4 ajar).
    pub state: u32,
}

/// A physics prop the server simulates (`CPhysicsPropMultiplayer`: the map's `prop_physics_multiplayer`, de_nuke's
/// hanging hard hats and mugs) at one tick: bullets, blasts and players push it about. CS2 makes every prop again
/// where the map entity puts it at each round's start (de_nuke round 5: all 25 at their entity lump's origins and
/// angles, some placed lying down) and sometimes within a round, under other entity indices.
#[derive(Debug, Clone, PartialEq)]
pub struct PropSample {
    /// The prop's entity index.
    pub entity: u32,
    /// The entity's serial number: a prop made again can take a freed slot (`entity`) under another serial.
    pub serial: u32,
    /// Its model (resource id).
    pub model: u64,
    /// Its origin (body component's cell and offset).
    pub position: [f32; 3],
    /// Its rotation, degrees: pitch, yaw, roll (`CBodyComponent.m_angRotation`).
    pub angles: [f32; 3],
}

/// The classes of the physics props [`PropSample`] reads.
const PROP_CLASSES: [&str; 3] = [
    "CPhysicsPropMultiplayer",
    "CPhysicsProp",
    "CPhysicsPropOverride",
];

/// Index of the flashbangs in the pawn's ammo counts (`m_pWeaponServices.m_iAmmo`). Observed on the local match
/// demo, from the grenades each player carried: 13 HE, 14 flashbangs (2 with one flashbang entity), 15 smoke,
/// 16 molotov or incendiary.
pub const FLASHBANG_AMMO: i32 = 14;
/// Item definition index of the flashbang.
pub const FLASHBANG_ITEM: u32 = 43;

/// What CS2's player animation is driven by at one tick, besides position and view: the inputs of OFAST's replay
/// rig (`docs/decisions/013-replay-rig-baked-in-rust.md`). "Rig inputs" in `docs/formats/demo.md` lists the fields
/// and how they were checked.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RigInputs {
    /// Which way the body faces, degrees like [`PlayerSample::yaw`] (`CBodyComponent` → `m_angRotation` yaw, the
    /// rotation the model is drawn with). It follows the eye yaw with a lag and limits, so it can differ from it by
    /// tens of degrees while the player stands and looks around.
    pub body_yaw: f32,
    /// Velocity in units per second (`m_vecVelocity`): what the locomotion blend follows (running is 215 – 250
    /// units/s depending on the weapon, walking about half).
    pub velocity: [f32; 3],
    /// Standing on the ground or on an object (`m_fFlags` bit 0, `FL_ONGROUND`); off while jumping or falling.
    pub on_ground: bool,
    /// How far crouched, from 0 (standing) to 1 (`m_pMovementServices.m_flDuckAmount`), gradual while going down or
    /// up.
    pub duck_amount: f32,
    /// The walk key is held (`m_bIsWalking`).
    pub walking: bool,
    /// Looking through a scope (`m_bIsScoped`).
    pub scoped: bool,
    /// Defusing the bomb (`m_bIsDefusing`).
    pub defusing: bool,
    /// Climbing a ladder (`m_MoveType` = [`MOVETYPE_LADDER`]).
    pub on_ladder: bool,
    /// Demo tick of the player's latest jump (`m_pMovementServices.m_nLastJumpTick`, a game tick), if any so far.
    pub last_jump_tick: Option<u32>,
    /// Demo tick of the player's latest landing (`m_pMovementServices.m_ModernJump.m_nLastLandedTick`).
    pub last_landing_tick: Option<u32>,
    /// The weapon in hand (`m_pWeaponServices.m_hActiveWeapon`), if any.
    pub weapon: Option<ActiveWeapon>,
    /// How long the latest flash blinds the player, in seconds (`m_flFlashDuration`); 0 if never flashed.
    pub flash_duration: f32,
    /// How white the latest flash makes the screen at most, 0 – 255 (`m_flFlashMaxAlpha`).
    pub flash_max_alpha: f32,
    /// The view punch as last set: (pitch, yaw) degrees, negative pitch up (`m_vecCsViewPunchAngle`), and when, in
    /// demo ticks to a fraction (`m_nCsViewPunchAngleTick` + `m_flCsViewPunchAngleTickRatio`, a game tick); `None`
    /// before any. It decays from then on ([`super::recoil::view_punch_at`]).
    pub view_punch: Option<super::recoil::ViewPunch>,
    /// The AnimGraph2 tasks the server's graph ran for the pawn this tick, as sent ([`super::recipe`]): `None` while
    /// dead or in a demo without them.
    pub pose_recipe: Option<super::recipe::PoseRecipe>,
}

/// Whether a pawn's flash duration ([`RigInputs::flash_duration`]) going from `seen` to `duration` is a new blinding.
///
/// A flashbang sets the duration on the tick it goes off, and the server clears it some time later, at no tick that
/// means anything (`docs/formats/demo.md`, "Rig inputs"). So a blinding is a change to a value above 0. `seen` is the
/// value at the previous sample of the living pawn, `None` after a gap (dead, absent, or the first sample): a value
/// left from before the gap, or from the previous round, starts nothing. The animation bake (a blinded player's
/// flinch) and the replay's `blindings` (the white screen) both use this, so they see the same flashes.
pub fn flash_starts(seen: Option<f32>, duration: f32) -> bool {
    duration > 0.0 && seen.is_some_and(|seen| seen != duration)
}

/// The weapon a player holds at one tick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ActiveWeapon {
    /// The weapon's entity index (its entry in [`PlayerSample::weapons`]).
    pub entity: u32,
    /// Item definition index (`m_AttributeManager.m_Item.m_iItemDefinitionIndex`), the game's weapon id: 7 = AK-47,
    /// 16 = M4A4, 60 = M4A1-S, 42 = CT knife, 59 = T knife, 49 = C4 (`docs/formats/demo.md`, "Rig inputs").
    pub item: u32,
    /// The weapon entity's class (`CAK47`, `CWeaponM4A1`, `CKnife`, `CC4`, …).
    pub class: String,
    /// What the weapon's animation is doing (`m_iWeaponGameplayAnimState`): idle, deploying, shooting, reloading…
    /// The values are listed in `docs/formats/demo.md`.
    pub anim_state: i64,
    /// Demo tick at which `anim_state` last (re)started (`m_flWeaponGameplayAnimStateTimestamp`, game seconds): a
    /// second knife swing restarts the state with the same value.
    pub state_tick: Option<u32>,
    /// Reloading (`m_bInReload`).
    pub reloading: bool,
    /// How hard a grenade is being thrown, 0 – 1 (`m_flThrowStrength`: 1 for a left-click throw, 0 for a right-click
    /// lob, 0.5 for both); 0 for other weapons.
    pub throw_strength: f32,
    /// Rounds in the magazine (`m_iClip1`); `None` for weapons without one (knives, grenades read −1).
    pub clip: Option<u32>,
    /// Rounds left besides the magazine (`m_pReserveAmmo[0]`); `None` for weapons without reserve ammunition.
    pub reserve: Option<u32>,
    /// The silencer is on (`m_bSilencerOn`: the M4A1-S and USP-S, while screwed on).
    pub silenced: bool,
}

/// `m_fFlags` bit: the pawn stands on something.
pub const FL_ONGROUND: u64 = 1;
/// `m_MoveType` while on a ladder.
pub const MOVETYPE_LADDER: u64 = 9;

impl PlayerSample {
    /// Where the first-person camera is: [`position`](Self::position) + [`view_offset`](Self::view_offset).
    ///
    /// This is exactly the camera position CS2 prints with `getpos` while spectating the player in first person
    /// (checked to 6 decimals, standing, crouched and running — "Checked in the game" in `docs/formats/demo.md`).
    /// Point the camera along [`pitch`](Self::pitch) and [`yaw`](Self::yaw) to get the player's view.
    pub fn eye_position(&self) -> [f32; 3] {
        let [x, y, z] = self.position;
        let [dx, dy, dz] = self.view_offset;
        [x + dx, y + dy, z + dz]
    }
}

/// Every player at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct TickSample {
    /// Demo tick.
    pub tick: u32,
    /// Players with a pawn, in controller order.
    pub players: Vec<PlayerSample>,
    /// Weapons lying in the world, in entity order.
    pub dropped: Vec<DroppedWeapon>,
    /// The map's rotating doors, in entity order.
    pub doors: Vec<DoorSample>,
    /// The map's physics props, in entity order.
    pub props: Vec<PropSample>,
    /// Thrown grenades (in flight, resting, or a smoke's cloud), in entity order ([`super::grenades`]).
    pub grenades: Vec<GrenadeSample>,
    /// Fires of molotovs and incendiary grenades, in entity order.
    pub fires: Vec<FireSample>,
}

/// The players of one round, tick by tick.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTrack {
    /// Round number (1 = first round after warmup).
    pub round: u32,
    /// First tick of the round (freeze time starts).
    pub start_tick: u32,
    /// Last tick that belongs to the round.
    pub end_tick: u32,
    /// Seconds per tick.
    pub tick_interval: f32,
    /// Map name from the demo header.
    pub map_name: String,
    /// One sample per tick that had entity updates.
    pub samples: Vec<TickSample>,
    /// The game events of the round (`player_death`, `round_freeze_end`, `round_end`, `bomb_planted`, …), oldest
    /// first. See [`super::events`].
    pub events: Vec<GameEvent>,
    /// Each smoke's voxel log, a snapshot each time it changed, in order of the smokes' first snapshot.
    pub smoke_logs: Vec<SmokeLog>,
    /// The round's gun shots, oldest first, each with the shooter's controller entity index (see [`super::shots`]).
    pub shots: Vec<(u32, Shot)>,
    /// Where the round's bullets met players (from effect dispatches; a few are other effects, matching no shot:
    /// [`super::shots::hit_of`]).
    pub player_hits: Vec<PlayerHit>,
    /// Every shot of the demo before the round (warmup and earlier rounds), oldest first: CS2's clients keep the
    /// bullet holes they left from round to round.
    pub earlier_shots: Vec<Shot>,
    /// Where those shots' bullets met players.
    pub earlier_hits: Vec<PlayerHit>,
    /// How far the server's tick runs ahead of the demo's (`CNETMsg_Tick` minus the frame's tick; constant within a
    /// demo, [`DemoParser::server_tick`]), from the round's first tick that had both; `None` without any. CS2's game
    /// clock (`curtime`, which HLAE's camera files use) is the server tick times the tick interval.
    pub server_tick_offset: Option<i64>,
    /// Each weapon's finish by entity index, read when the weapon is first seen carried or lying in the round (read
    /// again when the entity index comes back as another item). A weapon picked up keeps its finish: CS2 shows the
    /// buyer's skin in anyone's hands.
    pub skins: BTreeMap<u32, Skin>,
    /// The StatTrak counts that changed during the round, by weapon entity index: `(tick, count)` in order, after
    /// the count its finish was read with ([`Skin::stattrak`]). The demo counts a kill on the kill's tick
    /// (YEKINDAR's M4A1-S, round 17 of Aurora vs FURIA: 1471 → 1472 at tick 155067, as MAJ3R falls).
    pub stattrak: BTreeMap<u32, Vec<(u32, u32)>>,
}

impl RoundTrack {
    /// The StatTrak count of weapon entity `entity` at `tick`, `None` without one.
    pub fn stattrak_at(&self, entity: u32, tick: u32) -> Option<u32> {
        let first = self.skins.get(&entity)?.stattrak?;
        let changes = self.stattrak.get(&entity).map_or(&[][..], Vec::as_slice);
        Some(
            changes
                .iter()
                .take_while(|&&(at, _)| at <= tick)
                .last()
                .map_or(first, |&(_, count)| count),
        )
    }
}

/// Where one round is in the demo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundInfo {
    /// Round number (1 = first round after warmup).
    pub round: u32,
    /// First tick (freeze time starts).
    pub start_tick: u32,
    /// Last tick that belongs to the round.
    pub end_tick: u32,
}

/// Errors specific to round tracking.
#[derive(Debug, thiserror::Error)]
pub enum TrackError {
    /// The demo itself could not be read.
    #[error(transparent)]
    Demo(#[from] DemoError),
    /// A class or field the tracker needs is missing (a CS2 update may have renamed it).
    #[error("demo has no {0}")]
    MissingField(String),
    /// The demo ended before the requested round started.
    #[error("round {0} not found in the demo")]
    RoundNotFound(u32),
    /// No round starts at this tick.
    #[error("no round starts at tick {0} in the demo")]
    RoundStartNotFound(u32),
}

/// Field paths the tracker reads, looked up once by name.
pub(super) struct Paths {
    pub(super) controller_class: String,
    pawn_class: String,
    pub(super) rules_class: String,
    pawn_handle: FieldPath,
    pub(super) name: FieldPath,
    pub(super) steam_id: FieldPath,
    pub(super) team: FieldPath,
    cell: [FieldPath; 3],
    offset: [FieldPath; 3],
    view_offset: [FieldPath; 3],
    eye_angles: FieldPath,
    life_state: FieldPath,
    rounds_played: FieldPath,
    freeze_period: FieldPath,
    warmup_period: FieldPath,
    game_phase: FieldPath,
    rig: RigPaths,
    appearance: AppearancePaths,
    death: DeathPaths,
    viewmodel: ViewmodelPaths,
    doors: Option<DoorPaths>,
    props: Vec<PropPaths>,
    grenades: GrenadePaths,
}

/// Field paths of a physics prop class ([`PROP_CLASSES`]) the demo has.
#[derive(Debug)]
struct PropPaths {
    class: String,
    model: FieldPath,
    cell: [FieldPath; 3],
    offset: [FieldPath; 3],
    rotation: FieldPath,
}

impl PropPaths {
    /// One per class of [`PROP_CLASSES`] the demo has.
    fn find(serializers: &Serializers) -> Vec<Self> {
        PROP_CLASSES
            .iter()
            .filter_map(|&class| {
                let prop = serializers.by_name(class)?;
                let path = |field: &str| serializers.path_of(prop, field);
                let axes = |prefix: &str| -> Option<[FieldPath; 3]> {
                    Some([
                        path(&format!("{prefix}X"))?,
                        path(&format!("{prefix}Y"))?,
                        path(&format!("{prefix}Z"))?,
                    ])
                };
                Some(Self {
                    class: class.into(),
                    model: path(MODEL_FIELD)?,
                    cell: axes("CBodyComponent.m_cell")?,
                    offset: axes("CBodyComponent.m_vec")?,
                    rotation: path("CBodyComponent.m_angRotation")?,
                })
            })
            .collect()
    }
}

/// Field paths of a pawn's viewmodel settings. Each is optional (older demos may lack them).
#[derive(Debug, Default)]
struct ViewmodelPaths {
    fov: Option<FieldPath>,
    offset: [Option<FieldPath>; 3],
    left_handed: Option<FieldPath>,
}

impl ViewmodelPaths {
    fn find(serializers: &Serializers) -> Self {
        let pawn = serializers.by_name("CCSPlayerPawn");
        let path = |field: &str| pawn.and_then(|s| serializers.path_of(s, field));
        Self {
            fov: path("m_flViewmodelFOV"),
            offset: ["X", "Y", "Z"].map(|axis| path(&format!("m_flViewmodelOffset{axis}"))),
            left_handed: path("m_bLeftHanded"),
        }
    }

    /// The pawn's settings; each one never sent (or missing from the demo) is CS2's default.
    fn read(&self, pawn: &Entity) -> ViewmodelSettings {
        let default = ViewmodelSettings::default();
        let float = |path: &Option<FieldPath>, fallback: f32| {
            path.as_ref()
                .and_then(|p| pawn.get(p))
                .and_then(FieldValue::as_f32)
                .unwrap_or(fallback)
        };
        ViewmodelSettings {
            fov: float(&self.fov, default.fov),
            offset: [0, 1, 2].map(|k| float(&self.offset[k], default.offset[k])),
            left_handed: self
                .left_handed
                .as_ref()
                .and_then(|p| pawn.get(p))
                .and_then(FieldValue::as_bool)
                .unwrap_or(default.left_handed),
        }
    }
}

/// Field paths of a rotating door (`CPropDoorRotating`), when the demo has the class.
#[derive(Debug)]
struct DoorPaths {
    class: String,
    model: FieldPath,
    closed_position: FieldPath,
    closed_angles: FieldPath,
    cell: [FieldPath; 3],
    offset: [FieldPath; 3],
    rotation: FieldPath,
    state: FieldPath,
}

impl DoorPaths {
    fn find(serializers: &Serializers) -> Option<Self> {
        let class = "CPropDoorRotating";
        let door = serializers.by_name(class)?;
        let path = |field: &str| serializers.path_of(door, field);
        let axes = |prefix: &str| -> Option<[FieldPath; 3]> {
            Some([
                path(&format!("{prefix}X"))?,
                path(&format!("{prefix}Y"))?,
                path(&format!("{prefix}Z"))?,
            ])
        };
        Some(Self {
            class: class.into(),
            model: path(MODEL_FIELD)?,
            closed_position: path("m_closedPosition")?,
            closed_angles: path("m_closedAngles")?,
            cell: axes("CBodyComponent.m_cell")?,
            offset: axes("CBodyComponent.m_vec")?,
            rotation: path("CBodyComponent.m_angRotation")?,
            state: path("m_eDoorState")?,
        })
    }
}

/// Field paths of what killed a pawn. Each is optional.
#[derive(Debug, Default)]
struct DeathPaths {
    bone: Option<FieldPath>,
    force: Option<FieldPath>,
    position: Option<FieldPath>,
    headshot: Option<FieldPath>,
    weapon: Option<FieldPath>,
}

impl DeathPaths {
    fn find(serializers: &Serializers) -> Self {
        let pawn = serializers.by_name("CCSPlayerPawn");
        let path = |field: &str| pawn.and_then(|s| serializers.path_of(s, field));
        Self {
            bone: path("m_nRagdollDamageBone"),
            force: path("m_vRagdollDamageForce"),
            position: path("m_vRagdollDamagePosition"),
            headshot: path("m_bRagdollDamageHeadshot"),
            weapon: path("m_szRagdollDamageWeaponName"),
        }
    }
}

/// The model's resource id and mesh groups, at the same place in every class with a body.
const MODEL_FIELD: &str = "CBodyComponent.m_skeletonInstance.m_modelState.m_hModel";
const MESH_GROUPS_FIELD: &str = "CBodyComponent.m_skeletonInstance.m_modelState.m_MeshGroupMask";

/// Field paths of a pawn's appearance and weapon list. Each is optional, like the rig's.
#[derive(Debug, Default)]
struct AppearancePaths {
    model: Option<FieldPath>,
    mesh_groups: Option<FieldPath>,
    gloves: Option<FieldPath>,
    /// The gloves' econ attributes (their length; each entry's index and value one level deeper).
    glove_attributes: Option<FieldPath>,
    /// The weapon handle array (its length; the handles one level deeper).
    weapons: Option<FieldPath>,
    /// The ammo counts by ammo type (a fixed array: element `i` one level deeper).
    ammo: Option<FieldPath>,
}

impl AppearancePaths {
    fn find(serializers: &Serializers) -> Self {
        let pawn = serializers.by_name("CCSPlayerPawn");
        let path = |field: &str| pawn.and_then(|s| serializers.path_of(s, field));
        Self {
            model: path(MODEL_FIELD),
            mesh_groups: path(MESH_GROUPS_FIELD),
            gloves: path("m_EconGloves.m_iItemDefinitionIndex"),
            glove_attributes: path("m_EconGloves.m_NetworkedDynamicAttributes.m_Attributes"),
            weapons: path("m_pWeaponServices.m_hMyWeapons"),
            ammo: path("m_pWeaponServices.m_iAmmo"),
        }
    }
}

/// Field paths of the rig inputs. Each is optional: a missing one reads as its default.
#[derive(Debug, Default)]
struct RigPaths {
    body_rotation: Option<FieldPath>,
    velocity: Option<[FieldPath; 3]>,
    flags: Option<FieldPath>,
    move_type: Option<FieldPath>,
    walking: Option<FieldPath>,
    scoped: Option<FieldPath>,
    defusing: Option<FieldPath>,
    duck_amount: Option<FieldPath>,
    last_jump: Option<FieldPath>,
    last_landing: Option<FieldPath>,
    active_weapon: Option<FieldPath>,
    flash_duration: Option<FieldPath>,
    flash_max_alpha: Option<FieldPath>,
    view_punch: Option<[FieldPath; 3]>,
    recipe: Option<super::recipe::RecipePaths>,
    /// Per weapon class id: every class with an item definition index (each weapon has its own serializer, so the
    /// same field sits at a different path in each).
    weapons: HashMap<u32, WeaponPaths>,
}

/// Field paths in one weapon class.
#[derive(Debug)]
struct WeaponPaths {
    item: FieldPath,
    anim_state: Option<FieldPath>,
    state_time: Option<FieldPath>,
    reloading: Option<FieldPath>,
    throw_strength: Option<FieldPath>,
    clip: Option<FieldPath>,
    /// The reserve ammunition array (element 0 one level deeper).
    reserve: Option<FieldPath>,
    silenced: Option<FieldPath>,
    model: Option<FieldPath>,
    mesh_groups: Option<FieldPath>,
    /// Who carries it (`m_hOwnerEntity`) and what it is attached to (`CBodyComponent.m_hParent`).
    owner: Option<FieldPath>,
    parent: Option<FieldPath>,
    /// Where it lies when nobody does: the body component's cell, offset and rotation.
    cell: Option<[FieldPath; 3]>,
    offset: Option<[FieldPath; 3]>,
    rotation: Option<FieldPath>,
    /// Its finish ([`super::skins`]): the econ attribute list and the name tag.
    attributes: Option<FieldPath>,
    name_tag: Option<FieldPath>,
}

impl RigPaths {
    fn find(serializers: &Serializers, classes: &[Option<ServerClass>]) -> Self {
        let pawn = serializers.by_name("CCSPlayerPawn");
        let path = |field: &str| pawn.and_then(|s| serializers.path_of(s, field));
        let velocity = ["X", "Y", "Z"].map(|axis| path(&format!("m_vecVelocity.m_vec{axis}")));
        let weapons = classes
            .iter()
            .flatten()
            .filter_map(|class| {
                let field = |name: &str| serializers.path_of(class.serializer, name);
                let axes = |prefix: &str| -> Option<[FieldPath; 3]> {
                    Some([
                        field(&format!("{prefix}X"))?,
                        field(&format!("{prefix}Y"))?,
                        field(&format!("{prefix}Z"))?,
                    ])
                };
                let paths = WeaponPaths {
                    item: field("m_AttributeManager.m_Item.m_iItemDefinitionIndex")?,
                    anim_state: field("m_iWeaponGameplayAnimState"),
                    state_time: field("m_flWeaponGameplayAnimStateTimestamp"),
                    reloading: field("m_bInReload"),
                    throw_strength: field("m_flThrowStrength"),
                    clip: field("m_iClip1"),
                    reserve: field("m_pReserveAmmo"),
                    silenced: field("m_bSilencerOn"),
                    model: field(MODEL_FIELD),
                    mesh_groups: field(MESH_GROUPS_FIELD),
                    owner: field("m_hOwnerEntity"),
                    parent: field("CBodyComponent.m_hParent"),
                    cell: axes("CBodyComponent.m_cell"),
                    offset: axes("CBodyComponent.m_vec"),
                    rotation: field("CBodyComponent.m_angRotation"),
                    attributes: field(
                        "m_AttributeManager.m_Item.m_NetworkedDynamicAttributes.m_Attributes",
                    ),
                    name_tag: field("m_AttributeManager.m_Item.m_szCustomName"),
                };
                Some((class.id, paths))
            })
            .collect();
        Self {
            body_rotation: path("CBodyComponent.m_angRotation"),
            velocity: match velocity {
                [Some(x), Some(y), Some(z)] => Some([x, y, z]),
                _ => None,
            },
            flags: path("m_fFlags"),
            move_type: path("m_MoveType"),
            walking: path("m_bIsWalking"),
            scoped: path("m_bIsScoped"),
            defusing: path("m_bIsDefusing"),
            duck_amount: path("m_pMovementServices.m_flDuckAmount"),
            last_jump: path("m_pMovementServices.m_nLastJumpTick"),
            last_landing: path("m_pMovementServices.m_ModernJump.m_nLastLandedTick"),
            active_weapon: path("m_pWeaponServices.m_hActiveWeapon"),
            flash_duration: path("m_flFlashDuration"),
            flash_max_alpha: path("m_flFlashMaxAlpha"),
            view_punch: (|| {
                Some([
                    path("m_pCameraServices.m_vecCsViewPunchAngle")?,
                    path("m_pCameraServices.m_nCsViewPunchAngleTick")?,
                    path("m_pCameraServices.m_flCsViewPunchAngleTickRatio")?,
                ])
            })(),
            recipe: super::recipe::RecipePaths::find(serializers),
            weapons,
        }
    }
}

impl Paths {
    fn find(
        serializers: &Serializers,
        classes: &[Option<ServerClass>],
    ) -> Result<Self, TrackError> {
        let path = |class: &str, field: &str| {
            serializers
                .by_name(class)
                .and_then(|s| serializers.path_of(s, field))
                .ok_or_else(|| TrackError::MissingField(format!("{class}.{field}")))
        };
        let controller = "CCSPlayerController";
        let pawn = "CCSPlayerPawn";
        let rules = "CCSGameRulesProxy";
        // The x, y and z members of a vector field on the pawn, e.g. `CBodyComponent.m_cellX` / `Y` / `Z`.
        let axes = |prefix: &str| -> Result<[FieldPath; 3], TrackError> {
            Ok([
                path(pawn, &format!("{prefix}X"))?,
                path(pawn, &format!("{prefix}Y"))?,
                path(pawn, &format!("{prefix}Z"))?,
            ])
        };
        Ok(Self {
            controller_class: controller.into(),
            pawn_class: pawn.into(),
            rules_class: rules.into(),
            pawn_handle: path(controller, "m_hPlayerPawn")?,
            name: path(controller, "m_iszPlayerName")?,
            steam_id: path(controller, "m_steamID")?,
            team: path(controller, "m_iTeamNum")?,
            cell: axes("CBodyComponent.m_cell")?,
            offset: axes("CBodyComponent.m_vec")?,
            // The full name is needed: the pawn also has `m_vecVelocity.m_vecX` etc. (see `Serializers::path_of`).
            view_offset: axes("m_vecViewOffset.m_vec")?,
            eye_angles: path(pawn, "m_angEyeAngles")?,
            life_state: path(pawn, "m_lifeState")?,
            rounds_played: path(rules, "m_pGameRules.m_totalRoundsPlayed")?,
            freeze_period: path(rules, "m_pGameRules.m_bFreezePeriod")?,
            warmup_period: path(rules, "m_pGameRules.m_bWarmupPeriod")?,
            game_phase: path(rules, "m_pGameRules.m_gamePhase")?,
            rig: RigPaths::find(serializers, classes),
            appearance: AppearancePaths::find(serializers),
            death: DeathPaths::find(serializers),
            viewmodel: ViewmodelPaths::find(serializers),
            doors: DoorPaths::find(serializers),
            props: PropPaths::find(serializers),
            grenades: GrenadePaths::find(serializers, classes),
        })
    }
}

/// `m_gamePhase` value once the match is over (Valve's `GAMEPHASE_MATCH_ENDED`).
const GAME_PHASE_MATCH_ENDED: i64 = 5;

/// Round state read from the game rules entity.
#[derive(Debug, Clone, Copy, Default)]
struct RulesState {
    rounds_played: u32,
    freeze: bool,
    warmup: bool,
    match_ended: bool,
}

/// What happened at a tick, according to [`RoundDetector`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoundEvent {
    /// A round ended on the previous sampled tick (its info is complete).
    Ended(RoundInfo),
    /// A round started at this tick (`Ended` for the previous round comes first if one was running).
    Started(u32),
}

/// Turns the game rules state of successive ticks into round start / end events.
#[derive(Debug, Default)]
struct RoundDetector {
    was_freeze: bool,
    current: Option<RoundInfo>,
}

impl RoundDetector {
    /// Feeds the state of one tick; returns up to two events (an end, then a start).
    fn update(&mut self, tick: u32, rules: RulesState) -> Vec<RoundEvent> {
        let mut events = Vec::new();
        let starts = rules.freeze && !self.was_freeze && !rules.warmup && !rules.match_ended;
        self.was_freeze = rules.freeze;
        // A return to warmup ends a round too (some servers warm up again after a knife round): its respawning
        // deathmatch is no round.
        if (starts || rules.match_ended || rules.warmup)
            && let Some(mut done) = self.current.take()
        {
            // The match ends on the tick its last round is decided: that tick (the final kill) is the round's.
            if rules.match_ended && !starts {
                done.end_tick = tick;
            }
            events.push(RoundEvent::Ended(done));
        }
        if starts {
            let round = rules.rounds_played + 1;
            self.current = Some(RoundInfo {
                round,
                start_tick: tick,
                end_tick: tick,
            });
            events.push(RoundEvent::Started(round));
        } else if let Some(current) = self.current.as_mut() {
            current.end_tick = tick;
        }
        events
    }

    /// The round still running when the demo ended.
    fn finish(self) -> Option<RoundInfo> {
        self.current
    }
}

/// What [`walk_rounds`] hands its callback after every tick that changed entities.
pub(super) struct TickView<'p, 'd> {
    pub(super) parser: &'p DemoParser<'d>,
    pub(super) paths: &'p Paths,
    pub(super) tick: u32,
    /// Round starts / ends detected at this tick.
    pub(super) rounds: &'p [RoundEvent],
    /// The round running now, if any.
    pub(super) current: Option<RoundInfo>,
    /// Game events read since the previous call (they can be from earlier ticks without entity changes).
    pub(super) events: &'p [GameEvent],
    /// The shot and effect messages read since the previous call ([`shots::FIRE_BULLETS`],
    /// [`shots::EFFECT_DISPATCH`]), likewise.
    messages: &'p [RawMessage],
}

/// Calls `on_tick` after every tick that changed entities; it returns `false` to stop reading.
pub(super) fn walk_rounds(
    data: &[u8],
    mut on_tick: impl FnMut(TickView<'_, '_>) -> bool,
) -> Result<Option<RoundInfo>, TrackError> {
    let mut parser = DemoParser::new(data)?;
    parser.keep_messages(&[shots::FIRE_BULLETS, shots::EFFECT_DISPATCH]);
    let mut paths: Option<Paths> = None;
    let mut detector = RoundDetector::default();
    let mut events = Vec::new();
    let mut messages = Vec::new();
    while let Some(frame) = parser.next_frame()? {
        events.extend(parser.take_events());
        messages.extend(parser.take_messages());
        let (Some(tick), true) = (frame.tick, frame.entities_changed) else {
            continue;
        };
        if paths.is_none() {
            let serializers = parser.serializers().ok_or(DemoError::MissingSendTables)?;
            paths = Some(Paths::find(serializers, parser.classes())?);
        }
        let paths = paths.as_ref().expect("set above");
        let Some(rules) = read_rules(&parser, paths) else {
            continue;
        };
        let rounds = detector.update(tick, rules);
        let go_on = on_tick(TickView {
            parser: &parser,
            paths,
            tick,
            rounds: &rounds,
            current: detector.current,
            events: &events,
            messages: &messages,
        });
        events.clear();
        messages.clear();
        if !go_on {
            return Ok(None);
        }
    }
    Ok(detector.finish())
}

/// Lists every round in the demo with its tick range (reads the whole demo).
pub fn list_rounds(data: &[u8]) -> Result<Vec<RoundInfo>, TrackError> {
    let mut rounds = Vec::new();
    let last = walk_rounds(data, |view| {
        rounds.extend(view.rounds.iter().filter_map(|e| match e {
            RoundEvent::Ended(info) => Some(*info),
            RoundEvent::Started(_) => None,
        }));
        true
    })?;
    rounds.extend(last);
    Ok(rounds)
}

/// Reads the demo up to the end of `round` and returns every player's position, view offset and eye angles for each
/// tick of it, and the round's game events. A restart or a restored backup plays a round number again: this is the
/// first round with the number ([`track_round_at`] picks a round by its start).
pub fn track_round(data: &[u8], round: u32) -> Result<RoundTrack, TrackError> {
    track_round_where(data, |r| r.round == round)?.ok_or(TrackError::RoundNotFound(round))
}

/// [`track_round`] for the round that starts at `start_tick` ([`RoundInfo::start_tick`], as [`list_rounds`] and the
/// demo summary give it).
pub fn track_round_at(data: &[u8], start_tick: u32) -> Result<RoundTrack, TrackError> {
    track_round_where(data, |r| r.start_tick == start_tick)?
        .ok_or(TrackError::RoundStartNotFound(start_tick))
}

/// [`track_round`] for the first round `wanted` picks; `None` when the demo ends before one.
fn track_round_where(
    data: &[u8],
    wanted: impl Fn(&RoundInfo) -> bool,
) -> Result<Option<RoundTrack>, TrackError> {
    let mut track: Option<RoundTrack> = None;
    // Each smoke's voxel update counter when its log was last read.
    let mut read_at: HashMap<(u32, u32), i64> = HashMap::new();
    // The shot and effect messages read before the round starts.
    let mut earlier: Vec<RawMessage> = Vec::new();
    walk_rounds(data, |view| {
        let TickView {
            parser,
            paths,
            tick,
            rounds,
            current,
            events,
            messages,
        } = view;
        let ended = rounds.iter().find_map(|e| match e {
            RoundEvent::Ended(info) if wanted(info) => Some(*info),
            _ => None,
        });
        // The match ends on the tick its last round is decided, and no round starts then: that tick is the round's
        // last sample (the final kill), read below.
        let match_end = ended.is_some()
            && track.is_some()
            && !rounds.iter().any(|e| matches!(e, RoundEvent::Started(_)));
        if let Some(done) = ended
            && !match_end
        {
            // Events read up to now can still belong to the round's last ticks.
            if let Some(track) = track.as_mut() {
                track
                    .events
                    .extend(events.iter().filter(|e| e.tick <= done.end_tick).cloned());
                let late: Vec<RawMessage> = messages
                    .iter()
                    .filter(|m| m.tick <= done.end_tick)
                    .cloned()
                    .collect();
                add_shots(track, &late, parser, paths);
            }
            return false; // our round is complete
        }
        // Before the round: every shot and hit, for the bullet holes they left (CS2 keeps them from round to round).
        if track.is_none() {
            earlier.extend(messages.iter().cloned());
        }
        if let Some(info) = current.filter(&wanted).or(ended.filter(|_| match_end)) {
            if track.is_none() {
                let mut new = RoundTrack {
                    round: info.round,
                    start_tick: tick,
                    end_tick: tick,
                    tick_interval: parser.tick_interval().unwrap_or(1.0 / 64.0),
                    map_name: parser.header().map_name().to_owned(),
                    samples: Vec::new(),
                    events: Vec::new(),
                    smoke_logs: Vec::new(),
                    shots: Vec::new(),
                    player_hits: Vec::new(),
                    earlier_shots: Vec::new(),
                    earlier_hits: Vec::new(),
                    server_tick_offset: None,
                    skins: BTreeMap::new(),
                    stattrak: BTreeMap::new(),
                };
                let effects = parser.string_tables().by_name(shots::EFFECT_TABLE);
                for m in earlier.drain(..).filter(|m| m.tick < tick) {
                    if m.kind == shots::FIRE_BULLETS {
                        new.earlier_shots
                            .extend(shots::decode_fire_bullets(m.tick, &m.body));
                    } else if m.kind == shots::EFFECT_DISPATCH {
                        new.earlier_hits
                            .extend(shots::decode_player_hit(m.tick, &m.body, effects));
                    }
                }
                track = Some(new);
            }
            let track = track.as_mut().expect("set above");
            if track.server_tick_offset.is_none() {
                track.server_tick_offset = parser
                    .server_tick()
                    .zip(parser.tick())
                    .map(|(server, demo)| i64::from(server) - i64::from(demo));
            }
            let start = track.start_tick;
            track
                .events
                .extend(events.iter().filter(|e| e.tick >= start).cloned());
            let current: Vec<RawMessage> = messages
                .iter()
                .filter(|m| m.tick >= start)
                .cloned()
                .collect();
            add_shots(track, &current, parser, paths);
            let sample = TickSample {
                tick,
                players: read_players(parser, paths),
                dropped: read_dropped(parser, paths),
                doors: read_doors(parser, paths),
                props: read_props(parser, paths),
                grenades: paths
                    .grenades
                    .grenades(parser, &pawn_controllers(parser, paths)),
                fires: paths.grenades.fires(parser),
            };
            let weapons = sample.players.iter().flat_map(|p| &p.weapons);
            let seen = weapons
                .map(|w| (w.entity, w.item))
                .chain(sample.dropped.iter().map(|w| (w.entity, w.item)));
            for (entity, item) in seen {
                match track.skins.get(&entity).filter(|skin| skin.item == item) {
                    None => {
                        if let Some(skin) = read_skin(parser, paths, entity) {
                            track.skins.insert(entity, skin);
                            track.stattrak.remove(&entity);
                        }
                    }
                    // A StatTrak weapon's count, read again: CS2 counts a kill on its tick.
                    Some(skin) => {
                        if let Some(first) = skin.stattrak {
                            let last = track
                                .stattrak
                                .get(&entity)
                                .and_then(|counts| counts.last())
                                .map_or(first, |&(_, count)| count);
                            if let Some(now) = read_skin(parser, paths, entity)
                                .and_then(|s| s.stattrak)
                                .filter(|&now| now != last)
                            {
                                track
                                    .stattrak
                                    .entry(entity)
                                    .or_default()
                                    .push((sample.tick, now));
                            }
                        }
                    }
                }
            }
            // A smoke's log, read again only when its update counter moved and kept when its bytes changed.
            for g in &sample.grenades {
                let Some(cloud) = g.smoke else { continue };
                let index = match track
                    .smoke_logs
                    .iter()
                    .position(|l| l.entity == g.entity && l.serial == g.serial)
                {
                    Some(i) => i,
                    None => {
                        track.smoke_logs.push(SmokeLog {
                            entity: g.entity,
                            serial: g.serial,
                            snapshots: Vec::new(),
                        });
                        track.smoke_logs.len() - 1
                    }
                };
                let log = &mut track.smoke_logs[index];
                if read_at.insert((g.entity, g.serial), cloud.voxel_update)
                    == Some(cloud.voxel_update)
                {
                    continue;
                }
                if let Some(bytes) = paths.grenades.voxel_log(parser, g.entity)
                    && log.snapshots.last().is_none_or(|(_, b)| *b != bytes)
                {
                    log.snapshots.push((tick, bytes));
                }
            }
            // Several frames can share a tick; the last state of the tick wins.
            match track.samples.last_mut() {
                Some(last) if last.tick == tick => *last = sample,
                _ => track.samples.push(sample),
            }
            track.end_tick = tick;
        }
        !match_end
    })?;
    Ok(track)
}

/// Adds the shots and player hits among `messages` to `track`; a shot's pawn is turned into its controller with the
/// pawns' controllers at this tick (looked up only when there is a shot; a shot whose pawn has no controller is left
/// out).
fn add_shots(
    track: &mut RoundTrack,
    messages: &[RawMessage],
    parser: &DemoParser<'_>,
    paths: &Paths,
) {
    let controllers = if messages.iter().any(|m| m.kind == shots::FIRE_BULLETS) {
        pawn_controllers(parser, paths)
    } else {
        HashMap::new()
    };
    let effects = parser.string_tables().by_name(shots::EFFECT_TABLE);
    for m in messages {
        if m.kind == shots::FIRE_BULLETS {
            if let Some(shot) = shots::decode_fire_bullets(m.tick, &m.body)
                && let Some(&controller) = controllers.get(&shot.pawn)
            {
                track.shots.push((controller, shot));
            }
        } else if m.kind == shots::EFFECT_DISPATCH
            && let Some(hit) = shots::decode_player_hit(m.tick, &m.body, effects)
        {
            track.player_hits.push(hit);
        }
    }
}

fn read_rules(parser: &DemoParser<'_>, paths: &Paths) -> Option<RulesState> {
    let rules = parser
        .entities()
        .iter()
        .find(|e| parser.class_name(e.class_id) == Some(paths.rules_class.as_str()))?;
    Some(RulesState {
        rounds_played: rules
            .get(&paths.rounds_played)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0) as u32,
        freeze: rules
            .get(&paths.freeze_period)
            .and_then(FieldValue::as_bool)
            .unwrap_or(false),
        warmup: rules
            .get(&paths.warmup_period)
            .and_then(FieldValue::as_bool)
            .unwrap_or(false),
        match_ended: rules.get(&paths.game_phase).and_then(FieldValue::as_i64)
            == Some(GAME_PHASE_MATCH_ENDED),
    })
}

fn read_players(parser: &DemoParser<'_>, paths: &Paths) -> Vec<PlayerSample> {
    let entities = parser.entities();
    let mut players = Vec::new();
    for controller in entities.iter() {
        if parser.class_name(controller.class_id) != Some(paths.controller_class.as_str()) {
            continue;
        }
        // Only the two teams play. Tournament demos also carry spectators (team 1: the observer, casters): their
        // controller points at a pawn that is never alive and sits at the world origin.
        let team = controller
            .get(&paths.team)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0) as u32;
        if !matches!(team, TERRORISTS | COUNTER_TERRORISTS) {
            continue;
        }
        let Some(pawn) = controller
            .get(&paths.pawn_handle)
            .and_then(FieldValue::as_u64)
            .and_then(handle_to_index)
            .and_then(|i| entities.get(i))
            .filter(|p| parser.class_name(p.class_id) == Some(paths.pawn_class.as_str()))
        else {
            continue;
        };
        let Some(position) = position(pawn, paths) else {
            continue;
        };
        // A field the server never sent still holds its C++ default, which is 0 for a vector.
        let view_offset = paths
            .view_offset
            .each_ref()
            .map(|p| pawn.get(p).and_then(FieldValue::as_f32).unwrap_or(0.0));
        let angles = pawn
            .get(&paths.eye_angles)
            .and_then(FieldValue::as_vector3)
            .unwrap_or_default();
        let alive = pawn
            .get(&paths.life_state)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0)
            == 0;
        players.push(PlayerSample {
            controller: controller.index,
            name: controller
                .get(&paths.name)
                .and_then(FieldValue::as_str)
                .unwrap_or_default()
                .to_owned(),
            steam_id: controller
                .get(&paths.steam_id)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0),
            team,
            position,
            view_offset,
            pitch: angles[0],
            yaw: angles[1],
            alive,
            rig: read_rig(parser, &paths.rig, pawn),
            appearance: read_appearance(&paths.appearance, pawn),
            weapons: read_weapons(parser, paths, pawn),
            death: (!alive).then(|| read_death(&paths.death, pawn)),
            viewmodel: paths.viewmodel.read(pawn),
        });
    }
    players
}

/// What killed the pawn (its ragdoll damage fields; defaults where not sent).
fn read_death(paths: &DeathPaths, pawn: &Entity) -> Death {
    let get = |path: &Option<FieldPath>| path.as_ref().and_then(|p| pawn.get(p));
    let vector = |path: &Option<FieldPath>| {
        get(path)
            .and_then(FieldValue::as_vector3)
            .unwrap_or_default()
    };
    Death {
        bone: get(&paths.bone)
            .and_then(FieldValue::as_i64)
            .and_then(|b| u32::try_from(b).ok()),
        force: vector(&paths.force),
        position: vector(&paths.position),
        headshot: get(&paths.headshot)
            .and_then(FieldValue::as_bool)
            .unwrap_or(false),
        weapon: get(&paths.weapon)
            .and_then(FieldValue::as_str)
            .unwrap_or_default()
            .to_owned(),
    }
}

/// The pawn's model, mesh groups and gloves (0 where not sent).
fn read_appearance(paths: &AppearancePaths, pawn: &Entity) -> Appearance {
    let number = |path: &Option<FieldPath>| {
        path.as_ref()
            .and_then(|p| pawn.get(p))
            .and_then(FieldValue::as_u64)
            .unwrap_or(0)
    };
    let gloves = number(&paths.gloves) as u32;
    let finish = paths
        .glove_attributes
        .as_ref()
        .map(|list| skin_from_attributes(gloves, &read_attributes(pawn, list), ""))
        .filter(|skin| skin.paint_kit != 0)
        .map_or([0; 3], |skin| {
            [skin.paint_kit, skin.seed, skin.wear.to_bits()]
        });
    Appearance {
        model: number(&paths.model),
        mesh_groups: number(&paths.mesh_groups),
        gloves,
        glove_finish: finish,
    }
}

/// An econ item's attributes from its `m_Attributes` list at `list` (definition index, raw 32-bit value: a float
/// field holds the bits as they were sent).
fn read_attributes(entity: &Entity, list: &FieldPath) -> Vec<(u32, u32)> {
    let count = entity.get(list).and_then(FieldValue::as_u64).unwrap_or(0);
    let mut attributes = Vec::new();
    for k in 0..count.min(64) as i32 {
        let entry = |member: i32| list.child(k)?.child(member);
        let index = entry(0).and_then(|p| entity.get(&p)?.as_u64());
        let bits = entry(1).and_then(|p| match entity.get(&p)? {
            FieldValue::Float(f) => Some(f.to_bits()),
            value => value.as_u64().map(|v| v as u32),
        });
        if let (Some(index), Some(bits)) = (index, bits) {
            attributes.push((index as u32, bits));
        }
    }
    attributes
}

/// The weapons in the pawn's `m_hMyWeapons` whose entities exist and have an item definition index.
fn read_weapons(parser: &DemoParser<'_>, paths: &Paths, pawn: &Entity) -> Vec<CarriedWeapon> {
    let Some(array) = &paths.appearance.weapons else {
        return Vec::new();
    };
    let count = pawn.get(array).and_then(FieldValue::as_u64).unwrap_or(0);
    let flashbangs = paths
        .appearance
        .ammo
        .as_ref()
        .and_then(|p| p.child(FLASHBANG_AMMO))
        .and_then(|p| pawn.get(&p))
        .and_then(FieldValue::as_u64)
        .unwrap_or(1)
        .clamp(1, 2) as u32;
    let entities = parser.entities();
    (0..count.min(64) as i32)
        .filter_map(|k| {
            let handle = pawn.get(&array.child(k)?)?.as_u64()?;
            let weapon = entities.get(handle_to_index(handle)?)?;
            let fields = paths.rig.weapons.get(&weapon.class_id)?;
            let number = |path: &Option<FieldPath>| {
                path.as_ref()
                    .and_then(|p| weapon.get(p))
                    .and_then(FieldValue::as_u64)
                    .unwrap_or(0)
            };
            let item = weapon.get(&fields.item)?.as_u64()? as u32;
            Some(CarriedWeapon {
                entity: weapon.index,
                item,
                model: number(&fields.model),
                mesh_groups: number(&fields.mesh_groups),
                count: if item == FLASHBANG_ITEM {
                    flashbangs
                } else {
                    1
                },
            })
        })
        .collect()
}

/// The finish of the weapon entity `entity` ([`super::skins`]): its econ attributes (definition index, raw 32-bit
/// value: a float field holds the bits as they were sent) and name tag. `None` when the entity or its class's item
/// field is missing.
fn read_skin(parser: &DemoParser<'_>, paths: &Paths, entity: u32) -> Option<Skin> {
    let weapon = parser.entities().get(entity)?;
    let fields = paths.rig.weapons.get(&weapon.class_id)?;
    let item = weapon.get(&fields.item)?.as_u64()? as u32;
    let attributes = fields
        .attributes
        .as_ref()
        .map(|list| read_attributes(weapon, list))
        .unwrap_or_default();
    let name_tag = fields
        .name_tag
        .as_ref()
        .and_then(|p| weapon.get(p)?.as_str())
        .unwrap_or("");
    Some(skin_from_attributes(item, &attributes, name_tag))
}

/// The weapons nobody carries: weapon entities whose owner and parent are both the invalid handle (or were never
/// sent), with their origin and rotation. A class whose owner and parent fields both cannot be found (renamed by a
/// CS2 update) is skipped: every weapon of it would otherwise count as dropped.
fn read_dropped(parser: &DemoParser<'_>, paths: &Paths) -> Vec<DroppedWeapon> {
    let mut out = Vec::new();
    for weapon in parser.entities().iter() {
        let Some(fields) = paths.rig.weapons.get(&weapon.class_id) else {
            continue;
        };
        if fields.owner.is_none() && fields.parent.is_none() {
            continue;
        }
        let handle = |path: &Option<FieldPath>| {
            path.as_ref()
                .and_then(|p| weapon.get(p))
                .and_then(FieldValue::as_u64)
                .and_then(handle_to_index)
        };
        if handle(&fields.owner).is_some() || handle(&fields.parent).is_some() {
            continue;
        }
        let (Some(cell), Some(offset), Some(item)) = (
            &fields.cell,
            &fields.offset,
            weapon.get(&fields.item).and_then(FieldValue::as_u64),
        ) else {
            continue;
        };
        // The cell must have been sent; an offset never sent is its default, 0. Exactly the world origin is what a
        // carried weapon sends (it follows its owner's hand): one that lost its owner for a moment (a grenade being
        // thrown, the C4 being planted) has not been placed yet.
        let Some(position) = origin(weapon, cell, offset).filter(|p| *p != [0.0; 3]) else {
            continue;
        };
        let number = |path: &Option<FieldPath>| {
            path.as_ref()
                .and_then(|p| weapon.get(p))
                .and_then(FieldValue::as_u64)
                .unwrap_or(0)
        };
        out.push(DroppedWeapon {
            entity: weapon.index,
            item: item as u32,
            class: parser
                .class_name(weapon.class_id)
                .unwrap_or_default()
                .to_owned(),
            model: number(&fields.model),
            mesh_groups: number(&fields.mesh_groups),
            position,
            angles: fields
                .rotation
                .as_ref()
                .and_then(|p| weapon.get(p))
                .and_then(FieldValue::as_vector3)
                .unwrap_or_default(),
        });
    }
    out
}

/// Each pawn's controller: pawn entity index → controller entity index (`m_hPlayerPawn`).
fn pawn_controllers(parser: &DemoParser<'_>, paths: &Paths) -> HashMap<u32, u32> {
    parser
        .entities()
        .iter()
        .filter(|c| parser.class_name(c.class_id) == Some(paths.controller_class.as_str()))
        .filter_map(|c| {
            let pawn = c
                .get(&paths.pawn_handle)
                .and_then(FieldValue::as_u64)
                .and_then(handle_to_index)?;
            Some((pawn, c.index))
        })
        .collect()
}

/// The physics props (a prop without its origin cell is skipped).
fn read_props(parser: &DemoParser<'_>, paths: &Paths) -> Vec<PropSample> {
    let mut out = Vec::new();
    for prop in parser.entities().iter() {
        let class = parser.class_name(prop.class_id);
        let Some(fields) = paths.props.iter().find(|p| Some(p.class.as_str()) == class) else {
            continue;
        };
        let Some(position) = origin(prop, &fields.cell, &fields.offset) else {
            continue;
        };
        let model = prop
            .get(&fields.model)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0);
        let angles = prop
            .get(&fields.rotation)
            .and_then(FieldValue::as_vector3)
            .unwrap_or_default();
        out.push(PropSample {
            entity: prop.index,
            serial: prop.serial,
            model,
            position,
            angles,
        });
    }
    out
}

/// The rotating doors (fields never sent read as their defaults: 0; a door without its origin cell is skipped).
fn read_doors(parser: &DemoParser<'_>, paths: &Paths) -> Vec<DoorSample> {
    let Some(fields) = &paths.doors else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for door in parser.entities().iter() {
        if parser.class_name(door.class_id) != Some(fields.class.as_str()) {
            continue;
        }
        let vector = |path: &FieldPath| {
            door.get(path)
                .and_then(FieldValue::as_vector3)
                .unwrap_or_default()
        };
        let Some(position) = origin(door, &fields.cell, &fields.offset) else {
            continue;
        };
        out.push(DoorSample {
            entity: door.index,
            model: door
                .get(&fields.model)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0),
            closed_position: vector(&fields.closed_position),
            closed_angles: vector(&fields.closed_angles),
            position,
            angles: vector(&fields.rotation),
            state: door
                .get(&fields.state)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0) as u32,
        });
    }
    out
}

/// The rig inputs of one pawn (fields the server never sent read as their defaults: 0, false, none).
fn read_rig(parser: &DemoParser<'_>, paths: &RigPaths, pawn: &Entity) -> RigInputs {
    let get = |path: &Option<FieldPath>| path.as_ref().and_then(|p| pawn.get(p));
    let flag = |path: &Option<FieldPath>| get(path).and_then(FieldValue::as_bool).unwrap_or(false);
    let number = |path: &Option<FieldPath>| get(path).and_then(FieldValue::as_u64).unwrap_or(0);
    // A game tick of 0 means "never".
    let demo_tick = |path: &Option<FieldPath>| {
        get(path)
            .and_then(FieldValue::as_u64)
            .filter(|&t| t > 0)
            .and_then(|t| u32::try_from(t).ok())
            .and_then(|t| parser.game_tick_to_demo(t))
    };
    let entities = parser.entities();
    let weapon = get(&paths.active_weapon)
        .and_then(FieldValue::as_u64)
        .and_then(handle_to_index)
        .and_then(|i| entities.get(i))
        .and_then(|w| {
            let fields = paths.weapons.get(&w.class_id)?;
            let value = |path: &Option<FieldPath>| path.as_ref().and_then(|p| w.get(p));
            // Game seconds → game tick → demo tick (0 means never).
            let state_tick = value(&fields.state_time)
                .and_then(FieldValue::as_f32)
                .filter(|&t| t > 0.0)
                .zip(parser.tick_interval())
                .and_then(|(t, interval)| u32::try_from((t / interval).round() as i64).ok())
                .and_then(|t| parser.game_tick_to_demo(t));
            Some(ActiveWeapon {
                entity: w.index,
                item: w.get(&fields.item)?.as_u64()? as u32,
                class: parser.class_name(w.class_id)?.to_owned(),
                anim_state: value(&fields.anim_state)
                    .and_then(FieldValue::as_i64)
                    .unwrap_or(0),
                state_tick,
                reloading: value(&fields.reloading)
                    .and_then(FieldValue::as_bool)
                    .unwrap_or(false),
                throw_strength: value(&fields.throw_strength)
                    .and_then(FieldValue::as_f32)
                    .unwrap_or(0.0),
                clip: value(&fields.clip)
                    .and_then(FieldValue::as_i64)
                    .and_then(|c| u32::try_from(c).ok()),
                reserve: fields
                    .reserve
                    .as_ref()
                    .and_then(|p| p.child(0))
                    .and_then(|p| w.get(&p))
                    .and_then(FieldValue::as_i64)
                    .and_then(|c| u32::try_from(c).ok()),
                silenced: value(&fields.silenced)
                    .and_then(FieldValue::as_bool)
                    .unwrap_or(false),
            })
        });
    RigInputs {
        body_yaw: get(&paths.body_rotation)
            .and_then(FieldValue::as_vector3)
            .map_or(0.0, |angles| angles[1]),
        velocity: paths
            .velocity
            .as_ref()
            .map(|axes| {
                axes.each_ref()
                    .map(|p| pawn.get(p).and_then(FieldValue::as_f32).unwrap_or(0.0))
            })
            .unwrap_or_default(),
        on_ground: number(&paths.flags) & FL_ONGROUND != 0,
        duck_amount: get(&paths.duck_amount)
            .and_then(FieldValue::as_f32)
            .unwrap_or(0.0),
        walking: flag(&paths.walking),
        scoped: flag(&paths.scoped),
        defusing: flag(&paths.defusing),
        on_ladder: number(&paths.move_type) == MOVETYPE_LADDER,
        last_jump_tick: demo_tick(&paths.last_jump),
        last_landing_tick: demo_tick(&paths.last_landing),
        weapon,
        flash_duration: get(&paths.flash_duration)
            .and_then(FieldValue::as_f32)
            .unwrap_or(0.0),
        flash_max_alpha: get(&paths.flash_max_alpha)
            .and_then(FieldValue::as_f32)
            .unwrap_or(0.0),
        view_punch: paths.view_punch.as_ref().and_then(|[angle, tick, ratio]| {
            let angle = pawn.get(angle)?.as_vector3()?;
            let tick = pawn.get(tick)?.as_i64().filter(|&t| t > 0)?;
            let tick = parser.game_tick_to_demo(u32::try_from(tick).ok()?)?;
            let ratio = pawn.get(ratio).and_then(FieldValue::as_f32).unwrap_or(0.0);
            Some(([angle[0], angle[1]], f64::from(tick) + f64::from(ratio)))
        }),
        pose_recipe: paths
            .recipe
            .as_ref()
            .and_then(|r| super::recipe::read(pawn, r)),
    }
}

/// `cell × 512 − 16384 + offset` for each axis.
fn position(pawn: &Entity, paths: &Paths) -> Option<[f32; 3]> {
    let mut out = [0.0; 3];
    for ((value, cell), offset) in out.iter_mut().zip(&paths.cell).zip(&paths.offset) {
        let cell = pawn.get(cell)?.as_u64()? as f32;
        let offset = pawn.get(offset)?.as_f32()?;
        *value = cell * CELL_WIDTH - MAX_COORD + offset;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    //! End-to-end test on a tiny demo built byte by byte from the format description in `docs/formats/demo.md`:
    //! file header → server info + string table → send tables → class info → entity packets. It runs in CI, where
    //! no real demo is available.

    use prost::Message;

    use super::*;
    use crate::demo::bits::BitWriter;
    use crate::demo::fieldpath::write_paths;
    use crate::demo::frame::testing::{DemoBuilder, push_varint};
    use crate::demo::proto::{
        CDemoClassInfo, CDemoFileHeader, CDemoPacket, CDemoSendTables, CnetMsgTick,
        CsvcMsgCreateStringTable, CsvcMsgPacketEntities, CsvcMsgServerInfo,
        CsvcMsgUpdateStringTable, EDemoCommands, NetMessages, SvcMessages, c_demo_class_info,
    };
    use crate::demo::serializer::testing::{F, message};

    /// The game rules of one tick: in freeze time or not, rounds played, warmup, the match over.
    fn rules(freeze: bool, rounds_played: u32, warmup: bool, match_ended: bool) -> RulesState {
        RulesState {
            rounds_played,
            freeze,
            warmup,
            match_ended,
        }
    }

    #[test]
    fn rounds_run_from_freeze_to_freeze_and_end_with_the_match() {
        let mut detector = RoundDetector::default();
        assert!(
            detector.update(10, rules(false, 0, true, false)).is_empty(),
            "warmup"
        );
        assert_eq!(
            detector.update(20, rules(true, 0, false, false)),
            [RoundEvent::Started(1)]
        );
        assert!(
            detector
                .update(30, rules(false, 0, false, false))
                .is_empty()
        );
        let next = detector.update(40, rules(true, 1, false, false));
        let first = RoundInfo {
            round: 1,
            start_tick: 20,
            end_tick: 30,
        };
        assert_eq!(next, [RoundEvent::Ended(first), RoundEvent::Started(2)]);
        detector.update(50, rules(false, 1, false, false));
        // The match ends on the tick its last round is decided: that tick belongs to the round.
        let last = RoundInfo {
            round: 2,
            start_tick: 40,
            end_tick: 60,
        };
        assert_eq!(
            detector.update(60, rules(false, 2, false, true)),
            [RoundEvent::Ended(last)]
        );
        assert_eq!(detector.finish(), None);
    }

    #[test]
    fn a_return_to_warmup_ends_the_round() {
        let mut detector = RoundDetector::default();
        detector.update(20, rules(true, 0, false, false));
        detector.update(30, rules(false, 0, false, false));
        let knife = RoundInfo {
            round: 1,
            start_tick: 20,
            end_tick: 30,
        };
        assert_eq!(
            detector.update(40, rules(false, 0, true, false)),
            [RoundEvent::Ended(knife)]
        );
        assert!(detector.update(50, rules(false, 0, true, false)).is_empty());
        assert_eq!(
            detector.update(60, rules(true, 0, false, false)),
            [RoundEvent::Started(1)]
        );
    }

    // Class ids (4 classes → 3 bits) and entity indices used by the fixture.
    const RULES_CLASS: u32 = 0;
    const CONTROLLER_CLASS: u32 = 1;
    const PAWN_CLASS: u32 = 2;
    const WEAPON_CLASS: u32 = 3;
    const CLASS_COUNT: u32 = 4;
    const CLASS_ID_BITS: u32 = 3;
    const RULES: u32 = 3;
    const CONTROLLER: u32 = 1;
    const PAWN: u32 = 10;
    const WEAPON: u32 = 20;
    /// The server's game tick runs this far ahead of the demo tick (2403 on the local match demo).
    const SERVER_TICK_OFFSET: u32 = 1000;

    fn send_tables() -> Vec<u8> {
        let f = |name, ty| F {
            name,
            ty,
            ..Default::default()
        };
        let noscale = |name| F {
            bits: Some(32),
            ..f(name, "CNetworkedQuantizedFloat")
        };
        // The view offset as CS2 sends it (`demo_inspect … fields CCSPlayerPawn`): x and y 10 bits in −64..64 with
        // the encode-zero flag, z 12 bits up to 64 with the round-up flag and no low value — which the decoder turns
        // into 12 plain bits, value = (raw + 1) / 64.
        let view_offset = |name, bits, low, flags| F {
            bits: Some(bits),
            low,
            high: Some(64.0),
            flags: Some(flags),
            send_node: Some("m_vecViewOffset"),
            ..f(name, "CNetworkedQuantizedFloat")
        };
        let msg = message(&[
            (
                "CBodyComponentBaseAnimGraph",
                vec![
                    f("m_cellX", "uint16"),
                    f("m_cellY", "uint16"),
                    f("m_cellZ", "uint16"),
                    noscale("m_vecX"),
                    noscale("m_vecY"),
                    noscale("m_vecZ"),
                    // The model (fields 6 – 7), flattened under their send node like CS2 sends them.
                    F {
                        send_node: Some("m_skeletonInstance.m_modelState"),
                        ..f("m_hModel", "CStrongHandle< InfoForResourceTypeCModel >")
                    },
                    F {
                        send_node: Some("m_skeletonInstance.m_modelState"),
                        ..f("m_MeshGroupMask", "uint64")
                    },
                ],
            ),
            // The pawn's services: declared before the pawn that points to them.
            (
                "CCSPlayer_MovementServices",
                vec![
                    f("m_flDuckAmount", "float32"),
                    f("m_nLastJumpTick", "GameTick_t"),
                ],
            ),
            (
                "CCSPlayer_WeaponServices",
                vec![
                    f("m_hActiveWeapon", "CHandle< CBasePlayerWeapon >"),
                    f(
                        "m_hMyWeapons",
                        "CNetworkUtlVectorBase< CHandle< CBasePlayerWeapon > >",
                    ),
                ],
            ),
            (
                "CCSPlayerPawn",
                vec![
                    F {
                        serializer: Some("CBodyComponentBaseAnimGraph"),
                        ..f("CBodyComponent", "CBodyComponent")
                    },
                    F {
                        encoder: Some("qangle_precise"),
                        bits: Some(0),
                        ..f("m_angEyeAngles", "QAngle")
                    },
                    f("m_lifeState", "uint8"),
                    view_offset("m_vecX", 10, Some(-64.0), 4),
                    view_offset("m_vecY", 10, Some(-64.0), 4),
                    view_offset("m_vecZ", 12, None, 2),
                    // Rig inputs (fields 6 – 9).
                    f("m_fFlags", "uint32"),
                    f("m_bIsWalking", "bool"),
                    F {
                        serializer: Some("CCSPlayer_MovementServices"),
                        ..f("m_pMovementServices", "CPlayer_MovementServices*")
                    },
                    F {
                        serializer: Some("CCSPlayer_WeaponServices"),
                        ..f("m_pWeaponServices", "CPlayer_WeaponServices*")
                    },
                    // Field 10: the gloves.
                    F {
                        send_node: Some("m_EconGloves"),
                        ..f("m_iItemDefinitionIndex", "uint16")
                    },
                    // Field 11: the viewmodel's field of view (the offsets are never sent: CS2's defaults).
                    f("m_flViewmodelFOV", "float32"),
                ],
            ),
            (
                // A weapon: CS2 sends the item definition index flattened under its send node.
                "CAK47",
                vec![
                    F {
                        send_node: Some("m_AttributeManager.m_Item"),
                        ..f("m_iItemDefinitionIndex", "uint16")
                    },
                    f("m_iWeaponGameplayAnimState", "WeaponGameplayAnimState"),
                    f("m_bInReload", "bool"),
                    F {
                        serializer: Some("CBodyComponentBaseAnimGraph"),
                        ..f("CBodyComponent", "CBodyComponent")
                    },
                ],
            ),
            (
                "CCSPlayerController",
                vec![
                    f("m_iTeamNum", "uint8"),
                    f("m_iszPlayerName", "char[128]"),
                    F {
                        encoder: Some("fixed64"),
                        ..f("m_steamID", "uint64")
                    },
                    f("m_hPlayerPawn", "CHandle< CCSPlayerPawn >"),
                ],
            ),
            (
                "CCSGameRules",
                vec![
                    f("m_bFreezePeriod", "bool"),
                    f("m_bWarmupPeriod", "bool"),
                    f("m_gamePhase", "int32"),
                    f("m_totalRoundsPlayed", "int32"),
                ],
            ),
            (
                "CCSGameRulesProxy",
                vec![F {
                    serializer: Some("CCSGameRules"),
                    ..f("m_pGameRules", "CCSGameRules*")
                }],
            ),
        ]);
        let body = msg.encode_to_vec();
        let mut data = Vec::new();
        push_varint(&mut data, body.len() as u64);
        data.extend_from_slice(&body);
        CDemoSendTables { data: Some(data) }.encode_to_vec()
    }

    fn class_info() -> Vec<u8> {
        let class = |id: u32, name: &str| c_demo_class_info::ClassT {
            class_id: Some(id as i32),
            network_name: Some(name.into()),
            table_name: None,
        };
        CDemoClassInfo {
            classes: vec![
                class(RULES_CLASS, "CCSGameRulesProxy"),
                class(CONTROLLER_CLASS, "CCSPlayerController"),
                class(PAWN_CLASS, "CCSPlayerPawn"),
                class(WEAPON_CLASS, "CAK47"),
            ],
        }
        .encode_to_vec()
    }

    /// A packet frame payload holding the given (message type, protobuf) pairs.
    fn packet(messages: &[(SvcMessages, Vec<u8>)]) -> Vec<u8> {
        let raw: Vec<(u32, Vec<u8>)> = messages
            .iter()
            .map(|(kind, body)| (*kind as u32, body.clone()))
            .collect();
        packet_raw(&raw)
    }

    /// [`packet`] with message type numbers, for messages of other enums (`NetMessages::NetTick`).
    fn packet_raw(messages: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut w = BitWriter::default();
        for (kind, body) in messages {
            w.ubitvar(*kind).varint(body.len() as u64).bytes(body);
        }
        CDemoPacket {
            data: Some(w.finish()),
        }
        .encode_to_vec()
    }

    fn entities(updated: i32, delta: bool, w: &BitWriter) -> (SvcMessages, Vec<u8>) {
        let msg = CsvcMsgPacketEntities {
            updated_entries: Some(updated),
            legacy_is_delta: Some(delta),
            entity_data: Some(w.finish()),
            ..Default::default()
        };
        (SvcMessages::SvcPacketEntities, msg.encode_to_vec())
    }

    /// Writes "update entity `index`" (`previous` = index of the entity written before, or -1).
    fn update_header(w: &mut BitWriter, previous: i64, index: u32) {
        w.ubitvar((i64::from(index) - previous - 1) as u32)
            .bits(0b00, 2);
    }

    /// Writes "create entity `index` of class `class`".
    fn create_header(w: &mut BitWriter, previous: i64, index: u32, class: u32) {
        w.ubitvar((i64::from(index) - previous - 1) as u32)
            .bits(0b10, 2)
            .bits(u64::from(class), CLASS_ID_BITS)
            .bits(1, 17)
            .varint(0);
    }

    /// Game rules values: pointer present, freeze, no warmup, phase 2 (first half), rounds played.
    fn rules_fields(w: &mut BitWriter, freeze: bool, rounds: i64) {
        write_paths(w, &[&[0], &[0, 0], &[0, 1], &[0, 2], &[0, 3]]);
        w.bool(true)
            .bool(freeze)
            .bool(false)
            .varint_signed(2)
            .varint_signed(rounds);
    }

    /// Mesh groups of the synthetic agent (defuse kit hidden, third-person body) and its gloves' item.
    const AGENT_GROUPS: u64 = 5;
    const GLOVES: u64 = 5030;
    /// Mesh groups of the synthetic AK-47 (its CS:GO-era model).
    const AK_GROUPS: u64 = 2;
    /// The synthetic player's `viewmodel_fov`.
    const VIEWMODEL_FOV: f32 = 68.0;

    fn agent_model() -> u64 {
        crate::resource::resource_id("agents/models/ctm_sas/ctm_sas.vmdl")
    }

    fn ak_model() -> u64 {
        crate::resource::resource_id("weapons/models/ak47/weapon_rif_ak47.vmdl")
    }

    /// Pawn position (cell 32 on each axis, so world position = offset), eye angles (20-bit precise angles) and the
    /// view offset's z (12 bits, value = (raw + 1) / 64). The view offset's x and y are never sent, so they keep
    /// their default 0 (CS2's are 0 too). With `rig`, also the rig inputs: on the ground, walking, half crouched,
    /// last jump at game tick 1011, holding the AK-47 entity; and the appearance: the SAS agent, its mesh groups,
    /// gloves, and the AK-47 as the only weapon carried.
    fn pawn_fields(
        w: &mut BitWriter,
        with_pointer: bool,
        pos: [f32; 3],
        pitch_raw: u64,
        yaw_raw: u64,
        eye_raw: u64,
        rig: bool,
    ) {
        let mut paths: Vec<&[i32]> = Vec::new();
        if with_pointer {
            paths.extend([&[0][..], &[0, 0], &[0, 1], &[0, 2]]);
        }
        paths.extend([&[0, 3][..], &[0, 4], &[0, 5]]);
        if rig {
            paths.extend([&[0, 6][..], &[0, 7]]);
        }
        paths.extend([&[1][..], &[5]]);
        if rig {
            paths.extend([
                &[6][..],
                &[7],
                &[8],
                &[8, 0],
                &[8, 1],
                &[9],
                &[9, 0],
                &[9, 1],
                &[9, 1, 0],
                &[10],
                &[11],
            ]);
        }
        write_paths(w, &paths);
        if with_pointer {
            // Present, then cells: 32 × 512 − 16384 = 0.
            w.bool(true).varint(32).varint(32).varint(32);
        }
        for v in pos {
            w.bits(u64::from(v.to_bits()), 32);
        }
        if rig {
            w.varint(agent_model()).varint(AGENT_GROUPS);
        }
        w.bool(true)
            .bool(true)
            .bool(false)
            .bits(pitch_raw, 20)
            .bits(yaw_raw, 20)
            .bits(eye_raw, 12);
        if rig {
            w.varint(FL_ONGROUND) // m_fFlags
                .bool(true) // m_bIsWalking
                .bool(true) // m_pMovementServices present
                .bits(u64::from(0.5f32.to_bits()), 32) // m_flDuckAmount
                .varint_signed(1011) // m_nLastJumpTick: a signed (zigzag) varint
                .bool(true) // m_pWeaponServices present
                .varint((3 << 14) | u64::from(WEAPON)) // m_hActiveWeapon: serial 3, entity WEAPON
                .varint(1) // m_hMyWeapons: one weapon…
                .varint((3 << 14) | u64::from(WEAPON)) // … the same AK-47
                .varint(GLOVES) // m_EconGloves.m_iItemDefinitionIndex
                .bits(u64::from(VIEWMODEL_FOV.to_bits()), 32); // m_flViewmodelFOV
        }
    }

    fn synthetic_demo() -> Vec<u8> {
        let mut demo = DemoBuilder::new();
        let header = CDemoFileHeader {
            demo_file_stamp: "PBDEMS2\0".into(),
            map_name: Some("de_synthetic".into()),
            ..Default::default()
        };
        demo.frame(
            EDemoCommands::DemFileHeader,
            None,
            &header.encode_to_vec(),
            false,
        );

        let server_info = CsvcMsgServerInfo {
            max_classes: Some(CLASS_COUNT as i32),
            tick_interval: Some(1.0 / 64.0),
            ..Default::default()
        };
        let baselines = CsvcMsgCreateStringTable {
            name: Some("instancebaseline".into()),
            num_entries: Some(0),
            flags: Some(1),
            using_varint_bitcounts: Some(true),
            string_data: Some(Vec::new()),
            ..Default::default()
        };
        let signon = packet(&[
            (SvcMessages::SvcServerInfo, server_info.encode_to_vec()),
            (SvcMessages::SvcCreateStringTable, baselines.encode_to_vec()),
        ]);
        demo.frame(EDemoCommands::DemSignonPacket, None, &signon, false);
        demo.frame(EDemoCommands::DemSendTables, None, &send_tables(), true);
        demo.frame(EDemoCommands::DemClassInfo, None, &class_info(), false);

        // Tick 10: full snapshot. Controller ("synk", Steam id, pawn handle), rules (no freeze), pawn, and an AK-47
        // (item 7) in its idle state (50), reloading.
        let mut w = BitWriter::default();
        create_header(&mut w, -1, CONTROLLER, CONTROLLER_CLASS);
        write_paths(&mut w, &[&[1], &[2], &[3]]);
        w.bytes(b"synk\0")
            .bits(76561197960287930, 64)
            .varint((7 << 14) | u64::from(PAWN));
        create_header(&mut w, i64::from(CONTROLLER), RULES, RULES_CLASS);
        rules_fields(&mut w, false, 0);
        create_header(&mut w, i64::from(RULES), PAWN, PAWN_CLASS);
        // Standing: eye height (4093 + 1) / 64 = 63.96875.
        pawn_fields(
            &mut w,
            true,
            [100.0, 200.0, -50.0],
            1 << 19,
            1 << 19,
            4093,
            false,
        );
        create_header(&mut w, i64::from(PAWN), WEAPON, WEAPON_CLASS);
        write_paths(&mut w, &[&[0], &[1], &[2], &[3], &[3, 6], &[3, 7]]);
        w.varint(7)
            .varint(50)
            .bool(true)
            .bool(true) // CBodyComponent present
            .varint(ak_model())
            .varint(AK_GROUPS);
        // The controller's team (3) only comes from its class baseline, which this same packet adds to the
        // `instancebaseline` table *after* the entity message: string tables must be applied first.
        let mut baseline = BitWriter::default();
        write_paths(&mut baseline, &[&[0]]);
        baseline.varint(3);
        let baseline = baseline.finish();
        let mut entry = BitWriter::default();
        entry
            .bool(true)
            .bool(true)
            .bool(false)
            .bytes(format!("{CONTROLLER_CLASS}\0").as_bytes());
        entry
            .bool(true)
            .bool(false)
            .ubitvar(baseline.len() as u32)
            .bytes(&baseline);
        let update = CsvcMsgUpdateStringTable {
            table_id: Some(0),
            num_changed_entries: Some(1),
            string_data: Some(entry.finish()),
        };
        let messages = [
            entities(4, false, &w),
            (SvcMessages::SvcUpdateStringTable, update.encode_to_vec()),
        ];
        demo.frame(
            EDemoCommands::DemPacket,
            Some(10),
            &packet(&messages),
            false,
        );

        // Tick 11: freeze time starts → round 1 starts. Ticks 12-13: the pawn moves; tick 13 also ends freeze time and
        // sends the rig inputs, after the server's tick (13 + SERVER_TICK_OFFSET).
        let mut w = BitWriter::default();
        update_header(&mut w, -1, RULES);
        rules_fields(&mut w, true, 0);
        demo.frame(
            EDemoCommands::DemPacket,
            Some(11),
            &packet(&[entities(1, true, &w)]),
            false,
        );
        for (tick, x) in [(12, 104.0), (13, 108.0)] {
            let mut w = BitWriter::default();
            let mut count = 1;
            let mut previous = -1;
            if tick == 13 {
                update_header(&mut w, -1, RULES);
                rules_fields(&mut w, false, 0);
                previous = i64::from(RULES);
                count = 2;
            }
            update_header(&mut w, previous, PAWN);
            // pitch: 3/8 turn − 180° = −45°; yaw: 5/8 turn − 180° = 45°; crouched: (2941 + 1) / 64 = 45.96875.
            let rig = tick == 13;
            pawn_fields(
                &mut w,
                false,
                [x, 200.0, -50.0],
                3 << 17,
                5 << 17,
                2941,
                rig,
            );
            let server_tick = CnetMsgTick {
                tick: Some(tick + SERVER_TICK_OFFSET),
                ..Default::default()
            };
            let (kind, body) = entities(count, true, &w);
            let messages = [
                (NetMessages::NetTick as u32, server_tick.encode_to_vec()),
                (kind as u32, body),
            ];
            demo.frame(
                EDemoCommands::DemPacket,
                Some(tick),
                &packet_raw(&messages),
                false,
            );
        }

        // Tick 14: round over (1 round played) and the next freeze time starts → round 2.
        let mut w = BitWriter::default();
        update_header(&mut w, -1, RULES);
        rules_fields(&mut w, true, 1);
        demo.frame(
            EDemoCommands::DemPacket,
            Some(14),
            &packet(&[entities(1, true, &w)]),
            false,
        );
        demo.frame(EDemoCommands::DemStop, Some(15), &[], false);
        demo.bytes
    }

    #[test]
    fn a_flash_is_a_change_to_a_duration_above_zero() {
        // A new flash, a stronger or weaker one on top of it.
        assert!(flash_starts(Some(0.0), 2.5));
        assert!(flash_starts(Some(2.5), 3.1));
        assert!(flash_starts(Some(3.1), 0.8));
        // The same value again, the server clearing it, nothing.
        assert!(!flash_starts(Some(2.5), 2.5));
        assert!(!flash_starts(Some(2.5), 0.0));
        assert!(!flash_starts(Some(0.0), 0.0));
        // Seen first, or after a gap: left from before, not a flash.
        assert!(!flash_starts(None, 2.5));
    }

    #[test]
    fn tracks_players_through_a_synthetic_demo() {
        let data = synthetic_demo();
        let track = track_round(&data, 1).unwrap();
        assert_eq!(track.map_name, "de_synthetic");
        assert_eq!((track.start_tick, track.end_tick), (11, 13));
        assert_eq!(track.tick_interval, 1.0 / 64.0);
        let ticks: Vec<u32> = track.samples.iter().map(|s| s.tick).collect();
        assert_eq!(ticks, [11, 12, 13]);

        let first = &track.samples[0].players[0];
        assert_eq!(first.name, "synk");
        assert_eq!(first.steam_id, 76561197960287930);
        assert_eq!(first.team, 3);
        assert_eq!(first.controller, CONTROLLER);
        assert!(first.alive);
        assert_eq!(first.position, [100.0, 200.0, -50.0]);
        // x and y were never sent, so they keep their default 0. z is (raw + 1) / 64 in exact arithmetic; the
        // quantized float formula (Valve's, also used by demoparser2) can land one float step (~4e-6) off.
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5);
        assert!(close(first.view_offset, [0.0, 0.0, 63.96875]));
        assert!(close(first.eye_position(), [100.0, 200.0, 13.96875]));
        assert_eq!((first.pitch, first.yaw), (0.0, 0.0));

        let last = &track.samples[2].players[0];
        assert_eq!(last.position, [108.0, 200.0, -50.0]);
        assert!(close(last.view_offset, [0.0, 0.0, 45.96875]));
        assert!(close(last.eye_position(), [108.0, 200.0, -4.03125]));
        assert_eq!((last.pitch, last.yaw), (-45.0, 45.0));

        // Rig inputs: defaults until sent (tick 12), then what tick 13 sent. The jump at game tick 1011 is demo tick
        // 11 (the server ran 1000 ticks ahead). Velocity was never sent: zero.
        assert_eq!(track.samples[1].players[0].rig, RigInputs::default());
        assert_eq!(
            last.rig,
            RigInputs {
                velocity: [0.0; 3],
                on_ground: true,
                duck_amount: 0.5,
                walking: true,
                last_jump_tick: Some(11),
                weapon: Some(ActiveWeapon {
                    entity: WEAPON,
                    item: 7,
                    class: "CAK47".into(),
                    anim_state: 50,
                    reloading: true,
                    ..ActiveWeapon::default()
                }),
                ..RigInputs::default()
            }
        );
        // Viewmodel settings: CS2's defaults until sent, then the FOV sent (the offsets never were).
        assert_eq!(
            track.samples[1].players[0].viewmodel,
            ViewmodelSettings::default()
        );
        assert_eq!(
            last.viewmodel,
            ViewmodelSettings {
                fov: VIEWMODEL_FOV,
                ..ViewmodelSettings::default()
            }
        );
        // Appearance and weapons: nothing until sent, then the agent and the AK-47 by resource id.
        assert_eq!(
            track.samples[1].players[0].appearance,
            Appearance::default()
        );
        assert!(track.samples[1].players[0].weapons.is_empty());
        assert_eq!(
            last.appearance,
            Appearance {
                model: agent_model(),
                mesh_groups: AGENT_GROUPS,
                gloves: GLOVES as u32,
                glove_finish: [0; 3],
            }
        );
        assert_eq!(
            last.weapons,
            [CarriedWeapon {
                entity: WEAPON,
                item: 7,
                model: ak_model(),
                mesh_groups: AK_GROUPS,
                count: 1,
            }]
        );

        let rounds = list_rounds(&data).unwrap();
        let round = |round, start_tick, end_tick| RoundInfo {
            round,
            start_tick,
            end_tick,
        };
        assert_eq!(rounds, [round(1, 11, 13), round(2, 14, 14)]);
        assert!(matches!(
            track_round(&data, 3),
            Err(TrackError::RoundNotFound(3))
        ));
    }
}
