import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeshData } from "./store";
import type { RunResult } from "./repair/types";

// useKeyboardShortcuts adds its window listeners in an effect. Run outside React, the effect runs at once, so the
// tests below catch the real listeners on a stand-in window.
vi.mock("react", async (importOriginal) => ({
  ...(await importOriginal<typeof import("react")>()),
  useEffect: (effect: () => void) => void effect(),
}));

// Repair mode: Rust and the viewport are stand-ins. runRepair answers each run with the mesh unchanged unless a test
// says otherwise, and zooms are recorded.
const ipc = vi.hoisted(() => ({ runRepair: vi.fn(), cancelRepair: vi.fn(), checkMeshes: vi.fn() }));
const view = vi.hoisted(() => ({ zoomTo: vi.fn(), zoomToBounds: vi.fn(), setViewPreset: vi.fn(), centreOn: vi.fn(), viewDirection: vi.fn() }));
vi.mock("./repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("./repair/ipc")>()), ...ipc }));
vi.mock("./viewport/api", () => ({ viewport: view, registerViewport: vi.fn() }));

import { useBoolean } from "./boolean/store";
import { featureById, registryProblems } from "./features";
import { isBrowserShortcut, shortcutFor, useKeyboardShortcuts } from "./keyboard";
import { makeBox } from "./mesh";
import { enterRepair, isRepairing, runAction, selectTriangles } from "./repair/session";
import { useRepair } from "./repair/store";
import { shownGizmo, useStore } from "./store";
import { useUi } from "./ui";

type Mods = Partial<Record<"ctrlKey" | "metaKey" | "shiftKey" | "altKey", boolean>>;

/** A key press as the webview reports it: `key` is what the layout types, `code` the physical key. */
const press = (key: string, code: string, mods: Mods = {}) => ({
  key,
  code,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

const ctrlShift = { ctrlKey: true, shiftKey: true };
const cmdShift = { metaKey: true, shiftKey: true };

describe("Always show gizmo shortcut", () => {
  it("is Ctrl+Shift+G, and Cmd+Shift+G on macOS", () => {
    expect(shortcutFor(press("G", "KeyG", ctrlShift), false)).toBe("view.always-show-gizmo");
    expect(shortcutFor(press("G", "KeyG", cmdShift), true)).toBe("view.always-show-gizmo");
  });

  it("goes by the physical key on a Cyrillic layout", () => {
    expect(shortcutFor(press("п", "KeyG", ctrlShift), false)).toBe("view.always-show-gizmo");
    expect(shortcutFor(press("П", "KeyG", ctrlShift), false)).toBe("view.always-show-gizmo");
  });

  it("needs both Ctrl and Shift, and no Alt (Ctrl+Alt is AltGr on Windows)", () => {
    expect(shortcutFor(press("g", "KeyG", { ctrlKey: true }), false)).toBeNull();
    expect(shortcutFor(press("G", "KeyG", { shiftKey: true }), false)).toBeNull();
    expect(shortcutFor(press("G", "KeyG", { ...ctrlShift, altKey: true }), false)).toBeNull();
  });

  it("is still kept from the webview, which would open find", () => {
    expect(isBrowserShortcut(press("G", "KeyG", ctrlShift))).toBe(true);
    expect(isBrowserShortcut(press("G", "KeyG", cmdShift))).toBe(true);
    expect(isBrowserShortcut(press("g", "KeyG", { ctrlKey: true }))).toBe(true);
  });

  it("runs a feature that exists, labelled with the same keys", () => {
    expect(registryProblems()).toEqual([]);
    expect(featureById("view.always-show-gizmo")?.shortcut).toMatch(/^(Ctrl|Cmd)\+Shift\+G$/);
  });
});

describe("gizmo keys", () => {
  it("W and E pick the move and rotate gizmo, R the scale gizmo", () => {
    expect(shortcutFor(press("w", "KeyW"), false)).toBe("gizmo.combined");
    expect(shortcutFor(press("e", "KeyE"), false)).toBe("gizmo.combined");
    expect(shortcutFor(press("r", "KeyR"), false)).toBe("gizmo.scale");
    expect(shortcutFor(press("к", "KeyR"), false)).toBe("gizmo.scale");
  });

  it("ignore the letters with Shift or Ctrl", () => {
    expect(shortcutFor(press("W", "KeyW", { shiftKey: true }), false)).toBeNull();
    expect(shortcutFor(press("e", "KeyE", { ctrlKey: true }), false)).toBeNull();
  });

  it("leave Esc to the escape action, not to a gizmo", () => {
    expect(shortcutFor(press("Escape", "Escape"), false)).toBe("escape");
  });
});

type Listener = (e: KeyboardEvent) => void;

interface Delivery {
  /** Auto-repeat of a held key. */
  repeat?: boolean;
  /** The focused control used the key and marked it handled. */
  claimed?: boolean;
  target?: EventTarget | null;
}

/**
 * Adds the app's shortcuts to a stand-in window and returns `send`, which delivers one keydown to them the way the
 * webview does: the capture-phase listeners first, then the focused control (which may claim the key), then the
 * bubble-phase listeners. The returned event says whether the webview's own action was blocked.
 */
function keyboard() {
  const capture: Listener[] = [];
  const bubble: Listener[] = [];
  vi.stubGlobal("window", {
    addEventListener: (type: string, listener: Listener, useCapture?: boolean) => {
      if (type === "keydown") (useCapture ? capture : bubble).push(listener);
    },
    removeEventListener: () => {},
    setTimeout: () => 0,
  });
  useKeyboardShortcuts();
  return (key: string, code: string, mods: Mods = {}, { repeat = false, claimed = false, target = null }: Delivery = {}) => {
    const e = {
      ...press(key, code, mods),
      repeat,
      isComposing: false,
      target,
      defaultPrevented: false,
      preventDefault() {
        this.defaultPrevented = true;
      },
    };
    for (const listener of capture) listener(e as unknown as KeyboardEvent);
    if (claimed) e.preventDefault();
    for (const listener of bubble) listener(e as unknown as KeyboardEvent);
    return e;
  };
}

const s = () => useStore.getState();

describe("shortcuts on the window", () => {
  beforeEach(() => {
    // Node has no DOM: keyboard.ts checks the focused element against these classes, and no target here is a control.
    for (const name of ["HTMLElement", "HTMLInputElement", "HTMLTextAreaElement", "HTMLSelectElement"]) vi.stubGlobal(name, class {});
    // Saving the remembered setting then fails quietly, whatever the Node version offers.
    vi.stubGlobal("localStorage", undefined);
    s().newProject();
    s().setView({ alwaysShowGizmo: true });
    useUi.setState({ dialog: null, toasts: [] });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("run Ctrl+Shift+G although blockBrowserShortcut marked it handled, and keep it from the webview's find", () => {
    const send = keyboard();
    expect(send("G", "KeyG", ctrlShift).defaultPrevented).toBe(true);
    expect(s().view.alwaysShowGizmo).toBe(false);
    send("G", "KeyG", cmdShift);
    expect(s().view.alwaysShowGizmo).toBe(true);
  });

  it("leave a key the focused control claimed, such as Delete", () => {
    const send = keyboard();
    s().addTestCube();
    send("Delete", "Delete", {}, { claimed: true });
    expect(s().parts).toHaveLength(1);
    send("Delete", "Delete");
    expect(s().parts).toHaveLength(0);
  });

  it("flip Always show gizmo once while Ctrl+Shift+G is held, and keep undo repeating", () => {
    const send = keyboard();
    send("G", "KeyG", ctrlShift);
    for (let i = 0; i < 5; i++) {
      // Still kept from the webview's find on every repeat.
      expect(send("G", "KeyG", ctrlShift, { repeat: true }).defaultPrevented).toBe(true);
    }
    expect(s().view.alwaysShowGizmo).toBe(false);

    s().addTestCube();
    s().addTestCube();
    send("z", "KeyZ", { ctrlKey: true });
    send("z", "KeyZ", { ctrlKey: true }, { repeat: true });
    expect(s().parts).toHaveLength(0);
  });

  it("step Esc back from Scale to Select while Always show gizmo is on, then clear the selection", () => {
    const send = keyboard();
    s().addTestCube();
    send("r", "KeyR");
    expect(s().gizmoMode).toBe("scale");
    send("Escape", "Escape");
    // The move and rotate gizmo shows under Select anyway, so the next Esc has something visible to do.
    expect(s().gizmoMode).toBeNull();
    expect(shownGizmo(s())).toBe("combined");
    expect(s().selection).toHaveLength(1);
    send("Escape", "Escape");
    expect(s().selection).toEqual([]);
  });

  it("step Esc back from Scale to the move and rotate gizmo while Always show gizmo is off, then Select, then clear", () => {
    const send = keyboard();
    s().setView({ alwaysShowGizmo: false });
    s().addTestCube();
    send("r", "KeyR");
    send("Escape", "Escape");
    expect(s().gizmoMode).toBe("combined");
    send("Escape", "Escape");
    expect(s().gizmoMode).toBeNull();
    expect(shownGizmo(s())).toBeNull();
    expect(s().selection).toHaveLength(1);
    send("Escape", "Escape");
    expect(s().selection).toEqual([]);
  });

  it("drop a picked move and rotate tool when Always show gizmo turns off, and pick none when it turns on", () => {
    const send = keyboard();
    s().addTestCube();
    send("w", "KeyW");
    send("G", "KeyG", ctrlShift);
    expect(s().view.alwaysShowGizmo).toBe(false);
    expect(s().gizmoMode).toBeNull();
    expect(shownGizmo(s())).toBeNull();

    // On again from Select: the gizmo shows under Select, with no tool picked that the next Esc would only drop.
    send("G", "KeyG", ctrlShift);
    expect(s().view.alwaysShowGizmo).toBe(true);
    expect(s().gizmoMode).toBeNull();
    expect(shownGizmo(s())).toBe("combined");

    // Off again, then E picks the tool and turning the setting on keeps it.
    send("G", "KeyG", ctrlShift);
    send("e", "KeyE");
    send("G", "KeyG", ctrlShift);
    expect(s().view.alwaysShowGizmo).toBe(true);
    expect(s().gizmoMode).toBe("combined");

    // Scale is a tool of its own and stays, whichever way the setting turns.
    send("r", "KeyR");
    send("G", "KeyG", ctrlShift);
    expect(s().view.alwaysShowGizmo).toBe(false);
    expect(s().gizmoMode).toBe("scale");
    send("G", "KeyG", ctrlShift);
    expect(s().view.alwaysShowGizmo).toBe(true);
    expect(s().gizmoMode).toBe("scale");
    expect(shownGizmo(s())).toBe("scale");
  });

  it("copy and paste with Ctrl+C and Ctrl+V, except in a text field, a dialog, repair mode or Boolean operations", () => {
    const send = keyboard();
    expect(shortcutFor(press("c", "KeyC", { ctrlKey: true }), false)).toBe("home.copy");
    expect(shortcutFor(press("v", "KeyV", { metaKey: true }), true)).toBe("home.paste");
    expect(shortcutFor(press("C", "KeyC", { ctrlKey: true, shiftKey: true }), false)).toBeNull();

    s().addTestCube();
    send("c", "KeyC", { ctrlKey: true });
    expect(s().clipboard).toHaveLength(1);
    send("v", "KeyV", { ctrlKey: true });
    expect(s().parts).toHaveLength(2);

    const input = new (globalThis.HTMLInputElement as new () => { type: string })();
    input.type = "text";
    send("v", "KeyV", { ctrlKey: true }, { target: input as unknown as EventTarget });
    expect(s().parts).toHaveLength(2);

    useUi.setState({ dialog: { kind: "about" } });
    send("v", "KeyV", { ctrlKey: true });
    expect(s().parts).toHaveLength(2);
    useUi.setState({ dialog: null });

    useRepair.setState({ session: {} as never });
    send("c", "KeyC", { ctrlKey: true });
    expect(s().clipboard).toHaveLength(1);
    useRepair.setState({ session: null });

    useBoolean.setState({ session: {} as never });
    send("v", "KeyV", { ctrlKey: true });
    expect(s().parts).toHaveLength(2);
    useBoolean.setState({ session: null });
  });
});

describe("repair mode shortcuts", () => {
  const repairKey = (key: string, code: string, mods: Mods = {}, isMac = false) => shortcutFor(press(key, code, mods), isMac, true);

  it("act on the working copy: Del, Esc, = or +, -, F and A", () => {
    expect(repairKey("Delete", "Delete")).toBe("repair.remove-triangles");
    expect(repairKey("Backspace", "Backspace", {}, true)).toBe("repair.remove-triangles");
    expect(repairKey("Backspace", "Backspace")).toBeNull();
    expect(repairKey("Escape", "Escape")).toBe("repair.select-none");
    expect(repairKey("=", "Equal")).toBe("repair-ctx.expand");
    // + is Shift+= on a US layout, its own key on a German one, and on the number pad.
    expect(repairKey("+", "Equal", { shiftKey: true })).toBe("repair-ctx.expand");
    expect(repairKey("+", "BracketRight")).toBe("repair-ctx.expand");
    expect(repairKey("+", "NumpadAdd")).toBe("repair-ctx.expand");
    expect(repairKey("-", "Minus")).toBe("repair-ctx.shrink");
    expect(repairKey("-", "NumpadSubtract")).toBe("repair-ctx.shrink");
    expect(repairKey("f", "KeyF")).toBe("repair.zoom-selected");
    expect(repairKey("a", "KeyA")).toBe("repair.zoom-part");
    expect(repairKey("ф", "KeyA")).toBe("repair.zoom-part");
  });

  it("pick no gizmo with W, E or R, and ignore Shift with letters and Alt with anything", () => {
    for (const [key, code] of [["w", "KeyW"], ["e", "KeyE"], ["r", "KeyR"]]) expect(repairKey(key, code)).toBeNull();
    expect(repairKey("F", "KeyF", { shiftKey: true })).toBeNull();
    expect(repairKey("_", "Minus", { shiftKey: true })).toBeNull();
    expect(repairKey("=", "Equal", { altKey: true })).toBeNull();
  });

  it("keep the Ctrl and Cmd shortcuts, Undo and Redo among them", () => {
    expect(repairKey("z", "KeyZ", { ctrlKey: true })).toBe("modify.undo");
    expect(repairKey("Z", "KeyZ", ctrlShift)).toBe("modify.redo");
    expect(repairKey("y", "KeyY", { metaKey: true })).toBe("modify.redo");
    expect(repairKey("s", "KeyS", { ctrlKey: true })).toBe("file.save");
    expect(repairKey("o", "KeyO", { ctrlKey: true })).toBe("file.open");
    expect(repairKey("G", "KeyG", ctrlShift)).toBe("view.always-show-gizmo");
  });

  it("run features that exist, labelled with the same keys", () => {
    const labels: Record<string, string> = {
      "repair.remove-triangles": "Del",
      "repair-ctx.remove-selected": "Del",
      "repair.select-none": "Esc",
      "repair-ctx.expand": "=",
      "repair-ctx.shrink": "-",
      "repair.zoom-selected": "F",
      "repair.zoom-part": "A",
    };
    for (const [id, key] of Object.entries(labels)) expect([id, featureById(id)?.shortcut]).toEqual([id, key]);
  });
});

describe("shortcuts on the window in repair mode", () => {
  const unchanged = (mesh: MeshData): RunResult => ({
    changed: false,
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
  const session = () => {
    const current = useRepair.getState().session;
    if (!current) throw new Error("not repairing");
    return current;
  };
  const idle = () => vi.waitFor(() => expect(useRepair.getState().busy).toBeNull());
  /** Runs since entering repair mode: [action of the first step, selection] each. */
  const runs = () => ipc.runRepair.mock.calls.slice(1).map((call) => [call[1][0]?.type, Array.from(call[2].selection)]);

  let partId = "";

  beforeEach(async () => {
    for (const name of ["HTMLElement", "HTMLInputElement", "HTMLTextAreaElement", "HTMLSelectElement"]) vi.stubGlobal(name, class {});
    vi.stubGlobal("localStorage", undefined);
    vi.stubGlobal("isTauri", true);
    for (const f of [...Object.values(ipc), ...Object.values(view)]) f.mockReset();
    ipc.runRepair.mockImplementation(async (mesh: MeshData) => unchanged(mesh));
    ipc.cancelRepair.mockResolvedValue(undefined);
    // A catalog in place keeps entering repair mode from loading one.
    useRepair.setState({ catalog: { actions: [], scripts: [] }, session: null, busy: null, selectTool: "triangle" });
    s().newProject();
    useUi.setState({ dialog: null, toasts: [], contextMenu: null });
    s().addTestCube();
    partId = s().selection[0];
    enterRepair(partId);
    await idle();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("remove the selected triangles with Del, and do nothing while none is selected", async () => {
    const send = keyboard();
    expect(isRepairing()).toBe(true);
    expect(send("Delete", "Delete").defaultPrevented).toBe(false);
    expect(runs()).toEqual([]);
    selectTriangles([2, 3], "replace");
    expect(send("Delete", "Delete").defaultPrevented).toBe(true);
    await idle();
    expect(runs()).toEqual([["remove_selected_triangles", [2, 3]]]);
    // The part itself stays.
    expect(s().parts).toHaveLength(1);
  });

  it("clear the triangle selection with Esc, leaving the part selected", () => {
    const send = keyboard();
    selectTriangles([1, 4], "replace");
    send("Escape", "Escape");
    expect(session().selectedCount).toBe(0);
    expect(s().selection).toEqual([partId]);
  });

  it("grow the selection with = or + and shrink it with -, and do nothing without a selection", () => {
    const send = keyboard();
    expect(send("=", "Equal").defaultPrevented).toBe(false);
    expect(session().selectedCount).toBe(0);
    selectTriangles([0], "replace");
    send("=", "Equal");
    const grown = session().selectedCount;
    expect(grown).toBeGreaterThan(1);
    send("-", "Minus");
    expect(session().selectedCount).toBeLessThan(grown);
    selectTriangles([0], "replace");
    send("+", "Equal", { shiftKey: true });
    expect(session().selectedCount).toBe(grown);
  });

  it("undo the last repair step with Ctrl+Z, never a main step, and say when there is nothing to undo", async () => {
    const send = keyboard();
    const start = session().mesh;
    ipc.runRepair.mockResolvedValueOnce({ ...unchanged(makeBox([5, 5, 5])), changed: true });
    await runAction("close_all_holes");
    expect(session().mesh).not.toBe(start);
    send("z", "KeyZ", { ctrlKey: true });
    expect(session().mesh).toBe(start);
    useUi.setState({ toasts: [] });
    send("z", "KeyZ", { ctrlKey: true });
    expect(useUi.getState().toasts.map((t) => t.message)).toEqual(["Nothing to undo."]);
    expect(s().parts).toHaveLength(1);
    send("y", "KeyY", { ctrlKey: true });
    expect(session().mesh).not.toBe(start);
  });

  it("pick no gizmo with W, E or R", () => {
    const send = keyboard();
    for (const [key, code] of [["w", "KeyW"], ["e", "KeyE"], ["r", "KeyR"]]) {
      expect(send(key, code).defaultPrevented).toBe(false);
      expect(s().gizmoMode).toBeNull();
    }
  });

  it("zoom to the selected triangles or the part with F, and to the part with A", () => {
    const send = keyboard();
    const part = { min: [90, 90, 0], max: [110, 110, 20] };
    send("f", "KeyF");
    expect(view.zoomToBounds).toHaveBeenLastCalledWith(part);
    // Triangle 0 is half of the test cube's bottom face.
    selectTriangles([0], "replace");
    send("f", "KeyF");
    expect(view.zoomToBounds).toHaveBeenLastCalledWith({ min: [90, 90, 0], max: [110, 110, 0] });
    send("a", "KeyA");
    expect(view.zoomToBounds).toHaveBeenLastCalledWith(part);
    expect(view.zoomTo).not.toHaveBeenCalled();
  });

  it("go back to the part shortcuts once repair mode is closed", async () => {
    const send = keyboard();
    useRepair.setState({ session: null });
    send("Delete", "Delete");
    expect(s().parts).toHaveLength(0);
  });
});
