import { describe, expect, it } from "vitest";
import { DEFAULT_TAB, REPAIR_TAB, TABS, ribbonTabs, shownTab } from "./features";
import { dialogSlotFree, prompt, useUi, type PlaceAnswer } from "./ui";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("ribbon tab", () => {
  it("starts on the default tab, and a tab change clears the hovered button's hint", () => {
    expect(useUi.getInitialState().ribbonTab).toBe(DEFAULT_TAB);
    useUi.getState().setHoverHint("Cut the selected parts in two along a plane.");
    useUi.getState().setRibbonTab("view");
    expect(useUi.getState().ribbonTab).toBe("view");
    expect(useUi.getState().hoverHint).toBeNull();
  });

  it("shows the contextual Repair tab only in repair mode, after the others", () => {
    expect(ribbonTabs(false)).toEqual(TABS);
    expect(ribbonTabs(false).map((t) => t.id)).toEqual(["home", "analyze", "modify", "arrange", "view"]);
    const repairing = ribbonTabs(true);
    expect(repairing.map((t) => t.id)).toEqual(["home", "analyze", "modify", "arrange", "view", "repair"]);
    expect(repairing[5]).toBe(REPAIR_TAB);
    expect([REPAIR_TAB.label, REPAIR_TAB.contextual]).toEqual(["Repair", true]);
    expect(TABS.some((t) => t.contextual)).toBe(false);
  });

  it("shows the selected tab while it is on show, and the default tab once the Repair tab is gone", () => {
    expect(shownTab("repair", ribbonTabs(true))).toBe("repair");
    expect(shownTab("arrange", ribbonTabs(true))).toBe("arrange");
    expect(shownTab("repair", ribbonTabs(false))).toBe(DEFAULT_TAB);
    // Ids that name no ribbon tab (the File menu, menus, the quick strip) never show as one.
    for (const id of ["file", "context", "repair-context", "quick"] as const) expect(shownTab(id, ribbonTabs(true))).toBe(DEFAULT_TAB);
  });
});

describe("prompt", () => {
  it("counts a prompt replaced by another dialog as cancelled, and the queue keeps going", async () => {
    const first = prompt<PlaceAnswer>((resolve) => ({ kind: "place-on-import", fileName: "a.stl", resolve }));
    await flush();
    expect(useUi.getState().dialog?.kind).toBe("place-on-import");

    // Settings opened from a menu that was still open over the prompt.
    useUi.getState().openDialog({ kind: "settings" });
    await expect(first).resolves.toBeNull();

    const second = prompt<PlaceAnswer>((resolve) => ({ kind: "place-on-import", fileName: "b.stl", resolve }));
    await flush();
    expect(useUi.getState().dialog?.kind).toBe("settings");
    useUi.getState().closeDialog();
    await flush();
    const dialog = useUi.getState().dialog;
    expect(dialog?.kind === "place-on-import" && dialog.fileName).toBe("b.stl");
    if (dialog?.kind === "place-on-import") dialog.resolve({ place: true, remember: false });
    useUi.getState().closeDialog();
    await expect(second).resolves.toEqual({ place: true, remember: false });
  });

  it("never replaces a dialog that took the freed slot first, and waits for that one to close too", async () => {
    useUi.getState().openDialog({ kind: "about" });
    // Something else waits for the slot as well (the repair progress dialog does) and gets it first.
    void dialogSlotFree().then(() => useUi.getState().openDialog({ kind: "formats" }));
    const answer = prompt<PlaceAnswer>((resolve) => ({ kind: "place-on-import", fileName: "c.stl", resolve }));
    await flush();
    useUi.getState().closeDialog();
    await flush();
    expect(useUi.getState().dialog?.kind).toBe("formats");
    useUi.getState().closeDialog();
    await flush();
    const dialog = useUi.getState().dialog;
    expect(dialog?.kind === "place-on-import" && dialog.fileName).toBe("c.stl");
    if (dialog?.kind === "place-on-import") dialog.resolve({ place: false, remember: true });
    useUi.getState().closeDialog();
    await expect(answer).resolves.toEqual({ place: false, remember: true });
  });
});
