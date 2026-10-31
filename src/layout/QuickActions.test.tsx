import { renderToStaticMarkup } from "react-dom/server";
import { Box } from "lucide-react";
import { describe, expect, it } from "vitest";
import { FEATURES, soonMessage, type Feature } from "../features";
import { QuickActions, quickFeatures } from "./QuickActions";

const feature = (id: string, over: Partial<Feature> = {}): Feature => ({
  id,
  tab: "quick",
  group: "Quick",
  label: id,
  icon: Box,
  status: "soon",
  ...over,
});

describe("quickFeatures", () => {
  const list = [
    feature("quick.a", { status: "ready", run: () => {} }),
    feature("quick.b"),
    feature("home.c", { tab: "home" }),
    feature("quick.child", { parent: "quick.a" }),
    feature("quick.d"),
  ];

  it("takes the quick tab's top-level entries in order", () => {
    expect(quickFeatures(list, false).map((f) => f.id)).toEqual(["quick.a", "quick.b", "quick.d"]);
  });

  it("drops the Soon ones while those are hidden", () => {
    expect(quickFeatures(list, true).map((f) => f.id)).toEqual(["quick.a"]);
  });

  it("finds the five quick-action buttons in the registry, Repair and Collision detection ready and the others Soon", () => {
    const quick = quickFeatures(FEATURES, false);
    expect(quick.map((f) => f.id)).toEqual(["quick.repair", "quick.model-package", "quick.slice", "quick.file-preview", "quick.collisions"]);
    expect(quick.map((f) => f.status)).toEqual(["ready", "soon", "soon", "soon", "ready"]);
  });
});

describe("QuickActions", () => {
  it("draws an icon button per entry, with the label for screen readers and the usual tooltip; Soon ones greyed", () => {
    const html = renderToStaticMarkup(<QuickActions />);
    expect(html).toMatch(/^<div class="quick-actions" role="group" aria-label="Quick actions">/);
    const buttons = [...html.matchAll(/<button type="button" class="([^"]+)" aria-label="([^"]+)" title="([^"]+)">/g)];
    const quick = quickFeatures(FEATURES, false);
    expect(buttons.map((b) => b[2])).toEqual(quick.map((f) => f.label));
    expect(buttons.map((b) => b[1])).toEqual(quick.map((f) => (f.status === "soon" ? "quick-btn is-soon" : "quick-btn")));
    for (const [i, f] of quick.entries()) {
      if (f.status === "soon") expect(buttons[i][3]).toBe(soonMessage(f));
      else expect(buttons[i][3]).toContain(f.label);
    }
    // Icons only: no visible text.
    expect(html.replace(/<[^>]+>/g, "")).toBe("");
  });
});
