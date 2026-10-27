import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { SplitManyDialog } from "./SplitManyDialog";

const render = (partName: string | null) =>
  renderToStaticMarkup(<SplitManyDialog count={51234} partName={partName} resolve={() => {}} onClose={() => {}} />);
const buttons = (html: string) => [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((m) => [m[0].includes("dlg-btn-primary"), m[1]]);

describe("SplitManyDialog", () => {
  it("names the part and the count, with Repair first as the default and Split anyway beside it", () => {
    const html = render("Gearbox");
    expect(html).toContain("Gearbox</strong> would split into 51,234 parts.");
    expect(buttons(html)).toEqual([
      [false, "Split anyway"],
      [true, "Repair first"],
    ]);
  });

  it("speaks of the selected parts when several would split", () => {
    expect(render(null)).toContain("The selected parts would split into 51,234 parts.");
  });
});
