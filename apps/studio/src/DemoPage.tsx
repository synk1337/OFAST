// A demo's page: its rounds (winner, score, the round's best moments), a finder for the moments worth filming
// (highlights, or every kill, by player and weapon), and the chosen round's kill feed.
import { useEffect, useMemo, useState } from "react";
import { DemoEntry, DemoSummary, Kill, RoundSummary, Side, summarizeDemo } from "./api";
import {
  HIGHLIGHT_FILTERS,
  WEAPON_FILTERS,
  clock,
  highlightName,
  outcome,
  tagName,
  weaponName,
} from "./names";

export function DemoPage({ demo, onBack }: { demo: DemoEntry; onBack: () => void }) {
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
      {summary && <DemoView summary={summary} />}
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

function DemoView({ summary }: { summary: DemoSummary }) {
  const teams = useMemo(() => teamsOf(summary), [summary]);
  // Rounds go by their index in the summary: a restarted round's number comes again.
  const [selected, setSelected] = useState(0);
  const [marked, setMarked] = useState<{ round: number; kills: number[] } | null>(null);
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
        <Finder summary={summary} onPick={pick} />
        {round && (
          <RoundFeed
            summary={summary}
            teams={teams}
            round={round}
            marked={marked?.round === selected ? marked.kills : []}
          />
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
}: {
  summary: DemoSummary;
  teams: Teams;
  round: RoundSummary;
  marked: number[];
}) {
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
      </div>
      {round.kills.length === 0 && <p className="muted">No kills.</p>}
      {round.kills.map((k, i) => (
        <div key={i} className={marked.includes(i) ? "kill marked" : "kill"}>
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
        </div>
      ))}
    </section>
  );
}
