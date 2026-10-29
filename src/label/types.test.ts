import { describe, expect, it } from "vitest";
import { captionFace, labelFrame, onPlane, plateSize } from "./frame";
import { BUNDLED_FONT, listEntries, newLabel, runningNumbers, shapeText, startDepth, toSpec, type ListSource } from "./types";

const close = (got: number[], want: number[]) => got.forEach((g, i) => expect(g).toBeCloseTo(want[i], 9));

describe("list labels", () => {
  it("count from the start in steps, padded with zeros, between the prefix and the suffix", () => {
    expect(runningNumbers({ kind: "numbers", start: 1, step: 1, count: 5, prefix: "SN-", suffix: "", digits: 3 })).toEqual([
      "SN-001",
      "SN-002",
      "SN-003",
      "SN-004",
      "SN-005",
    ]);
    expect(runningNumbers({ kind: "numbers", start: 2, step: 2, count: 3, prefix: "", suffix: " ASTM", digits: 0 })).toEqual(["2 ASTM", "4 ASTM", "6 ASTM"]);
    expect(runningNumbers({ kind: "numbers", start: 1, step: -1, count: 3, prefix: "", suffix: "", digits: 2 })).toEqual(["01", "00", "-01"]);
    expect(runningNumbers({ kind: "numbers", start: 1, step: 1, count: 0, prefix: "", suffix: "", digits: 1 })).toEqual([]);
  });

  it("take a CSV column, skipping the header row and empty cells", () => {
    const csv: ListSource = { kind: "csv", file: "copies.csv", rows: [["Key", "Text"], ["a", "Main copy"], ["b", ""], ["c", "Lab copy"]], column: 1, header: true };
    expect(listEntries(csv)).toEqual(["Main copy", "Lab copy"]);
    expect(listEntries({ ...csv, header: false })).toEqual(["Text", "Main copy", "Lab copy"]);
    expect(listEntries({ ...csv, column: 3 })).toEqual([]);
  });

  it("draw their first entry, and send every entry to Rust", () => {
    const label = { ...newLabel("l1", "list", "Box", [0, 0, 10], [0, 0, 1]), list: { ...(newLabel("x", "list", "", [0, 0, 0], [0, 0, 1]).list as Extract<ListSource, { kind: "numbers" }>), count: 2 } };
    expect(shapeText(label)).toBe("001");
    expect(toSpec(label, 12).entries).toEqual(["001", "002"]);
    expect(toSpec({ ...label, kind: "text" }, 12).entries).toEqual([]);
  });
});

describe("new labels", () => {
  it("start fresh, at most 10 mm, Keep aspect ratio on, waiting to be fitted where they are placed", () => {
    for (const kind of ["text", "image", "list", "tag"] as const) {
      expect(newLabel("a", kind, "Bracket", [1, 2, 3], [0, 0, 1])).toMatchObject({
        caption: "Bracket",
        height: 10,
        // A tag's caption: 30 % of its 2 mm plate.
        depth: kind === "tag" ? 0.6 : 5,
        keepAspect: true,
        build: "add",
        font: BUNDLED_FONT,
        angle: 0,
        fitting: true,
        sizeAuto: true,
        depthAuto: true,
        partThickness: null,
      });
    }
  });

  it("start the depth at the smaller of 5 mm and half the height; sunk, at most 40 % of what they sink into", () => {
    const label = newLabel("a", "text", "", [0, 0, 0], [0, 0, 1]);
    expect(startDepth({ ...label, height: 10 })).toBe(5);
    expect(startDepth({ ...label, height: 3 })).toBe(1.5);
    expect(startDepth({ ...label, height: 10, build: "subtract", partThickness: 4 })).toBe(1.6);
    expect(startDepth({ ...label, height: 10, build: "subtract-mirrored", partThickness: 40 })).toBe(5);
    // Thickness unknown (an open part): only the first rule.
    expect(startDepth({ ...label, height: 10, build: "subtract", partThickness: null })).toBe(5);
    // Raised, the part's thickness does not matter.
    expect(startDepth({ ...label, height: 10, partThickness: 1 })).toBe(5);
  });

  it("start a tag's caption at 30 % of its plate's thickness, raised or sunk, whatever its height", () => {
    const tag = newLabel("t", "tag", "", [0, 0, 0], [0, 0, 1]);
    for (const build of ["add", "add-mirrored", "subtract", "subtract-mirrored"] as const) {
      expect(startDepth({ ...tag, build, height: 10, thickness: 2 })).toBe(0.6);
      expect(startDepth({ ...tag, build, height: 1, thickness: 5 })).toBe(1.5);
    }
    expect(startDepth({ ...tag, thickness: 3, partThickness: 0.5 })).toBe(0.9);
  });
});

describe("the label frame (the same as src-tauri/src/label/emboss.rs)", () => {
  it("runs the text along X with Y up on a level surface, and upright on a wall", () => {
    const top = labelFrame([0, 0, 10], [0, 0, 1], 0)!;
    close([...top.u, ...top.v, ...top.w], [1, 0, 0, 0, 1, 0, 0, 0, 1]);
    const front = labelFrame([0, 0, 0], [0, -1, 0], 0)!;
    close([...front.u, ...front.v], [1, 0, 0, 0, 0, 1]);
    const right = labelFrame([0, 0, 0], [1, 0, 0], 0)!;
    close([...right.u, ...right.v], [0, 1, 0, 0, 0, 1]);
    const underneath = labelFrame([0, 0, 0], [0, 0, -1], 0)!;
    close([...underneath.u, ...underneath.v], [-1, 0, 0, 0, 1, 0]);
  });

  it("turns anticlockwise seen from outside", () => {
    const turned = labelFrame([0, 0, 0], [0, 0, 1], 90)!;
    close([...turned.u, ...turned.v], [0, 1, 0, -1, 0, 0]);
  });

  it("finds where a ray meets its plane", () => {
    const f = labelFrame([5, 5, 10], [0, 0, 1], 0)!;
    close(onPlane(f, [7, 4, 50], [0, 0, -1])!, [2, -1]);
    expect(onPlane(f, [0, 0, 20], [1, 0, 0])).toBeNull();
  });

  it("puts a tag's caption on its front when it stands up, and on top when it sticks out of a wall", () => {
    const standing = captionFace({ origin: [0, 0, 0], u: [1, 0, 0], v: [0, 1, 0], w: [0, 0, 1] }, 2, 10);
    close([...standing.w, ...standing.u, ...standing.v, ...standing.origin], [0, -1, 0, 1, 0, 0, 0, 0, 1, 0, -1, 6]);
    const shelf = captionFace({ origin: [0, 0, 0], u: [1, 0, 0], v: [0, 0, 1], w: [0, -1, 0] }, 2, 10);
    close([...shelf.w, ...shelf.v, ...shelf.u, ...shelf.origin], [0, 0, 1, 0, 1, 0, 1, 0, 0, 0, -6, 1]);
  });

  it("sizes a tag's plate round its caption", () => {
    expect(plateSize(20, 5)).toEqual([22, 7]);
    expect(plateSize(20, 10)).toEqual([24, 14]);
  });
});
