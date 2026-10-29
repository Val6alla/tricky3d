import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnalyzedMesh } from "../io/ipc";
import { makeBox, translation } from "../mesh";
import { CANCELLED } from "../repair/ipc";
import { useStore } from "../store";
import { useUi } from "../ui";
import type { ShellReply } from "./ipc";
import { openGenerateShell, runGenerateShell, SHELL_LABEL } from "./session";
import { SETTINGS_KEY } from "./settings";
import type { ShellSettings } from "./types";

// Rust is replaced by a stand-in that answers with ready meshes; everything else runs as in the app.
const ipc = vi.hoisted(() => ({ generateShells: vi.fn() }));
vi.mock("./ipc", () => ipc);

const s = () => useStore.getState();
const names = () => s().parts.map((p) => p.name);
const labels = () => s().history.past.map((e) => e.label);
const lastToast = () => useUi.getState().toasts[useUi.getState().toasts.length - 1]?.message;

const HOLLOW: ShellSettings = { kind: "hollow-part", thickness: 2, accuracy: 0.5, smooth: false };

function made(size: number): AnalyzedMesh {
  const mesh = makeBox([size, size, size]);
  return {
    mesh,
    stats: { triangles: 12, vertices: 8, volume: size ** 3, area: 6 * size * size, shells: 1, openEdges: 0, nonManifoldEdges: 0 },
    edges: new Float32Array(),
  };
}

/** Three cubes, A, B and C; B and C selected. */
function threeParts() {
  const ids = s().addParts([
    { name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(0, 0, 5) },
    { name: "B", mesh: makeBox([20, 20, 20]), matrix: translation(30, 0, 10) },
    { name: "C", mesh: makeBox([20, 20, 20]), matrix: translation(60, 0, 10) },
  ]);
  s().select([ids[1], ids[2]]);
  return ids;
}

function fakeStorage() {
  const data = new Map<string, string>();
  return { data, getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => void data.set(key, value) };
}

beforeEach(() => {
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
  vi.stubGlobal("localStorage", fakeStorage());
  ipc.generateShells.mockReset();
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, tasks: [] });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Generate shell's run", () => {
  it("adds each result right after its part, as one undo step that undo and redo take back and bring back", async () => {
    const [a, b, c] = threeParts();
    const before = s().parts;
    const reply: ShellReply = {
      kind: "made",
      results: [
        { result: made(20), warnings: [] },
        { result: made(20), warnings: ["Walls thinner than 4 mm stay solid, with no cavity: about 3 % of the surface."] },
      ],
    };
    ipc.generateShells.mockResolvedValue(reply);
    const outcome = await runGenerateShell(HOLLOW, false, 7);
    expect(outcome).toEqual({ kind: "done", made: 2, warnings: ["C: Walls thinner than 4 mm stay solid, with no cavity: about 3 % of the surface."] });
    // Rust got the selected parts with their own meshes and matrices, the settings and the job.
    const [parts, settings, job] = ipc.generateShells.mock.calls[0];
    expect(parts.map((p: { name: string }) => p.name)).toEqual(["B", "C"]);
    expect(parts[0].matrix).toEqual(before[1].matrix);
    expect(settings).toEqual(HOLLOW);
    expect(job).toBe(7);
    expect(names()).toEqual(["A", "B", "B (hollow)", "C", "C (hollow)"]);
    expect(labels()).toEqual(["Add parts", SHELL_LABEL]);
    // The new parts sit exactly over the originals: world-space meshes with an identity matrix.
    const added = s().parts.filter((p) => p.name.endsWith("(hollow)"));
    expect(added.map((p) => p.matrix)).toEqual([translation(0, 0, 0), translation(0, 0, 0)]);
    expect(s().selection).toEqual(added.map((p) => p.id));
    // They stand out from their originals.
    expect(added[0].color).not.toBe(s().parts[1].color);
    s().undo();
    expect(s().parts).toBe(before);
    expect(s().parts.map((p) => p.id)).toEqual([a, b, c]);
    s().redo();
    expect(names()).toEqual(["A", "B", "B (hollow)", "C", "C (hollow)"]);
  });

  it("with Remove original part, puts each result in its part's place", async () => {
    const [a] = threeParts();
    ipc.generateShells.mockResolvedValue({ kind: "made", results: [{ result: made(16), warnings: [] }, { result: made(16), warnings: [] }] });
    await runGenerateShell({ ...HOLLOW, kind: "inner-offset" }, true, 8);
    expect(names()).toEqual(["A", "B (inner offset)", "C (inner offset)"]);
    expect(s().parts[0].id).toBe(a);
    expect(labels()).toEqual(["Add parts", SHELL_LABEL]);
    s().undo();
    expect(names()).toEqual(["A", "B", "C"]);
  });

  it("leaves a part that gave nothing as it is, even with Remove original part", async () => {
    threeParts();
    ipc.generateShells.mockResolvedValue({
      kind: "made",
      results: [
        { result: null, warnings: ["Nothing is left of the part: every wall is thinner than 4 mm, twice the shell thickness. No part was made."] },
        { result: made(16), warnings: [] },
      ],
    });
    const outcome = await runGenerateShell({ ...HOLLOW, kind: "inner-offset" }, true, 9);
    expect(outcome).toMatchObject({ kind: "done", made: 1 });
    expect(names()).toEqual(["A", "B", "C (inner offset)"]);
  });

  it("changes nothing when cancelled", async () => {
    threeParts();
    const before = s().parts;
    const history = s().history;
    ipc.generateShells.mockRejectedValue(CANCELLED);
    expect(await runGenerateShell(HOLLOW, true, 10)).toEqual({ kind: "cancelled" });
    expect(s().parts).toBe(before);
    expect(s().history).toBe(history);
  });

  it("changes nothing when cancelled too late for Rust to stop it", async () => {
    threeParts();
    const before = s().parts;
    ipc.generateShells.mockResolvedValue({ kind: "made", results: [{ result: made(16), warnings: [] }, { result: made(16), warnings: [] }] });
    expect(await runGenerateShell(HOLLOW, false, 17, undefined, () => true)).toEqual({ kind: "cancelled" });
    expect(s().parts).toBe(before);
  });

  it("changes nothing for a part that is not closed, and names it", async () => {
    const [, b] = threeParts();
    const before = s().parts;
    ipc.generateShells.mockResolvedValue({
      kind: "refused",
      refused: [{ source: 0, closed: false, oriented: true, holes: 2, borderEdges: 8, nonManifoldEdges: 0, invertedTriangles: 0 }],
    });
    expect(await runGenerateShell(HOLLOW, false, 11)).toEqual({ kind: "refused", parts: [{ id: b, text: "“B” is not closed (2 holes)." }] });
    expect(s().parts).toBe(before);
  });

  it("drops results when the parts changed while it ran", async () => {
    const [, b] = threeParts();
    ipc.generateShells.mockImplementation(async () => {
      s().renamePart(b, "B2");
      return { kind: "made", results: [{ result: made(16), warnings: [] }, { result: made(16), warnings: [] }] };
    });
    const outcome = await runGenerateShell(HOLLOW, false, 12);
    expect(outcome.kind).toBe("failed");
    expect(names()).toEqual(["A", "B2", "C"]);
  });

  it("reports Rust's sentence when it fails", async () => {
    threeParts();
    ipc.generateShells.mockRejectedValue("Generate shell made a result that has crossing triangles, so no part was added. Try a smaller accuracy value.");
    expect(await runGenerateShell(HOLLOW, false, 13)).toEqual({
      kind: "failed",
      message: "Generate shell made a result that has crossing triangles, so no part was added. Try a smaller accuracy value.",
    });
  });

  it("remembers the settings it ran with", async () => {
    threeParts();
    ipc.generateShells.mockResolvedValue({ kind: "made", results: [{ result: null, warnings: [] }, { result: null, warnings: [] }] });
    await runGenerateShell({ kind: "outer-offset", thickness: 1.2, accuracy: 0.3, smooth: true }, true, 14);
    expect(JSON.parse((localStorage.getItem(SETTINGS_KEY) as string) ?? "")).toEqual({ kind: "outer-offset", thickness: 1.2, accuracy: 0.3, smooth: true, removeOriginal: true });
  });

  it("runs one at a time", async () => {
    threeParts();
    let finish: (r: ShellReply) => void = () => {};
    ipc.generateShells.mockReturnValue(new Promise<ShellReply>((resolve) => (finish = resolve)));
    const first = runGenerateShell(HOLLOW, false, 15);
    expect(await runGenerateShell(HOLLOW, false, 16)).toEqual({ kind: "failed", message: "Wait for the current operation to finish." });
    finish({ kind: "made", results: [{ result: null, warnings: [] }, { result: null, warnings: [] }] });
    expect((await first).kind).toBe("done");
  });
});

describe("Generate shell's button", () => {
  it("opens the dialog for selected parts, and says why not otherwise", () => {
    openGenerateShell();
    expect(useUi.getState().dialog).toBeNull();
    expect(lastToast()).toBe("Select the parts to make a shell of first.");
    const [a] = threeParts();
    s().setPartLocked(a, true);
    s().select([a]);
    openGenerateShell();
    expect(lastToast()).toBe("Locked parts cannot be used for a shell. Unlock them first.");
    threeParts();
    openGenerateShell();
    expect(useUi.getState().dialog).toEqual({ kind: "generate-shell" });
  });
});
