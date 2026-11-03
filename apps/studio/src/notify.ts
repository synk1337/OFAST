// The end of a long job (a round sent to Unreal, its cameras updated, a render), for a user who went to do something
// else meanwhile: the Studio's sound (sounds/done.mp3), and its taskbar button flashing until the window is looked at.
// While the job runs, the taskbar button shows its progress too (SendBar).
import { ProgressBarStatus, UserAttentionType, getCurrentWindow } from "@tauri-apps/api/window";
import doneSound from "./sounds/done.mp3";

const sound = new Audio(doneSound);

/** A job ended, done or failed: plays the sound and flashes the taskbar button if the window is not in front. */
export function jobEnded() {
  sound.currentTime = 0;
  // A webview that refuses to play (no click in the window yet) stays silent; the job's bar still says it.
  sound.play().catch(() => {});
  try {
    const window = getCurrentWindow();
    window.isFocused().then((focused) => {
      if (!focused) window.requestUserAttention(UserAttentionType.Informational).catch(() => {});
    });
  } catch {
    // Outside Tauri (the front end alone in a browser): no window to flash.
  }
}

/** The job's progress on the taskbar button: `share` done (0 to 1), `failed` in red, or none once `share` is null. */
export function taskbarProgress(share: number | null, failed = false) {
  try {
    getCurrentWindow()
      .setProgressBar(
        share === null
          ? { status: ProgressBarStatus.None }
          : { status: failed ? ProgressBarStatus.Error : ProgressBarStatus.Normal, progress: Math.round(share * 100) },
      )
      .catch(() => {});
  } catch {
    // Outside Tauri: no taskbar.
  }
}
