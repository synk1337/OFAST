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

export const environment = () => invoke<Environment>("environment");
export const listDemos = (folder: string) => invoke<DemoEntry[]>("list_demos", { folder });
