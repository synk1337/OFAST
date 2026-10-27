// A demo's page: its rounds (winner, score, the round's best moments), a finder for the moments worth filming
// (highlights, or every kill, by player and weapon), and the chosen round's kill feed with each kill's camera.
import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CameraPlan,
  DemoEntry,
  DemoSummary,
  Kill,
  NO_PLAN,
  RenderPreset,
  RoundSummary,
  ShotKind,
  Side,
  sentRounds,
  summarizeDemo,
} from "./api";
import { RadarView } from "./RadarView";
import {
  HIGHLIGHT_FILTERS,
  RENDER_PRESETS,
  SHOTS,
  SMOOTHING,
  WEAPON_FILTERS,
  clock,
  highlightName,
  outcome,
  tagName,
  weaponName,
} from "./names";

/** What the Studio does with a round in Unreal; each missing while Unreal or its project is not found, or while a
 * round is on its way. */
export interface UnrealActions {
  /** Sends a round with its cameras. */
  send?: (round: RoundSummary, plan: CameraPlan) => void;
  /** Changes the cameras of a round sent before. */
  updateCameras?: (round: RoundSummary, plan: CameraPlan) => void;
  /** Renders a round sent before, between two demo ticks or whole (null). */
  render?: (round: RoundSummary, preset: RenderPreset, ticks: [number, number] | null) => void;
  /** Changes when a job ends: the rounds in Unreal are looked up again. */
  version: number;
}

export function DemoPage({
  demo,
  onBack,
  unreal,
}: {
  demo: DemoEntry;
  onBack: () => void;
  unreal: UnrealActions;
}) {
  const [summary, setSummary] = useState<DemoSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    summarizeDemo(demo.path).then(
      (s) => current && setSummary(s),
      (e) => current && setError(String(e)),
    );
    return () => {
      current = false;
    };
  }, [demo.path]);

  return (
    <div className="demo-page">
      <div className="demo-head">
        <button onClick={onBack}>← Demos</button>
        <h2>{demo.name}</h2>
        <span className="muted">
          {demo.map} · {demo.server}
        </span>
        {summary && <FinalScore summary={summary} />}
      </div>
      {error && <p className="error">{error}</p>}
      {!summary && !error && (
        <p className="muted">Reading the demo… A whole match takes a few seconds; it is remembered afterwards.</p>
      )}
      {summary && <DemoView summary={summary} path={demo.path} unreal={unreal} />}
    </div>
  );
}

/** The two teams over the match: each round's players decide which side a team played (sides swap at half time). */
interface Teams {
  names: [string, string];
  /** The side team 0 played in a round. */
  sideOf: (round: RoundSummary) => Side;
}

function teamsOf(summary: DemoSummary): Teams {
  const first = summary.rounds[0];
  const team0 = new Set(first?.players.filter((p) => p.side === "t").map((p) => p.player));
  const sideOf = (round: RoundSummary): Side => {
    let t = 0;
    let ct = 0;
    for (const p of round.players) if (team0.has(p.player)) p.side === "t" ? t++ : ct++;
    return ct > t ? "ct" : "t";
  };
  // A round that knows the clan names (tournament demos); matchmaking has none.
  const named = summary.rounds.find((r) => r.t.name || r.ct.name);
  const names: [string, string] = ["Team A", "Team B"];
  if (named) {
    const side = sideOf(named);
    names[0] = named[side].name || names[0];
    names[1] = named[side === "t" ? "ct" : "t"].name || names[1];
  }
  return { names, sideOf };
}

/** Team 0's and team 1's score after a round. */
function scoreAfter(teams: Teams, round: RoundSummary): [number, number] {
  const side = teams.sideOf(round);
  return [round[side].score, round[side === "t" ? "ct" : "t"].score];
}

function FinalScore({ summary }: { summary: DemoSummary }) {
  const teams = teamsOf(summary);
  const last = [...summary.rounds].reverse().find((r) => r.winner && !r.restarted);
  if (!last) return null;
  const [a, b] = scoreAfter(teams, last);
  return (
    <span className="final-score">
      {teams.names[0]} <b>{a}</b> – <b>{b}</b> {teams.names[1]}
    </span>
  );
}

function DemoView({ summary, path, unreal }: { summary: DemoSummary; path: string; unreal: UnrealActions }) {
  const teams = useMemo(() => teamsOf(summary), [summary]);
  // Rounds go by their index in the summary: a restarted round's number comes again.
  const [selected, setSelected] = useState(0);
  // Each round's cameras as picked (by round index), and the round numbers already in Unreal.
  const [plans, setPlans] = useState<Record<number, CameraPlan>>({});
  const [sent, setSent] = useState<number[]>([]);
  useEffect(() => {
    let current = true;
    sentRounds(path).then(
      (rounds) => current && setSent(rounds),
      () => {},
    );
    return () => {
      current = false;
    };
  }, [path, unreal.version]);
  const plan = plans[selected] ?? NO_PLAN;
  const setPlan = (p: CameraPlan) => setPlans((all) => ({ ...all, [selected]: p }));
  const [marked, setMarked] = useState<{ round: number; kills: number[] } | null>(null);
  // The 2D replay instead of the finder, and the kill it was last asked to jump to.
  const [replay, setReplay] = useState(false);
  const [seek, setSeek] = useState<{ tick: number; n: number } | null>(null);
  const round = summary.rounds[selected] ?? null;

  const pick = (round: number, kills: number[]) => {
    setSelected(round);
    setMarked({ round, kills });
  };

  return (
    <div className="demo-body">
      <RoundList
        summary={summary}
        teams={teams}
        selected={selected}
        onSelect={(r) => {
          setSelected(r);
          setMarked(null);
        }}
      />
      <div className="demo-main">
        {!replay && <Finder summary={summary} onPick={pick} />}
        {round && (
          <div className={replay ? "round-area replay" : "round-area"}>
            {replay && <RadarView key={selected} path={path} summary={summary} roundIndex={selected} seek={seek} />}
            <RoundFeed
              summary={summary}
              teams={teams}
              round={round}
              marked={marked?.round === selected ? marked.kills : []}
              replay={replay}
              onReplay={() => setReplay((r) => !r)}
              plan={plan}
              onPlan={setPlan}
              onSend={unreal.send && (() => unreal.send?.(round, plan))}
              onUpdateCameras={
                unreal.updateCameras && sent.includes(round.round)
                  ? () => unreal.updateCameras?.(round, plan)
                  : undefined
              }
              onRender={
                unreal.render && sent.includes(round.round)
                  ? (preset, ticks) => unreal.render?.(round, preset, ticks)
                  : undefined
              }
              onKill={replay ? (tick) => setSeek((s) => ({ tick, n: (s?.n ?? 0) + 1 })) : undefined}
            />
          </div>
        )}
      </div>
    </div>
  );
}

function RoundList({
  summary,
  teams,
  selected,
  onSelect,
}: {
  summary: DemoSummary;
  teams: Teams;
  selected: number;
  onSelect: (round: number) => void;
}) {
  return (
    <section className="panel rounds">
      <div className="round-row head muted">
        <span>Round</span>
        <span className="num">
          {short(teams.names[0])} – {short(teams.names[1])}
        </span>
        <span />
      </div>
      {summary.rounds.map((r, index) => {
        const [a, b] = scoreAfter(teams, r);
        const best = summary.highlights.filter(
          (h) => h.round_index === index && (h.kind === "multi_kill" || h.kind === "clutch"),
        );
        const classes = ["round-row", index === selected && "selected", r.restarted && "restarted"];
        return (
          <div
            key={index}
            className={classes.filter(Boolean).join(" ")}
            onClick={() => onSelect(index)}
            title={
              outcome(r, r.winner?.toUpperCase() ?? "") + (r.restarted ? " (played again, did not count)" : "")
            }
          >
            <span>
              <span className={`side-dot ${r.winner ?? "none"}`} />
              {r.round}
              {r.restarted && "*"}
            </span>
            <span className="num">{r.winner ? `${a} – ${b}` : "–"}</span>
            <span className="badges">
              {best.map((h, i) => (
                <span key={i} className={`badge ${h.kind}`}>
                  {highlightName(h)}
                </span>
              ))}
            </span>
          </div>
        );
      })}
    </section>
  );
}

/** The first word of a long team name, for narrow columns. */
const short = (name: string) => (name.length > 10 ? name.split(" ")[0].slice(0, 10) : name);

function Finder({ summary, onPick }: { summary: DemoSummary; onPick: (round: number, kills: number[]) => void }) {
  const [tab, setTab] = useState<"highlights" | "kills">("highlights");
  const [kind, setKind] = useState("");
  const [player, setPlayer] = useState(-1);
  const [weapon, setWeapon] = useState("");
  const weaponFilter = WEAPON_FILTERS.find((w) => w.value === weapon) ?? WEAPON_FILTERS[0];
  const kindFilter = HIGHLIGHT_FILTERS.find((k) => k.value === kind) ?? HIGHLIGHT_FILTERS[0];
  const matchesWeapon = (k: Kill) => weaponFilter.matches(k.weapon, k.weapon_class);
  const time = (round: RoundSummary, tick: number) =>
    clock((tick - (round.freeze_end_tick ?? round.start_tick)) * summary.tick_interval);

  const highlights = summary.highlights.filter(
    (h) =>
      kindFilter.matches(h) &&
      (player < 0 || h.player === player) &&
      (weapon === "" || h.kills.some((k) => matchesWeapon(summary.rounds[h.round_index].kills[k]))),
  );
  const kills = summary.rounds.flatMap((r, roundIndex) =>
    r.kills
      .map((k, i) => ({ round: r, roundIndex, kill: k, index: i }))
      .filter(({ kill }) => (player < 0 || kill.attacker === player) && matchesWeapon(kill) && !kill.team_kill),
  );
  // Players by name for the filter.
  const players = summary.players.map((p, i) => ({ ...p, index: i })).sort((a, b) => a.name.localeCompare(b.name));

  return (
    <section className="panel finder">
      <div className="panel-head">
        <div className="tabs">
          <button className={tab === "highlights" ? "on" : ""} onClick={() => setTab("highlights")}>
            Highlights
          </button>
          <button className={tab === "kills" ? "on" : ""} onClick={() => setTab("kills")}>
            All kills
          </button>
        </div>
        <select value={player} onChange={(e) => setPlayer(Number(e.target.value))}>
          <option value={-1}>All players</option>
          {players.map((p) => (
            <option key={p.index} value={p.index}>
              {p.name}
            </option>
          ))}
        </select>
        <select value={weapon} onChange={(e) => setWeapon(e.target.value)}>
          {WEAPON_FILTERS.map((w) => (
            <option key={w.value} value={w.value}>
              {w.label}
            </option>
          ))}
        </select>
      </div>
      {tab === "highlights" && (
        <div className="chips">
          {HIGHLIGHT_FILTERS.map((k) => (
            <button key={k.value} className={kind === k.value ? "chip on" : "chip"} onClick={() => setKind(k.value)}>
              {k.label}
            </button>
          ))}
        </div>
      )}
      <div className="finder-list">
        {tab === "highlights" &&
          (highlights.length === 0 ? (
            <p className="muted">No highlight matches.</p>
          ) : (
            highlights.map((h, i) => {
              const round = summary.rounds[h.round_index];
              const weapons = [...new Set(h.kills.map((k) => weaponName(round.kills[k].weapon)))];
              return (
                <div key={i} className="finder-row" onClick={() => onPick(h.round_index, h.kills)}>
                  <span className="muted">
                    R{round.round}
                    {round.restarted && "*"}
                  </span>
                  <span className="muted">{time(round, h.start_tick)}</span>
                  <span className="name">{summary.players[h.player]?.name}</span>
                  <span className={`badge ${h.kind}`}>{highlightName(h)}</span>
                  <span className="muted">{weapons.join(", ")}</span>
                </div>
              );
            })
          ))}
        {tab === "kills" &&
          (kills.length === 0 ? (
            <p className="muted">No kill matches.</p>
          ) : (
            kills.map(({ round, roundIndex, kill, index }) => (
              <div
                key={`${roundIndex}-${index}`}
                className="finder-row"
                onClick={() => onPick(roundIndex, [index])}
              >
                <span className="muted">
                  R{round.round}
                  {round.restarted && "*"}
                </span>
                <span className="muted">{time(round, kill.tick)}</span>
                <span className="name">{name(summary, kill.attacker)}</span>
                <span>{weaponName(kill.weapon)}</span>
                <span className="muted">{name(summary, kill.victim)}</span>
              </div>
            ))
          ))}
      </div>
    </section>
  );
}

const name = (summary: DemoSummary, player: number | null) =>
  player === null ? "World" : (summary.players[player]?.name ?? "?");

function RoundFeed({
  summary,
  teams,
  round,
  marked,
  replay,
  onReplay,
  plan,
  onPlan,
  onKill,
  onSend,
  onUpdateCameras,
  onRender,
}: {
  summary: DemoSummary;
  teams: Teams;
  round: RoundSummary;
  marked: number[];
  /** The 2D replay is shown beside the feed. */
  replay: boolean;
  onReplay: () => void;
  /** The round's cameras as picked. */
  plan: CameraPlan;
  onPlan: (plan: CameraPlan) => void;
  /** A kill clicked: the replay jumps there. */
  onKill?: (tick: number) => void;
  /** Sends the round to Unreal (missing when it cannot be now). */
  onSend?: () => void;
  /** Changes its cameras in Unreal (missing when it cannot be now, or the round was not sent). */
  onUpdateCameras?: () => void;
  /** Renders it in Unreal (missing when it cannot be now, or the round was not sent). */
  onRender?: (preset: RenderPreset, ticks: [number, number] | null) => void;
}) {
  const setShot = (tick: number, kind: ShotKind) => {
    const shots = { ...plan.shots };
    if (kind === "pov") delete shots[tick];
    else shots[tick] = kind;
    onPlan({ ...plan, shots });
  };
  const addHlae = async () => {
    const picked = await open({
      multiple: true,
      title: "HLAE camera files",
      filters: [{ name: "HLAE campath or camio recording", extensions: ["xml", "cam"] }],
    });
    const files = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (files.length) onPlan({ ...plan, hlae: [...plan.hlae, ...files.filter((f) => !plan.hlae.includes(f))] });
  };
  const sides = new Map(round.players.map((p) => [p.player, p.side]));
  const side0 = teams.sideOf(round);
  const [a, b] = scoreAfter(teams, round);
  const start = round.freeze_end_tick ?? round.start_tick;
  const who = (player: number | null) => (
    <span className={`player ${player === null ? "" : (sides.get(player) ?? "")}`}>{name(summary, player)}</span>
  );
  return (
    <section className="panel feed">
      <div className="panel-head">
        <h2>Round {round.round}</h2>
        <span className="muted">
          {outcome(round, round.winner ? teams.names[round.winner === side0 ? 0 : 1] : "")}
          {round.decided_tick !== null &&
            ` at ${clock((round.decided_tick - start) * summary.tick_interval)} · ${teams.names[0]} ${a} – ${b} ${teams.names[1]}`}
          {round.restarted && " · played again, did not count"}
        </span>
        <span className="muted hint" title="CS2's own clock counts down; these count up from the end of freeze time.">
          Times from the end of freeze time
        </span>
        <button className={replay ? "on" : ""} onClick={onReplay}>
          {replay ? "Back to the finder" : "2D replay"}
        </button>
        <button
          className="primary"
          onClick={onSend}
          disabled={!onSend}
          title={
            onSend
              ? "Export this round with its players and build it in OFAST's Unreal project"
              : "Needs Unreal Engine 5.8 and OFAST's Unreal project, and no other round on its way"
          }
        >
          Send to Unreal
        </button>
      </div>
      <div className="cameras-row">
        <label title="Smooths the mouse's jitter out of the players' own views; the aim stays exact at every shot">
          First-person view{" "}
          <select value={plan.smoothing} onChange={(e) => onPlan({ ...plan, smoothing: Number(e.target.value) })}>
            {SMOOTHING.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        </label>
        <button onClick={addHlae} title="A camera path made in CS2 with HLAE (mirv_campath .xml or mirv_camio .cam)">
          HLAE camera…
        </button>
        {plan.hlae.map((file) => (
          <span key={file} className="chip on" title={file}>
            {file.split(/[\\/]/).pop()}{" "}
            <span
              className="remove"
              onClick={() => onPlan({ ...plan, hlae: plan.hlae.filter((f) => f !== file) })}
              title="Remove"
            >
              ×
            </span>
          </span>
        ))}
        <button
          onClick={onUpdateCameras}
          disabled={!onUpdateCameras}
          title={
            onUpdateCameras
              ? "Change this round's cameras in Unreal without sending it again (about a minute)"
              : "For a round already sent to Unreal"
          }
        >
          Update cameras
        </button>
      </div>
      <RenderRow key={round.start_tick} summary={summary} round={round} marked={marked} onRender={onRender} />
      {round.kills.length === 0 && <p className="muted">No kills.</p>}
      {round.kills.map((k, i) => (
        <div
          key={i}
          className={["kill", marked.includes(i) && "marked", onKill && "clickable"].filter(Boolean).join(" ")}
          onClick={onKill && (() => onKill(k.tick))}
          title={onKill && "Replay from a few seconds before"}
        >
          <span className="muted time">{clock((k.tick - start) * summary.tick_interval)}</span>
          {who(k.attacker)}
          {k.assister !== null && (
            <span className="muted">
              {" "}
              + {k.flash_assist ? "flash " : ""}
              {name(summary, k.assister)}
            </span>
          )}
          <span className="weapon">
            {weaponName(k.weapon)}
            {k.headshot && <span className="flag">headshot</span>}
            {k.wallbang && <span className="flag">{tagName("wallbang")}</span>}
            {k.noscope && <span className="flag">{tagName("noscope")}</span>}
            {k.through_smoke && <span className="flag">{tagName("through_smoke")}</span>}
            {k.blind && <span className="flag">{tagName("blind")}</span>}
            {k.airborne && <span className="flag">{tagName("airborne")}</span>}
            {k.team_kill && <span className="flag bad">team kill</span>}
          </span>
          {who(k.victim)}
          {k.attacker !== null && (
            <select
              className="shot"
              value={plan.shots[k.tick] ?? "pov"}
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => setShot(k.tick, e.target.value as ShotKind)}
              title="The camera this kill is seen through in Unreal: the last 4 seconds before it"
            >
              {SHOTS.filter(([kind]) => kind !== "victim" || k.victim !== null).map(([kind, label]) => (
                <option key={kind} value={kind}>
                  {label}
                </option>
              ))}
            </select>
          )}
        </div>
      ))}
    </section>
  );
}

/** Seconds a render "around the kills" starts before a round's first kill (a shot camera's length) and ends after
 * its last. */
const BEFORE_KILLS_S = 4;
const AFTER_KILLS_S = 2;

/** A round's render: the preset, how much of the round (around the kills marked by the finder, else all of them),
 * and the button (Movie Render Queue, in Unreal). */
function RenderRow({
  summary,
  round,
  marked,
  onRender,
}: {
  summary: DemoSummary;
  round: RoundSummary;
  /** The finder's kills in the round (indices into its kills). */
  marked: number[];
  onRender?: (preset: RenderPreset, ticks: [number, number] | null) => void;
}) {
  const [preset, setPreset] = useState<RenderPreset>("preview");
  const [whole, setWhole] = useState(false);
  const kills = (marked.length ? marked.map((i) => round.kills[i]) : round.kills).map((k) => k.tick);
  const seconds = (s: number) => Math.round(s / summary.tick_interval);
  const ticks: [number, number] | null =
    whole || kills.length === 0
      ? null
      : [
          Math.max(round.start_tick, Math.min(...kills) - seconds(BEFORE_KILLS_S)),
          Math.min(round.end_tick, Math.max(...kills) + seconds(AFTER_KILLS_S)),
        ];
  const length = (ticks ? ticks[1] - ticks[0] : round.end_tick - round.start_tick) * summary.tick_interval;
  return (
    <div className="cameras-row">
      <label title={RENDER_PRESETS.find(([p]) => p === preset)?.[2]}>
        Render{" "}
        <select value={preset} onChange={(e) => setPreset(e.target.value as RenderPreset)}>
          {RENDER_PRESETS.map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </label>
      <select
        value={ticks ? "kills" : "round"}
        onChange={(e) => setWhole(e.target.value === "round")}
        title={`From ${BEFORE_KILLS_S} seconds before the first kill to ${AFTER_KILLS_S} after the last, or all of it`}
      >
        <option value="kills" disabled={kills.length === 0}>
          {marked.length ? "around the marked kills" : "around the kills"}
        </option>
        <option value="round">the whole round</option>
      </select>
      <span className="muted">{clock(length)}</span>
      <button
        onClick={onRender && (() => onRender(preset, ticks))}
        disabled={!onRender}
        title={
          onRender
            ? "Render this round with its cameras in Unreal, into OFAST in your Videos folder (Unreal must be closed)"
            : "For a round already sent to Unreal, while nothing else runs in Unreal"
        }
      >
        Render
      </button>
    </div>
  );
}
