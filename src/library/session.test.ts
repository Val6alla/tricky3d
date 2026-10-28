import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LibraryPrimitive } from "./ipc";

// Rust is a stand-in that builds a small box for any primitive; the thumbnails are a stand-in too (no WebGL here).
const ipc = vi.hoisted(() => ({ fetchLibraryCatalog: vi.fn(), buildPrimitive: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("./thumbnails", () => ({ thumbnail: () => "data:image/png;base64,", releaseThumbnails: () => {}, THUMB_WIDTH: 1, THUMB_HEIGHT: 1 }));
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { makeBox } from "../mesh";
import { cachedStats } from "../meshCache";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import { closePartLibrary, createPart, defaultName, fieldProblem, openPartLibrary, parseValues, pickPrimitive, setName, setValue, cancelCreate } from "./session";
import { useLibrary } from "./store";

const CATALOG: LibraryPrimitive[] = [
  {
    id: "box",
    label: "Box",
    params: [
      { key: "length", label: "Length (X)", unit: "mm", default: 20, min: 0.001, max: 10000, integer: false },
      { key: "width", label: "Width (Y)", unit: "mm", default: 20, min: 0.001, max: 10000, integer: false },
    ],
  },
  {
    id: "sphere",
    label: "Sphere",
    params: [
      { key: "radius", label: "Radius", unit: "mm", default: 10, min: 0.001, max: 10000, integer: false },
      { key: "segments", label: "Segments", unit: "", default: 64, min: 3, max: 2048, integer: true },
    ],
  },
];

const built = () => {
  const mesh = makeBox([4, 4, 4]);
  return { mesh, stats: { triangles: 12, vertices: 8, volume: 64, area: 96, shells: 1, openEdges: 0, nonManifoldEdges: 0 }, edges: new Float32Array(0) };
};
const s = () => useStore.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("isTauri", true);
  ipc.fetchLibraryCatalog.mockResolvedValue(CATALOG);
  ipc.buildPrimitive.mockImplementation(async () => built());
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  useLibrary.setState({ catalog: null, session: null, busy: null, thumbs: {} });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "home" });
});

afterEach(() => {
  closePartLibrary();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("the Part library form", () => {
  it("checks every field against the catalog's range", () => {
    const [box, sphere] = CATALOG;
    expect(fieldProblem(box.params[0], "12,5")).toBeNull();
    expect(fieldProblem(box.params[0], "abc")).toBe("Enter a number.");
    expect(fieldProblem(box.params[0], "0")).toBe("Enter 0.001 mm to 10000 mm.");
    expect(fieldProblem(sphere.params[1], "7.5")).toBe("Enter a whole number.");
    expect(parseValues(sphere, { radius: "5", segments: "2" })).toEqual({ params: { radius: 5 }, errors: { segments: "Enter 3 to 2048." } });
    // A missing field takes its default.
    expect(parseValues(box, { length: "30" }).params).toEqual({ length: 30, width: 20 });
  });

  it("names a part after its primitive, numbered when the name is taken", () => {
    expect(defaultName("Box", [])).toBe("Box");
    expect(defaultName("Box", ["Box", "Box 2", "Sphere"])).toBe("Box 3");
  });
});

describe("the Part library mode", () => {
  it("opens on the first primitive, follows the picked tile, and keeps a typed name", async () => {
    s().addParts([{ name: "Box", mesh: makeBox([1, 1, 1]) }]);
    await openPartLibrary();
    expect(useUi.getState().ribbonTab).toBe("library");
    expect(useLibrary.getState().session).toMatchObject({ category: "primitive", primitive: "box", name: "Box 2", values: { length: "20", width: "20" } });
    pickPrimitive("sphere");
    expect(useLibrary.getState().session).toMatchObject({ primitive: "sphere", name: "Sphere", values: { radius: "10", segments: "64" } });
    setName("Ball");
    pickPrimitive("box");
    expect(useLibrary.getState().session?.name).toBe("Ball");
    closePartLibrary();
    expect(useLibrary.getState().session).toBeNull();
    expect(useUi.getState().ribbonTab).toBe("home");
  });

  it("creates the part Rust builds on the middle of the platform as one undo step, and closes", async () => {
    await openPartLibrary();
    pickPrimitive("sphere");
    setValue("radius", "12,5");
    const steps = s().history.past.length;
    await createPart();
    expect(ipc.buildPrimitive).toHaveBeenLastCalledWith("sphere", { radius: 12.5, segments: 64 }, expect.any(Number));
    expect(s().parts).toHaveLength(1);
    const part = s().parts[0];
    expect(part.name).toBe("Sphere");
    expect(part.matrix.slice(12, 15)).toEqual([100, 100, 0]);
    expect(cachedStats(part.mesh)?.volume).toBe(64);
    expect(s().history.past).toHaveLength(steps + 1);
    expect(s().history.past[s().history.past.length - 1]?.label).toBe("Create Sphere");
    expect(s().selection).toEqual([part.id]);
    expect(useLibrary.getState().session).toBeNull();
    s().undo();
    expect(s().parts).toHaveLength(0);
  });

  it("refuses a wrong field, shows Rust's refusal, and adds nothing when cancelled", async () => {
    await openPartLibrary();
    setValue("length", "-3");
    await createPart();
    expect(lastToast()).toBe("Length (X): Enter 0.001 mm to 10000 mm.");
    setValue("length", "3");
    ipc.buildPrimitive.mockRejectedValueOnce("The turns would touch.");
    await createPart();
    expect(lastToast()).toBe("The turns would touch.");
    expect(useLibrary.getState().session).not.toBeNull();

    let finish!: (v: ReturnType<typeof built>) => void;
    ipc.buildPrimitive.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    const run = createPart();
    await vi.waitFor(() => expect(useLibrary.getState().busy).not.toBeNull());
    cancelCreate();
    expect(repairIpc.cancelRepair).toHaveBeenCalledWith(useLibrary.getState().busy?.job);
    finish(built());
    await run;
    expect(s().parts).toHaveLength(0);
    expect(useLibrary.getState().busy).toBeNull();
    // Rust's CANCELLED is no error to show.
    ipc.buildPrimitive.mockRejectedValueOnce(CANCELLED);
    useUi.setState({ toasts: [] });
    await createPart();
    expect(lastToast()).toBeUndefined();
  });
});
