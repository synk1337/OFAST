// The Studio's window: the setup checks on top, then the demo list. Opening a demo leads to its page (rounds, kill
// feed, highlights), which comes next.
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { DemoEntry, Environment, environment, listDemos } from "./api";

export function App() {
  const [env, setEnv] = useState<Environment | null>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const [demos, setDemos] = useState<DemoEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

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
            <DemoList demos={demos} />
          ) : (
            <p className="muted">CS2's replays folder was not found: pick the folder that holds your demos.</p>
          )}
        </section>
      </main>
    </div>
  );
}

/** What was found on this computer: CS2, Unreal Engine 5.8 and OFAST's Unreal project. */
function Setup({ env }: { env: Environment | null }) {
  if (!env) return <section className="panel muted">Looking for CS2 and Unreal…</section>;
  const items: [string, string | null, string][] = [
    ["Counter-Strike 2", env.cs2, "Install CS2 through Steam, or set OFAST_CS2_DIR to its folder."],
    [
      "Unreal Engine 5.8",
      env.unreal,
      "Install Unreal Engine 5.8 from the Epic Games Launcher, or set OFAST_UE_DIR to its folder.",
    ],
    ["OFAST Unreal project", env.project, "The Studio must run from the OFAST folder."],
  ];
  return (
    <section className="panel setup">
      {items.map(([name, path, hint]) => (
        <div key={name} className={path ? "check ok" : "check missing"}>
          <span className="dot" />
          <span className="name">{name}</span>
          <span className="muted">{path ?? hint}</span>
        </div>
      ))}
    </section>
  );
}

function DemoList({ demos }: { demos: DemoEntry[] | null }) {
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
          <tr key={d.path} title={d.path}>
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
