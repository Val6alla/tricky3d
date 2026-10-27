import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ActionSpec } from "../types";
import {
  ALL_BUTTONS,
  ActionsTab,
  CLOSE_HOLES,
  FIX_FLIPPED,
  MORE_ACTIONS,
  SELF_INTERSECTIONS,
  buttonState,
  intersectionNote,
  selectionNote,
} from "./ActionsTab";

const spec = (over: Partial<ActionSpec> = {}): ActionSpec => ({
  id: "close_all_holes",
  label: "Close all holes",
  implemented: true,
  inScripts: true,
  needsSelection: false,
  hint: "Fill every hole.",
  params: [],
  ...over,
});

describe("the buttons", () => {
  it("are the designed ones, in order and with their labels", () => {
    expect(CLOSE_HOLES.map((b) => b.label)).toEqual(["Trivial", "Stitch triangles", "All", "Defined"]);
    expect(FIX_FLIPPED.label).toBe("Fix flipped triangles");
    expect(SELF_INTERSECTIONS.map((b) => b.label)).toEqual(["Detect", "Split off", "Remove"]);
    expect(MORE_ACTIONS.map((b) => b.label)).toEqual([
      "Remove double triangles",
      "Remove degenerate faces",
      "Wrap part surface",
      "Remove problem areas",
      "Split non-oriented edges",
    ]);
  });

  it("run the actions their labels name", () => {
    expect(ALL_BUTTONS.map((b) => b.id)).toEqual([
      "close_trivial_holes",
      "stitch_triangles",
      "close_all_holes",
      "close_defined_holes",
      "fix_flipped_triangles",
      "detect_self_intersections",
      "split_off_self_intersections",
      "remove_self_intersections",
      "remove_double_triangles",
      "remove_degenerate_faces",
      "wrap_part_surface",
      "remove_problem_areas",
      "split_non_oriented_edges",
    ]);
  });
});

describe("buttonState", () => {
  it("runs with the action's hint as the tooltip", () => {
    expect(buttonState(spec(), false, 0)).toEqual({ disabled: false, title: "Fill every hole." });
    expect(buttonState(spec(), false, 12)).toEqual({ disabled: false, title: "Fill every hole." });
  });

  it("is off until the catalog is there, and for Soon actions", () => {
    expect(buttonState(undefined, false, 0)).toEqual({ disabled: true, title: "The repair actions are not loaded." });
    expect(buttonState(spec({ implemented: false, label: "Remesh" }), false, 0)).toEqual({ disabled: true, title: "Remesh is coming in a future version." });
  });

  it("needs selected triangles for Defined", () => {
    const defined = spec({ id: "close_defined_holes", label: "Close defined holes", needsSelection: true, hint: "Fill the holes that border the selected triangles." });
    expect(buttonState(defined, false, 0)).toEqual({ disabled: true, title: "Fill the holes that border the selected triangles. Select triangles first." });
    expect(buttonState(defined, false, 1).disabled).toBe(false);
  });

  it("is off while a run is going", () => {
    expect(buttonState(spec(), true, 0)).toEqual({ disabled: true, title: "Fill every hole. Wait for the current repair to finish." });
  });
});

describe("notes", () => {
  it("say what the actions work on", () => {
    expect(selectionNote(0)).toBe("No triangles selected: actions work on the whole part.");
    expect(selectionNote(1)).toBe("1 triangle selected: actions work on the selection.");
    expect(selectionNote(1204)).toBe("1,204 triangles selected: actions work on the selection.");
  });

  it("say what Detect found, once it ran", () => {
    expect(intersectionNote(null)).toBeNull();
    expect(intersectionNote(new Uint32Array(0))).toBe("Detect found no self-intersections.");
    expect(intersectionNote(Uint32Array.of(4))).toBe("Detect found 1 self-intersecting triangle.");
    expect(intersectionNote(new Uint32Array(1200))).toBe("Detect found 1,200 self-intersecting triangles.");
  });
});

describe("ActionsTab", () => {
  // Server rendering reads the repair store's initial state: no session, no catalog.
  const html = renderToStaticMarkup(<ActionsTab missing="Loading the repair actions…" />);

  it("shows every group with its counts, ? while there is no status", () => {
    const titles = [...html.matchAll(/<h3 class="rp-title">(.*?)<\/h3>/g)].map((m) => m[1]);
    expect(titles).toEqual(["Close holes", "Flipped triangles", "Self-intersections", "More actions"]);
    const counts = [...html.matchAll(/<dt>(.*?)<\/dt><dd>(.*?)<\/dd>/g)].map((m) => [m[1], m[2]]);
    expect(counts).toEqual([
      ["Border edges", "?"],
      ["Holes", "?"],
      ["Inv. orientation", "?"],
    ]);
    expect(html).toContain("Loading the repair actions…");
    expect(html).toContain("No triangles selected: actions work on the whole part.");
  });

  it("keeps every button off until the catalog is loaded", () => {
    const buttons = [...html.matchAll(/<button ([^>]*)>(.*?)<\/button>/g)];
    expect(buttons.map((b) => b[2])).toEqual(ALL_BUTTONS.map((b) => b.label));
    for (const [, attrs] of buttons) {
      expect(attrs).toContain('disabled=""');
      expect(attrs).toContain('title="The repair actions are not loaded."');
    }
  });
});
