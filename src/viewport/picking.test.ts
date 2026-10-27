import { Vector3 } from "three";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeshData } from "../store";
import type { RunResult } from "../repair/types";

// Rust is a stand-in: runRepair answers what each test says. The canvas is a stand-in too, which hands its listeners
// the pointer events of a click or a drag; the pick functions say what is under the pointer.
const ipc = vi.hoisted(() => ({
  runRepair: vi.fn(),
  cancelRepair: vi.fn(),
  fetchCatalog: vi.fn(),
  loadStoredScripts: vi.fn(),
  saveStoredScripts: vi.fn(),
  checkMeshes: vi.fn(),
}));
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...ipc }));

import { makeBox } from "../mesh";
import { enterRepair, runAction, selectTriangles } from "../repair/session";
import { maskToIds } from "../repair/selection";
import { useRepair } from "../repair/store";
import type { RepairSession, SelectTool } from "../repair/types";
import { useStore } from "../store";
import { useUi } from "../ui";
import type { ScreenRect } from "./boxSelect";
import { attachPicking, type BoxSelector, type TrianglePick } from "./picking";

const result = (mesh: MeshData, changed: boolean): RunResult => ({
  changed,
  mesh,
  stats: null,
  edges: null,
  selection: new Uint32Array(0),
  selected: false,
  reports: [],
  status: null,
  highlights: null,
  intersecting: null,
  warnings: [],
});

type Keys = Partial<{ shiftKey: boolean; ctrlKey: boolean; metaKey: boolean; altKey: boolean }>;

/** The window's listeners (attachPicking listens for Esc there). */
const windowKeys = new Map<string, (e: unknown) => void>();

/**
 * A canvas stand-in at (10, 20) on the page, which hands the listeners attachPicking added: click(button) a press and
 * a release at one spot, drag(from, to, keys) a left press, two moves and a release, and key(name) a key press to the
 * window.
 */
function stubCanvas() {
  const listeners = new Map<string, (e: unknown) => void>();
  const canvas = {
    addEventListener: (type: string, listener: (e: unknown) => void) => listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
    getBoundingClientRect: () => ({ left: 10, top: 20, width: 800, height: 600 }),
    setPointerCapture: () => {},
  } as unknown as HTMLCanvasElement;
  const event = (button: number, x: number, y: number, keys: Keys = {}) => ({
    button,
    pointerId: 1,
    clientX: x,
    clientY: y,
    shiftKey: false,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    ...keys,
  });
  const click = (button: number) => {
    listeners.get("pointerdown")?.(event(button, 30, 40));
    listeners.get("pointerup")?.(event(button, 30, 40));
  };
  const press = (x: number, y: number, button = 0) => listeners.get("pointerdown")?.(event(button, x, y));
  const move = (x: number, y: number) => listeners.get("pointermove")?.(event(0, x, y));
  const release = (x: number, y: number, keys: Keys = {}, button = 0) => listeners.get("pointerup")?.(event(button, x, y, keys));
  const drag = (from: [number, number], to: [number, number], keys: Keys = {}) => {
    press(...from);
    move((from[0] + to[0]) / 2, (from[1] + to[1]) / 2);
    move(...to);
    release(...to, keys);
  };
  const key = (name: string) => windowKeys.get("keydown")?.({ key: name, preventDefault() {}, stopPropagation() {} });
  return { canvas, click, press, move, release, drag, key, attached: () => listeners.size };
}

/** A box selector that records what it is asked to draw and to select. */
function recordBox() {
  const drawn: (ScreenRect | null)[] = [];
  const selected: [ScreenRect, string][] = [];
  const box: BoxSelector = { draw: (r) => drawn.push(r), select: (r, mode) => selected.push([r, mode]) };
  return { box, drawn, selected };
}

const selected = () => Array.from(maskToIds(useRepair.getState().session?.selection ?? new Uint8Array(0)));

beforeEach(() => {
  windowKeys.clear();
  vi.stubGlobal("window", {
    setTimeout: () => 0,
    confirm: () => true,
    addEventListener: (type: string, listener: (e: unknown) => void) => windowKeys.set(type, listener),
    removeEventListener: (type: string) => windowKeys.delete(type),
  });
  vi.stubGlobal("localStorage", undefined);
  vi.stubGlobal("isTauri", true);
  for (const f of Object.values(ipc)) f.mockReset();
  ipc.cancelRepair.mockResolvedValue(undefined);
  useRepair.setState({ catalog: { actions: [], scripts: [] }, session: null, busy: null, selectTool: "triangle" });
  useStore.getState().newProject();
  useUi.setState({ toasts: [], dialog: null, contextMenu: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("attachPicking in repair mode", () => {
  it("selects triangles and opens the repair menu, except while a run is going, whose result replaces that mesh", async () => {
    const [id] = useStore.getState().addParts([{ name: "Part", mesh: makeBox([20, 20, 20]) }], "Add Part");
    ipc.runRepair.mockImplementationOnce(async (mesh: MeshData) => result(mesh, false));
    enterRepair(id);
    await vi.waitFor(() => expect(useRepair.getState().busy).toBeNull());
    const hit: TrianglePick = { triangle: 4, point: [1, 2, 3] };
    const { canvas, click, attached } = stubCanvas();
    const detach = attachPicking(canvas, () => null, () => hit, () => false, recordBox().box);

    // A run starts; until its progress dialog covers the window, the canvas still gets clicks. They do nothing: a
    // pick or the menu's triangle would point into the mesh the result is about to replace.
    selectTriangles([1], "replace");
    let finish!: (r: RunResult) => void;
    ipc.runRepair.mockReturnValueOnce(new Promise<RunResult>((resolve) => (finish = resolve)));
    const run = runAction("close_all_holes");
    expect(useRepair.getState().busy).not.toBeNull();
    click(0);
    expect(selected()).toEqual([1]);
    click(2);
    expect(useUi.getState().contextMenu).toBeNull();
    finish(result(makeBox([1, 1, 1]), true));
    await run;

    // Once it is over, a click selects and a right click opens the menu on the triangle under the pointer.
    click(0);
    expect(selected()).toEqual([4]);
    click(2);
    expect(useUi.getState().contextMenu).toEqual({ x: 30, y: 40, point: [1, 2, 3], triangle: 4 });
    detach();
    expect(attached()).toBe(0);
    expect(windowKeys.size).toBe(0);
  });
});

describe("attachPicking box selection", () => {
  it("draws a box from empty space and selects with it on release, with the modifiers held then", () => {
    const { box, drawn, selected } = recordBox();
    const { canvas, drag } = stubCanvas();
    attachPicking(canvas, () => null, () => null, () => false, box);
    drag([110, 220], [60, 320]);
    // Canvas pixels (the canvas starts at 10, 20), whichever way the drag went.
    const rect = { left: 50, top: 200, right: 100, bottom: 300 };
    expect(drawn[drawn.length - 2]).toEqual(rect);
    expect(drawn[drawn.length - 1]).toBeNull();
    expect(selected).toEqual([[rect, "replace"]]);
    drag([110, 220], [60, 320], { shiftKey: true });
    drag([110, 220], [60, 320], { ctrlKey: true });
    drag([110, 220], [60, 320], { metaKey: true });
    expect(selected.map((s) => s[1])).toEqual(["replace", "add", "toggle", "toggle"]);
  });

  it("leaves drags from a part, from the gizmo and with other buttons alone; a short one stays a click", () => {
    const { box, drawn, selected } = recordBox();
    const [id] = useStore.getState().addParts([{ name: "Part", mesh: makeBox([20, 20, 20]) }], "Add Part");
    let under: { partId: string; point: Vector3 } | null = { partId: id, point: new Vector3() };
    let gizmo = false;
    const { canvas, drag, press, move, release } = stubCanvas();
    attachPicking(canvas, () => under, () => null, () => gizmo, box);
    drag([110, 220], [300, 400]);
    under = null;
    gizmo = true;
    drag([110, 220], [300, 400]);
    gizmo = false;
    // A right drag orbits and a middle drag pans.
    for (const button of [1, 2]) {
      press(110, 220, button);
      move(300, 400);
      release(300, 400, {}, button);
    }
    expect(drawn).toEqual([]);
    expect(selected).toEqual([]);
    // Within the click slop: a click on empty space, which clears the selection.
    useStore.getState().select([id]);
    press(110, 220);
    move(112, 221);
    release(112, 221);
    expect(drawn).toEqual([]);
    expect(useStore.getState().selection).toEqual([]);
  });

  it("drops the box on Esc, and makes none while a repair run goes", () => {
    const { box, drawn, selected } = recordBox();
    const { canvas, press, move, release, key } = stubCanvas();
    attachPicking(canvas, () => null, () => null, () => false, box);
    press(110, 220);
    move(200, 300);
    key("Escape");
    expect(drawn[drawn.length - 1]).toBeNull();
    release(200, 300);
    expect(selected).toEqual([]);
    useRepair.setState({ busy: { job: 1, label: "Wrap part surface", progress: null, step: null, detail: null, local: null, cancelling: false } });
    press(110, 220);
    move(200, 300);
    release(200, 300);
    expect(selected).toEqual([]);
  });

  it("draws the rectangle from a triangle when area select is on, and brush select paints triangles instead", () => {
    const mesh = makeBox([20, 20, 20]);
    const session: RepairSession = {
      partId: "part-1",
      partName: "Part",
      initialMesh: mesh,
      mesh,
      selection: new Uint8Array(12),
      selectedCount: 0,
      past: [],
      future: [],
      status: null,
      highlights: null,
      intersecting: null,
      lastRun: null,
      hiddenTriangles: null,
    };
    const open = (tool: SelectTool) => useRepair.setState({ session: { ...session, selection: new Uint8Array(12), selectedCount: 0 }, selectTool: tool });
    const hit = (triangle: number): TrianglePick => ({ triangle, point: [0, 0, 0] });

    open("triangle");
    const blocked = recordBox();
    const fromPart = stubCanvas();
    attachPicking(fromPart.canvas, () => null, () => hit(1), () => false, blocked.box);
    fromPart.drag([110, 220], [300, 400]);
    expect(blocked.drawn).toEqual([]);

    open("area");
    const area = recordBox();
    const anywhere = stubCanvas();
    attachPicking(anywhere.canvas, () => null, () => hit(1), () => false, area.box);
    anywhere.drag([110, 220], [300, 400]);
    expect(area.selected).toHaveLength(1);

    open("brush");
    let triangle = 2;
    const brush = recordBox();
    const painted = stubCanvas();
    attachPicking(painted.canvas, () => null, () => hit(triangle), () => false, brush.box);
    painted.press(110, 220);
    painted.move(160, 260);
    triangle = 5;
    painted.move(220, 320);
    painted.release(220, 320);
    expect(brush.drawn).toEqual([]);
    expect(selected()).toEqual([2, 5]);
  });
});
