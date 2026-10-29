// Home › Generate shell and Modify › Generate shell: which parts it acts on, running it in Rust (src/shell/ipc.ts)
// with progress and Cancel, and adding the results. It follows the mesh operations' rules (src/ops/meshOps.ts): one
// operation at a time, none in repair mode, nothing added when the parts changed meanwhile or the run was cancelled,
// and all results one undo step.
import { isTauri } from "@tauri-apps/api/core";
import { errorMessage } from "../io/files";
import {
  commit,
  isStale,
  meshOperationRunning,
  runMeshOperation,
  scopeTargets,
  targetProblem,
  WAIT_MESSAGE,
  type OpWords,
  type Plan,
} from "../ops/meshOps";
import { CANCELLED } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST, enterRepair, isRepairing } from "../repair/session";
import { useStore, type Part } from "../store";
import { toast, useUi } from "../ui";
import { generateShells, type ShellReply } from "./ipc";
import { refusalText, resultName, saveFields } from "./settings";
import type { ShellProgress, ShellSettings } from "./types";

export const SHELL_WORDS: OpWords = { verb: "make a shell of", past: "used for a shell" };

/** The undo step's name. */
export const SHELL_LABEL = "Generate shell";

/** The parts a run acts on: the selected parts that are shown and unlocked, in tree order. */
export function shellTargets(): Part[] {
  return scopeTargets(useStore.getState(), "selected").parts;
}

/** Generate shell's button: opens the dialog when there are parts to act on, else says why not. */
export function openGenerateShell(): void {
  if (isRepairing()) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  if (meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return;
  }
  const problem = targetProblem(scopeTargets(useStore.getState(), "selected"), SHELL_WORDS, "selected");
  if (problem) {
    toast(problem.message, problem.kind);
    return;
  }
  useUi.getState().openDialog({ kind: "generate-shell" });
}

/** A part the run would not touch, with the sentence saying why. */
export interface Refused {
  id: string;
  text: string;
}

/** How a run ended, for the dialog. */
export type RunOutcome =
  | { kind: "done"; made: number; warnings: string[] }
  | { kind: "refused"; parts: Refused[] }
  | { kind: "cancelled" }
  | { kind: "failed"; message: string };

/**
 * The results of a run as one plan: per input part that gave a result, a group that puts it right after the part, or
 * in its place with `removeOriginal`. Parts that gave nothing are left alone, even with `removeOriginal`.
 */
export function planShells(inputs: readonly Part[], reply: Extract<ShellReply, { kind: "made" }>, settings: ShellSettings, removeOriginal: boolean): Plan {
  const plan: Plan = { groups: [] };
  inputs.forEach((p, i) => {
    const result = reply.results[i]?.result;
    if (!result) return;
    plan.groups.push({
      removeIds: removeOriginal ? [p.id] : [],
      after: removeOriginal ? undefined : p.id,
      made: [{ name: resultName(p.name, settings.kind), result }],
    });
  });
  return plan;
}

/** The warnings of every part, named by part when there are several. */
export function collectWarnings(inputs: readonly Part[], reply: Extract<ShellReply, { kind: "made" }>): string[] {
  return inputs.flatMap((p, i) => (reply.results[i]?.warnings ?? []).map((w) => (inputs.length > 1 ? `${p.name}: ${w}` : w)));
}

/**
 * Runs Generate shell on the targets and adds the results as one undo step "Generate shell". `job` is what
 * cancelRepair(job) stops it with. `stopped` says whether the user cancelled: when the run ends anyway (the cancel
 * reached Rust after it was done, or before it had started), nothing is added. Remembers the settings for the next
 * time the dialog opens.
 */
export async function runGenerateShell(
  settings: ShellSettings,
  removeOriginal: boolean,
  job: number,
  onProgress?: (p: ShellProgress) => void,
  stopped: () => boolean = () => false,
): Promise<RunOutcome> {
  if (!isTauri()) return { kind: "failed", message: "Generate shell needs the tricky3D desktop app." };
  if (isRepairing()) return { kind: "failed", message: CLOSE_REPAIR_FIRST };
  if (meshOperationRunning()) return { kind: "failed", message: WAIT_MESSAGE };
  const s = useStore.getState();
  const inputs = shellTargets();
  const problem = targetProblem(scopeTargets(s, "selected"), SHELL_WORDS, "selected");
  if (problem) return { kind: "failed", message: problem.message };
  saveFields(settings, removeOriginal);
  const generation = s.generation;
  let outcome: RunOutcome = { kind: "cancelled" };
  const parts = inputs.length === 1 ? "1 part" : `${inputs.length} parts`;
  await runMeshOperation(`Generating shells of ${parts}`, "Could not generate the shell", async () => {
    try {
      const reply = await generateShells(
        inputs.map((p) => ({ name: p.name, mesh: p.mesh, matrix: p.matrix })),
        settings,
        job,
        onProgress,
      );
      if (stopped()) {
        outcome = { kind: "cancelled" };
        return;
      }
      if (reply.kind === "refused") {
        outcome = { kind: "refused", parts: reply.refused.map((r) => ({ id: inputs[r.source].id, text: refusalText(inputs[r.source].name, r) })) };
        return;
      }
      if (isStale(inputs, generation, useStore.getState())) {
        outcome = { kind: "failed", message: "Generate shell was stopped because the parts changed while it ran." };
        return;
      }
      const plan = planShells(inputs, reply, settings, removeOriginal);
      if (plan.groups.length) commit(plan, inputs, SHELL_LABEL);
      outcome = { kind: "done", made: plan.groups.length, warnings: collectWarnings(inputs, reply) };
    } catch (err) {
      outcome = err === CANCELLED ? { kind: "cancelled" } : { kind: "failed", message: errorMessage(err, "Could not generate the shell") };
    }
  });
  return outcome;
}

/** "Repair part" on a refusal: repair mode for the part, or the choice of part when several were refused. */
export function repairRefused(ids: string[]): void {
  if (ids.length === 1) enterRepair(ids[0]);
  else useUi.getState().openDialog({ kind: "repair-choose-part", ids });
}
