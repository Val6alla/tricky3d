import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from "react";
import { createPortal } from "react-dom";
import { ChevronRight } from "lucide-react";
import { useBoolean } from "../boolean/store";
import { activate, blockedReason, effectiveStatus, featureById, hoverHintFor, isMenu, tooltipFor, type Feature } from "../features";
import { useMeasure } from "../measure/store";
import { useRepair } from "../repair/store";
import { useStore } from "../store";
import { useUi } from "../ui";
import { submenuEntries, tidy, type MenuEntry } from "./entries";
import { placeMenu, pointRect, rectOf, type Rect } from "./place";
import "./menus.css";

let openMenus = 0;

/** True while a dropdown or the context menu is open, so global shortcuts can stand aside. */
export function isMenuOpen(): boolean {
  return openMenus > 0;
}

/** Drop down below an element (ribbon), or open at a point (context menu). */
export type MenuAnchor = { el: HTMLElement } | { x: number; y: number };

interface MenuProps {
  entries: MenuEntry[];
  anchor: MenuAnchor;
  /** Accessible name of the menu. */
  label: string;
  onClose(): void;
}

const cx = (...names: (string | false)[]) => names.filter(Boolean).join(" ");

/**
 * A dropdown or context menu with nested submenus, rendered over everything and kept inside the
 * window. It closes on Esc, Tab, a click outside, scrolling, resizing, or after an item runs.
 */
export function Menu({ entries, anchor, label, onClose }: MenuProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const anchorEl = "el" in anchor ? anchor.el : null;
  const [at] = useState(() => ("el" in anchor ? rectOf(anchor.el) : pointRect(anchor.x, anchor.y)));
  const closeRef = useRef(onClose);
  useEffect(() => {
    closeRef.current = onClose;
  });

  useEffect(() => {
    openMenus++;
    const close = () => closeRef.current();
    // Clicks on the anchor button are left to its own toggle.
    const inside = (t: EventTarget | null) =>
      t instanceof Node && (rootRef.current?.contains(t) === true || anchorEl?.contains(t) === true);
    const onOutside = (e: Event) => {
      if (!inside(e.target)) close();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Tab") close();
      if (e.key !== "Escape") return;
      // Handled here so the global shortcut handler does not also clear the selection.
      e.preventDefault();
      e.stopPropagation();
      close();
      anchorEl?.focus();
    };
    window.addEventListener("pointerdown", onOutside, true);
    window.addEventListener("scroll", onOutside, true);
    window.addEventListener("wheel", onOutside, { capture: true, passive: true });
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    // A dialog that opens while a menu is open (an import prompt) covers the app; the menu must not stay on top of it.
    const unsubscribeUi = useUi.subscribe((s, prev) => {
      if (s.dialog && s.dialog !== prev.dialog) close();
    });
    return () => {
      unsubscribeUi();
      openMenus--;
      window.removeEventListener("pointerdown", onOutside, true);
      window.removeEventListener("scroll", onOutside, true);
      window.removeEventListener("wheel", onOutside, true);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      // The hovered row unmounts without a mouseleave, so its status-bar hint would stick.
      useUi.getState().setHoverHint(null);
    };
  }, [anchorEl]);

  return createPortal(
    <MenuPanel
      entries={entries}
      anchor={at}
      side={anchorEl ? "below" : "right"}
      label={label}
      onClose={() => closeRef.current()}
      panelRef={rootRef}
      focus="panel"
    />,
    document.body,
  );
}

interface OpenSubmenu {
  id: string;
  item: HTMLElement;
  anchor: Rect;
  focusFirst: boolean;
}

interface PanelProps {
  entries: MenuEntry[];
  anchor: Rect;
  side: "below" | "right";
  label: string;
  onClose(): void;
  panelRef?: RefObject<HTMLDivElement | null>;
  /** Once placed, focus the panel itself (root menu) or its first item (submenu opened by keyboard). */
  focus?: "panel" | "first";
  /** ArrowLeft in a submenu: close it and return focus to its row in the parent. */
  onBack?(): void;
  onPointerEnter?(): void;
}

function itemsOf(panel: HTMLElement | null): HTMLButtonElement[] {
  return panel ? Array.from(panel.querySelectorAll<HTMLButtonElement>(":scope > .menu-item")) : [];
}

/** Submenus line up with their row and slightly overlap the parent panel. */
function submenuAnchor(item: HTMLElement, panel: HTMLElement | null): Rect {
  const row = rectOf(item);
  const box = panel ? rectOf(panel) : row;
  return { left: box.left + 2, right: box.right - 2, top: row.top - 5, bottom: row.bottom };
}

function MenuPanel({ entries, anchor, side, label, onClose, panelRef, focus, onBack, onPointerEnter }: PanelProps) {
  const ownRef = useRef<HTMLDivElement>(null);
  const ref = panelRef ?? ownRef;
  const hideSoon = useStore((s) => s.hideSoon);
  const rows = tidy(entries, hideSoon);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const [sub, setSub] = useState<OpenSubmenu | null>(null);
  const closeTimer = useRef(0);
  const placed = pos !== null;

  // Measure while hidden, then place before the first paint.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const size = { width: el.offsetWidth, height: el.offsetHeight };
    setPos(placeMenu(anchor, size, { width: window.innerWidth, height: window.innerHeight }, side));
  }, [anchor, side, rows.length, ref]);

  useEffect(() => {
    if (!placed) return;
    if (focus === "panel") ref.current?.focus({ preventScroll: true });
    if (focus === "first") itemsOf(ref.current)[0]?.focus({ preventScroll: true });
  }, [placed, focus, ref]);

  useEffect(() => () => window.clearTimeout(closeTimer.current), []);

  function openSubmenu(f: Feature, item: HTMLElement, focusFirst: boolean) {
    window.clearTimeout(closeTimer.current);
    setSub({ id: f.id, item, anchor: submenuAnchor(item, ref.current), focusFirst });
  }

  function hover(f: Feature, item: HTMLElement) {
    useUi.getState().setHoverHint(hoverHintFor(f, blockedReason(f)));
    window.clearTimeout(closeTimer.current);
    if (isMenu(f)) {
      if (sub?.id !== f.id) openSubmenu(f, item, false);
    } else if (sub) {
      // A short grace period lets the pointer cross other rows on its way into the open submenu.
      closeTimer.current = window.setTimeout(() => setSub(null), 250);
    }
  }

  function choose(f: Feature, item: HTMLElement) {
    if (isMenu(f)) {
      // Hovering has usually opened it already, so a click keeps it open; a Soon menu also toasts.
      activate(f);
      if (sub?.id !== f.id) openSubmenu(f, item, false);
      return;
    }
    // Run before closing: "Centre view here" reads the context menu's point, and "Add this triangle to selection" its
    // triangle, which closing clears.
    activate(f);
    onClose();
  }

  function onKeyDown(e: ReactKeyboardEvent<HTMLDivElement>) {
    const items = itemsOf(ref.current);
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const focusAt = (n: number) => items[(n + items.length) % items.length]?.focus();
    const current = i >= 0 ? featureById(items[i].dataset.id ?? "") : undefined;
    if (e.key === "ArrowDown") focusAt(i + 1);
    else if (e.key === "ArrowUp") focusAt(i < 0 ? -1 : i - 1);
    else if (e.key === "Home") focusAt(0);
    else if (e.key === "End") focusAt(-1);
    else if (e.key === "ArrowRight" && current && isMenu(current)) openSubmenu(current, items[i], true);
    else if (e.key === "ArrowLeft" && onBack) onBack();
    else return;
    e.preventDefault();
    e.stopPropagation();
  }

  if (rows.length === 0) return null;

  return (
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      tabIndex={-1}
      style={pos ? { left: pos.x, top: pos.y } : { left: 0, top: 0, visibility: "hidden" }}
      onKeyDown={onKeyDown}
      onPointerEnter={onPointerEnter}
      onContextMenu={(e) => e.preventDefault()}
    >
      {rows.map((row, i) =>
        row === "-" ? (
          <div key={`sep-${i}`} className="menu-sep" role="separator" />
        ) : (
          <MenuItem key={row.id} f={row} open={sub?.id === row.id} onHover={hover} onChoose={choose} />
        ),
      )}
      {sub && (
        <MenuPanel
          key={sub.id}
          entries={submenuEntries(sub.id)}
          anchor={sub.anchor}
          side="right"
          label={featureById(sub.id)?.label ?? ""}
          onClose={onClose}
          focus={sub.focusFirst ? "first" : undefined}
          onBack={() => {
            setSub(null);
            sub.item.focus();
          }}
          onPointerEnter={() => window.clearTimeout(closeTimer.current)}
        />
      )}
    </div>
  );
}

export interface ItemProps {
  f: Feature;
  open: boolean;
  onHover(f: Feature, item: HTMLElement): void;
  onChoose(f: Feature, item: HTMLElement): void;
}

/** A row. A blocked one (blockedReason) is greyed out with the reason in its tooltip; choosing it shows the reason. */
export function MenuItem({ f, open, onHover, onChoose }: ItemProps) {
  const active = useStore((s) => f.isActive?.(s) ?? false);
  const repairActive = useRepair((r) => f.repairActive?.(r) ?? false);
  const measureActive = useMeasure((m) => f.measureActive?.(m) ?? false);
  const triangle = useUi((s) => s.contextMenu?.triangle);
  const booleanOpen = useBoolean((b) => b.session !== null);
  const blocked = useRepair((r) => blockedReason(f, r, triangle, booleanOpen));
  const soon = effectiveStatus(f) === "soon";
  const submenu = isMenu(f);
  const toggle = f.isActive !== undefined || f.repairActive !== undefined || f.measureActive !== undefined;
  const checked = active || repairActive || measureActive;
  const Icon = f.icon;
  return (
    <button
      type="button"
      className={cx("menu-item", soon && "is-soon", blocked !== null && "is-disabled", checked && "is-active", open && "is-open")}
      role={toggle ? "menuitemcheckbox" : "menuitem"}
      aria-checked={toggle ? checked : undefined}
      aria-disabled={blocked !== null ? true : undefined}
      aria-haspopup={submenu ? "menu" : undefined}
      aria-expanded={submenu ? open : undefined}
      aria-label={f.label}
      title={tooltipFor(f, blocked)}
      data-id={f.id}
      tabIndex={-1}
      onMouseEnter={(e) => onHover(f, e.currentTarget)}
      onMouseLeave={() => useUi.getState().setHoverHint(null)}
      onClick={(e) => onChoose(f, e.currentTarget)}
    >
      <Icon className="menu-icon" size={16} strokeWidth={1.75} aria-hidden />
      <span className="menu-label">{f.label}</span>
      {f.shortcut && <span className="menu-shortcut">{f.shortcut}</span>}
      {soon && <span className="soon-pill">Soon</span>}
      {submenu && <ChevronRight className="menu-chevron" size={14} aria-hidden />}
    </button>
  );
}
