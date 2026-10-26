// The Studio's commands (apps/studio/src-tauri/src/commands.rs), typed for the UI.
import { invoke } from "@tauri-apps/api/core";

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
