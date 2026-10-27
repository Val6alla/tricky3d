import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RepairCheck, RepairStatus } from "./types";

const ipc = vi.hoisted(() => ({ checkMeshes: vi.fn() }));
vi.mock("./ipc", async (importOriginal) => ({ ...(await importOriginal<typeof import("./ipc")>()), ...ipc }));

import { makeBox } from "../mesh";
import { useStore } from "../store";
import { badgeText, cachedCheck, checkFromStatus, primeCheck, startBadgeChecks, useBadges } from "./badges";

const good: RepairCheck = { closed: true, oriented: true, holes: 0, borderEdges: 0, nonManifoldEdges: 0, invertedTriangles: 0 };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("badge text", () => {
  it("is null for a closed, oriented mesh", () => {
    expect(badgeText(good)).toBeNull();
  });

  it("names holes, non-manifold edges and inverted triangles with thousands separators", () => {
    expect(badgeText({ ...good, closed: false, oriented: false, holes: 4, borderEdges: 40, invertedTriangles: 80437 })).toBe(
      "Needs repair: 4 holes, 80,437 inverted triangles",
    );
    expect(badgeText({ ...good, closed: false, nonManifoldEdges: 3 })).toBe("Needs repair: 3 non-manifold edges");
    expect(badgeText({ ...good, closed: false, holes: 1, borderEdges: 3, nonManifoldEdges: 1 })).toBe("Needs repair: 1 hole, 1 non-manifold edge");
    expect(badgeText({ ...good, oriented: false, invertedTriangles: 1 })).toBe("Needs repair: 1 inverted triangle");
  });

  it("names open edges only when there are border edges but no hole was counted", () => {
    expect(badgeText({ ...good, closed: false, borderEdges: 12 })).toBe("Needs repair: 12 open edges");
    expect(badgeText({ ...good, closed: false, borderEdges: 1 })).toBe("Needs repair: 1 open edge");
  });

  it("still says what is wrong when no count explains it", () => {
    expect(badgeText({ ...good, oriented: false })).toBe("Needs repair: the mesh is not oriented");
    expect(badgeText({ ...good, closed: false, oriented: false })).toBe("Needs repair: the mesh is not closed and not oriented");
  });

  it("comes from a repair status", () => {
    const status: RepairStatus = { ...good, edges: 1, triangles: 2, vertices: 3, shells: 1, degenerateTriangles: 5, volume: 1, area: 2, holes: 2, borderEdges: 7 };
    expect(checkFromStatus(status)).toEqual({ ...good, holes: 2, borderEdges: 7 });
  });
});

describe("cache", () => {
  it("keeps checks per mesh object and bumps the version", () => {
    const mesh = makeBox([1, 1, 1]);
    const version = useBadges.getState().version;
    expect(cachedCheck(mesh)).toBeUndefined();
    primeCheck(mesh, good);
    expect(cachedCheck(mesh)).toBe(good);
    expect(cachedCheck({ ...mesh })).toBeUndefined();
    expect(useBadges.getState().version).toBe(version + 1);
  });
});

describe("background checks", () => {
  let stop: () => void = () => {};

  beforeEach(() => {
    vi.stubGlobal("localStorage", undefined);
    ipc.checkMeshes.mockReset();
    useStore.getState().newProject();
  });

  afterEach(() => {
    stop();
    vi.unstubAllGlobals();
  });

  it("do nothing outside the desktop app", () => {
    vi.stubGlobal("isTauri", false);
    useStore.getState().addParts([{ name: "a", mesh: makeBox([1, 1, 1]) }]);
    stop = startBadgeChecks();
    expect(ipc.checkMeshes).not.toHaveBeenCalled();
  });

  it("check one unchecked mesh at a time, pick up new parts, and skip meshes already known or failed", async () => {
    vi.stubGlobal("isTauri", true);
    const known = makeBox([1, 1, 1]);
    primeCheck(known, good);
    const a = makeBox([2, 2, 2]);
    const b = makeBox([3, 3, 3]);
    useStore.getState().addParts([
      { name: "known", mesh: known },
      { name: "a", mesh: a },
    ]);
    const first = deferred<RepairCheck[]>();
    ipc.checkMeshes.mockReturnValueOnce(first.promise);
    stop = startBadgeChecks();
    expect(ipc.checkMeshes).toHaveBeenCalledTimes(1);
    expect(ipc.checkMeshes.mock.calls[0][0]).toEqual([a]);

    // A part added while a check runs waits for it: never two at once.
    ipc.checkMeshes.mockRejectedValueOnce("Could not check the part.");
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    useStore.getState().addParts([{ name: "b", mesh: b }]);
    expect(ipc.checkMeshes).toHaveBeenCalledTimes(1);
    const bad = { ...good, closed: false, holes: 1, borderEdges: 4 };
    first.resolve([bad]);
    await vi.waitFor(() => expect(ipc.checkMeshes).toHaveBeenCalledTimes(2));
    expect(ipc.checkMeshes.mock.calls[1][0]).toEqual([b]);
    expect(cachedCheck(a)).toEqual(bad);
    await vi.waitFor(() => expect(errors).toHaveBeenCalled());
    errors.mockRestore();
    expect(cachedCheck(b)).toBeUndefined();

    // b failed and is not tried again; a change of parts with nothing new sends nothing.
    useStore.getState().renamePart(useStore.getState().parts[0].id, "renamed");
    await Promise.resolve();
    expect(ipc.checkMeshes).toHaveBeenCalledTimes(2);

    // After stopping, new parts are left alone.
    stop();
    useStore.getState().addParts([{ name: "c", mesh: makeBox([4, 4, 4]) }]);
    await Promise.resolve();
    expect(ipc.checkMeshes).toHaveBeenCalledTimes(2);
  });
});
