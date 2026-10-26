// The Studio's commands (apps/studio/src-tauri/src/commands.rs), typed for the UI.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** What the Studio needs on this computer, each null when not found. */
export interface Environment {
  cs2: string | null;
  replays: string | null;
  unreal: string | null;
  project: string | null;
}

/** One demo file, from its header. */
export interface DemoEntry {
  path: string;
  name: string;
  size: number;
  /** Seconds since 1970. */
  modified: number;
  map: string;
  server: string;
}

// A demo's summary (`ofast_core::demo::summary`, field for field). Player fields are indices into `players`.

export type Side = "t" | "ct";
export type RoundEnd = "bomb_exploded" | "bomb_defused" | "eliminated" | "time_ran_out" | "surrender" | "draw" | "other";
export type WeaponClass = "sniper" | "rifle" | "smg" | "heavy" | "pistol" | "knife" | "grenade" | "taser" | "other";
export type HighlightKind = "multi_kill" | "clutch" | "collateral" | "special_kill";
export type KillTag = "noscope" | "wallbang" | "through_smoke" | "airborne" | "blind" | "knife";

export interface Player {
  name: string;
  /** Text: a JavaScript number cannot hold a 64-bit Steam id. "0" for a bot. */
  steam_id: string;
}

export interface Team {
  /** The clan's name, empty in matchmaking. */
  name: string;
  /** The score once the round was over. */
  score: number;
}

export interface Kill {
  tick: number;
  victim: number | null;
  /** null: the world (a fall, the bomb). */
  attacker: number | null;
  assister: number | null;
  /** The kill feed's name: "ak47", "awp", "knife_karambit", "inferno"… */
  weapon: string;
  weapon_class: WeaponClass;
  headshot: boolean;
  wallbang: boolean;
  noscope: boolean;
  through_smoke: boolean;
  blind: boolean;
  airborne: boolean;
  flash_assist: boolean;
  team_kill: boolean;
}

export interface RoundSummary {
  round: number;
  start_tick: number;
  freeze_end_tick: number | null;
  /** When the winner was known; later kills are the seconds before the next round. */
  decided_tick: number | null;
  end_tick: number;
  /** null: the demo stops before the round is decided. */
  winner: Side | null;
  reason: RoundEnd | null;
  /** Played again (a restart, a backup restored): its result did not count. */
  restarted: boolean;
  t: Team;
  ct: Team;
  players: { player: number; side: Side }[];
  kills: Kill[];
}

export interface Highlight {
  kind: HighlightKind;
  /** Index into the summary's rounds (a restarted round's number comes again). */
  round_index: number;
  player: number;
  /** Kills (multi-kill, collateral), opponents (clutch: the X of 1vX), 1 (special kill). */
  count: number;
  tags: KillTag[];
  start_tick: number;
  end_tick: number;
  /** Indices into the round's kills. */
  kills: number[];
}

export interface DemoSummary {
  map: string;
  server: string;
  /** Seconds per tick. */
  tick_interval: number;
  players: Player[];
  rounds: RoundSummary[];
  highlights: Highlight[];
}

export const environment = () => invoke<Environment>("environment");
export const listDemos = (folder: string) => invoke<DemoEntry[]>("list_demos", { folder });
export const summarizeDemo = (path: string) => invoke<DemoSummary>("summarize_demo", { path });

// A round for the 2D replay (`radar_round` in commands.rs).

export interface Overview {
  /** The world x of the radar image's left edge. */
  pos_x: number;
  /** The world y of its top edge. */
  pos_y: number;
  /** World units per image pixel (the image is 1024 pixels wide). */
  scale: number;
  /** The levels of a map on two floors ("default", "lower"), each with its own image. */
  sections: { name: string; altitude_min: number; altitude_max: number }[];
}

export interface RadarPlayer {
  /** Index into the round's players. */
  slot: number;
  /** 2 = T, 3 = CT. */
  team: number;
  x: number;
  y: number;
  z: number;
  /** Degrees, 0 = +x, counter-clockwise. */
  yaw: number;
  alive: boolean;
}

export interface RadarFrame {
  tick: number;
  players: RadarPlayer[];
  grenades: [number, number, number][];
  smokes: [number, number][];
  fires: [number, number][];
}

export interface RadarRound {
  map: string;
  overview: Overview | null;
  players: { name: string; steam_id: string }[];
  frames: RadarFrame[];
}

export const radarRound = (path: string, startTick: number) =>
  invoke<RadarRound>("radar_round", { path, startTick });
/** A radar image's PNG bytes. */
export const radarImage = (map: string, section: string) => invoke<ArrayBuffer>("radar_image", { map, section });

// "Send to Unreal" (`unreal.rs`).

export type SendStep = "round" | "map_export" | "map_import" | "round_import" | "cameras";

/** Which camera a kill is shown through (`camera::shots::ShotKind`). */
export type ShotKind = "pov" | "victim" | "behind" | "orbit" | "tripod" | "above";

/** A round's cameras as the user picked them (`export::cameras::CameraPlan`). */
export interface CameraPlan {
  /** By kill tick: the kills not shown through the killer's eyes. */
  shots: Record<number, ShotKind>;
  /** First-person smoothing, 0 (none) to 1 (strong). */
  smoothing: number;
  /** HLAE camera files (`mirv_campath` .xml, `mirv_camio` .cam). */
  hlae: string[];
}

export const NO_PLAN: CameraPlan = { shots: {}, smoothing: 0, hlae: [] };

/** One line of a sending's progress. */
export interface SendProgress {
  step: SendStep;
  message: string;
  /** A warning the bar keeps showing (later lines replace the others). */
  warning: boolean;
}

/** Where to look in Unreal once a round is sent. */
export interface Sent {
  /** /Game/OFAST/Maps/<map> */
  level: string;
  /** /Game/OFAST/Replays/<demo>_r<round> */
  sequence: string;
}

export const sendToUnreal = (path: string, startTick: number, plan: CameraPlan) =>
  invoke<Sent>("send_to_unreal", { path, startTick, plan });
export const updateCameras = (path: string, startTick: number, plan: CameraPlan) =>
  invoke<Sent>("update_cameras", { path, startTick, plan });
/** The round numbers of a demo already in OFAST's Unreal project. */
export const sentRounds = (path: string) => invoke<number[]>("sent_rounds", { path });
export const openInUnreal = (sent: Sent) => invoke<void>("open_in_unreal", { ...sent });
export const onSendProgress = (handler: (progress: SendProgress) => void) =>
  listen<SendProgress>("unreal-progress", (event) => handler(event.payload));
