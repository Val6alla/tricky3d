import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// Cancelling is the session's work; the dialog only asks for it.
const session = vi.hoisted(() => ({ cancelRun: vi.fn() }));
vi.mock("../session", () => session);

import type { RepairBusy } from "../types";
import {
  RunProgressDialog,
  RunProgressView,
  onProgressKeyDown,
  progressDetail,
  progressPercent,
  progressStep,
} from "./RunProgressDialog";

const BUSY: RepairBusy = {
  job: 7,
  label: "Extended repair",
  progress: 0.424,
  step: "Wrap part surface (9 of 11)",
  detail: null,
  local: null,
  cancelling: false,
};

const render = (busy: RepairBusy) => renderToStaticMarkup(<RunProgressView busy={busy} onCancel={() => {}} />);
const cancel = (html: string) => html.match(/<button[^>]*>Cancel<\/button>/)?.[0] ?? "";
const bar = (html: string) => html.match(/<div class="rp-progress[^"]*"[^>]*>/)?.[0] ?? "";

describe("progressPercent", () => {
  it("is a whole percentage clamped to 0..100, or null while unknown", () => {
    expect(progressPercent(null)).toBeNull();
    expect(progressPercent(Number.NaN)).toBeNull();
    expect(progressPercent(Infinity)).toBeNull();
    expect(progressPercent(0)).toBe(0);
    expect(progressPercent(0.424)).toBe(42);
    expect(progressPercent(0.995)).toBe(100);
    expect(progressPercent(1.2)).toBe(100);
    expect(progressPercent(-0.5)).toBe(0);
  });
});

describe("progressStep", () => {
  it("shows the action running, what the run does before the first report, and the cancel", () => {
    expect(progressStep(BUSY)).toBe("Wrap part surface (9 of 11)");
    expect(progressStep({ ...BUSY, step: null })).toBe("Working…");
    expect(progressStep({ ...BUSY, cancelling: true })).toBe("Cancelling…");
  });
});

describe("progressDetail", () => {
  it("shows the step inside the action, and nothing once the run is cancelled", () => {
    expect(progressDetail(BUSY)).toBeNull();
    expect(progressDetail({ ...BUSY, detail: "Finding intersections" })).toBe("Finding intersections");
    expect(progressDetail({ ...BUSY, detail: "Finding intersections", local: 0.404 })).toBe("Finding intersections · 40%");
    expect(progressDetail({ ...BUSY, detail: "Finding intersections", local: 0.4, cancelling: true })).toBeNull();
    expect(progressDetail({ ...BUSY, local: 0.4 })).toBeNull();
  });
});

describe("RunProgressView", () => {
  it("shows the run's name, the action running, the percentage and a progress bar", () => {
    const html = render(BUSY);
    expect(html.startsWith('<div class="dlg-backdrop"><div class="dlg dlg-rp-run" role="dialog" aria-modal="true"')).toBe(true);
    expect(html).toMatch(/<h2 id="([^"]+)" class="dlg-title">Extended repair<\/h2>/);
    expect(html).toMatch(/<span id="[^"]+" class="rp-run-step">Wrap part surface \(9 of 11\)<\/span><span class="rp-run-percent">42%<\/span>/);
    const titleId = html.match(/<h2 id="([^"]+)"/)?.[1];
    expect(bar(html)).toBe(
      `<div class="rp-progress" role="progressbar" aria-labelledby="${titleId}" aria-valuemin="0" aria-valuemax="100" aria-valuenow="42">`,
    );
    expect(html).toContain('<div class="rp-progress-fill" style="width:42%"></div>');
    // The step line describes the dialog.
    const stepId = html.match(/<span id="([^"]+)" class="rp-run-step"/)?.[1];
    expect(html).toContain(`aria-describedby="${stepId}"`);
  });

  it("shows the step inside the action under its line, when the action names one", () => {
    expect(render(BUSY)).not.toContain("rp-run-detail");
    const html = render({ ...BUSY, detail: "Mending what rounding broke" });
    expect(html).toContain('</div><div class="rp-run-detail">Mending what rounding broke</div><div class="rp-progress"');
  });

  it("has Cancel as its only button, and no Close, OK or primary button", () => {
    const html = render(BUSY);
    expect(html.match(/<button/g)).toHaveLength(1);
    expect(cancel(html)).toBe('<button type="button" class="dlg-btn">Cancel</button>');
    expect(html).not.toContain("dlg-btn-primary");
  });

  it("sweeps an indeterminate bar until the first progress report", () => {
    const html = render({ ...BUSY, progress: null, step: null });
    expect(bar(html)).toContain('class="rp-progress is-indeterminate"');
    expect(bar(html)).not.toContain("aria-valuenow");
    expect(html).not.toContain("rp-run-percent");
    expect(html).toContain('<div class="rp-progress-fill"></div>');
    expect(html).toContain(">Working…</span>");
  });

  it("disables Cancel and says so while the run stops", () => {
    const html = render({ ...BUSY, cancelling: true });
    expect(cancel(html)).toContain('disabled=""');
    expect(html).toContain(">Cancelling…</span>");
  });
});

describe("onProgressKeyDown", () => {
  const press = (key: string) => {
    const e = { key, preventDefault: vi.fn(), stopPropagation: vi.fn() };
    onProgressKeyDown(e);
    return e;
  };

  it("ignores Esc, so the dialog stays open until the run ends", () => {
    const e = press("Escape");
    expect(e.preventDefault).toHaveBeenCalled();
    expect(e.stopPropagation).toHaveBeenCalled();
  });

  it("keeps Tab on Cancel, the only control", () => {
    expect(press("Tab").preventDefault).toHaveBeenCalled();
  });

  it("lets Enter and Space press Cancel, and keeps every key from the app's shortcuts", () => {
    for (const key of ["Enter", " ", "z", "Delete", "f"]) {
      const e = press(key);
      expect(e.preventDefault).not.toHaveBeenCalled();
      expect(e.stopPropagation).toHaveBeenCalled();
    }
  });
});

describe("RunProgressDialog", () => {
  it("draws nothing when no run is going", () => {
    // Server rendering reads the repair store's initial state: busy is null.
    expect(renderToStaticMarkup(<RunProgressDialog onClose={() => {}} />)).toBe("");
  });
});
