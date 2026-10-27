import { afterEach, describe, expect, it } from "vitest";
import { boundsOf, IDENTITY, makeBox, translation, unionBounds } from "../mesh";
import { useStore, type MeshData, type NewPart, type Vec3 } from "../store";
import { useUi, type ObjectsAnswer, type PlaceAnswer } from "../ui";
import {
  askPlacement,
  besidePath,
  countParts,
  createSaveQueue,
  discardChoice,
  errorMessage,
  exportBaseName,
  exportTarget,
  extensionOf,
  fileColor,
  fileName,
  fileStem,
  importFile,
  importFormatOf,
  placedMatrices,
  placementOffset,
  placementRule,
  preTranslate,
  projectPart,
  projectPlatform,
  rememberedPlacement,
  sanitizeFileName,
  unsupportedMessage,
  withExtension,
  type ImportEnv,
} from "./files";
import type { ImportResult, LoadedMesh } from "./ipc";

describe("paths", () => {
  it("splits Windows and POSIX paths", () => {
    expect(fileName("C:\\Users\\me\\Parts\\Bracket v2.STL")).toBe("Bracket v2.STL");
    expect(fileName("/home/me/part.obj")).toBe("part.obj");
    expect(fileName("part.ply")).toBe("part.ply");
    expect(extensionOf("C:\\a.b\\Bracket v2.STL")).toBe("stl");
    expect(extensionOf("/x/archive.tar.gz")).toBe("gz");
    expect(extensionOf("/x.y/noext")).toBe("");
    expect(extensionOf("/x/.hidden")).toBe("");
    expect(fileStem("C:\\p\\box.3mf")).toBe("box");
    expect(fileStem("/p/model.v2.stl")).toBe("model.v2");
    expect(fileStem("/p/noext")).toBe("noext");
    expect(besidePath("C:\\proj\\box.3mf", "bracket.stl")).toBe("C:\\proj\\bracket.stl");
    expect(besidePath("/home/me/box.3mf", "a.obj")).toBe("/home/me/a.obj");
    expect(besidePath(null, "a.obj")).toBe("a.obj");
  });

  it("knows the import formats by extension, in any case", () => {
    expect(importFormatOf("C:\\x\\PART.STL")).toBe("stl");
    expect(importFormatOf("/x/a.obj")).toBe("obj");
    expect(importFormatOf("/x/a.Ply")).toBe("ply");
    expect(importFormatOf("/x/a.3mf")).toBe("3mf");
    expect(importFormatOf("/x/a.amf")).toBe("amf");
    expect(importFormatOf("/x/a.OFF")).toBe("off");
    expect(importFormatOf("/x/a.step")).toBeNull();
    expect(importFormatOf("/x/stl")).toBeNull();
  });

  it("adds the export extension only when it is missing", () => {
    expect(withExtension("C:\\out\\box", "stl")).toBe("C:\\out\\box.stl");
    expect(withExtension("C:\\out\\box.STL", "stl")).toBe("C:\\out\\box.STL");
    expect(withExtension("/out/box.obj", "stl")).toBe("/out/box.obj.stl");
    expect(withExtension("/out.d/box", "3mf")).toBe("/out.d/box.3mf");
  });

  it("picks the Home › Export format from the extension; .stl is binary", () => {
    expect(exportTarget("/o/box.stl")).toEqual({ path: "/o/box.stl", format: "stl-binary" });
    expect(exportTarget("/o/box.STL")).toEqual({ path: "/o/box.STL", format: "stl-binary" });
    expect(exportTarget("/o/box.obj")).toEqual({ path: "/o/box.obj", format: "obj" });
    expect(exportTarget("/o/box.ply")).toEqual({ path: "/o/box.ply", format: "ply" });
    expect(exportTarget("/o/box.3mf")).toEqual({ path: "/o/box.3mf", format: "3mf" });
    expect(exportTarget("/o/box.amf")).toEqual({ path: "/o/box.amf", format: "amf" });
    expect(exportTarget("/o/box")).toEqual({ path: "/o/box.stl", format: "stl-binary" });
    expect(exportTarget("/o/box.txt")).toEqual({ path: "/o/box.txt.stl", format: "stl-binary" });
  });

  it("sanitises file names for every platform", () => {
    expect(sanitizeFileName('a<b>c:d"e/f\\g|h?i*j')).toBe("a_b_c_d_e_f_g_h_i_j");
    expect(sanitizeFileName("tab\there\u0001")).toBe("tab_here_");
    expect(sanitizeFileName("  Bracket (copy). . ")).toBe("Bracket (copy)");
    expect(sanitizeFileName("CON")).toBe("CON_");
    expect(sanitizeFileName("lpt1")).toBe("lpt1_");
    expect(sanitizeFileName("console")).toBe("console");
    expect(sanitizeFileName("...")).toBe("Untitled");
    expect(sanitizeFileName("", "part")).toBe("part");
    expect(sanitizeFileName("x".repeat(300))).toHaveLength(200);
  });

  it("names up to three unsupported files and points at the formats help", () => {
    expect(unsupportedMessage(["C:\\a\\notes.txt"])).toBe(
      `Can't import "notes.txt". tricky3D reads STL, OBJ, PLY, 3MF, AMF and OFF files; see File › Supported formats.`,
    );
    expect(unsupportedMessage(["/a.step", "/b.iges", "/c.x3d", "/d.wrl", "/e"])).toMatch(/^Can't import "a.step", "b.iges", "c.x3d" and 2 more\. /);
  });

  it("names an export after the one part, or after the project", () => {
    expect(exportBaseName([{ name: "Bracket: left" }], "Untitled")).toBe("Bracket_ left");
    expect(exportBaseName([{ name: "a" }, { name: "b" }], "Desk set")).toBe("Desk set");
    expect(countParts(1)).toBe("1 part");
    expect(countParts(2)).toBe("2 parts");
  });

  it("shows Rust's error sentences as they are and wraps JS errors", () => {
    const rust = 'Could not read "part.stl": the file ends early (expected 1,200 triangles, found 1,003).';
    expect(errorMessage(rust, "Could not import")).toBe(rust);
    expect(errorMessage(new Error("Mesh data is truncated."), 'Could not import "a.stl"')).toBe('Could not import "a.stl": Mesh data is truncated.');
    expect(errorMessage(undefined, "Could not save")).toBe("Could not save: undefined");
  });
});

/** Rotation by 90° about Z, then a translation, column-major. */
const rotZ90 = (tx: number, ty: number, tz: number) => [0, 1, 0, 0, -1, 0, 0, 0, 0, 0, 1, 0, tx, ty, tz, 1];

describe("placement", () => {
  const platform: Vec3 = [200, 150, 100];

  it("rests a box on Z = 0 with its centre over the platform centre", () => {
    expect(placementOffset({ min: [10, 20, -5], max: [30, 60, 15] }, platform)).toEqual([80, 35, 5]);
  });

  it("pre-multiplies the translation and keeps the matrix's own rotation", () => {
    const m = rotZ90(1, 2, 3);
    const out = preTranslate(m, [10, 20, 30]);
    expect(out).toEqual([0, 1, 0, 0, -1, 0, 0, 0, 0, 0, 1, 0, 11, 22, 33, 1]);
    expect(m[12]).toBe(1); // input untouched
  });

  it("moves all of a file's meshes by one translation, from their combined box", () => {
    const meshes = [
      { mesh: makeBox([20, 20, 20]), matrix: null },
      { mesh: makeBox([10, 40, 4]), matrix: rotZ90(100, 0, 50) }, // 40 × 10 × 4 in the world after the rotation
    ];
    const placed = placedMatrices(meshes, platform)!;
    const worldBox = unionBounds(meshes.map((m, i) => boundsOf(m.mesh, placed[i])))!;
    expect(worldBox.min[2]).toBeCloseTo(0, 9);
    expect((worldBox.min[0] + worldBox.max[0]) / 2).toBeCloseTo(100, 9);
    expect((worldBox.min[1] + worldBox.max[1]) / 2).toBeCloseTo(75, 9);
    // Same offset for both: their relative position is kept.
    const d0 = placed[0][12] - IDENTITY[12];
    const d1 = placed[1][12] - 100;
    expect(d0).toBeCloseTo(d1, 9);
    expect(placed[1].slice(0, 12)).toEqual(rotZ90(0, 0, 0).slice(0, 12));
  });

  it("has nothing to place for empty meshes", () => {
    const empty: MeshData = { positions: new Float32Array(), indices: new Uint32Array() };
    expect(placedMatrices([{ mesh: empty, matrix: null }], platform)).toBeNull();
  });

  it("maps the setting and a remembered answer", () => {
    expect(placementRule("ask")).toBe("ask");
    expect(placementRule("always")).toBe(true);
    expect(placementRule("never")).toBe(false);
    expect(rememberedPlacement({ place: true, remember: true })).toBe("always");
    expect(rememberedPlacement({ place: false, remember: true })).toBe("never");
    expect(rememberedPlacement({ place: true, remember: false })).toBeNull();
  });
});

describe("project fields", () => {
  it("keeps a usable platform from the file and falls back otherwise", () => {
    const current: Vec3 = [200, 200, 200];
    expect(projectPlatform([250, 210, 220], current)).toEqual([250, 210, 220]);
    expect(projectPlatform(null, current)).toBe(current);
    expect(projectPlatform([0, 100, 100], current)).toBe(current);
    expect(projectPlatform([NaN, 100, 100], current)).toBe(current);
  });

  it("accepts only plain hex colours from a file", () => {
    expect(fileColor("#a9B1bc")).toBe("#a9B1bc");
    expect(fileColor("red")).toBeUndefined();
    expect(fileColor("#abc")).toBeUndefined();
    expect(fileColor("#a9b1bc;background:url(x)")).toBeUndefined();
    expect(fileColor(undefined)).toBeUndefined();
  });

  it("turns a project mesh into a part with its display fields", () => {
    const mesh = makeBox([1, 1, 1]);
    const loaded: LoadedMesh = { name: "Lid", matrix: translation(1, 2, 3), mesh, stats: null, edges: null, color: "#bda3a3", visible: false, transparent: true, locked: true };
    expect(projectPart(loaded)).toEqual({ name: "Lid", mesh, matrix: translation(1, 2, 3), color: "#bda3a3", visible: false, transparent: true, locked: true });
    expect(projectPart({ ...loaded, matrix: null, color: undefined }).matrix).toBeUndefined();
  });

  it("reads the unsaved-changes answer from the pressed button's label", () => {
    expect(discardChoice("Save")).toBe("save");
    expect(discardChoice("Don't save")).toBe("discard");
    expect(discardChoice("Cancel")).toBe("cancel");
    expect(discardChoice("Yes")).toBe("save");
    expect(discardChoice("No")).toBe("discard");
    expect(discardChoice("")).toBe("cancel");
  });
});

// ---------------------------------------------------------------------------------------------

function loaded(name: string, mesh: MeshData, matrix: number[] | null = null): LoadedMesh {
  return { name, matrix, mesh, stats: null, edges: null };
}

function result(meshes: LoadedMesh[], extra: Partial<ImportResult> = {}): ImportResult {
  return { format: "stl", objectCount: meshes.length, splittable: false, warnings: [], meshes, ...extra };
}

interface FakeEnv extends ImportEnv {
  reads: { path: string; merge: boolean }[];
  asked: string[];
  added: { parts: NewPart[]; label: string }[];
  warnings: string[];
}

function fakeEnv(opts: {
  read: (merge: boolean) => ImportResult | Promise<ImportResult>;
  objects?: ObjectsAnswer | null;
  scale?: boolean | null;
  place?: boolean | null;
  /** Whether the project is still open, asked at each check. */
  current?: () => boolean;
}): FakeEnv {
  const env: FakeEnv = {
    reads: [],
    asked: [],
    added: [],
    warnings: [],
    read: async (path, merge) => {
      env.reads.push({ path, merge });
      return opts.read(merge);
    },
    askObjects: async (name, count) => {
      env.asked.push(`objects:${name}:${count}`);
      return opts.objects ?? null;
    },
    askScale: async (name, size) => {
      env.asked.push(`scale:${name}:${size}`);
      return opts.scale === undefined ? false : opts.scale;
    },
    askPlace: async (name) => {
      env.asked.push(`place:${name}`);
      return opts.place === undefined ? false : opts.place;
    },
    platform: () => [200, 200, 200],
    current: opts.current ?? (() => true),
    add: (parts, label) => env.added.push({ parts, label }),
    warn: (text) => env.warnings.push(text),
  };
  return env;
}

describe("importFile", () => {
  const box = makeBox([20, 20, 20]);

  it("places a single-mesh file and adds it as one step named after the file", async () => {
    const env = fakeEnv({ read: () => result([loaded("bracket", box)]), place: true });
    expect(await importFile("C:\\in\\bracket.stl", env)).toBe("added");
    expect(env.reads).toEqual([{ path: "C:\\in\\bracket.stl", merge: false }]);
    expect(env.asked).toEqual(["place:bracket.stl"]);
    expect(env.added).toHaveLength(1);
    expect(env.added[0].label).toBe("Import bracket.stl");
    const [part] = env.added[0].parts;
    expect(part.name).toBe("bracket");
    expect(part.mesh).toBe(box); // never copied
    expect(part.matrix).toEqual(translation(100, 100, 10));
  });

  it("keeps the file's matrices when not placing", async () => {
    const m = rotZ90(5, 6, 7);
    const env = fakeEnv({ read: () => result([loaded("a - one", box, m), loaded("a - two", box)], { format: "3mf" }), place: false });
    await importFile("/in/a.3mf", env);
    expect(env.added[0].parts.map((p) => p.matrix)).toEqual([m, undefined]);
  });

  it("asks once about a multi-object OBJ; One part reads it again merged", async () => {
    const split = result([loaded("a - x", box), loaded("a - y", box), loaded("a - z", box)], { format: "obj", splittable: true });
    const merged = result([loaded("a", box)], { format: "obj", objectCount: 3 });
    const env = fakeEnv({ read: (merge) => (merge ? merged : split), objects: "single", place: false });
    await importFile("/in/a.obj", env);
    expect(env.asked).toEqual(["objects:a.obj:3", "place:a.obj"]);
    expect(env.reads.map((r) => r.merge)).toEqual([false, true]);
    expect(env.added[0].parts.map((p) => p.name)).toEqual(["a"]);
  });

  it("One part per object keeps the first read", async () => {
    const split = result([loaded("a - x", box), loaded("a - y", box)], { format: "obj", splittable: true });
    const env = fakeEnv({ read: () => split, objects: "split", place: true });
    await importFile("/in/a.obj", env);
    expect(env.reads).toHaveLength(1);
    expect(env.added).toHaveLength(1);
    expect(env.added[0].parts.map((p) => p.name)).toEqual(["a - x", "a - y"]);
  });

  it("skips the file when a prompt is cancelled", async () => {
    const split = result([loaded("a - x", box), loaded("a - y", box)], { format: "obj", splittable: true });
    const cancelObjects = fakeEnv({ read: () => split, objects: null });
    expect(await importFile("/in/a.obj", cancelObjects)).toBe("skipped");
    expect(cancelObjects.asked).toEqual(["objects:a.obj:2"]);
    expect(cancelObjects.added).toEqual([]);

    const cancelPlace = fakeEnv({ read: () => result([loaded("b", box)]), place: null });
    expect(await importFile("/in/b.stl", cancelPlace)).toBe("skipped");
    expect(cancelPlace.added).toEqual([]);
  });

  it("passes on Rust's warnings after adding, and warns about an empty file", async () => {
    const env = fakeEnv({ read: () => result([loaded("c", box)], { warnings: ["Skipped 2 faces with fewer than 3 vertices."] }) });
    await importFile("/in/c.ply", env);
    expect(env.warnings).toEqual(["Skipped 2 faces with fewer than 3 vertices."]);

    const empty = fakeEnv({ read: () => result([]) });
    expect(await importFile("/in/empty.obj", empty)).toBe("skipped");
    expect(empty.asked).toEqual([]);
    expect(empty.warnings).toEqual(['"empty.obj" has no triangles to import.']);
  });

  it("offers to scale a file without units that is under 1 mm across by 1000, before placing it", async () => {
    const tiny = makeBox([0.04, 0.02, 0.01]);
    const env = fakeEnv({ read: () => result([loaded("m", tiny)]), scale: true, place: true });
    expect(await importFile("/in/m.stl", env)).toBe("added");
    expect(env.asked).toEqual([`scale:m.stl:${Math.fround(0.02) * 2}`, "place:m.stl"]);
    const [part] = env.added[0].parts;
    expect(part.mesh.positions).toEqual(tiny.positions.map((v) => v * 1000));
    expect(part.mesh.positions[0]).toBe(-20);
    expect(part.mesh.indices).toBe(tiny.indices);
    // Placed by its scaled size: resting on the platform, 10 mm high.
    expect(part.matrix).toEqual(translation(100, 100, 5));

    const keep = fakeEnv({ read: () => result([loaded("m", tiny)]), scale: false });
    await importFile("/in/m.stl", keep);
    expect(keep.added[0].parts[0].mesh).toBe(tiny);
    const cancel = fakeEnv({ read: () => result([loaded("m", tiny)]), scale: null });
    expect(await importFile("/in/m.stl", cancel)).toBe("skipped");
    expect(cancel.asked).toEqual([`scale:m.stl:${Math.fround(0.02) * 2}`]);

    // Not asked: a normal size, or a format with units.
    const normal = fakeEnv({ read: () => result([loaded("n", box)]) });
    await importFile("/in/n.stl", normal);
    expect(normal.asked).toEqual(["place:n.stl"]);
    const withUnits = fakeEnv({ read: () => result([loaded("u", tiny)], { format: "3mf" }) });
    await importFile("/in/u.3mf", withUnits);
    expect(withUnits.asked).toEqual(["place:u.3mf"]);
  });

  it("stops when its project is closed while the file is read, before any prompt", async () => {
    const env = fakeEnv({ read: () => result([loaded("e", box)]), place: true, current: () => false });
    expect(await importFile("C:\\in\\e.stl", env)).toBe("skipped");
    expect(env.asked).toEqual([]);
    expect(env.added).toEqual([]);
    expect(env.warnings).toEqual(['Stopped importing "e.stl" because its project was closed.']);
  });

  it("stops when its project is closed during the merged re-read or while waiting to be placed", async () => {
    const split = result([loaded("a - x", box), loaded("a - y", box)], { format: "obj", splittable: true });
    let open = true;
    const merging = fakeEnv({
      read: (merge) => {
        if (merge) open = false; // New project while the merged read runs
        return merge ? result([loaded("a", box)]) : split;
      },
      objects: "single",
      current: () => open,
    });
    expect(await importFile("/in/a.obj", merging)).toBe("skipped");
    expect(merging.asked).toEqual(["objects:a.obj:2"]);
    expect(merging.added).toEqual([]);

    let checks = 0;
    const waiting = fakeEnv({ read: () => result([loaded("b", box)]), place: true, current: () => ++checks < 2 });
    expect(await importFile("/in/b.stl", waiting)).toBe("skipped");
    expect(waiting.asked).toEqual(["place:b.stl"]);
    expect(waiting.added).toEqual([]);
    expect(waiting.warnings).toHaveLength(1);
  });

  it("rejects with the reader's error so the caller can show it", async () => {
    const env = fakeEnv({ read: () => Promise.reject('Could not read "d.stl": the file is empty.') });
    await expect(importFile("/in/d.stl", env)).rejects.toBe('Could not read "d.stl": the file is empty.');
    expect(env.added).toEqual([]);
  });
});

describe("store generation", () => {
  it("changes only for New project and Open project, not for edits, undo or saving", () => {
    const updates: string[] = [];
    let label = "";
    const unsubscribe = useStore.subscribe((next, prev) => {
      if (next.generation !== prev.generation) updates.push(label);
    });
    const s = () => useStore.getState();
    const act = (name: string, run: () => void) => {
      label = name;
      run();
    };
    act("add", () => s().addTestCube());
    act("move", () => s().setPartMatrices(new Map([[s().parts[0].id, translation(1, 2, 3)]]), "Move"));
    act("undo", () => s().undo());
    act("save", () => s().markSaved("C:\\p\\a.3mf", "a"));
    act("platform", () => s().setPlatform([180, 180, 180]));
    act("new", () => s().newProject());
    act("open", () => s().loadProject({ name: "b", path: "C:\\p\\b.3mf", platform: [200, 200, 200], parts: [{ name: "x", mesh: makeBox([1, 1, 1]) }] }));
    act("new again", () => s().newProject());
    unsubscribe();
    expect(updates).toEqual(["new", "open", "new again"]);
  });
});

describe("createSaveQueue", () => {
  /** A save the test finishes by hand. */
  function deferred() {
    let finish!: (ok: boolean) => void;
    let fail!: (err: unknown) => void;
    const promise = new Promise<boolean>((res, rej) => {
      finish = res;
      fail = rej;
    });
    return { promise, finish, fail };
  }

  it("runs saves one after another, each with its own result", async () => {
    const queue = createSaveQueue();
    const log: string[] = [];
    const first = deferred();
    const a = queue.add((behind) => {
      log.push(`a start behind=${behind}`);
      return first.promise;
    });
    const b = queue.add(async (behind) => {
      log.push(`b start behind=${behind}`);
      return false;
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(log).toEqual(["a start behind=false"]); // b waits for a
    expect(queue.busy()).toBe(true);
    first.finish(true);
    expect(await a).toBe(true);
    expect(await b).toBe(false);
    expect(log).toEqual(["a start behind=false", "b start behind=true"]);
    expect(queue.busy()).toBe(false);
    // Asked for when nothing runs: not behind another.
    expect(await queue.add(async (behind) => behind)).toBe(false);
  });

  it("a failing save does not block the next, and idle waits for all of them", async () => {
    const queue = createSaveQueue();
    const first = deferred();
    const a = queue.add(() => first.promise);
    const b = queue.add(async () => true);
    let idle = false;
    void queue.idle().then(() => (idle = true));
    first.fail(new Error("disk full"));
    await expect(a).rejects.toThrow("disk full");
    expect(await b).toBe(true);
    await queue.idle();
    expect(idle).toBe(true);
  });
});

describe("askPlacement", () => {
  afterEach(() => {
    useStore.getState().setImportPlacement("ask");
    useUi.setState({ dialog: null });
  });

  /** Waits until a place-on-import prompt is open and returns it. */
  async function openPrompt() {
    for (let i = 0; i < 50; i++) {
      const d = useUi.getState().dialog;
      if (d?.kind === "place-on-import") return d;
      await new Promise((r) => setTimeout(r, 1));
    }
    throw new Error("no prompt opened");
  }

  function answer(d: { resolve(a: PlaceAnswer | null): void }, a: PlaceAnswer | null) {
    d.resolve(a);
    useUi.getState().closeDialog();
  }

  it("applies the setting without asking", async () => {
    useStore.getState().setImportPlacement("always");
    expect(await askPlacement("a.stl")).toBe(true);
    useStore.getState().setImportPlacement("never");
    expect(await askPlacement("a.stl")).toBe(false);
    expect(useUi.getState().dialog).toBeNull();
  });

  it("a remembered answer also answers files already waiting", async () => {
    const first = askPlacement("a.stl");
    const second = askPlacement("b.stl");
    const prompt = await openPrompt();
    expect(prompt.fileName).toBe("a.stl");
    answer(prompt, { place: true, remember: true });
    expect(await first).toBe(true);
    expect(await second).toBe(true);
    expect(useStore.getState().importPlacement).toBe("always");
  });

  it("asks each file in turn and cancels with null", async () => {
    const first = askPlacement("a.stl");
    const second = askPlacement("b.stl");
    answer(await openPrompt(), null);
    expect(await first).toBeNull();
    const next = await openPrompt();
    expect(next.fileName).toBe("b.stl");
    answer(next, { place: false, remember: false });
    expect(await second).toBe(false);
    expect(useStore.getState().importPlacement).toBe("ask");
  });
});
