import { useEffect } from "react";
import { useRepair } from "../repair/store";
import { useUi, type ContextMenuState } from "../ui";
import { contextMenuEntries, repairContextMenuEntries } from "./entries";
import { Menu } from "./Menu";

/**
 * True when a right-click menu is the repair one: in repair mode, opened on the working copy. The viewport passes
 * `triangle` there (the triangle under the cursor, or null); a menu opened elsewhere, such as on a part in the tree,
 * has none and stays the part menu.
 */
export function isRepairMenu(menu: ContextMenuState, repairing: boolean): boolean {
  return repairing && menu.triangle !== undefined;
}

/**
 * Closes the right-click menu when repair mode starts or ends, and whenever a run or an undo replaces the working
 * copy: what the entries act on, and the repair menu's triangle under the cursor (an index into the mesh it opened
 * on), would no longer be what the menu was opened for. Returns the function that stops it.
 */
export function closeMenuOnRepairChange(): () => void {
  return useRepair.subscribe((r, prev) => {
    if (r.session?.mesh !== prev.session?.mesh) useUi.getState().closeContextMenu();
  });
}

/** The right-click menu, part or repair (isRepairMenu). The viewport and parts tree open it with openContextMenu. */
export function ContextMenu() {
  const menu = useUi((s) => s.contextMenu);
  const repairing = useRepair((r) => r.session !== null);
  useEffect(closeMenuOnRepairChange, []);
  if (!menu) return null;
  const repairMenu = isRepairMenu(menu, repairing);
  return (
    <Menu
      // A new right-click elsewhere starts a fresh menu at the new spot.
      key={`${menu.x},${menu.y},${repairMenu}`}
      entries={repairMenu ? repairContextMenuEntries() : contextMenuEntries()}
      anchor={{ x: menu.x, y: menu.y }}
      label={repairMenu ? "Part repair" : "Part"}
      onClose={() => useUi.getState().closeContextMenu()}
    />
  );
}
