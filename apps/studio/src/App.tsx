// The Studio's window: the setup checks on top, then the demo list. Opening a demo leads to its page (rounds, kill
// feed, highlights: DemoPage.tsx).
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CameraPlan,
  DemoEntry,
  Environment,
  Look,
  RenderPreset,
  RoundSummary,
  VersionCheck,
  environment,
  listDemos,
  renderRound,
  sendToUnreal,
  updateCameras,
} from "./api";
import { DemoPage } from "./DemoPage";
import { SendBar, SendJob } from "./SendBar";

export function App() {
  const [env, setEnv] = useState<Environment | null>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const [demos, setDemos] = useState<DemoEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opened, setOpened] = useState<DemoEntry | null>(null);
  // The round on its way to Unreal or being rendered (one at a time: one Unreal), kept while the user browses. Each
  // job gets its own bar.
  const [job, setJob] = useState<SendJob | null>(null);
  const busy = job !== null && !job.sent && !job.folder && !job.error;
  const canSend = Boolean(env?.unreal && env?.project) && !busy;
  // Counts the jobs done: the demo page looks up the rounds in Unreal again.
  const [version, setVersion] = useState(0);
  const run = (heading: string, title: string, work: Promise<Pick<SendJob, "sent" | "folder">>) => {
    const id = Date.now();
    setJob({ id, heading, title });
    work.then(
      (done) => {
        setJob({ id, heading, title, ...done });
        setVersion((v) => v + 1);
      },
      (e) => setJob({ id, heading, title, error: String(e) }),
    );
  };
  const send = (demo: DemoEntry, round: RoundSummary, plan: CameraPlan, look: Look) =>
    run(
      "Send to Unreal",
      `${demo.name}, round ${round.round}`,
      sendToUnreal(demo.path, round.start_tick, plan, look).then((sent) => ({ sent })),
    );
  const cameras = (demo: DemoEntry, round: RoundSummary, plan: CameraPlan) =>
    run(
      "Send to Unreal",
      `${demo.name}, round ${round.round}: cameras`,
      updateCameras(demo.path, round.start_tick, plan).then((sent) => ({ sent })),
    );
  const render = (demo: DemoEntry, round: RoundSummary, preset: RenderPreset, ticks: [number, number] | null) =>
    run(
      "Render",
      `${demo.name}, round ${round.round}, ${preset}`,
      renderRound(demo.path, round.round, preset, ticks).then((folder) => ({ folder })),
    );

  useEffect(() => {
    environment().then(
      (e) => {
        setEnv(e);
        setFolder(e.replays);
      },
      (e) => setError(String(e)),
    );
  }, []);

  useEffect(() => {
    setError(null);
    setDemos(null);
    if (!folder) return;
    // A slow folder's answer arriving after the user picked another one is dropped.
    let current = true;
    listDemos(folder).then(
      (d) => current && setDemos(d),
      (e) => {
        if (!current) return;
        setDemos([]);
        setError(String(e));
      },
    );
    return () => {
      current = false;
    };
  }, [folder]);

  const pickFolder = async () => {
    const picked = await open({ directory: true, title: "Folder with .dem files" });
    if (typeof picked === "string") setFolder(picked);
  };

  return (
    <div className="app">
      <header className="top">
        <span className="logo">OFAST</span>
        <span className="subtitle">Studio</span>
      </header>
      {opened ? (
        <main className="content">
          <DemoPage
            demo={opened}
            onBack={() => setOpened(null)}
            unreal={{
              send: canSend ? (round, plan, look) => send(opened, round, plan, look) : undefined,
              updateCameras: canSend ? (round, plan) => cameras(opened, round, plan) : undefined,
              render: canSend ? (round, preset, ticks) => render(opened, round, preset, ticks) : undefined,
              version,
            }}
          />
        </main>
      ) : (
        <main className="content">
          <Setup env={env} />
          {error && <p className="error">{error}</p>}
          <section className="panel">
            <div className="panel-head">
              <h2>Demos</h2>
              <span className="muted">{folder ?? "No folder"}</span>
              <button onClick={pickFolder}>Other folder…</button>
            </div>
            {folder ? (
              <DemoList demos={demos} onOpen={setOpened} />
            ) : (
              <p className="muted">CS2's replays folder was not found: pick the folder that holds your demos.</p>
            )}
          </section>
        </main>
      )}
      {job && <SendBar key={job.id} job={job} onClose={() => setJob(null)} />}
    </div>
  );
}

/** One line of the setup checks: green, orange (works, but might break) or red. */
interface Check {
  name: string;
  state: "ok" | "warn" | "missing";
  text: string;
}

/** A check for something found on this computer: its path, or how to get it. */
const found = (name: string, path: string | null, hint: string): Check => ({
  name,
  state: path ? "ok" : "missing",
  text: path ?? hint,
});

/** CS2's version against those OFAST was checked on: a CS2 patch can change what OFAST reads without warning. */
function versionCheck({ installed, synced, checked }: VersionCheck): Check {
  const name = "CS2 version";
  if (!installed) {
    return { name, state: "warn", text: `Unreadable: OFAST might not be synced with this CS2 (checked on ${checked}).` };
  }
  return synced
    ? { name, state: "ok", text: `${installed}: OFAST is synced with this CS2 version.` }
    : {
        name,
        state: "warn",
        text: `${installed}: OFAST is not synced with this CS2 version yet (checked on ${checked}). It might break.`,
      };
}

/** What was found on this computer: CS2 and its version, Unreal Engine 5.8 and OFAST's Unreal project. */
function Setup({ env }: { env: Environment | null }) {
  if (!env) return <section className="panel muted">Looking for CS2 and Unreal…</section>;
  const checks = [
    found("Counter-Strike 2", env.cs2, "Install CS2 through Steam, or set OFAST_CS2_DIR to its folder."),
    ...(env.cs2_version ? [versionCheck(env.cs2_version)] : []),
    found(
      "Unreal Engine 5.8",
      env.unreal,
      "Install Unreal Engine 5.8 from the Epic Games Launcher, or set OFAST_UE_DIR to its folder.",
    ),
    found(
      "OFAST Unreal project",
      env.project,
      "OFAST's Unreal scripts were not found next to the Studio: reinstall OFAST Studio.",
    ),
  ];
  return (
    <section className="panel setup">
      {checks.map(({ name, state, text }) => (
        <div key={name} className={`check ${state}`}>
          <span className="dot" />
          <span className="name">{name}</span>
          <span className="muted">{text}</span>
        </div>
      ))}
    </section>
  );
}

function DemoList({ demos, onOpen }: { demos: DemoEntry[] | null; onOpen: (demo: DemoEntry) => void }) {
  if (!demos) return <p className="muted">Reading demos…</p>;
  if (demos.length === 0) return <p className="muted">No demos here. Download one from CS2's Watch tab.</p>;
  return (
    <table className="demos">
      <thead>
        <tr>
          <th>Demo</th>
          <th>Map</th>
          <th>Server</th>
          <th>Date</th>
          <th className="num">Size</th>
        </tr>
      </thead>
      <tbody>
        {demos.map((d) => (
          <tr key={d.path} title={d.path} onClick={() => onOpen(d)}>
            <td>{d.name}</td>
            <td>{d.map || "?"}</td>
            <td className="muted">{d.server}</td>
            <td>{d.modified ? new Date(d.modified * 1000).toLocaleDateString() : ""}</td>
            <td className="num">{size(d.size)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** A file size for people: "312 MB", "0.4 MB". */
function size(bytes: number): string {
  const mb = bytes / 1e6;
  return `${mb < 10 ? mb.toFixed(1) : mb.toFixed(0)} MB`;
}
