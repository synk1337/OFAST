// "Send to Unreal" and "Render" while they run and once they are done: a bar at the bottom of the window, kept while
// the user browses other demos (the work goes on in the background), with "Open in Unreal" or "Open folder" at the end.
// Its progress bar (and the taskbar button's) follows the job's progress lines: each says how much is done and where
// the bar will be once what runs now is done, and the bar creeps towards there meanwhile, never back.
import { useEffect, useState } from "react";
import { SendProgress, SendStep, Sent, onSendProgress, openInUnreal, openRenderFolder } from "./api";
import { taskbarProgress } from "./notify";

/** A round on its way to Unreal, or being rendered (App starts it: an effect would start it twice in development). */
export interface SendJob {
  /** Which job it is: each gets a bar of its own. */
  id: number;
  /** What it does: "Send to Unreal", "Render". */
  heading: string;
  /** What it does it to, for the bar: "aurora-vs-furia-m1-dust2, round 12". */
  title: string;
  /** Once sent. */
  sent?: Sent;
  /** Once rendered: the frames' folder. */
  folder?: string;
  /** Once failed. */
  error?: string;
}

/** How long "Open in Unreal" stays disabled once pressed. */
const OPENING_MS = 30_000;

/** How the bar creeps towards the next line's place while it waits: over the time the line says it takes (wait_s),
 * never more than this share of the way (the next line says when it is there): about 80 % of it after that time. */
const CREEP_MAX = 0.9;
/** How often the bar moves while it creeps, ms. */
const CREEP_MS = 250;

/** Where the last line put the bar, and when it came. */
interface Target {
  done: number;
  next: number;
  wait_s: number;
  at: number;
}

const STEPS: Record<SendStep, string> = {
  round: "Exporting the round",
  map_export: "Exporting the map",
  map_import: "Importing the map into Unreal",
  round_import: "Building the round in Unreal",
  cameras: "Updating the cameras",
  render: "Rendering",
};

export function SendBar({ job, onClose }: { job: SendJob; onClose: () => void }) {
  const [last, setLast] = useState<SendProgress | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState<string | null>(null);
  const [target, setTarget] = useState<Target>({ done: 0, next: 0, wait_s: 0, at: performance.now() });
  const [shown, setShown] = useState(0);
  const done = Boolean(job.sent || job.folder);
  const ended = done || Boolean(job.error);
  const fill = done ? 1 : shown;

  useEffect(() => {
    const unlisten = onSendProgress((p) => {
      if (p.warning) setWarning(p.message);
      else setLast(p);
      // Unreal's lines often keep the bar where it is: the creep goes on from when it last moved.
      setTarget((t) =>
        t.done === p.done && t.next === p.next
          ? t
          : { done: p.done, next: p.next, wait_s: p.wait_s, at: performance.now() },
      );
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    if (ended) return;
    const creep = () => {
      const waited = (performance.now() - target.at) / 1000;
      const way = 1 - Math.exp((-2 * waited) / Math.max(target.wait_s, 1));
      const crept = target.done + (target.next - target.done) * CREEP_MAX * way;
      setShown((s) => Math.max(s, crept));
    };
    creep();
    const timer = setInterval(creep, CREEP_MS);
    return () => clearInterval(timer);
  }, [target, ended]);

  // The taskbar button: the bar while it runs, red if it failed, nothing once done or closed.
  const percent = Math.floor(fill * 100);
  useEffect(() => {
    taskbarProgress(done ? null : percent / 100, Boolean(job.error));
  }, [percent, done, job.error]);
  useEffect(() => () => taskbarProgress(null), []);

  const open = () => {
    if (!job.sent) return;
    setOpening(true);
    setOpenError(null);
    openInUnreal(job.sent).catch((e) => setOpenError(String(e)));
    // Unreal takes a while to show its window; after that the button opens it again (it was closed, or crashed).
    setTimeout(() => setOpening(false), OPENING_MS);
  };
  const showFolder = () => {
    if (!job.folder) return;
    setOpenError(null);
    openRenderFolder(job.folder).catch((e) => setOpenError(String(e)));
  };
  const error = job.error ?? openError;

  return (
    <div className={`send-bar ${error ? "failed" : done ? "done" : "running"}`}>
      <div className="send-title">
        <b>{job.heading}</b> <span className="muted">{job.title}</span>
      </div>
      <div className="send-state" title={error ?? job.folder ?? last?.message}>
        {error && <span className="error">{error}</span>}
        {!error && job.sent && (
          <span>
            Done: <code>{job.sent.sequence}</code> in <code>{job.sent.level}</code>
          </span>
        )}
        {!error && job.folder && (
          <span>
            Done: <code>{job.folder}</code>
          </span>
        )}
        {!error && !done && (
          <span>
            <b className="send-percent">{percent} %</b> {last ? STEPS[last.step] : "Starting"}…{" "}
            <span className="muted">{last?.message}</span>
          </span>
        )}
        {warning && !done && !job.error && <span className="send-warning">{warning}</span>}
      </div>
      <div
        className="send-progress"
        role="progressbar"
        aria-label={job.heading}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <div className="send-progress-fill" style={{ width: `${fill * 100}%` }} />
      </div>
      {job.sent && (
        <button onClick={open} disabled={opening}>
          {opening ? "Unreal is starting…" : "Open in Unreal"}
        </button>
      )}
      {job.folder && <button onClick={showFolder}>Open folder</button>}
      {(done || job.error) && (
        <button onClick={onClose} title="Close">
          ×
        </button>
      )}
    </div>
  );
}
