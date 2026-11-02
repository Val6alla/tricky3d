// Modify › Hollow (and Home › Hollow): a part made hollow with drain holes, in one panel on the right. The cavity is
// worked out in Rust in the background after every change of the wall settings, the run before it cancelled, and shown
// inside the part drawn see-through, with a section through the part's middle (the Y clip plane keeping the far half,
// which the Clip planes panel moves). A click on the part's outside adds a drain hole there; a hole's handle drags it along the surface; the
// selected hole takes its own size; Delete removes it. Rust checks every hole against the cavity and gives its shape
// to draw; a hole that would not reach the cavity is drawn red with the reason, and Apply leaves it out. Apply makes
// "<name> (hollow)" with all the holes cut (and a plug per hole when asked) as one undo step; Close changes nothing.
import { create } from "zustand";
import { useClip, offsetRange, rowDirection, type ClipSettings } from "../clip";
import { closeAnalysisView } from "../analysis/session";
import { isBooleanOpen } from "../boolean/session";
import { browserOnly, errorMessage } from "../io/files";
import { nextJob } from "../jobs";
import { isLabelOpen } from "../label/session";
import { isMeasureOpen } from "../measure/session";
import { boundsOf, IDENTITY } from "../mesh";
import { bakedCopy, useModify } from "../modify/session";
import { meshOperationRunning, runMeshOperation, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { isRepairing } from "../repair/session";
import { useRepair } from "../repair/store";
import { loadSetting, saveSetting, useStore, type MeshData, type NewPart, type Part, type Vec3 } from "../store";
import { toast } from "../ui";
import { applyHollow, forgetHollow, placeHoles, previewHollow } from "./ipc";
import type { Hole, HoleCheck, HoleShape, HollowSettings, ThinWalls } from "./types";

export interface PlacedHole {
  id: string;
  hole: Hole;
  /** Rust's check against the cavity on show; null until it answered. */
  check: HoleCheck | null;
  /** Its solid, to draw; null until Rust answered. */
  solid: MeshData | null;
}

export interface HollowPreview {
  key: number;
  /** Null when no cavity fits. */
  cavity: MeshData | null;
  partVolume: number;
  cavityVolume: number;
  thin: ThinWalls | null;
  warnings: string[];
}

export interface HollowRun {
  job: number;
  fraction: number;
  stage: string | null;
  cancelling?: boolean;
}

export interface HollowSession {
  partId: string;
  /** The part's mesh and matrix when the panel opened: the result is dropped if they changed. */
  source: { mesh: MeshData; matrix: number[] };
  /** The part in world space, as Rust works on it. */
  base: MeshData;
  preview: HollowPreview | null;
  /** The cavity being worked out. */
  computing: (HollowRun & { key: number }) | null;
  /** Why there is no cavity to show: a refusal or a failure, as a sentence. */
  failed: string | null;
  holes: PlacedHole[];
  selected: string | null;
  applying: HollowRun | null;
}

/** The panel's settings, remembered between runs. */
export interface HollowOptions {
  settings: HollowSettings;
  /** New holes' shape. */
  shape: HoleShape;
  autoCount: number;
  plugs: boolean;
  /** How much smaller than its hole a plug is, all round, mm. */
  clearance: number;
  removeOriginal: boolean;
  /** The section through the part's middle. */
  section: boolean;
}

export const DEFAULT_OPTIONS: HollowOptions = {
  settings: { thickness: 2, accuracy: 0.4, smoothness: 1 },
  shape: { diameter: 3, taper: 0, flare: 0 },
  autoCount: 2,
  plugs: false,
  clearance: 0.15,
  removeOriginal: false,
  section: true,
};

export interface HollowState {
  session: HollowSession | null;
  options: HollowOptions;
}

const OPTIONS_KEY = "tricky3d.hollow";

/** The options used last, over the defaults: only known keys of the right type. */
export function loadOptions(): HollowOptions {
  const out: HollowOptions = structuredClone(DEFAULT_OPTIONS);
  try {
    const raw = JSON.parse(loadSetting(OPTIONS_KEY) ?? "null") as Record<string, unknown> | null;
    if (!raw || typeof raw !== "object") return out;
    const fill = (into: Record<string, unknown>, from: unknown) => {
      if (!from || typeof from !== "object") return;
      for (const key of Object.keys(into)) {
        const v = (from as Record<string, unknown>)[key];
        if (typeof v === typeof into[key] && (typeof v !== "number" || Number.isFinite(v))) into[key] = v;
      }
    };
    fill(out as unknown as Record<string, unknown>, { ...raw, settings: undefined, shape: undefined });
    fill(out.settings as unknown as Record<string, unknown>, raw.settings);
    fill(out.shape as unknown as Record<string, unknown>, raw.shape);
  } catch {
    // Damaged settings: the defaults.
  }
  return out;
}

export const useHollow = create<HollowState>(() => ({ session: null, options: loadOptions() }));

const h = () => useHollow.getState();
const s = () => useStore.getState();

/** The cavity's stand-in part in the view. */
export const CAVITY_ID = "hollow-cavity";

export const PICK_ONE_PART = "Select the part to hollow first.";
export const ONE_PART_ONLY = "Hollow works on one part at a time. Select just one part.";

export function isHollowOpen(): boolean {
  return h().session !== null;
}

function setSession(patch: Partial<HollowSession>): void {
  const open = h().session;
  if (open) useHollow.setState({ session: { ...open, ...patch } });
}

function saveOptions(options: HollowOptions): void {
  saveSetting(OPTIONS_KEY, JSON.stringify(options));
  useHollow.setState({ options });
}

/** Says why Hollow cannot open now, or returns null when it can. */
function refusal(): string | null {
  if (isRepairing()) return "Close repair mode first.";
  if (isBooleanOpen()) return "Close Boolean operations first.";
  if (isLabelOpen()) return "Close label generation first.";
  if (isMeasureOpen()) return "Close Measure first.";
  if (useModify.getState().session) return "Close Manipulate mesh first.";
  if (meshOperationRunning() || useRepair.getState().busy) return WAIT_MESSAGE;
  const ids = s().selection;
  if (ids.length > 1) return ONE_PART_ONLY;
  const part = s().parts.find((p) => p.id === ids[0]);
  if (!part) return PICK_ONE_PART;
  if (part.locked) return "The part is locked. Unlock it first.";
  return null;
}

/** Home › Hollow, Modify › Hollow: opens the panel on the selected part and works out its cavity. */
export function openHollow(): void {
  if (h().session) return;
  if (browserOnly("Hollow")) return;
  const why = refusal();
  if (why) {
    toast(why);
    return;
  }
  const part = s().parts.find((p) => p.id === s().selection[0]);
  if (!part) return;
  closeAnalysisView();
  useHollow.setState({
    session: {
      partId: part.id,
      source: { mesh: part.mesh, matrix: part.matrix },
      base: bakedCopy(part),
      preview: null,
      computing: null,
      failed: null,
      holes: [],
      selected: null,
      applying: null,
    },
  });
  if (h().options.section) showSection(true);
  void runPreview();
}

/** Close: nothing changes; a run going is cancelled. */
export function closeHollow(): void {
  const open = h().session;
  if (!open) return;
  if (open.computing) cancelRepair(open.computing.job).catch(() => {});
  if (open.applying) cancelRepair(open.applying.job).catch(() => {});
  useHollow.setState({ session: null });
  showSection(false);
  forgetHollow().catch(() => {});
}

// ---------------------------------------------------------------------------------------------
// The cavity

let previewTimer: ReturnType<typeof setTimeout> | null = null;
/** How long settings must stay unchanged before the cavity is worked out again, ms. */
export const PREVIEW_DELAY = 300;

/** Changes the wall settings: the run going stops at once, and the cavity is worked out again once they settle. */
export function editSettings(patch: Partial<HollowSettings>): void {
  const options = h().options;
  saveOptions({ ...options, settings: { ...options.settings, ...patch } });
  const open = h().session;
  if (!open) return;
  if (open.computing) {
    cancelRepair(open.computing.job).catch(() => {});
    setSession({ computing: null });
  }
  if (previewTimer) clearTimeout(previewTimer);
  previewTimer = setTimeout(() => {
    previewTimer = null;
    void runPreview();
  }, PREVIEW_DELAY);
}

/** Changes the other options (hole shape for new holes, plugs, Remove original, Auto holes' count). */
export function editOptions(patch: Partial<Omit<HollowOptions, "settings">>): void {
  saveOptions({ ...h().options, ...patch });
  if (patch.section !== undefined && h().session) showSection(patch.section);
}

/** Works out the cavity for the settings on show. Resolves when it is shown, failed or was replaced. */
export async function runPreview(): Promise<void> {
  const open = h().session;
  if (!open) return;
  const job = nextJob();
  const key = job;
  setSession({ computing: { job, key, fraction: 0, stage: null }, failed: null });
  const current = () => h().session?.computing?.job === job;
  try {
    const reply = await previewHollow(open.base, h().options.settings, job, key, (p) => {
      const now = h().session;
      if (now?.computing?.job === job) setSession({ computing: { ...now.computing, fraction: p.fraction, stage: p.stage } });
    });
    if (!current()) return;
    if (reply.kind === "refused") {
      const r = reply.refused;
      const what = [!r.closed && `not closed (${r.holes} ${r.holes === 1 ? "hole" : "holes"})`, !r.oriented && "not oriented"].filter(Boolean).join(" and ");
      setSession({ computing: null, preview: null, failed: `The part is ${what}. Hollow needs a closed part whose triangles all face the same way: repair it first.` });
      return;
    }
    setSession({
      computing: null,
      preview: { key: reply.key, cavity: reply.cavity, partVolume: reply.partVolume, cavityVolume: reply.cavityVolume, thin: reply.thin, warnings: reply.warnings },
    });
    await refreshHoles();
  } catch (err) {
    if (!current()) return;
    setSession({ computing: null, failed: err === CANCELLED ? null : errorMessage(err, "Could not hollow the part") });
  }
}

// ---------------------------------------------------------------------------------------------
// Holes

let holeSeq = 0;
let checkSeq = 0;

/** Asks Rust for every hole's check and solid against the cavity on show; a later ask wins. */
export async function refreshHoles(auto: { count: number } | null = null): Promise<void> {
  const open = h().session;
  if (!open?.preview) return;
  const seq = ++checkSeq;
  const shape = h().options.shape;
  try {
    const reply = await placeHoles(
      open.preview.key,
      open.holes.map((p) => p.hole),
      auto ? { count: auto.count, template: { at: [0, 0, 0], normal: [0, 0, -1], ...shape } } : null,
    );
    const now = h().session;
    if (seq !== checkSeq || !now || now.preview?.key !== open.preview.key) return;
    // The holes as they are now (a drag may have moved one meanwhile): the answer counts for those it was asked about.
    const asked = open.holes.map((p) => p.id);
    const holes: PlacedHole[] = reply.holes.map((hole, i) => {
      const id = asked[i] ?? `hole-${++holeSeq}`;
      const was = now.holes.find((p) => p.id === id);
      const same = was && was.hole === open.holes[i]?.hole;
      return { id, hole: was && !same ? was.hole : hole, check: was && !same ? null : reply.checks[i], solid: was && !same ? was.solid : (reply.solids[i] ?? null) };
    });
    // Holes added while Rust worked keep their place, unchecked until the next answer.
    for (const p of now.holes) if (!holes.some((q) => q.id === p.id) && !asked.includes(p.id)) holes.push(p);
    setSession({ holes });
    if (auto && reply.holes.length === open.holes.length) toast("No more holes fit at the lowest points of the cavity.");
  } catch (err) {
    if (seq === checkSeq) toast(errorMessage(err, "Could not check the holes"), "warn");
  }
}

/** A click on the part's outside: a hole there, along the surface's normal, with the shape for new holes. */
export function addHoleAt(hit: { partId: string; point: Vec3; normal: Vec3 } | null): void {
  const open = h().session;
  if (!open || open.applying || !hit || hit.partId !== open.partId) return;
  const id = `hole-${++holeSeq}`;
  const hole: Hole = { at: hit.point, normal: hit.normal, ...h().options.shape };
  setSession({ holes: [...open.holes, { id, hole, check: null, solid: null }], selected: id });
  void refreshHoles();
}

export function selectHole(id: string | null): void {
  setSession({ selected: id });
}

/** Moves a hole to a point of the part's outside (its handle's drag). */
export function moveHole(id: string, hit: { partId: string; point: Vec3; normal: Vec3 } | null): void {
  const open = h().session;
  if (!open || !hit || hit.partId !== open.partId) return;
  setSession({ holes: open.holes.map((p) => (p.id === id ? { ...p, hole: { ...p.hole, at: hit.point, normal: hit.normal }, check: null } : p)) });
}

/** A drag ended, or a field changed: Rust checks the holes again. */
export function settleHoles(): void {
  void refreshHoles();
}

/** The selected hole's own size; also the shape new holes get. */
export function editHole(id: string, patch: Partial<HoleShape>): void {
  const open = h().session;
  if (!open) return;
  setSession({ holes: open.holes.map((p) => (p.id === id ? { ...p, hole: { ...p.hole, ...patch }, check: null } : p)) });
  editOptions({ shape: { ...h().options.shape, ...patch } });
  void refreshHoles();
}

export function deleteHole(id: string): void {
  const open = h().session;
  if (!open) return;
  setSession({ holes: open.holes.filter((p) => p.id !== id), selected: open.selected === id ? null : open.selected });
  void refreshHoles();
}

/** Delete: removes the selected hole. True when there was one. */
export function deleteSelectedHole(): boolean {
  const id = h().session?.selected;
  if (!id) return false;
  deleteHole(id);
  return true;
}

export function removeAllHoles(): void {
  setSession({ holes: [], selected: null });
}

/** Auto holes: the chosen number of holes at the lowest points of the cavity, spread apart, after those placed. */
export function autoHoles(): void {
  const open = h().session;
  if (!open?.preview) {
    toast("Wait for the cavity to be worked out first.");
    return;
  }
  if (!open.preview.cavity) {
    toast("No cavity fits in this part, so there is nowhere to drain.");
    return;
  }
  void refreshHoles({ count: Math.max(1, Math.round(h().options.autoCount)) });
}

// ---------------------------------------------------------------------------------------------
// Apply

/** The new parts: "<name> (hollow)", then "<name> (plug n)", in the part's colour and settings, world space. */
export function hollowParts(part: Part, mesh: MeshData, plugs: MeshData[]): NewPart[] {
  const like = (name: string, m: MeshData): NewPart => ({
    name,
    mesh: m,
    matrix: [...IDENTITY],
    color: part.color,
    visible: part.visible,
    transparent: part.transparent,
    packing: part.packing,
    source: part.source,
  });
  return [like(`${part.name} (hollow)`, mesh), ...plugs.map((p, i) => like(plugs.length === 1 ? `${part.name} (plug)` : `${part.name} (plug ${i + 1})`, p))];
}

/** Apply: the hollow part with every hole that checks out, as one undo step "Hollow". Resolves true when it was added. */
export async function applyHollowNow(): Promise<boolean> {
  const open = h().session;
  if (!open || open.applying) return false;
  if (open.computing || !open.preview) {
    toast("Wait for the cavity to be worked out first.");
    return false;
  }
  if (!open.preview.cavity) {
    toast("No cavity fits in this part: every wall is thinner than twice the wall thickness.");
    return false;
  }
  const part = s().parts.find((p) => p.id === open.partId);
  if (!part) return false;
  const { plugs, clearance, removeOriginal } = h().options;
  const job = nextJob();
  setSession({ applying: { job, fraction: 0, stage: null } });
  let added = false;
  try {
    await runMeshOperation("Hollow", "Could not hollow the part", async () => {
      try {
        const reply = await applyHollow(
          job,
          open.preview!.key,
          open.holes.map((p) => p.hole),
          plugs ? clearance : null,
          (p) => {
            const now = h().session?.applying;
            if (now?.job === job) setSession({ applying: { ...now, fraction: p.fraction, stage: p.stage } });
          },
        );
        if (h().session?.applying?.cancelling) return;
        const now = s().parts.find((p) => p.id === open.partId);
        if (!now || now.mesh !== open.source.mesh || now.matrix !== open.source.matrix) {
          toast("The part changed while Hollow was open, so the result was dropped.", "warn");
          return;
        }
        const made = hollowParts(part, reply.mesh.mesh, reply.plugs.map((p) => p.mesh));
        s().replaceParts([{ remove: removeOriginal ? [part.id] : [], add: made, ...(removeOriginal ? {} : { after: part.id }) }], "Hollow");
        added = true;
        const cut = open.holes.length - reply.skipped.length;
        const parts = [`Made "${part.name} (hollow)"`];
        if (cut > 0) parts.push(`with ${cut} drain ${cut === 1 ? "hole" : "holes"}`);
        if (reply.plugs.length) parts.push(`and ${reply.plugs.length} ${reply.plugs.length === 1 ? "plug" : "plugs"}`);
        toast(`${parts.join(" ")}.`);
        for (const sk of reply.skipped) toast(`Hole ${sk.index + 1} was left out: ${sk.reason}`, "warn");
        for (const w of reply.warnings) toast(w, "warn");
      } catch (err) {
        if (err !== CANCELLED) throw err;
        toast("Hollow was cancelled. Nothing changed.");
      }
    });
  } finally {
    setSession({ applying: null });
  }
  if (added) closeHollow();
  return added;
}

/** Stops Apply: Rust stops at once and nothing changes. */
export function cancelApply(): void {
  const run = h().session?.applying;
  if (!run || run.cancelling) return;
  setSession({ applying: { ...run, cancelling: true } });
  cancelRepair(run.job).catch(() => {});
}

// ---------------------------------------------------------------------------------------------
// The section

let savedClip: Pick<ClipSettings, "rows"> | null = null;

/** The section view: the Y clip plane through the part's middle (the Clip planes panel moves it), or the planes back as they were. */
function showSection(on: boolean): void {
  const clip = useClip.getState();
  if (on) {
    const open = h().session;
    const box = open ? boundsOf(open.base, IDENTITY) : null;
    if (!box) return;
    savedClip ??= { rows: clip.rows };
    const middle = (box.min[1] + box.max[1]) / 2;
    const range = offsetRange(rowDirection("y", clip.customNormal), box);
    clip.setEnabled("y", true, range);
    clip.setOffset("y", middle);
    // The far half stays: from the usual views (from the front) the cut faces the camera, cavity and walls in view.
    clip.setFlipped("y", true);
  } else if (savedClip) {
    useClip.setState({ rows: savedClip.rows });
    savedClip = null;
  }
}

// ---------------------------------------------------------------------------------------------
// Words for the panel

/** "64.0 cm³" for mm³, one decimal (two below 10 cm³). */
export function cm3(mm3: number): string {
  const v = mm3 / 1000;
  return `${v.toFixed(Math.abs(v) < 10 ? 2 : 1)} cm³`;
}

/** The volumes line: "Part 64.0 → 17.3 cm³, resin saved 46.7 cm³ (73 %)". */
export function volumesText(p: HollowPreview): string {
  const after = p.partVolume - p.cavityVolume;
  const share = p.partVolume > 0 ? Math.round((100 * p.cavityVolume) / p.partVolume) : 0;
  return `${cm3(p.partVolume)} → ${cm3(after)}; resin saved ${cm3(p.cavityVolume)} (${share} %)`;
}

// The panel closes when New project or Open project replaces the document, when its part goes, and when another mode
// opens over it.
useStore.subscribe((state, prev) => {
  const open = h().session;
  if (!open) return;
  if (state.generation !== prev.generation || !state.parts.some((p) => p.id === open.partId)) closeHollow();
});
useRepair.subscribe((r, prev) => {
  if (r.session && !prev.session) closeHollow();
});
