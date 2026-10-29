import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ExportOptions } from "../store";
import { ExportOptionsDialog } from "./ExportOptionsDialog";

const render = (count: number, options: ExportOptions) =>
  renderToStaticMarkup(<ExportOptionsDialog count={count} options={options} resolve={() => {}} onClose={() => {}} />);
/** Each radio button's label and whether it is checked. */
const choices = (html: string) =>
  [...html.matchAll(/(<input type="radio"[^>]*>)<span><span class="dlg-check-label">([^<]*)</g)].map((m) => [m[2], m[1].includes('checked=""')]);
const buttons = (html: string) => [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((m) => [m[0].includes("dlg-btn-primary"), m[1]]);

describe("ExportOptionsDialog", () => {
  it("asks only for the STL type when one part is exported, opened on the options used last", () => {
    const html = render(1, { stl: "ascii", parts: "per-part" });
    expect(html).toContain("Export 1 part");
    expect(html).toContain("STL type");
    expect(html).not.toContain("Several parts");
    expect(choices(html)).toEqual([
      ["Binary", false],
      ["ASCII", true],
    ]);
    expect(html).toContain("exactly where they sit on the platform");
    expect(buttons(html)).toEqual([
      [false, "Cancel"],
      [true, "Continue"],
    ]);
  });

  it("adds one file or one file per part for several parts, binary STL and one file by default", () => {
    const html = render(3, { stl: "binary", parts: "one-file" });
    expect(html).toContain("Export 3 parts");
    expect(choices(html)).toEqual([
      ["Binary", true],
      ["ASCII", false],
      ["One file with all parts", true],
      ["One file per part", false],
    ]);
    expect(choices(render(3, { stl: "binary", parts: "per-part" })).slice(2)).toEqual([
      ["One file with all parts", false],
      ["One file per part", true],
    ]);
  });
});
