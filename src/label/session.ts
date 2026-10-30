// Label mode (Home and Modify › Generate label): entering and leaving, placing labels on the part, editing them, the
// mode's own Undo and Redo, the handles' drags, and Apply. UI code changes label state only through these functions;
// the state is in src/label/store.ts, the shapes and the labelled part come from Rust (src/label/ipc.ts).
//
// The flow follows the reference tool: pick a label kind with a part selected; a "Label generation" tab opens and the
// next click on the part places a label there, with handles to move, turn and size it and its options in the right
// panel. Several labels of any kind may be placed. Apply to part has Rust put them all into the part, then asks whether
// to remove the original (Yes) or keep both (No): one undo step. Close label generation leaves without applying.
import { CLOSE_BOOLEAN_FIRST, isBooleanOpen } from "../boolean/session";
import type { TabId } from "../features";
import { browserOnly, errorMessage } from "../io/files";
import { CLOSE_LIBRARY_FIRST, isLibraryOpen } from "../library/session";
import { pickPicture, type Picture } from "../library/ipc";
import { CLOSE_MEASURE_FIRST, isMeasureOpen } from "../measure/session";
import { boundsOf, IDENTITY, translation, type Bounds } from "../mesh";
import { primeMeshCaches } from "../meshCache";
import { meshOperationRunning, WAIT_MESSAGE } from "../ops/meshOps";
import { CANCELLED, cancelRepair } from "../repair/ipc";
import { CLOSE_REPAIR_FIRST, isRepairing } from "../repair/session";
import { useStore, type NewPart, type Part, type Vec3 } from "../store";
import { startTask, toast, useUi } from "../ui";
import { viewport } from "../viewport/api";
import { at, labelFrame, onPlane, plateSize, sub, TAG_GAP, unit, dot, add, scale } from "./frame";
import { applyLabels, fetchFonts, fitLabel, labelShape, pickCsv, type ShapeRequest } from "./ipc";
import { useLabel, type LabelSession, type LabelStep, type LabelTool, type ShapeEntry } from "./store";
import { BUNDLED_FONT, KIND_LABELS, listEntries, newLabel, shapeText, startDepth, type FontInfo, type LabelKind, type PlacedLabel } from "./types";
import { nextJob } from "../jobs";

/** Main-document commands that would change the parts refuse with this while the mode is open. */
export const CLOSE_LABEL_FIRST = "Close label generation first.";

const state = () => useLabel.getState();
const session = () => useLabel.getState().session;
const store = () => useStore.getState();

let tabBefore: TabId | null = null;
let labelSeq = 0;

/** True while label mode is open. */
export function isLabelOpen(): boolean {
  return session() !== null;
}

function setSession(patch: Partial<LabelSession>) {
  const current = session();
  if (current) useLabel.setState({ session: { ...current, ...patch } });
}

/** The part being labelled. */
export function labelPart(): Part | null {
  const current = session();
  return current ? (store().parts.find((p) => p.id === current.partId) ?? null) : null;
}

export function selectedLabel(): PlacedLabel | null {
  const current = session();
  return current?.labels.find((l) => l.id === current.selected) ?? null;
}

// ---------------------------------------------------------------------------------------------
// Entering and leaving

let watching = false;

/** New or Open project, or the part going away (undo, delete), ends the mode (subscribes once). */
function watchParts() {
  if (watching) return;
  watching = true;
  useStore.subscribe((s, prev) => {
    const current = session();
    if (!current) return;
    if (s.generation !== prev.generation || !s.parts.some((p) => p.id === current.partId)) leave();
  });
}

/**
 * Generate label › Text, Image, Text from list or Tag: opens the "Label generation" tab for the selected part with the
 * Place tool on, so the next click on the part places a label of `kind`. With the mode already open it only picks the
 * kind. Refused while another mode or a mesh operation is busy, and without a selected part.
 */
export function openLabelMode(kind: LabelKind): void {
  const ui = useUi.getState();
  if (isLabelOpen()) {
    placeKind(kind);
    ui.setRibbonTab("label");
    return;
  }
  if (isRepairing()) return toast(CLOSE_REPAIR_FIRST);
  if (isBooleanOpen()) return toast(CLOSE_BOOLEAN_FIRST);
  if (isLibraryOpen()) return toast(CLOSE_LIBRARY_FIRST);
  if (isMeasureOpen()) return toast(CLOSE_MEASURE_FIRST);
  if (meshOperationRunning()) return toast(WAIT_MESSAGE);
  if (browserOnly("Generating labels")) return;
  const s = store();
  const part = s.parts.find((p) => p.id === s.selection[0]);
  if (!part) return toast("Select the part to label first: click it in the parts list or the view.", "warn");
  if (part.locked) return toast(`${part.name} is locked. Unlock it to label it.`, "warn");
  if (s.selection.length > 1) toast(`Labels go on ${part.name}, the first part selected.`);
  useLabel.setState({
    session: { partId: part.id, labels: [], selected: null, tool: "place", kind, past: [], future: [] },
    busy: null,
  });
  watchParts();
  tabBefore = ui.ribbonTab === "label" ? null : ui.ribbonTab;
  ui.closeContextMenu();
  ui.setRibbonTab("label");
  if (s.gizmoMode) s.setGizmoMode(null);
}

function leave() {
  answerQuestion?.(false);
  const busy = state().busy;
  if (busy) cancelRepair(busy.job).catch(() => {});
  useLabel.setState({ session: null, busy: null });
  const ui = useUi.getState();
  if (ui.ribbonTab === "label") ui.setRibbonTab(tabBefore ?? "home");
  tabBefore = null;
}

/** Close label generation: leaves without applying, after asking when labels would be lost. */
export async function closeLabelMode(): Promise<void> {
  const current = session();
  if (!current) return;
  if (current.labels.length > 0 && !state().busy) {
    const question = current.labels.length === 1 ? "Leave without applying the label?" : `Leave without applying the ${current.labels.length} labels?`;
    if (!(await yesNo(question, "Close label generation"))) return;
  }
  leave();
}

let answerQuestion: ((yes: boolean) => void) | null = null;

/** A question with Yes and No, shown by the panel over the window (LabelPanel); resolves true for Yes. */
function yesNo(text: string, title: string): Promise<boolean> {
  answerQuestion?.(false);
  useLabel.setState({ question: { title, text } });
  return new Promise((resolve) => {
    answerQuestion = (yes) => {
      answerQuestion = null;
      useLabel.setState({ question: null });
      resolve(yes);
    };
  });
}

/** Yes or No (or the dialog closed: No) to the question on show. */
export function answer(yes: boolean): void {
  answerQuestion?.(yes);
}

// ---------------------------------------------------------------------------------------------
// Fonts and shapes

/** The fonts for the Font dialog, asked for once, when it first opens: the computer's fonts are looked through then. */
export async function loadFonts(): Promise<FontInfo[]> {
  const known = state().fonts;
  if (known) return known;
  try {
    const fonts = await fetchFonts();
    useLabel.setState({ fonts });
    return fonts;
  } catch (err) {
    toast(errorMessage(err, "Could not list the fonts"), "error");
    useLabel.setState({ fonts: [BUNDLED_FONT] });
    return [BUNDLED_FONT];
  }
}

const pictureIds = new WeakMap<Picture, number>();
let pictureSeq = 0;

/** What a label's shape depends on, as one string: equal keys draw the same shape. */
export function shapeKey(label: PlacedLabel): string {
  if (label.kind === "image") {
    if (!label.picture) return "image:none";
    let id = pictureIds.get(label.picture);
    if (id === undefined) pictureIds.set(label.picture, (id = ++pictureSeq));
    return `image:${id}:${label.threshold}`;
  }
  const text = label.kind === "list" ? listEntries(label.list).join("\u0000") : shapeText(label);
  return `${label.kind === "list" ? "list" : "text"}:${label.font.id}:${label.fontSize}:${text}`;
}

/** What Rust draws a label's shape from: its caption, or a list's entries (the widest sets the shape). */
function shapeRequest(label: PlacedLabel): ShapeRequest {
  const entries = label.kind === "list" ? listEntries(label.list) : [];
  return { kind: label.kind, text: shapeText(label), entries, font: label.font.id, fontSize: label.fontSize, threshold: label.threshold };
}

export function shapeOf(label: PlacedLabel): ShapeEntry | undefined {
  return state().shapes[shapeKey(label)];
}

/** Asks Rust for the label's shape unless it has it or is drawing it. */
export function ensureShape(label: PlacedLabel): Promise<void> {
  const key = shapeKey(label);
  const known = state().shapes[key];
  if (known && known.status !== "loading") return Promise.resolve();
  if (known) return waitForShape(key);
  const setEntry = (entry: ShapeEntry) => useLabel.setState((s) => ({ shapes: { ...s.shapes, [key]: entry } }));
  setEntry({ status: "loading" });
  return labelShape(shapeRequest(label), label.kind === "image" ? label.picture : null).then(
    (reply) => setEntry({ status: "ready", reply }),
    (err) => setEntry({ status: "error", message: errorMessage(err, "Could not draw the label") }),
  );
}

function waitForShape(key: string): Promise<void> {
  return new Promise((resolve) => {
    const stop = useLabel.subscribe((s) => {
      if (s.shapes[key]?.status !== "loading") {
        stop();
        resolve();
      }
    });
  });
}

/**
 * Sizes a label just placed (or given its picture) to where it is: Rust finds the flat area round the point and the
 * tallest label (up to 10 mm, at least 1 mm) that takes 80 % of the room there, and how thick the part is under it.
 * The depth then starts by the rule in startDepth. Not an undo step of its own: it belongs to the placing. Where not
 * even 1 mm fits, the label starts at 1 mm and the user is told it runs over an edge.
 */
export async function fitNewLabel(id: string): Promise<void> {
  const label = session()?.labels.find((l) => l.id === id);
  const part = labelPart();
  if (!label || !part) return;
  const finish = (patch: Partial<PlacedLabel>) => {
    const now = session();
    const placed = now?.labels.find((l) => l.id === id);
    if (!now || !placed) return;
    let next: PlacedLabel = { ...placed, ...patch, fitting: false };
    if (next.depthAuto) next = { ...next, depth: startDepth(next) };
    setSession({ labels: now.labels.map((l) => (l.id === id ? next : l)) });
    void ensureShape(next);
  };
  try {
    const at = { point: label.point, normal: label.normal, angle: label.angle };
    const fit = await fitLabel({ name: part.name, mesh: part.mesh, matrix: part.matrix }, shapeRequest(label), at, label.kind === "image" ? label.picture : null);
    // Sized meanwhile by the user: keep their size.
    if (session()?.labels.find((l) => l.id === id)?.sizeAuto === false) return finish({ partThickness: fit.thickness });
    finish({ height: Math.floor(fit.height * 100) / 100, partThickness: fit.thickness });
    if (!fit.fits) toast(OVER_EDGE, "warn");
  } catch (err) {
    finish({});
    toast(errorMessage(err, "Could not size the label"), "error");
  }
}

/** Rust's sentence for a label that runs over an edge (src-tauri/src/label/emboss.rs OVER_EDGE). */
export const OVER_EDGE = "The label runs over an edge or a steep slope of the part. Make it smaller or move it onto a flatter spot.";

/** The label's width in mm: its height × its shape's aspect while Keep aspect ratio is on, else as typed. */
export function labelWidth(label: PlacedLabel): number | null {
  if (!label.keepAspect) return label.width;
  const shape = shapeOf(label);
  return shape?.status === "ready" ? label.height * shape.reply.aspect : null;
}

// ---------------------------------------------------------------------------------------------
// Tools, placing and editing

export function pickLabelTool(tool: LabelTool): void {
  if (isLabelOpen()) setSession({ tool });
}

/** Generate label ▾ in the tab, and Place label: the next click on the part places a label of `kind`. */
export function placeKind(kind: LabelKind = session()?.kind ?? "text"): void {
  setSession({ kind, tool: "place" });
}

/** What the next click does, for the status bar. */
export function labelStatus(): { name: string; hint: string } | null {
  const current = session();
  if (!current) return null;
  if (current.tool === "place") {
    return { name: `Place label: ${KIND_LABELS[current.kind]}`, hint: "Click on the part where the label goes. Right-drag orbits, middle-drag pans, scroll zooms." };
  }
  return {
    name: "Select",
    hint: "Click a label to select it. Drag its centre to move it, its corner to size and turn it (Ctrl turns only, Shift sizes only).",
  };
}

/** The surface under the pointer in the view. */
export interface SurfaceHit {
  partId: string;
  point: Vec3;
  normal: Vec3;
}

/** A left click in the view: Place adds a label there, Select picks the label under it (or none). */
export function labelClick(hit: SurfaceHit | null): void {
  const current = session();
  if (!current || state().busy) return;
  if (current.tool === "place") {
    if (!hit) return;
    const part = labelPart();
    if (hit.partId !== current.partId) {
      toast(`These labels go on ${part?.name ?? "the part"}. To label another part, apply or close label generation first.`);
      return;
    }
    const label = newLabel(`label-${++labelSeq}`, current.kind, part?.name ?? "", hit.point, hit.normal, lastFont ?? BUNDLED_FONT);
    record(`Place ${KIND_LABELS[current.kind].toLowerCase()}`);
    setSession({ labels: [...current.labels, label], selected: label.id, tool: "select" });
    void ensureShape(label);
    void fitNewLabel(label.id);
    return;
  }
  selectLabel(hit && hit.partId === current.partId ? labelAt(hit.point) : null);
}

let lastFont: FontInfo | null = null;

/** The label whose frame holds `point` (a point on the part), the one drawn last first. */
export function labelAt(point: Vec3): string | null {
  const current = session();
  if (!current) return null;
  for (const label of [...current.labels].reverse()) {
    const f = labelFrame(label.point, label.normal, label.angle);
    if (!f) continue;
    const d = sub(point, f.origin);
    const [x, y, z] = [dot(d, f.u), dot(d, f.v), dot(d, f.w)];
    const w = (labelWidth(label) ?? label.height) / 2;
    if (Math.abs(x) <= w && Math.abs(y) <= label.height / 2 && Math.abs(z) <= Math.max(label.height, label.depth)) return label.id;
  }
  return null;
}

export function selectLabel(id: string | null): void {
  const current = session();
  if (!current || current.selected === id) return;
  setSession({ selected: id && current.labels.some((l) => l.id === id) ? id : null });
}

/** The last step recorded: the same change to the same label within a moment (typing a number) adds no new one. */
let lastRecord: { key: string; time: number } | null = null;
const SAME_STEP_MS = 1500;

/** Keeps the labels as they are now as an undo step named `label`, and drops the redo steps. */
function record(label: string, key: string | null = null) {
  const current = session();
  if (!current) return;
  const now = Date.now();
  if (key && lastRecord?.key === key && now - lastRecord.time < SAME_STEP_MS && current.past.length > 0) {
    lastRecord = { key, time: now };
    return;
  }
  lastRecord = key ? { key, time: now } : null;
  const step: LabelStep = { labels: current.labels, selected: current.selected, label };
  setSession({ past: [...current.past, step].slice(-100), future: [] });
}

/** Changes a label; with `step` this is an undo step of that name, without it a live change a drag records itself. */
export function updateLabel(id: string, patch: Partial<PlacedLabel>, step: string | null = "Change label"): void {
  const current = session();
  if (!current || state().busy) return;
  const label = current.labels.find((l) => l.id === id);
  if (!label) return;
  if (step) record(step, `${id}:${step}`);
  let next = { ...label, ...patch };
  // Turning Keep aspect ratio off keeps the width the label had.
  if (patch.keepAspect === false && label.keepAspect) next = { ...next, width: labelWidth(label) ?? label.width };
  // Sized or given a depth by the user, the label keeps it; until then the depth follows the starting rule.
  if ("height" in patch || "width" in patch) next.sizeAuto = false;
  if ("depth" in patch) next.depthAuto = false;
  else if (next.depthAuto) next.depth = startDepth(next);
  if (patch.font) lastFont = patch.font;
  setSession({ labels: (session() ?? current).labels.map((l) => (l.id === id ? next : l)) });
  void ensureShape(next);
}

export function removeLabel(id: string): void {
  const current = session();
  if (!current || state().busy || !current.labels.some((l) => l.id === id)) return;
  record("Delete label");
  setSession({ labels: current.labels.filter((l) => l.id !== id), selected: current.selected === id ? null : current.selected });
}

export function canLabelUndo(): boolean {
  return (session()?.past.length ?? 0) > 0;
}

export function canLabelRedo(): boolean {
  return (session()?.future.length ?? 0) > 0;
}

/** The names of the steps Undo (or Redo) would take, the next first. */
export function labelHistory(kind: "undo" | "redo"): string[] {
  const current = session();
  if (!current) return [];
  return kind === "undo" ? current.past.map((p) => p.label).reverse() : current.future.map((p) => p.label);
}

export function labelUndo(): void {
  const current = session();
  if (!current || state().busy) return;
  const step = current.past[current.past.length - 1];
  if (!step) return toast("Nothing to undo.");
  setSession({
    labels: step.labels,
    selected: step.selected,
    past: current.past.slice(0, -1),
    future: [{ labels: current.labels, selected: current.selected, label: step.label }, ...current.future],
  });
}

export function labelRedo(): void {
  const current = session();
  if (!current || state().busy) return;
  const [step, ...rest] = current.future;
  if (!step) return toast("Nothing to redo.");
  setSession({
    labels: step.labels,
    selected: step.selected,
    future: rest,
    past: [...current.past, { labels: current.labels, selected: current.selected, label: step.label }],
  });
}

// ---------------------------------------------------------------------------------------------
// Handles: the centre moves the label over the part, the corner sizes and turns it (Ctrl turns only, Shift sizes
// only), and a tag's third handle sets the direction its plate stands out in.

export type HandleKind = "centre" | "corner" | "tag";

interface Drag {
  id: string;
  kind: HandleKind;
  before: LabelStep;
  /** Corner: the label's half diagonal and its angle at the start, in its plane. */
  start: { size: number; angle: number; height: number; width: number };
}

let drag: Drag | null = null;

/** Where the corner handle sits: the frame's top-right corner, in the label's plane. */
export function cornerOf(label: PlacedLabel): [number, number] {
  return [(labelWidth(label) ?? label.height) / 2, label.height / 2];
}

/** Where the tag's handle sits: the top of its plate, in world space. */
export function tagHandleOf(label: PlacedLabel): Vec3 | null {
  const f = labelFrame(label.point, label.normal, label.angle);
  if (!f) return null;
  const d = unit(label.tagDirection ?? label.normal) ?? f.w;
  const [, plateH] = plateSize(labelWidth(label) ?? label.height, label.height);
  return add(label.point, scale(d, TAG_GAP + plateH));
}

export function startHandleDrag(id: string, kind: HandleKind): void {
  const current = session();
  const label = current?.labels.find((l) => l.id === id);
  if (!current || !label || state().busy) return;
  const [cx, cy] = cornerOf(label);
  drag = {
    id,
    kind,
    before: { labels: current.labels, selected: current.selected, label: kind === "centre" ? "Move label" : kind === "corner" ? "Size and turn label" : "Point tag" },
    start: { size: Math.hypot(cx, cy), angle: label.angle, height: label.height, width: labelWidth(label) ?? label.height },
  };
  selectLabel(id);
}

/** The pointer moved during a handle drag, at client point (x, y), with Ctrl (`turnOnly`) or Shift (`sizeOnly`). */
export function moveHandleDrag(x: number, y: number, turnOnly: boolean, sizeOnly: boolean): void {
  const current = session();
  const label = current?.labels.find((l) => l.id === drag?.id);
  if (!drag || !current || !label) return;
  if (drag.kind === "centre") {
    const hit = viewport.surfaceAt(x, y);
    if (hit && hit.partId === current.partId) updateLabel(label.id, { point: hit.point, normal: hit.normal }, null);
    return;
  }
  const ray = viewport.rayAt(x, y);
  if (!ray) return;
  const f = labelFrame(label.point, label.normal, label.angle);
  if (!f) return;
  if (drag.kind === "tag") {
    // Parallel to the screen through the handle: the plate points from the label's centre to where the pointer is.
    const handle = tagHandleOf(label);
    if (!handle) return;
    const viewDir = viewport.viewDirection();
    const t = dot(sub(handle, ray.origin), viewDir) / dot(ray.direction, viewDir);
    if (!Number.isFinite(t)) return;
    const p = add(ray.origin, scale(ray.direction, t));
    const d = unit(sub(p, label.point));
    // Never below the surface: at most 70° from the normal, as Rust allows.
    if (d && dot(d, f.w) >= Math.cos((70 * Math.PI) / 180)) updateLabel(label.id, { tagDirection: d }, null);
    return;
  }
  // The corner, in a frame turned back to the angle the drag started at.
  const start = labelFrame(label.point, label.normal, drag.start.angle);
  const q = start && onPlane(start, ray.origin, ray.direction);
  if (!q) return;
  const [cx, cy] = [drag.start.width / 2, drag.start.height / 2];
  const turn = ((Math.atan2(q[1], q[0]) - Math.atan2(cy, cx)) * 180) / Math.PI;
  const factor = Math.hypot(q[0], q[1]) / Math.max(drag.start.size, 1e-6);
  const patch: Partial<PlacedLabel> = {};
  if (!sizeOnly) patch.angle = Math.round((drag.start.angle + turn) * 10) / 10;
  if (!turnOnly) {
    patch.height = Math.max(0.1, Math.round(drag.start.height * factor * 100) / 100);
    if (!label.keepAspect) patch.width = Math.max(0.1, Math.round(drag.start.width * factor * 100) / 100);
  }
  updateLabel(label.id, patch, null);
}

/** The drag ended: one undo step for all of it, when the label changed. */
export function endHandleDrag(): void {
  const ended = drag;
  drag = null;
  const current = session();
  if (!ended || !current) return;
  const before = ended.before.labels.find((l) => l.id === ended.id);
  const after = current.labels.find((l) => l.id === ended.id);
  if (before && after && before !== after) setSession({ past: [...current.past, ended.before].slice(-100), future: [] });
}

export function isDraggingHandle(): boolean {
  return drag !== null;
}

// ---------------------------------------------------------------------------------------------
// The panel's pickers

/** Click to add image: the picture dialog; the label shows the new picture's shape. */
export async function pickLabelPicture(id: string): Promise<void> {
  try {
    const picture = await pickPicture();
    if (!picture || !isLabelOpen()) return;
    updateLabel(id, { picture }, "Pick picture");
    // Placed as a square; with its picture, a label nobody has sized yet is fitted again.
    if (session()?.labels.find((l) => l.id === id)?.sizeAuto) await fitNewLabel(id);
  } catch (err) {
    toast(errorMessage(err, "Could not read the picture"), "error");
  }
}

/** Load CSV file: the dialog, then the file's first column with text. */
export async function pickLabelCsv(id: string): Promise<void> {
  try {
    const file = await pickCsv();
    if (!file || !isLabelOpen()) return;
    if (file.rows.length === 0) return toast(`${file.name} has no rows.`, "warn");
    const width = Math.max(...file.rows.map((r) => r.length));
    const column = Math.max(0, [...Array(width).keys()].find((c) => file.rows.some((r) => (r[c] ?? "").trim() !== "")) ?? 0);
    updateLabel(id, { list: { kind: "csv", file: file.name, rows: file.rows, column, header: false } }, "Load CSV file");
  } catch (err) {
    toast(errorMessage(err, "Could not read the list"), "error");
  }
}

// ---------------------------------------------------------------------------------------------
// Zoom to

function labelBounds(label: PlacedLabel): Bounds | null {
  const f = labelFrame(label.point, label.normal, label.angle);
  if (!f) return null;
  const [w, h] = [(labelWidth(label) ?? label.height) / 2, label.height / 2];
  const corners = [at(f, -w, -h), at(f, w, -h), at(f, w, h), at(f, -w, h), at(f, 0, 0, label.depth)];
  const tag = label.kind === "tag" ? tagHandleOf(label) : null;
  if (tag) corners.push(tag);
  const min: Vec3 = [0, 1, 2].map((k) => Math.min(...corners.map((c) => c[k]))) as Vec3;
  const max: Vec3 = [0, 1, 2].map((k) => Math.max(...corners.map((c) => c[k]))) as Vec3;
  return { min, max };
}

export function zoomToLabelPart(): void {
  const part = labelPart();
  const box = part && boundsOf(part.mesh, part.matrix);
  if (box) viewport.zoomToBounds(box);
}

/** Zoom to label: the selected one, or all of them. */
export function zoomToLabel(): void {
  const current = session();
  if (!current) return;
  const labels = current.selected ? current.labels.filter((l) => l.id === current.selected) : current.labels;
  const boxes = labels.map(labelBounds).filter((b): b is Bounds => b !== null);
  if (boxes.length === 0) return toast("Place a label first: click on the part.");
  const min = [0, 1, 2].map((k) => Math.min(...boxes.map((b) => b.min[k]))) as Vec3;
  const max = [0, 1, 2].map((k) => Math.max(...boxes.map((b) => b.max[k]))) as Vec3;
  viewport.zoomToBounds({ min, max });
}

// ---------------------------------------------------------------------------------------------
// Apply

/** Gap between list copies laid out side by side, mm. */
const COPY_GAP = 5;

/**
 * Apply to part: Rust puts every label into the part (with a list, into one copy per entry), then the user says
 * whether the original goes (Yes) or stays (No). The new parts take its place, list copies side by side along X, and
 * the whole is one undo step. The mode then closes.
 */
export async function applyLabelMode(): Promise<void> {
  const current = session();
  if (!current || state().busy) return;
  const part = labelPart();
  if (!part) return leave();
  if (current.labels.length === 0) return toast("Place a label first: click on the part.", "warn");
  if (current.labels.some((l) => l.fitting)) return toast("A label is still being sized. Try again in a moment.");
  await Promise.all(current.labels.map(ensureShape));
  const labels = session()?.labels ?? [];
  for (const label of labels) {
    const shape = shapeOf(label);
    if (shape?.status === "error") {
      selectLabel(label.id);
      return toast(shape.message, "warn");
    }
  }
  const widths = labels.map((l) => labelWidth(l) ?? l.height);
  const job = nextJob();
  useLabel.setState({ busy: { job, cancelling: false, fraction: 0 } });
  const task = startTask(`Adding labels to ${part.name}`);
  try {
    const result = await applyLabels({ name: part.name, mesh: part.mesh, matrix: part.matrix }, labels, widths, job, (fraction) => {
      const busy = state().busy;
      if (busy?.job === job) useLabel.setState({ busy: { ...busy, fraction } });
      task.progress(fraction);
    });
    if (state().busy?.job !== job || state().busy?.cancelling || !isLabelOpen()) return;
    const remove = await yesNo(`Remove the original part "${part.name}"? Yes keeps only the labelled part, No keeps both.`, "Generate label");
    if (state().busy?.job !== job || !isLabelOpen() || !store().parts.some((p) => p.id === part.id)) return;
    const added: NewPart[] = [];
    const listed = labels.some((l) => l.kind === "list");
    let step = 0;
    result.copies.forEach((copy, i) => {
      primeMeshCaches(copy.mesh, { stats: copy.stats, edges: copy.edges });
      const box = boundsOf(copy.mesh, IDENTITY);
      if (i === 0 && box) step = box.max[0] - box.min[0] + COPY_GAP;
      // A list's copies stand side by side; a kept original keeps its place, so the copies start one step on.
      const slot = listed ? i + (remove ? 0 : 1) : 0;
      const name = listed ? `${part.name} ${copy.name}`.trim() : `${part.name} labelled`;
      added.push({ name, mesh: copy.mesh, matrix: slot ? translation(slot * step, 0, 0) : [...IDENTITY], source: part.source, color: part.color });
    });
    store().replaceParts([{ remove: remove ? [part.id] : [], add: added, after: part.id }], labels.length === 1 ? "Generate label" : "Generate labels");
    if (result.note) toast(result.note);
    useLabel.setState({ busy: null });
    leave();
  } catch (err) {
    if (err !== CANCELLED && isLabelOpen()) toast(errorMessage(err, "Could not add the labels"), "error");
  } finally {
    task.end();
    if (state().busy?.job === job) useLabel.setState({ busy: null });
  }
}

/** Stops an Apply in progress; nothing changes. */
export function cancelApply(): void {
  const busy = state().busy;
  if (!busy || busy.cancelling) return;
  useLabel.setState({ busy: { ...busy, cancelling: true } });
  cancelRepair(busy.job).catch(() => {});
}

/** A key label generation uses itself: Esc and Delete (or Backspace). True when it did something with it. */
export function labelKey(key: string): boolean {
  const current = session();
  if (!current) return false;
  if (key === "Escape") {
    if (current.tool === "place" && current.labels.length > 0) pickLabelTool("select");
    else if (current.selected) selectLabel(null);
    else return false;
    return true;
  }
  if ((key === "Delete" || key === "Backspace") && current.selected) {
    removeLabel(current.selected);
    return true;
  }
  return false;
}
