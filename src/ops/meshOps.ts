// Modify › Cut, Edit shells › Split shells into parts / Merge parts. Rust (src/io/ipc.ts) and the
// manifold-3d worker (./manifoldClient) do the geometry off the UI thread; this module picks the parts, names and
// colours the results, and commits them. Every decision is a plain exported function so tests run it without Tauri;
// the async glue around them stays thin and follows one rule set for all three operations (and Boolean operations, src/boolean/session.ts):
// - one operation at a time (a second request is refused, not queued: it would act on parts about to be replaced);
// - none while a part is in repair mode (refused with "Close repair mode first."): it could replace that part;
// - the status-bar task appears only once the work has run BUSY_DELAY_MS, and always ends, error or not;
// - results are thrown away when the input parts changed meanwhile, so nothing partial or outdated is ever added;
// - each operation commits exactly one replaceParts, which is one undo step, after priming meshCache for every result
//   (otherwise the viewport would measure problem edges on the main thread).
import { normalized, useClip, type CutScope } from "../clip";
import { browserOnly, countParts, errorMessage } from "../io/files";
import { analyzeMeshes, cutMeshes, mergeMeshes, splitShells, type AnalyzedMesh, type PartMesh } from "../io/ipc";
import { primeMeshCaches } from "../meshCache";
import { CLOSE_REPAIR_FIRST, enterRepair, isRepairing } from "../repair/session";
import { PART_COLORS, useStore, type AppState, type MeshData, type Part } from "../store";
import { leftOutParts, transformTargets, type Targets } from "../transformTools";
import { prompt, startTask, toast, useUi, type SplitAnswer, type TaskHandle, type ToastKind } from "../ui";
import { manifoldSplit } from "./manifoldClient";
import type { CutOptions, CutPlane } from "./types";

// ---------------------------------------------------------------------------------------------
// Targets and messages (pure)

/** How an operation is named in its messages: "Select the parts to <verb> first.", "Locked parts cannot be <past>.". */
export interface OpWords {
  verb: string;
  past: string;
}

export const SPLIT_WORDS: OpWords = { verb: "split", past: "split" };
export const MERGE_WORDS: OpWords = { verb: "merge", past: "merged" };
export const CUT_WORDS: OpWords = { verb: "cut", past: "cut" };

/**
 * The parts an operation acts on, with the ones left out counted: the selected parts that are shown and unlocked
 * (the Move/Rotate rule), or for "Cut all parts" every shown, unlocked part. Tree order either way.
 */
export function scopeTargets(s: Pick<AppState, "parts" | "selection">, scope: CutScope): Targets {
  return transformTargets(scope === "all" ? { parts: s.parts, selection: s.parts.map((p) => p.id) } : s);
}

/**
 * Why the targets are not enough to run on (at least `min` parts), in the words of the transform commands; null
 * when they are. When parts were left out, that is the reason given, since selecting more would not help.
 */
export function targetProblem(t: Targets, words: OpWords, scope: CutScope, min = 1): { message: string; kind: ToastKind } | null {
  if (t.parts.length >= min) return null;
  const { verb, past } = words;
  let reason = "";
  if (t.locked && t.hidden) reason = `Hidden or locked parts cannot be ${past}.`;
  else if (t.locked) reason = `Locked parts cannot be ${past}. Unlock them first.`;
  else if (t.hidden) reason = `Hidden parts cannot be ${past}. Show them first.`;
  const ask =
    min > 1 ? `Select at least ${min} parts to ${verb}.` : scope === "all" ? `There are no parts to ${verb}.` : `Select the parts to ${verb} first.`;
  if (!reason) return { message: ask, kind: "info" };
  // One usable part of the two Merge needs: both halves of the story matter.
  return { message: min > 1 && t.parts.length > 0 ? `${ask} ${reason}` : reason, kind: "warn" };
}

/** "1 hidden part and 2 locked parts were not cut."; null when nothing was left out. */
export function leftOutMessage(t: Pick<Targets, "hidden" | "locked">, past: string): string | null {
  const n = t.hidden + t.locked;
  return n ? `${leftOutParts(t)} ${n === 1 ? "was" : "were"} not ${past}.` : null;
}

export const WAIT_MESSAGE = "Wait for the current operation to finish.";

/** The cut warning, shown once per cut. */
export const OPEN_CUT_WARNING = "Part was not closed. Some cut surfaces could not be filled.";

export function staleMessage(op: string): string {
  return `${op} was stopped because the parts changed while it ran.`;
}

export function singleShellMessage(targets: number): string {
  return targets === 1 ? "Part has only one shell." : "Each selected part has only one shell.";
}

/** Info after a split where some of the parts had a single shell and stayed. */
export function keptSingleMessage(n: number): string {
  return n === 1 ? "1 part has only one shell and was left as it is." : `${n} parts have only one shell and were left as they are.`;
}

export function noCutMessage(scope: CutScope): string {
  return `The plane does not cut any of the ${scope === "all" ? "parts" : "selected parts"}.`;
}

/**
 * Decision 7: what several shells after a union say about the parts. `bodies` is, per input part, how many separate
 * solids it was on its own (the worker fuses each part's overlapping shells first). If the result has as many shells
 * as all of them together, nothing fused: the parts don't touch. If fewer, some fused; when every part was one solid,
 * the shells left over are parts that don't touch. Otherwise a part's own separate shells may be all that is left
 * apart, so nothing is claimed about touching. Null for a single shell.
 */
export function unionShellMessage(resultShells: number, bodies: readonly number[]): string | null {
  if (resultShells <= 1) return null;
  const total = bodies.reduce((n, k) => n + k, 0);
  const count = `The result has ${resultShells} separate shells.`;
  if (resultShells === total) return `Parts don't touch. ${count}`;
  return bodies.every((k) => k === 1) ? `Some parts don't touch. ${count}` : count;
}

// ---------------------------------------------------------------------------------------------
// Names, colours, staleness (pure)

export type CutSide = "below" | "above";

export const shellName = (name: string, k: number) => `${name} (shell ${k})`;

/** Split shells asks before making more parts than this: a damaged mesh can fall into tens of thousands of shells. */
export const MANY_PARTS = 500;
/** "(cut 1)" is the side the plane normal points away from, "(cut 2)" the side it points to. */
export const cutName = (name: string, side: CutSide) => `${name} (cut ${side === "below" ? 1 : 2})`;
export const mergeName = (n: number) => `Merge of ${n} parts`;

/**
 * Colours for `count` result parts: the palette entries none of the inputs use, cycled, so the results stand out from
 * what they replace and neighbours differ. Falls back to the whole palette when the inputs use every colour.
 */
export function freshColors(used: readonly string[], count: number): string[] {
  const taken = new Set(used.map((c) => c.toLowerCase()));
  const free = PART_COLORS.filter((c) => !taken.has(c));
  const pool = free.length ? free : PART_COLORS;
  return Array.from({ length: count }, (_, i) => pool[i % pool.length]);
}

/**
 * True when results computed from `inputs` no longer fit the scene: a New/Open project ran (generation), or any input
 * Part object was replaced (moved, renamed, deleted, undone). Parts are immutable, so identity says it all.
 */
export function isStale(inputs: readonly Part[], generation: number, now: Pick<AppState, "parts" | "generation">): boolean {
  if (now.generation !== generation) return true;
  const live = new Set(now.parts);
  return inputs.some((p) => !live.has(p));
}

/** The plane with a unit normal (the offset scaled to match), or null when it cannot cut anything. */
export function unitCutPlane(plane: CutPlane): CutPlane | null {
  const n = normalized(plane.normal);
  if (!n || !Number.isFinite(plane.offset)) return null;
  return { normal: n, offset: plane.offset / Math.hypot(plane.normal[0], plane.normal[1], plane.normal[2]) };
}

// ---------------------------------------------------------------------------------------------
// Results (pure)

/** A finished, measured result mesh and the name of the part it becomes. */
export interface Made {
  name: string;
  result: AnalyzedMesh;
}

/** Input parts (by id) and the results that take their place in the parts list (store.replaceParts). */
export interface Group {
  removeIds: string[];
  made: Made[];
}

/** One group per replaced part (Split, Cut), so each part's results stay where it was; Merge and Union have one. */
export interface Plan {
  groups: Group[];
}

export const madeOf = (plan: Plan): Made[] => plan.groups.flatMap((g) => g.made);

/** Split shells: every input with two or more shells is replaced by them, "(shell 1)" first (the largest, Rust sorts). */
export function planSplit(inputs: readonly Part[], results: readonly { shells: AnalyzedMesh[] }[]): Plan & { single: number } {
  const plan: Plan & { single: number } = { groups: [], single: 0 };
  inputs.forEach((p, i) => {
    const shells = results[i]?.shells ?? [];
    if (shells.length < 2) {
      plan.single++;
      return;
    }
    plan.groups.push({ removeIds: [p.id], made: shells.map((result, k) => ({ name: shellName(p.name, k + 1), result })) });
  });
  return plan;
}

/** Both measured sides of a part the plane crosses. */
export interface CutSides {
  below: AnalyzedMesh;
  above: AnalyzedMesh;
}

/**
 * The cut pieces of one part, or null when the plane does not cross it: a side came back empty, so the other is the
 * whole part (such parts stay as they are, not renamed).
 */
export function crossedSides(below: AnalyzedMesh | null | undefined, above: AnalyzedMesh | null | undefined): CutSides | null {
  return below && above ? { below, above } : null;
}

/** Cut: each crossed part is replaced by its "(cut 1)" and "(cut 2)" pieces; `sides[i]` null leaves part i alone. */
export function planCut(inputs: readonly Part[], sides: readonly (CutSides | null)[]): Plan {
  const plan: Plan = { groups: [] };
  inputs.forEach((p, i) => {
    const s = sides[i];
    if (!s) return;
    plan.groups.push({
      removeIds: [p.id],
      made: [
        { name: cutName(p.name, "below"), result: s.below },
        { name: cutName(p.name, "above"), result: s.above },
      ],
    });
  });
  return plan;
}

/** Decision 8: warn when capping was asked for and a crossed part went to the Rust clipper because it is not closed. */
export function cutNeedsWarning(cap: boolean, fellBack: readonly boolean[], sides: readonly (CutSides | null)[]): boolean {
  return cap && fellBack.some((f, i) => f && sides[i] !== null);
}

/** The invalid indices the worker reported, in part order, without any that name no input. */
export function badIndices(indices: readonly number[], count: number): number[] {
  return [...new Set(indices)].filter((i) => Number.isInteger(i) && i >= 0 && i < count).sort((a, b) => a - b);
}

// ---------------------------------------------------------------------------------------------
// Running an operation

/** How long an operation runs before the status bar shows it; quicker ones would only make it flicker. */
export const BUSY_DELAY_MS = 200;

/** True while a mesh operation runs. */
let busy = false;

/** True while a mesh operation runs; repair mode does not start meanwhile, since the result may replace the part. */
export function meshOperationRunning(): boolean {
  return busy;
}

/** Toasts and returns true while another mesh operation runs. */
function refuseWhileBusy(): boolean {
  if (busy) toast(WAIT_MESSAGE);
  return busy;
}

/** Toasts and returns true while a part is in repair mode: these operations would change the part being repaired. */
function refuseWhileRepairing(): boolean {
  const repairing = isRepairing();
  if (repairing) toast(CLOSE_REPAIR_FIRST);
  return repairing;
}

/** A status-bar task that only appears if end() has not been called within BUSY_DELAY_MS. */
function delayedTask(label: string): { end(): void } {
  let task: TaskHandle | null = null;
  const timer = setTimeout(() => {
    task = startTask(label);
  }, BUSY_DELAY_MS);
  return {
    end() {
      clearTimeout(timer);
      task?.end();
      task = null;
    },
  };
}

/** Runs `work` as the only mesh operation. Refuses with a toast when one is already running. */
export function runMeshOperation(task: string, failure: string, work: () => Promise<void>): Promise<void> {
  if (refuseWhileBusy()) return Promise.resolve();
  return exclusive({ task, failure }, work);
}

/**
 * Runs `work` as the one mesh operation: busy flag and delayed task around it, both cleared whatever happens. A
 * failure becomes an error toast: Rust's and the worker's messages (fromWorker) are full sentences and shown as they
 * are; any other exception goes after `failure`. Callers check refuseWhileBusy() first, with no await in between, so
 * the check and this set agree.
 */
async function exclusive(labels: { task: string; failure: string }, work: () => Promise<void>): Promise<void> {
  busy = true;
  const task = delayedTask(labels.task);
  try {
    await work();
  } catch (err) {
    toast(errorMessage(err, labels.failure), "error");
  } finally {
    task.end();
    busy = false;
  }
}

/** The targets, or null after toasting why there are none (or too few) and nothing else. */
function targetsOrExplain(t: Targets, words: OpWords, scope: CutScope, min = 1): Targets | null {
  const problem = targetProblem(t, words, scope, min);
  if (!problem) return t;
  toast(problem.message, problem.kind);
  return null;
}

function reportLeftOut(t: Targets, past: string) {
  const message = leftOutMessage(t, past);
  if (message) toast(message, "warn");
}

/** False (after saying so) when the scene changed under a running operation; its results must then be dropped. */
function stillCurrent(op: string, inputs: readonly Part[], generation: number): boolean {
  if (!isStale(inputs, generation, useStore.getState())) return true;
  toast(staleMessage(op), "warn");
  return false;
}

/**
 * Adds the results as one undo step `label`, each group in place of the parts it replaces, in fresh colours, world
 * space (identity matrix); replaceParts selects them. Their stats and problem edges go into meshCache first.
 */
export function commit(plan: Plan, inputs: readonly Part[], label: string): void {
  const colors = freshColors(inputs.map((p) => p.color), madeOf(plan).length);
  let next = 0;
  const replacements = plan.groups.map((g) => ({
    remove: g.removeIds,
    add: g.made.map((m) => {
      primeMeshCaches(m.result.mesh, { stats: m.result.stats, edges: m.result.edges });
      return { name: m.name, mesh: m.result.mesh, color: colors[next++] };
    }),
  }));
  useStore.getState().replaceParts(replacements, label);
}

const partMesh = (p: Part): PartMesh => ({ name: p.name, mesh: p.mesh, matrix: p.matrix });

/**
 * The worker rejects with Errors whose messages are full sentences ("Could not load the geometry engine: …."), as
 * Rust's error strings are. Passing the message on as a string lets errorMessage show it alone, not after a lead.
 */
export function fromWorker<T>(request: Promise<T>): Promise<T> {
  return request.catch((err: unknown) => Promise.reject(err instanceof Error ? err.message : String(err)));
}

/**
 * Stats and problem edges for meshes the worker made, measured in Rust. A count mismatch throws rather than let a
 * result in unmeasured (the info panel and the union's shell check read those stats).
 */
export async function measure(meshes: MeshData[]): Promise<AnalyzedMesh[]> {
  const measured = await analyzeMeshes(meshes);
  if (measured.length !== meshes.length) throw new Error("Some results came back without their measurements.");
  return meshes.map((mesh, i) => ({ mesh, ...measured[i] }));
}

// ---------------------------------------------------------------------------------------------
// Commands

/** Edit shells › Split shells into parts: each shell of each selected part becomes its own part. */
export async function splitShellsCommand(): Promise<void> {
  if (browserOnly("Splitting shells") || refuseWhileRepairing() || refuseWhileBusy()) return;
  const s = useStore.getState();
  const t = targetsOrExplain(scopeTargets(s, "selected"), SPLIT_WORDS, "selected");
  if (!t) return;
  reportLeftOut(t, SPLIT_WORDS.past);
  const inputs = t.parts;
  const generation = s.generation;
  const held: { plan: SplitPlan | null } = { plan: null };
  await exclusive({ task: `Splitting ${countParts(inputs.length)}`, failure: "Could not split the parts" }, async () => {
    const results = await splitShells(inputs.map(partMesh));
    if (!stillCurrent("Split shells", inputs, generation)) return;
    const plan = planSplit(inputs, results);
    if (plan.groups.length === 0) {
      toast(singleShellMessage(inputs.length));
      return;
    }
    // The question waits for the user, so it comes once this operation no longer holds the others (and repair) back.
    if (madeOf(plan).length > MANY_PARTS) held.plan = plan;
    else commitSplit(plan, inputs);
  });
  const plan = held.plan;
  if (!plan) return;
  const splitting = plan.groups.map((g) => g.removeIds[0]);
  const partName = splitting.length === 1 ? (inputs.find((p) => p.id === splitting[0])?.name ?? null) : null;
  const answer = await prompt<SplitAnswer>((resolve) => ({ kind: "split-many", count: madeOf(plan).length, partName, resolve }));
  if (answer === "repair") repairFirst(splitting);
  else if (answer === "split" && !refuseWhileRepairing() && !refuseWhileBusy() && stillCurrent("Split shells", inputs, generation)) {
    commitSplit(plan, inputs);
  }
}

type SplitPlan = ReturnType<typeof planSplit>;

function commitSplit(plan: SplitPlan, inputs: readonly Part[]): void {
  commit(plan, inputs, "Split shells");
  if (plan.single) toast(keptSingleMessage(plan.single));
}

/** Split shells' Repair first: repair mode for the part that would split, or the choice of part when several would. */
function repairFirst(ids: string[]): void {
  if (ids.length === 1) enterRepair(ids[0]);
  else useUi.getState().openDialog({ kind: "repair-choose-part", ids });
}

/**
 * Edit shells › Merge parts: the selected parts become one part with their triangles as they are (no welding, no
 * fusing), concatenated in tree order.
 */
export async function mergePartsCommand(): Promise<void> {
  if (browserOnly("Merging parts") || refuseWhileRepairing() || refuseWhileBusy()) return;
  const s = useStore.getState();
  const t = targetsOrExplain(scopeTargets(s, "selected"), MERGE_WORDS, "selected", 2);
  if (!t) return;
  reportLeftOut(t, MERGE_WORDS.past);
  const inputs = t.parts;
  const generation = s.generation;
  await exclusive({ task: `Merging ${countParts(inputs.length)}`, failure: "Could not merge the parts" }, async () => {
    const result = await mergeMeshes(inputs.map(partMesh));
    if (!stillCurrent("Merge parts", inputs, generation)) return;
    commit({ groups: [{ removeIds: inputs.map((p) => p.id), made: [{ name: mergeName(inputs.length), result }] }] }, inputs, "Merge parts");
  });
}

/** The shown, unlocked parts a cut with `scope` acts on, in tree order. The Cut dialog centres its plane on them. */
export function cutTargets(scope: CutScope): Part[] {
  return scopeTargets(useStore.getState(), scope).parts;
}

/** Cut button and the Cuts panel's Cut: opens the Cut dialog if the panel's scope has parts to cut. */
export function openCutDialog(): void {
  if (refuseWhileRepairing() || refuseWhileBusy()) return;
  const scope = useClip.getState().cutScope;
  if (targetsOrExplain(scopeTargets(useStore.getState(), scope), CUT_WORDS, scope)) useUi.getState().openDialog({ kind: "cut" });
}

/**
 * Cuts each part in two along the plane. With capping, manifold-3d tries first (closed solids in, closed solids out);
 * a part it rejects as not closed, and every part when capping is off, goes to the Rust clipper. Returns, per input,
 * its sides (null when the plane does not cross it) and whether it went to Rust.
 */
async function cutSides(
  parts: readonly Part[],
  plane: CutPlane,
  cap: boolean,
  gap: number,
): Promise<{ sides: (CutSides | null)[]; fellBack: boolean[] }> {
  const sides: (CutSides | null)[] = parts.map(() => null);
  const fellBack = parts.map(() => !cap);
  if (cap) {
    const outcomes = await fromWorker(Promise.all(parts.map((p) => manifoldSplit({ mesh: p.mesh, matrix: p.matrix }, plane, gap))));
    // Only crossed parts are measured; a part with an empty side stays as it is anyway.
    const crossed: { i: number; below: MeshData; above: MeshData }[] = [];
    outcomes.forEach((o, i) => {
      if (o.kind === "invalid") fellBack[i] = true;
      else if (o.below && o.above) crossed.push({ i, below: o.below, above: o.above });
    });
    if (crossed.length) {
      const measured = await measure(crossed.flatMap((c) => [c.below, c.above]));
      crossed.forEach((c, k) => {
        sides[c.i] = { below: measured[2 * k], above: measured[2 * k + 1] };
      });
    }
  }
  const rust = parts.flatMap((_, i) => (fellBack[i] ? [i] : []));
  if (rust.length) {
    const results = await cutMeshes(rust.map((i) => partMesh(parts[i])), plane, cap, gap);
    rust.forEach((i, k) => {
      const pieces = results[k]?.pieces ?? [];
      sides[i] = crossedSides(
        pieces.find((p) => p.side === "below"),
        pieces.find((p) => p.side === "above"),
      );
    });
  }
  return { sides, fellBack };
}

/** The Cut dialog's Execute. One undo step "Cut" for all parts the plane crosses; the others stay untouched. */
export async function runCut(options: CutOptions): Promise<void> {
  if (browserOnly("Cutting") || refuseWhileRepairing() || refuseWhileBusy()) return;
  const { scope, cap } = options;
  const plane = unitCutPlane(options.plane);
  if (!plane) {
    toast("The cut plane is not valid. Pick a clip plane or enter a position.", "error");
    return;
  }
  const gap = Number.isFinite(options.gap) && options.gap > 0 ? options.gap : 0;
  const s = useStore.getState();
  const t = targetsOrExplain(scopeTargets(s, scope), CUT_WORDS, scope);
  if (!t) return;
  reportLeftOut(t, CUT_WORDS.past);
  const inputs = t.parts;
  const generation = s.generation;
  await exclusive({ task: `Cutting ${countParts(inputs.length)}`, failure: "Could not cut the parts" }, async () => {
    const { sides, fellBack } = await cutSides(inputs, plane, cap, gap);
    if (!stillCurrent("Cut", inputs, generation)) return;
    const plan = planCut(inputs, sides);
    if (plan.groups.length === 0) {
      toast(noCutMessage(scope));
      return;
    }
    commit(plan, inputs, "Cut");
    if (cutNeedsWarning(cap, fellBack, sides)) toast(OPEN_CUT_WARNING, "warn");
  });
}
