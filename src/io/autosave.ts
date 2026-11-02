// Autosave and recovery. Every few minutes (File › Autosave, remembered) the open project, when it has changes that are
// not in a file and has changed since the last autosave, goes to Rust, which writes it into tricky3D's own data folder,
// never over the user's file, and keeps the newest three (src-tauri/src/autosave.rs). When the app starts after a run
// that did not end normally, it offers to restore that run's latest autosave.
import { isTauri } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { isDirty, useStore, type AppState, type Part, type Vec3 } from "../store";
import { toast } from "../ui";
import { errorMessage, loadRecovered, projectMesh } from "./files";
import { autosaveProject, autosaveRecovery, dismissAutosave, openAutosave, type Recovery } from "./ipc";

/** What the last autosave held, by identity: the store replaces parts and platform on every change. */
export interface Autosaved {
  parts: readonly Part[];
  platform: Vec3;
}

/** True when an autosave is due: the project has parts, changes not in a file, and changed since `last`. */
export function autosaveDue(s: Pick<AppState, "parts" | "platform" | "saved">, last: Autosaved | null): boolean {
  if (s.parts.length === 0 || !isDirty(s as AppState)) return false;
  return !last || last.parts !== s.parts || last.platform !== s.platform;
}

/** The sentence offering a crashed run's autosave. */
export function recoveryQuestion(r: Recovery, now = new Date()): string {
  const at = new Date(r.savedAt);
  const sameDay = at.toDateString() === now.toDateString();
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const when = sameDay ? `at ${time}` : `on ${at.toLocaleDateString()} at ${time}`;
  return `tricky3D did not close properly last time. Restore the project it saved automatically ${when}?`;
}

let last: Autosaved | null = null;
let writing = false;

/** Writes an autosave now if one is due. Resolves true when it wrote one. */
export async function autosaveNow(write = autosaveProject): Promise<boolean> {
  const s = useStore.getState();
  if (writing || !autosaveDue(s, last)) return false;
  writing = true;
  const snapshot = { parts: s.parts, platform: s.platform };
  try {
    await write(s.platform, s.parts.map(projectMesh));
    last = snapshot;
    return true;
  } catch (err) {
    // Said once, in the status bar; the next interval tries again.
    toast(errorMessage(err, "Could not save the project automatically"), "warn");
    return false;
  } finally {
    writing = false;
  }
}

/** Starts the autosave timer, following File › Autosave. Returns the function that stops it. */
export function startAutosave(): () => void {
  if (!isTauri()) return () => {};
  let timer = 0;
  const arm = (minutes: number) => {
    window.clearInterval(timer);
    timer = minutes > 0 ? window.setInterval(() => void autosaveNow(), minutes * 60_000) : 0;
  };
  arm(useStore.getState().autosaveMinutes);
  const unsubscribe = useStore.subscribe((s, prev) => {
    if (s.autosaveMinutes !== prev.autosaveMinutes) arm(s.autosaveMinutes);
    // A new or opened project starts over: its first change gets an autosave of its own.
    if (s.generation !== prev.generation) last = null;
  });
  return () => {
    window.clearInterval(timer);
    unsubscribe();
  };
}

/** At start: offers the autosave a crashed run left, and restores it when asked. Asked once either way. */
export async function offerRecovery(): Promise<void> {
  if (!isTauri()) return;
  let offer: Recovery | null;
  try {
    offer = await autosaveRecovery();
  } catch {
    return;
  }
  if (!offer) return;
  let restore = false;
  try {
    restore = await ask(recoveryQuestion(offer), {
      title: "Restore project",
      kind: "info",
      okLabel: "Restore",
      cancelLabel: "Don't restore",
    });
  } catch {
    restore = false;
  }
  if (restore) {
    try {
      loadRecovered(await openAutosave(offer.id), new Date(offer.savedAt));
    } catch (err) {
      toast(errorMessage(err, "Could not restore the project"), "error");
    }
  }
  await dismissAutosave().catch(() => {});
}

/** For tests: forgets the last autosave. */
export function resetAutosave(): void {
  last = null;
  writing = false;
}
