// A round replayed on the map's radar: players (side colour, view direction, name), the dead, grenades in flight,
// smokes and fires, with a timeline (kills marked), play / pause and speed. Clicking a kill in the feed beside it
// jumps there.
import { useEffect, useMemo, useRef, useState } from "react";
import { DemoSummary, RadarRound, RoundSummary, Side, radarImage, radarRound } from "./api";
import { clock, weaponName } from "./names";

/** How long before a kill the replay starts when one is picked, seconds. */
const LEAD_IN = 3;

const SPEEDS = [0.5, 1, 2, 4];

/** The longest step between two drawn frames, milliseconds. */
const MAX_FRAME_MS = 100;

export function RadarView({
  path,
  summary,
  roundIndex,
  seek,
}: {
  path: string;
  summary: DemoSummary;
  roundIndex: number;
  /** A tick to jump to (a kill picked in the feed), with a counter so picking the same kill again jumps again. */
  seek: { tick: number; n: number } | null;
}) {
  const round = summary.rounds[roundIndex];
  const [data, setData] = useState<RadarRound | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [images, setImages] = useState<Record<string, HTMLImageElement>>({});
  const [tick, setTick] = useState(round.freeze_end_tick ?? round.start_tick);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);

  // The round's positions (seconds to read: the demo is read up to the round's end).
  useEffect(() => {
    let current = true;
    setData(null);
    setError(null);
    setTick(round.freeze_end_tick ?? round.start_tick);
    setPlaying(false);
    radarRound(path, round.start_tick).then(
      (d) => current && setData(d),
      (e) => current && setError(String(e)),
    );
    return () => {
      current = false;
    };
  }, [path, round.start_tick, round.freeze_end_tick]);

  // The radar image of the main level (a map on two floors draws its lower level's players faded over it).
  useEffect(() => {
    if (!data?.overview) return;
    let current = true;
    let url: string | null = null;
    radarImage(data.map, "default")
      .then(async (bytes) => {
        if (!current) return;
        url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
        const image = new Image();
        image.src = url;
        await image.decode();
        if (current) setImages({ default: image });
      })
      .catch((e) => current && setError(String(e)));
    return () => {
      current = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [data]);

  // A kill picked in the feed: once the positions are there, play from a few seconds before it. A pick made before
  // this view opened (another round, the replay closed meanwhile) is not replayed.
  const handled = useRef(seek?.n ?? 0);
  useEffect(() => {
    if (!data || !seek || seek.n === handled.current) return;
    handled.current = seek.n;
    if (seek.tick >= round.start_tick && seek.tick <= round.end_tick) {
      setTick(Math.max(round.start_tick, seek.tick - LEAD_IN / summary.tick_interval));
      setPlaying(true);
    }
  }, [data, seek, round.start_tick, round.end_tick, summary.tick_interval]);

  // Playback: advance by the time elapsed, at the chosen speed. A long pause between frames (the window hidden)
  // counts as one frame's worth, so coming back does not jump to the end.
  useEffect(() => {
    if (!playing) return;
    let last = performance.now();
    let frame = 0;
    const step = (now: number) => {
      const seconds = Math.min(now - last, MAX_FRAME_MS) / 1000;
      last = now;
      setTick((t) => {
        const next = t + (seconds / summary.tick_interval) * speed;
        if (next >= round.end_tick) {
          setPlaying(false);
          return round.end_tick;
        }
        return next;
      });
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [playing, speed, summary.tick_interval, round.end_tick]);

  const start = round.freeze_end_tick ?? round.start_tick;
  const play = () => {
    // Play at the end starts again from the end of freeze time.
    if (!playing && tick >= round.end_tick) setTick(start);
    setPlaying((p) => !p);
  };
  return (
    <div className="radar">
      {error && <p className="error">{error}</p>}
      {!data && !error && <p className="muted">Reading the round's positions…</p>}
      {data && !data.overview && <p className="muted">This map has no radar overview in CS2's files.</p>}
      {data?.overview && <RadarCanvas data={data} summary={summary} round={round} images={images} tick={tick} />}
      <div className="radar-controls">
        <button onClick={play} disabled={!data}>
          {playing ? "Pause" : "Play"}
        </button>
        <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))}>
          {SPEEDS.map((s) => (
            <option key={s} value={s}>
              {s}×
            </option>
          ))}
        </select>
        <span className="muted time">{clock((tick - start) * summary.tick_interval)}</span>
        <Timeline round={round} tick={tick} onSeek={setTick} />
      </div>
    </div>
  );
}

/** The slider over the round, with its kills marked. */
function Timeline({ round, tick, onSeek }: { round: RoundSummary; tick: number; onSeek: (tick: number) => void }) {
  const span = round.end_tick - round.start_tick || 1;
  const at = (t: number) => `${((t - round.start_tick) / span) * 100}%`;
  return (
    <div className="timeline">
      <input
        type="range"
        min={round.start_tick}
        max={round.end_tick}
        step={1}
        value={tick}
        onChange={(e) => onSeek(Number(e.target.value))}
      />
      {round.freeze_end_tick !== null && <span className="mark freeze" style={{ left: at(round.freeze_end_tick) }} />}
      {round.kills.map((k, i) => (
        <span key={i} className="mark kill-mark" style={{ left: at(k.tick) }} title={weaponName(k.weapon)} />
      ))}
      {round.decided_tick !== null && <span className="mark decided" style={{ left: at(round.decided_tick) }} />}
    </div>
  );
}

const COLORS: Record<Side, string> = { t: "#e0a03c", ct: "#5b9bd5" };

/** The radar image with everything at `tick` drawn on it. */
function RadarCanvas({
  data,
  summary,
  round,
  images,
  tick,
}: {
  data: RadarRound;
  summary: DemoSummary;
  round: RoundSummary;
  images: Record<string, HTMLImageElement>;
  tick: number;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const overview = data.overview!;
  // Each sampled player's index in the summary's players, by Steam id (bots: by name).
  const playerIndex = useMemo(
    () =>
      data.players.map((p) =>
        summary.players.findIndex((s) => (p.steam_id !== "0" ? s.steam_id === p.steam_id : s.name === p.name)),
      ),
    [data, summary],
  );
  const sides = useMemo(() => new Map(round.players.map((p) => [p.player, p.side])), [round]);
  // The part of the radar the round's players use, as a square with a margin: maps that fill a small part of their
  // image (Nuke) get the whole canvas.
  const view = useMemo(() => framing(data), [data]);

  useEffect(() => {
    const c = canvas.current;
    const g = c?.getContext("2d");
    if (!c || !g) return;
    const size = c.width;
    const k = size / view.size;
    const toCanvas = (x: number, y: number): [number, number] => [
      ((x - overview.pos_x) / overview.scale - view.x) * k,
      ((overview.pos_y - y) / overview.scale - view.y) * k,
    ];
    g.clearRect(0, 0, size, size);
    const main = images["default"];
    // The image is 1024 pixels in the overview's units; one of another size is scaled to them.
    const s = main ? main.naturalWidth / RADAR_SIZE : 1;
    if (main) g.drawImage(main, view.x * s, view.y * s, view.size * s, view.size * s, 0, 0, size, size);

    // The two frames around `tick`, for smooth movement between samples.
    const frames = data.frames;
    let i = frames.findIndex((f) => f.tick > tick);
    if (i < 0) i = frames.length;
    const before = frames[Math.max(0, i - 1)];
    const after = frames[Math.min(frames.length - 1, i)];
    if (!before) return;
    const span = after.tick - before.tick;
    const w = span > 0 ? Math.min(1, Math.max(0, (tick - before.tick) / span)) : 0;

    // Smokes and fires under the players.
    for (const s of before.smokes) {
      const [x, y] = toCanvas(s[0], s[1]);
      g.fillStyle = "rgba(200, 200, 200, 0.45)";
      g.beginPath();
      g.arc(x, y, (144 / overview.scale) * k, 0, Math.PI * 2);
      g.fill();
    }
    for (const f of before.fires) {
      const [x, y] = toCanvas(f[0], f[1]);
      g.fillStyle = "rgba(255, 110, 30, 0.35)";
      g.beginPath();
      g.arc(x, y, (60 / overview.scale) * k, 0, Math.PI * 2);
      g.fill();
    }
    for (const n of before.grenades) {
      const [x, y] = toCanvas(n[0], n[1]);
      g.fillStyle = "#ddd";
      g.fillRect(x - 2, y - 2, 4, 4);
    }

    // Players: the earlier frame's state (alive or not), moved between the two samples.
    g.font = "11px Segoe UI, sans-serif";
    for (const p of before.players) {
      const next = after.players.find((q) => q.slot === p.slot) ?? p;
      const px = p.x + (next.x - p.x) * w;
      const py = p.y + (next.y - p.y) * w;
      const [x, y] = toCanvas(px, py);
      const index = playerIndex[p.slot];
      const side = sides.get(index) ?? (p.team === 2 ? "t" : "ct");
      // On a map on two floors, a player on the lower level is drawn faded over the main image.
      const below = !["", "default"].includes(sectionOf(overview, p.z));
      if (!p.alive) {
        g.strokeStyle = "rgba(220, 220, 220, 0.7)";
        g.lineWidth = 2;
        g.beginPath();
        g.moveTo(x - 4, y - 4);
        g.lineTo(x + 4, y + 4);
        g.moveTo(x + 4, y - 4);
        g.lineTo(x - 4, y + 4);
        g.stroke();
        continue;
      }
      const yaw = (-p.yaw * Math.PI) / 180;
      g.globalAlpha = below ? 0.45 : 1;
      g.strokeStyle = COLORS[side];
      g.lineWidth = 2;
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + Math.cos(yaw) * 14, y + Math.sin(yaw) * 14);
      g.stroke();
      g.fillStyle = COLORS[side];
      g.beginPath();
      g.arc(x, y, 6, 0, Math.PI * 2);
      g.fill();
      g.fillStyle = "#fff";
      g.fillText(summary.players[index]?.name ?? data.players[p.slot]?.name ?? "", x + 8, y - 6);
      g.globalAlpha = 1;
    }
  }, [data, summary, images, tick, overview, playerIndex, sides, view]);

  return <canvas ref={canvas} className="radar-canvas" width={720} height={720} />;
}

/** The name of the level a height is on ("" for a map on one level). */
function sectionOf(overview: NonNullable<RadarRound["overview"]>, z: number): string {
  return overview.sections.find((s) => z >= s.altitude_min && z < s.altitude_max)?.name ?? "";
}

/** The radar image's size, pixels per side. */
const RADAR_SIZE = 1024;

/** Margin around the players' area, radar pixels. */
const MARGIN = 48;

/** The square of the radar image (top-left corner and side, in its pixels) that holds every position of the round. */
function framing(data: RadarRound): { x: number; y: number; size: number } {
  const overview = data.overview;
  const whole = { x: 0, y: 0, size: RADAR_SIZE };
  if (!overview) return whole;
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const frame of data.frames) {
    for (const p of frame.players) {
      const x = (p.x - overview.pos_x) / overview.scale;
      const y = (overview.pos_y - p.y) / overview.scale;
      [x0, y0, x1, y1] = [Math.min(x0, x), Math.min(y0, y), Math.max(x1, x), Math.max(y1, y)];
    }
  }
  if (!Number.isFinite(x0)) return whole;
  const size = Math.min(RADAR_SIZE, Math.max(x1 - x0, y1 - y0, 256) + 2 * MARGIN);
  const clamp = (v: number) => Math.min(Math.max(v, 0), RADAR_SIZE - size);
  return { x: clamp((x0 + x1) / 2 - size / 2), y: clamp((y0 + y1) / 2 - size / 2), size };
}
