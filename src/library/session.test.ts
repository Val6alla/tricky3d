import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LibraryItem } from "./ipc";

// Rust is a stand-in that builds a small box for any item; the thumbnails are a stand-in too (no WebGL here).
const ipc = vi.hoisted(() => ({ fetchLibraryCatalog: vi.fn(), buildItem: vi.fn(), pickPicture: vi.fn() }));
const repairIpc = vi.hoisted(() => ({ cancelRepair: vi.fn() }));
vi.mock("./ipc", () => ipc);
vi.mock("./thumbnails", () => ({ thumbnail: () => "data:image/png;base64,", releaseThumbnails: () => {}, THUMB_WIDTH: 1, THUMB_HEIGHT: 1 }));
vi.mock("../repair/ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("../repair/ipc")>()), ...repairIpc }));

import { IDENTITY, makeBox, translation } from "../mesh";
import { cachedStats } from "../meshCache";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import {
  cancelCreate,
  closePartLibrary,
  createPart,
  defaultName,
  fieldProblem,
  openPartLibrary,
  parseValues,
  pickCategory,
  pickPrimitive,
  setName,
  setValue,
  shows,
  valueProblem,
} from "./session";
import { CATEGORIES, useLibrary } from "./store";

const mm = (key: string, label: string, def: number) =>
  ({ key, label, help: "", when: null, kind: "number", unit: "mm", default: def, min: 0.001, max: 10000, integer: false }) as const;

const CATALOG: LibraryItem[] = [
  {
    id: "box",
    label: "Box",
    category: "primitive",
    description: "A block.",
    soon: null,
    input: "none",
    params: [mm("length", "Length (X)", 20), mm("width", "Width (Y)", 20)],
  },
  {
    id: "sphere",
    label: "Sphere",
    category: "primitive",
    description: "A ball.",
    soon: null,
    input: "none",
    params: [
      mm("radius", "Radius", 10),
      { key: "segments", label: "Segments", help: "", when: null, kind: "number", unit: "", default: 64, min: 3, max: 2048, integer: true },
    ],
  },
  {
    id: "qr_code",
    label: "QR Code",
    category: "design",
    description: "A code.",
    soon: null,
    input: "none",
    params: [
      {
        key: "code",
        label: "Code type",
        help: "",
        when: null,
        kind: "choice",
        default: "qr",
        options: [
          { value: "qr", label: "QR code", soon: false },
          { value: "aztec", label: "Aztec code", soon: true },
        ],
      },
      { key: "data", label: "Text", help: "", when: null, kind: "text", default: "hello", maxLength: 5 },
      { key: "keep", label: "Keep aspect ratio", help: "", when: null, kind: "toggle", default: true },
      { ...mm("depth", "Depth (Y)", 40), when: "!keep" },
    ],
  },
  { id: "text", label: "Text", category: "design", description: "Letters.", soon: "it needs labels.", input: "none", params: [] },
  { id: "convex_hull", label: "Convex Hull", category: "mathematical", description: "A hull.", soon: null, input: "parts", params: [] },
  { id: "tensile", label: "Tensile Test Part", category: "quality", description: "A bar.", soon: null, input: "none", params: [] },
];

const built = (world = false) => {
  const mesh = makeBox([4, 4, 4]);
  return { mesh, stats: { triangles: 12, vertices: 8, volume: 64, area: 96, shells: 1, openEdges: 0, nonManifoldEdges: 0 }, edges: new Float32Array(0), world };
};
const s = () => useStore.getState();
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("isTauri", true);
  ipc.fetchLibraryCatalog.mockResolvedValue(CATALOG);
  ipc.buildItem.mockImplementation(async () => built());
  repairIpc.cancelRepair.mockResolvedValue(undefined);
  useLibrary.setState({ catalog: null, session: null, busy: null, picture: null, thumbs: {} });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "home" });
});

afterEach(() => {
  closePartLibrary();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("the Part library form", () => {
  it("checks every field against the catalog", () => {
    const [box, sphere, qr] = CATALOG;
    const length = box.params[0];
    if (length.kind !== "number") throw new Error("number expected");
    expect(fieldProblem(length, "12,5")).toBeNull();
    expect(fieldProblem(length, "abc")).toBe("Enter a number.");
    expect(fieldProblem(length, "0")).toBe("Enter 0.001 mm to 10000 mm.");
    expect(parseValues(sphere, { radius: "5", segments: "2" })).toEqual({ params: { radius: 5 }, errors: { segments: "Enter 3 to 2048." } });
    // A missing field takes its default.
    expect(parseValues(box, { length: "30" }).params).toEqual({ length: 30, width: 20 });
    // Texts have a length limit, choices may not be Soon, toggles pass as they are.
    expect(valueProblem(qr.params[1], "far too long")).toBe("Enter at most 5 characters.");
    expect(valueProblem(qr.params[0], "aztec")).toBe("Aztec code is not available yet.");
    expect(parseValues(qr, { keep: false, depth: "12" }).params).toEqual({ code: "qr", data: "hello", keep: false, depth: 12 });
  });

  it("shows a field only while the toggle it depends on says so; hidden fields keep their defaults", () => {
    const qr = CATALOG[2];
    const depth = qr.params[3];
    expect(shows(depth, {}, qr)).toBe(false);
    expect(shows(depth, { keep: false }, qr)).toBe(true);
    expect(parseValues(qr, { depth: "oops" }).errors).toEqual({});
    expect(parseValues(qr, { keep: false, depth: "oops" }).errors).toEqual({ depth: "Enter a number." });
  });

  it("names a part after its item, numbered when the name is taken", () => {
    expect(defaultName("Box", [])).toBe("Box");
    expect(defaultName("Box", ["Box", "Box 2", "Sphere"])).toBe("Box 3");
  });

  it("has the four categories in the reference's order", () => {
    expect(CATEGORIES.map((c) => c.label)).toEqual(["Primitive", "Design", "Mathematical", "Quality"]);
  });
});

describe("the Part library mode", () => {
  it("opens on the first primitive, follows the picked tile and category, and keeps a typed name", async () => {
    s().addParts([{ name: "Box", mesh: makeBox([1, 1, 1]) }]);
    await openPartLibrary();
    expect(useUi.getState().ribbonTab).toBe("library");
    expect(useLibrary.getState().session).toMatchObject({ category: "primitive", primitive: "box", name: "Box 2", values: { length: "20", width: "20" } });
    pickPrimitive("sphere");
    expect(useLibrary.getState().session).toMatchObject({ primitive: "sphere", name: "Sphere", values: { radius: "10", segments: "64" } });
    pickCategory("design");
    expect(useLibrary.getState().session).toMatchObject({ category: "design", primitive: "qr_code", values: { code: "qr", data: "hello", keep: true } });
    // A Soon tile says why and stays unpicked.
    pickPrimitive("text");
    expect(useLibrary.getState().session?.primitive).toBe("qr_code");
    expect(lastToast()).toBe("Text is not available yet: it needs labels.");
    setName("Ball");
    pickCategory("primitive");
    expect(useLibrary.getState().session?.name).toBe("Ball");
    closePartLibrary();
    expect(useLibrary.getState().session).toBeNull();
    expect(useUi.getState().ribbonTab).toBe("home");
  });

  it("draws the tiles and the category pictures from Rust's previews", async () => {
    await openPartLibrary();
    await vi.waitFor(() => expect(useLibrary.getState().thumbs.tensile).toBeDefined());
    const previews = ipc.buildItem.mock.calls.filter((c) => c[3]?.preview === true).map((c) => c[0]);
    expect(previews).toEqual(expect.arrayContaining(["box", "sphere", "tensile"]));
  });

  it("creates the part Rust builds on the middle of the platform as one undo step, and closes", async () => {
    await openPartLibrary();
    pickPrimitive("sphere");
    setValue("radius", "12,5");
    const steps = s().history.past.length;
    await createPart();
    // The build for the part itself, not for a tile picture.
    const create = ipc.buildItem.mock.calls.filter((c) => c[3]?.onProgress);
    expect(create).toHaveLength(1);
    expect(create[0].slice(0, 2)).toEqual(["sphere", { radius: 12.5, segments: 64 }]);
    expect(create[0][3].parts).toEqual([]);
    expect(s().parts).toHaveLength(1);
    const part = s().parts[0];
    expect(part.name).toBe("Sphere");
    expect(part.matrix).toEqual(translation(100, 100, 0));
    expect(cachedStats(part.mesh)?.volume).toBe(64);
    expect(s().history.past).toHaveLength(steps + 1);
    expect(s().history.past[s().history.past.length - 1]?.label).toBe("Create Sphere");
    expect(s().selection).toEqual([part.id]);
    expect(useLibrary.getState().session).toBeNull();
    s().undo();
    expect(s().parts).toHaveLength(0);
  });

  it("makes a hull from the selected part, where it is, keeping its source file", async () => {
    const [id] = s().addParts([{ name: "Bracket", mesh: makeBox([2, 2, 2]), matrix: translation(50, 60, 1), source: "D:/in/bracket.stl" }]);
    await openPartLibrary();
    pickCategory("mathematical");
    s().clearSelection();
    await createPart();
    expect(lastToast()).toMatch(/^Select the part to make the convex hull from first/);
    s().select([id]);
    ipc.buildItem.mockImplementationOnce(async () => built(true));
    await createPart();
    const call = ipc.buildItem.mock.calls.filter((c) => c[3]?.onProgress).pop()!;
    expect(call[0]).toBe("convex_hull");
    expect(call[3].parts).toEqual([{ name: "Bracket", mesh: s().parts[0].mesh, matrix: translation(50, 60, 1) }]);
    const hull = s().parts[1];
    expect(hull.matrix).toEqual([...IDENTITY]);
    expect(hull.source).toBe("D:/in/bracket.stl");
  });

  it("refuses a wrong field, shows Rust's refusal, and adds nothing when cancelled", async () => {
    await openPartLibrary();
    setValue("length", "-3");
    await createPart();
    expect(lastToast()).toBe("Length (X): Enter 0.001 mm to 10000 mm.");
    setValue("length", "3");
    ipc.buildItem.mockRejectedValueOnce("The turns would touch.");
    await createPart();
    expect(lastToast()).toBe("The turns would touch.");
    expect(useLibrary.getState().session).not.toBeNull();

    let finish!: (v: ReturnType<typeof built>) => void;
    ipc.buildItem.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    const run = createPart();
    await vi.waitFor(() => expect(useLibrary.getState().busy).not.toBeNull());
    // Rust's progress shows in the panel.
    const options = ipc.buildItem.mock.calls.filter((c) => c[3]?.onProgress).pop()![3];
    options.onProgress(0.4);
    expect(useLibrary.getState().busy?.fraction).toBe(0.4);
    cancelCreate();
    expect(repairIpc.cancelRepair).toHaveBeenCalledWith(useLibrary.getState().busy?.job);
    finish(built());
    await run;
    expect(s().parts).toHaveLength(0);
    expect(useLibrary.getState().busy).toBeNull();
    // Rust's CANCELLED is no error to show.
    ipc.buildItem.mockRejectedValueOnce(CANCELLED);
    useUi.setState({ toasts: [] });
    await createPart();
    expect(lastToast()).toBeUndefined();
  });
});
