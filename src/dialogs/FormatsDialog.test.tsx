import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { FormatsDialog } from "./FormatsDialog";

const render = () => renderToStaticMarkup(<FormatsDialog onClose={() => {}} />);

/** Text of every <li> inside the list labelled `label`, in order. */
function listItems(html: string, label: string): string[] {
  const list = html.match(new RegExp(`<ul[^>]*aria-label="${label}"[^>]*>(.*?)</ul>`))?.[1] ?? "";
  return [...list.matchAll(/<li[^>]*>(.*?)<\/li>/g)].map((m) => m[1]);
}

describe("FormatsDialog", () => {
  // Server rendering reads the store's initial state, where Soon items are shown.
  it("lists every supported, planned format and the units note", () => {
    const html = render();
    for (const name of ["STL", "OBJ", "PLY", "3MF", "AMF", "OFF", "VRML", "X3D", "3DS", "GTS", "STL (binary)", "STL (ASCII)", "PLY (binary)"]) {
      expect(html).toContain(`<span class="dlg-format-name">${name}</span>`);
    }
    // OFF, VRML, X3D, 3DS and GTS are read and written.
    for (const name of ["OFF", "VRML", "X3D", "3DS", "GTS"]) {
      expect(html.match(new RegExp(`<span class="dlg-format-name">${name}</span>`, "g"))).toHaveLength(2);
    }
    expect(listItems(html, "Import formats coming later")).toEqual(["Colour STL", "STEP", "IGES"]);
    expect(listItems(html, "Export formats coming later")).toEqual([
      "Colour STL",
      "BINVOX",
      "Compressed mesh",
      "Formlabs",
      "Slice",
    ]);
    expect(html.match(/Coming later/g)).toHaveLength(2);
    expect(html).toContain("STL, OBJ, PLY, OFF, VRML, 3DS and GTS files have no units, so tricky3D reads and writes them as millimetres;");
    expect(html).toContain("Every export writes each part where it sits on the platform.");
  });

  it("keeps the lists in a labelled, focusable scroll region so the keyboard can scroll them", () => {
    // Close is the only button; without a tab stop here a cut-off list could not be scrolled from the keyboard.
    const region = render().match(/<div class="dlg-formats-scroll"[^>]*>/)?.[0] ?? "";
    expect(region).toContain('role="region"');
    expect(region).toContain('aria-label="Format lists"');
    expect(region).toContain('tabindex="0"');
  });
});
