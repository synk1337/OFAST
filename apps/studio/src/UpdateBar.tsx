// A newer OFAST Studio: when the Studio starts, Tauri's updater reads the latest GitHub release's latest.json (the
// release workflow writes it, `.github/workflows/release.yml`); if its version is newer, a bar under the header says
// so, with its notes, and "Update" downloads its setup program, checks its signature against the public key in
// tauri.conf.json, installs it over this one and starts it again. Not in development (`npm run app`).
import { ReactElement, useEffect, useState } from "react";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type Stage = { kind: "offered" } | { kind: "downloading"; done: number; total: number | null } | { kind: "installing" };

/** `busy`: a round is being sent or rendered, which installing would stop (the Studio closes to update). */
export function UpdateBar({ busy }: { busy: boolean }) {
  const [update, setUpdate] = useState<Update | null>(null);
  const [stage, setStage] = useState<Stage>({ kind: "offered" });
  const [notes, setNotes] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (import.meta.env.DEV) return;
    // Offline, or GitHub unreachable: no bar, the Studio works as it is.
    check().then(setUpdate, (e) => console.warn("update check:", e));
  }, []);

  if (!update) return null;

  const install = async () => {
    setError(null);
    let done = 0;
    let total: number | null = null;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? null;
          setStage({ kind: "downloading", done, total });
        } else if (event.event === "Progress") {
          done += event.data.chunkLength;
          setStage({ kind: "downloading", done, total });
        } else {
          setStage({ kind: "installing" });
        }
      });
      // On Windows the setup program closes the Studio itself and starts the new one; elsewhere, start it here.
      await relaunch();
    } catch (e) {
      setStage({ kind: "offered" });
      setError(String(e));
    }
  };

  const working = stage.kind !== "offered";
  const state =
    stage.kind === "downloading"
      ? `Downloading… ${stage.total ? `${Math.round((100 * stage.done) / stage.total)} %` : ""}`
      : stage.kind === "installing"
        ? "Installing: the Studio restarts in a moment…"
        : null;

  return (
    <div className={`update-bar ${error ? "failed" : ""}`}>
      <div className="update-line">
        <span>
          <b>OFAST Studio {update.version}</b> is available{" "}
          <span className="muted">(you have {update.currentVersion})</span>
        </span>
        {update.body && (
          <button className="link" onClick={() => setNotes((n) => !n)}>
            {notes ? "Hide what's new" : "What's new"}
          </button>
        )}
        <span className="update-state">{error ? <span className="error">{error}</span> : state}</span>
        <button
          className="primary"
          disabled={busy || working}
          title={busy ? "Wait for the round being sent or rendered: updating closes the Studio." : undefined}
          onClick={install}
        >
          Update
        </button>
        {!working && <button onClick={() => setUpdate(null)}>Later</button>}
      </div>
      {notes && update.body && <Notes text={update.body} />}
    </div>
  );
}

/** The release notes (docs/releases/<version>.md) as the release page shows them, from the little Markdown they use:
 * `## ` headings, `- ` bullets, paragraphs, **bold** and `code`. */
function Notes({ text }: { text: string }) {
  const blocks: ReactElement[] = [];
  let bullets: string[] = [];
  const flush = () => {
    if (bullets.length) {
      blocks.push(
        <ul key={blocks.length}>
          {bullets.map((b, i) => (
            <li key={i}>{inline(b)}</li>
          ))}
        </ul>,
      );
      bullets = [];
    }
  };
  for (const line of text.split("\n").map((l) => l.trim())) {
    if (line.startsWith("- ")) {
      bullets.push(line.slice(2));
      continue;
    }
    flush();
    if (line.startsWith("## ")) blocks.push(<h4 key={blocks.length}>{inline(line.slice(3))}</h4>);
    else if (line) blocks.push(<p key={blocks.length}>{inline(line)}</p>);
  }
  flush();
  return <div className="update-notes">{blocks}</div>;
}

/** **bold** and `code` within a line. */
function inline(line: string) {
  return line
    .split(/(\*\*[^*]+\*\*|`[^`]+`)/)
    .map((part, i) =>
      part.startsWith("**") ? (
        <b key={i}>{part.slice(2, -2)}</b>
      ) : part.startsWith("`") ? (
        <code key={i}>{part.slice(1, -1)}</code>
      ) : (
        part
      ),
    );
}
