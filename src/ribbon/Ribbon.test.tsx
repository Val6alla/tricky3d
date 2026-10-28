import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { featureById, ribbonTabs, type Feature } from "../features";
import { RibbonButton, SplitButton, TabList } from "./Ribbon";

// Server rendering reads each store's initial state: not repairing, no triangle selected, the Triangle select tool.

const feature = (id: string): Feature => {
  const f = featureById(id);
  if (!f) throw new Error(`no feature ${id}`);
  return f;
};

/** The opening tag of the first button whose aria-label is `label`. */
function buttonTag(html: string, label: string): string {
  return html.match(new RegExp(`<button[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? "";
}

describe("TabList", () => {
  it("shows the fixed tabs outside repair mode", () => {
    const html = renderToStaticMarkup(<TabList tabs={ribbonTabs(false)} tab="modify" />);
    expect([...html.matchAll(/role="tab"[^>]*>(?:<svg.*?<\/svg>)?([^<]+)</g)].map((m) => m[1])).toEqual(["Home", "Analyze", "Modify", "Arrange", "View"]);
    expect(html).not.toContain("is-contextual");
    expect(html).toMatch(/id="rb-tab-modify"[^>]*class="rb-tab is-selected" aria-selected="true"/);
  });

  it("adds the Repair tab last, after a separator, tinted with its icon, and selected in repair mode", () => {
    const html = renderToStaticMarkup(<TabList tabs={ribbonTabs(true)} tab="repair" />);
    expect(html).toMatch(/<span class="rb-tabbar-sep" aria-hidden="true"><\/span><button id="rb-tab-repair"/);
    const repair = html.match(/<button id="rb-tab-repair"[^>]*>(.*?)<\/button>/);
    expect(repair?.[0]).toContain('class="rb-tab is-contextual is-selected" aria-selected="true"');
    expect(repair?.[0]).toContain('tabindex="0"');
    expect(repair?.[1]).toMatch(/^<svg[^>]*class="lucide lucide-wrench rb-tab-icon"[^>]*>.*<\/svg>Repair$/);
    expect(html).toMatch(/id="rb-tab-modify"[^>]*class="rb-tab" aria-selected="false"[^>]*tabindex="-1"/);
  });

  it("keeps the Repair tab tinted when another tab is selected", () => {
    const html = renderToStaticMarkup(<TabList tabs={ribbonTabs(true)} tab="view" />);
    expect(html).toContain('id="rb-tab-repair" type="button" role="tab" class="rb-tab is-contextual" aria-selected="false"');
  });
});

describe("RibbonButton", () => {
  it("greys out an entry that needs selected triangles while none is selected, with the reason in its tooltip", () => {
    const tag = buttonTag(renderToStaticMarkup(<RibbonButton f={feature("repair.remove-triangles")} />), "Remove triangle");
    expect(tag).toContain('class="rb-btn rb-small is-disabled"');
    expect(tag).toContain('aria-disabled="true"');
    expect(tag).toContain('title="Remove triangle (Del) — Select triangles first."');
  });

  it("shows the select tool in use pressed, and the others not", () => {
    const triangle = buttonTag(renderToStaticMarkup(<RibbonButton f={feature("repair.select-triangle")} />), "Triangle");
    expect(triangle).toContain('class="rb-btn rb-small is-active"');
    expect(triangle).toContain('aria-pressed="true"');
    const shell = buttonTag(renderToStaticMarkup(<RibbonButton f={feature("repair.select-shell")} />), "Shell");
    expect(shell).toContain('class="rb-btn rb-small"');
    expect(shell).toContain('aria-pressed="false"');
    expect(shell).not.toContain("aria-disabled");
  });

  it("shows a Soon entry with its pill and not as blocked, and a main command outside repair mode as ready", () => {
    const soon = renderToStaticMarkup(<RibbonButton f={feature("repair.z-comp")} />);
    expect(buttonTag(soon, "Z-comp")).toContain('class="rb-btn rb-small is-soon"');
    expect(soon).toContain('<span class="soon-pill rb-pill">Soon</span>');
    const cut = buttonTag(renderToStaticMarkup(<RibbonButton f={feature("modify.cut")} />), "Cut");
    expect(cut).toContain('class="rb-btn rb-large"');
    expect(cut).not.toContain("aria-disabled");
    expect(cut).toContain('title="Cut — Cut the selected parts in two along a plane."');
  });

  it("opens Extract parts as a menu", () => {
    const tag = buttonTag(renderToStaticMarkup(<RibbonButton f={feature("repair.extract")} />), "Extract parts");
    expect(tag).toContain('aria-haspopup="menu"');
    expect(tag).not.toContain("is-disabled");
  });
});

describe("SplitButton", () => {
  it("draws Undo as a button that runs and an arrow that opens its list of steps", () => {
    const html = renderToStaticMarkup(<SplitButton f={feature("home.undo")} />);
    const main = buttonTag(html, "Undo");
    expect(main).toContain('class="rb-btn rb-small rb-split-main"');
    expect(main).not.toContain("aria-haspopup");
    const arrow = buttonTag(html, "Undo several steps");
    expect(arrow).toContain('aria-haspopup="menu"');
    expect(arrow).toContain('aria-expanded="false"');
  });
});
