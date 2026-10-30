import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnalyzedMesh } from "./io/ipc";
import type { LibraryItem } from "./library/ipc";

// Rust's job table (src-tauri/src/jobs.rs RepairJobs) as a stand-in: a cancel for a job that is not running is kept
// only when its number is above every job started so far. The Part library and Generate shell talk to it the way
// they talk to Rust; everything else runs as in the app.
const rust = vi.hoisted(() => {
  const running = new Map<number, { cancelled: boolean }>();
  const early = new Set<number>();
  let highest: number | null = null;
  return {
    started: [] as number[],
    /** Jobs whose cancel had arrived before they started: Rust stops them at once. */
    stoppedAtStart: [] as number[],
    reset() {
      running.clear();
      early.clear();
      highest = null;
      this.started = [];
      this.stoppedAtStart = [];
    },
    start(job: number) {
      highest = highest === null ? job : Math.max(highest, job);
      const flag = { cancelled: early.delete(job) };
      for (const j of early) if (j <= job) early.delete(j);
      running.set(job, flag);
      this.started.push(job);
      if (flag.cancelled) this.stoppedAtStart.push(job);
      return flag;
    },
    finish(job: number) {
      running.delete(job);
    },
    cancel(job: number) {
      const flag = running.get(job);
      if (flag) flag.cancelled = true;
      else if (highest === null || job > highest) early.add(job);
    },
  };
});

const libraryIpc = vi.hoisted(() => ({ fetchLibraryCatalog: vi.fn(), buildItem: vi.fn(), pickPicture: vi.fn() }));
const shellIpc = vi.hoisted(() => ({ generateShells: vi.fn(), estimateShells: vi.fn() }));
vi.mock("./library/ipc", () => libraryIpc);
vi.mock("./library/thumbnails", () => ({ thumbnail: () => "data:image/png;base64,", releaseThumbnails: () => {}, THUMB_WIDTH: 1, THUMB_HEIGHT: 1 }));
vi.mock("./shell/ipc", () => shellIpc);
vi.mock("./repair/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./repair/ipc")>()),
  cancelRepair: async (job: number) => rust.cancel(job),
}));

import { nextJob } from "./jobs";
import { cancelCreate, closePartLibrary, createPart, openPartLibrary } from "./library/session";
import { makeBox, translation } from "./mesh";
import { CANCELLED, cancelRepair } from "./repair/ipc";
import { runGenerateShell } from "./shell/session";
import { useStore } from "./store";
import { useUi } from "./ui";

const BOX: LibraryItem = {
  id: "box",
  label: "Box",
  category: "primitive",
  description: "A block.",
  soon: null,
  input: "none",
  params: [],
};

function measured(size: number): AnalyzedMesh {
  return {
    mesh: makeBox([size, size, size]),
    stats: { triangles: 12, vertices: 8, volume: size ** 3, area: 6 * size * size, shells: 1, openEdges: 0, nonManifoldEdges: 0 },
    edges: new Float32Array(0),
  };
}

const s = () => useStore.getState();

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {}, confirm: () => true });
  vi.stubGlobal("isTauri", true);
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: () => {} });
  rust.reset();
  libraryIpc.fetchLibraryCatalog.mockResolvedValue([BOX]);
  // Each build reaches Rust a moment after it is sent, and runs there as a job of its own.
  libraryIpc.buildItem.mockImplementation(async (_id: string, _params: unknown, job: number) => {
    await Promise.resolve();
    const flag = rust.start(job);
    rust.finish(job);
    if (flag.cancelled) throw CANCELLED;
    return { ...measured(4), world: false };
  });
  shellIpc.generateShells.mockImplementation(async (_parts: unknown, _settings: unknown, job: number) => {
    const flag = rust.start(job);
    rust.finish(job);
    if (flag.cancelled) throw CANCELLED;
    return { kind: "made", results: [{ result: measured(18), warnings: [] }] };
  });
  s().newProject();
  useUi.setState({ toasts: [], dialog: null, tasks: [], ribbonTab: "home" });
});

afterEach(() => {
  closePartLibrary();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("job numbers", () => {
  it("rise across every feature", () => {
    const a = nextJob();
    const b = nextJob();
    expect(b).toBeGreaterThan(a);
  });

  it("a Generate shell cancelled in its first moment after a Part library build adds nothing", async () => {
    await openPartLibrary();
    await createPart();
    expect(s().parts.map((p) => p.name)).toEqual(["Box"]);
    const library = Math.max(...rust.started);

    // As the dialog does: a new job number, then Cancel before the run has reached Rust.
    s().select([s().parts[0].id]);
    const job = nextJob();
    await cancelRepair(job);
    // `stopped` stays false: only Rust knows of the cancel, as when the dialog is closed while it runs.
    const outcome = await runGenerateShell({ kind: "hollow-part", thickness: 2, accuracy: 0.5, smooth: false }, false, job, undefined, () => false);

    expect(job).toBeGreaterThan(library);
    expect(rust.stoppedAtStart).toEqual([job]);
    expect(outcome).toEqual({ kind: "cancelled" });
    expect(s().parts.map((p) => p.name)).toEqual(["Box"]);
    expect(s().history.past.map((e) => e.label)).toEqual(["Create Box"]);
  });

  it("a Part library build cancelled in its first moment after a Generate shell stops in Rust at once", async () => {
    s().addParts([{ name: "Cube", mesh: makeBox([10, 10, 10]), matrix: translation(50, 50, 5) }]);
    s().select([s().parts[0].id]);
    const shell = nextJob();
    await runGenerateShell({ kind: "inner-offset", thickness: 1, accuracy: 0.25, smooth: false }, false, shell);
    expect(s().parts.map((p) => p.name)).toEqual(["Cube", "Cube (inner offset)"]);

    await openPartLibrary();
    const thumbnails = rust.started.length;
    const creating = createPart();
    cancelCreate();
    await creating;

    const build = rust.started[thumbnails];
    expect(build).toBeGreaterThan(shell);
    expect(rust.stoppedAtStart).toEqual([build]);
    expect(s().parts.map((p) => p.name)).toEqual(["Cube", "Cube (inner offset)"]);
  });
});
