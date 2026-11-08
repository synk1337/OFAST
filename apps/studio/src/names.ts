// Names for people: weapons as the game's kill feed shows them, highlights, round ends.
import type {
  Highlight,
  KillTag,
  RenderOutput,
  RenderPower,
  RenderPreset,
  RoundEnd,
  RoundSummary,
  ShotKind,
  WeaponClass,
} from "./api";

/** The game's own names for the weapons of the kill feed (`player_death`'s `weapon` key). */
const WEAPONS: Record<string, string> = {
  ak47: "AK-47",
  aug: "AUG",
  awp: "AWP",
  bizon: "PP-Bizon",
  cz75a: "CZ75-Auto",
  deagle: "Desert Eagle",
  decoy: "Decoy",
  elite: "Dual Berettas",
  famas: "FAMAS",
  fiveseven: "Five-SeveN",
  flashbang: "Flashbang",
  g3sg1: "G3SG1",
  galilar: "Galil AR",
  glock: "Glock-18",
  hegrenade: "HE Grenade",
  hkp2000: "P2000",
  incgrenade: "Incendiary",
  inferno: "Fire",
  m249: "M249",
  m4a1: "M4A4",
  m4a1_silencer: "M4A1-S",
  m4a1_silencer_off: "M4A1-S",
  mac10: "MAC-10",
  mag7: "MAG-7",
  molotov: "Molotov",
  mp5sd: "MP5-SD",
  mp7: "MP7",
  mp9: "MP9",
  negev: "Negev",
  nova: "Nova",
  p250: "P250",
  p90: "P90",
  planted_c4: "Bomb",
  revolver: "R8 Revolver",
  sawedoff: "Sawed-Off",
  scar20: "SCAR-20",
  sg556: "SG 553",
  smokegrenade: "Smoke",
  ssg08: "SSG 08",
  taser: "Zeus x27",
  tec9: "Tec-9",
  ump45: "UMP-45",
  usp_silencer: "USP-S",
  usp_silencer_off: "USP-S",
  world: "World",
  xm1014: "XM1014",
};

/** A kill feed weapon's name: "AK-47", "Knife"; an unknown one as written. */
export function weaponName(weapon: string): string {
  if (weapon.startsWith("knife") || weapon === "bayonet") return "Knife";
  return WEAPONS[weapon] ?? weapon;
}

/** The weapon filter's choices: a class, or the AWP on its own. */
export const WEAPON_FILTERS: { value: string; label: string; matches: (weapon: string, cls: WeaponClass) => boolean }[] =
  [
    { value: "", label: "All weapons", matches: () => true },
    { value: "awp", label: "AWP", matches: (w) => w === "awp" },
    { value: "sniper", label: "Snipers", matches: (_, c) => c === "sniper" },
    { value: "rifle", label: "Rifles", matches: (_, c) => c === "rifle" },
    { value: "smg", label: "SMGs", matches: (_, c) => c === "smg" },
    { value: "heavy", label: "Shotguns and machine guns", matches: (_, c) => c === "heavy" },
    { value: "pistol", label: "Pistols", matches: (_, c) => c === "pistol" },
    { value: "knife", label: "Knives", matches: (_, c) => c === "knife" },
    { value: "grenade", label: "Grenades and fire", matches: (_, c) => c === "grenade" },
    { value: "taser", label: "Zeus", matches: (_, c) => c === "taser" },
  ];

const TAGS: Record<KillTag, string> = {
  noscope: "No-scope",
  wallbang: "Wallbang",
  through_smoke: "Through smoke",
  airborne: "Airborne",
  blind: "Blind",
  knife: "Knife",
};

export const tagName = (tag: KillTag) => TAGS[tag];

/** A highlight's short name: "ACE", "4K", "1v3 clutch", "Collateral", "No-scope · Wallbang". */
export function highlightName(h: Highlight): string {
  switch (h.kind) {
    case "multi_kill":
      return h.count >= 5 ? "ACE" : `${h.count}K`;
    case "clutch":
      return `1v${h.count} clutch`;
    case "collateral":
      return h.count > 2 ? `${h.count}-kill collateral` : "Collateral";
    case "special_kill":
      return h.tags.map(tagName).join(" · ");
  }
}

/** The highlight filter's choices. */
export const HIGHLIGHT_FILTERS: { value: string; label: string; matches: (h: Highlight) => boolean }[] = [
  { value: "", label: "All", matches: () => true },
  { value: "multi_kill", label: "3K and more", matches: (h) => h.kind === "multi_kill" },
  { value: "clutch", label: "Clutches", matches: (h) => h.kind === "clutch" },
  { value: "collateral", label: "Collaterals", matches: (h) => h.kind === "collateral" },
  ...(Object.keys(TAGS) as KillTag[]).map((tag) => ({
    value: tag,
    label: TAGS[tag],
    matches: (h: Highlight) => h.tags.includes(tag),
  })),
];

const ENDS: Record<RoundEnd, string> = {
  bomb_exploded: "bomb exploded",
  bomb_defused: "bomb defused",
  eliminated: "elimination",
  time_ran_out: "time ran out",
  surrender: "surrender",
  draw: "draw",
  other: "",
};

export const roundEndName = (end: RoundEnd | null) => (end ? ENDS[end] : "");

/** Seconds as "1:05". */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** How a round ended, for people: "FURIA won: bomb defused", "Draw", "Not finished in this demo". */
export function outcome(round: RoundSummary, winnerName: string): string {
  if (round.decided_tick === null) return "Not finished in this demo";
  if (!round.winner) return "Draw";
  const reason = roundEndName(round.reason);
  return reason ? `${winnerName} won: ${reason}` : `${winnerName} won`;
}

/** The cameras a kill can be seen through (`camera::shots`), in the order offered. */
export const SHOTS: [ShotKind, string][] = [
  ["pov", "Killer's eyes"],
  ["victim", "Victim's eyes"],
  ["behind", "Behind the killer"],
  ["orbit", "Around the killer"],
  ["tripod", "Fixed camera"],
  ["above", "From above"],
];

/** First-person smoothing strengths (`camera::smooth`). */
export const SMOOTHING: [number, string][] = [
  [0, "as played"],
  [0.35, "smoothed"],
  [0.7, "very smooth"],
];

/** Render presets, the picture's size (`tools/unreal/render_round.py`, also listed in `src-tauri/src/unreal.rs`):
 * name, label, what it makes. Each is rendered alike: 8 samples a frame (motion blur), the depth, the players layer
 * when asked. */
export const RENDER_PRESETS: [RenderPreset, string, string][] = [
  ["720p", "720p", "1280 × 720: the quickest, to check the cuts and the timing"],
  ["1080p", "1080p", "1920 × 1080 (Full HD)"],
  [
    "2k",
    "2K",
    "2560 × 1440 (1440p). On a graphics card too small for it Unreal shades the frames smaller and scales them up",
  ],
  [
    "4k",
    "4K",
    "3840 × 2160 (UHD). On a graphics card too small for it Unreal shades the frames smaller and scales them up",
  ],
];

/** What a render writes (`tools/unreal/render_round.py`'s OUTPUTS, `src-tauri/src/unreal.rs`'s OUTPUTS): name, label,
 * what it is. */
export const RENDER_OUTPUTS: [RenderOutput, string, string][] = [
  [
    "images",
    "Images",
    "A PNG image a frame (lossless), and an EXR a frame with every pass and the depth: import them as an image " +
      "sequence",
  ],
  [
    "prores",
    "ProRes video",
    "One Apple ProRes 4444 video (.mov), every cut in it: visually lossless, a tenth of the images' size (no EXR " +
      "or depth), the players layer with its transparency. Opens in VEGAS Pro, Premiere, After Effects and Resolve",
  ],
];

/** Render powers (`tools/unreal/render_round.py`'s POWER, `src-tauri/src/unreal.rs`'s Power): name, label, what it
 * does. */
export const RENDER_POWERS: [RenderPower, string, string][] = [
  ["full", "Full", "As fast as your PC goes: Unreal takes the graphics card whenever it can"],
  [
    "balanced",
    "Balanced",
    "The graphics card rests now and then and Unreal runs below your other programs, so the PC stays usable: " +
      "about 15 % longer",
  ],
  [
    "low",
    "Low",
    "The graphics card rests as long as it works (a third less power, cooler and quieter), Unreal below your other " +
      "programs: about 1.7 times as long",
  ],
];
