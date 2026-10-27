// "Send to Unreal" and "Render" while they run and once they are done: a bar at the bottom of the window, kept while
// the user browses other demos (the work goes on in the background), with "Open in Unreal" or "Open folder" at the end.
import { useEffect, useState } from "react";
import { SendProgress, SendStep, Sent, onSendProgress, openInUnreal, openRenderFolder } from "./api";

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
  const done = Boolean(job.sent || job.folder);

  useEffect(() => {
    const unlisten = onSendProgress((p) => (p.warning ? setWarning(p.message) : setLast(p)));
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

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
            {last ? STEPS[last.step] : "Starting"}… <span className="muted">{last?.message}</span>
          </span>
        )}
        {warning && !done && !job.error && <span className="send-warning">{warning}</span>}
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
