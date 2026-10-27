import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import "./dialogs.css";

const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

interface DialogProps {
  title: string;
  onClose(): void;
  /** Primary action; it decides whether to close. Without it the footer shows a single Close button. */
  onOk?: () => void;
  okLabel?: string;
  okDisabled?: boolean;
  /** A second choice shown in place of Cancel; Esc and a click outside still cancel. */
  secondary?: { label: string; onClick(): void };
  /** Extra class on the dialog box, e.g. to make it wider. */
  className?: string;
  /** Extra class on the backdrop, e.g. to dim less and move the box aside so the viewport shows a preview. */
  backdropClassName?: string;
  children: ReactNode;
}

/** Keeps Tab and Shift+Tab cycling inside the dialog. */
function trapTab(e: KeyboardEvent<HTMLDivElement>, panel: HTMLElement) {
  const items = Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE));
  if (items.length === 0) {
    e.preventDefault();
    return;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const active = document.activeElement;
  if (e.shiftKey && (active === first || active === panel)) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && active === last) {
    e.preventDefault();
    first.focus();
  }
}

/** Modal frame shared by every dialog: title, body, footer with Cancel and a primary OK. */
export function Dialog({ title, onClose, onOk, okLabel = "OK", okDisabled = false, secondary, className, backdropClassName, children }: DialogProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const pressedBackdrop = useRef(false);
  const titleId = useId();

  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const field = panel.querySelector<HTMLElement>("input, select, textarea");
    (field ?? panel.querySelector<HTMLElement>(".dlg-btn-primary") ?? panel).focus();
    if (field instanceof HTMLInputElement && (field.type === "text" || field.type === "number")) field.select();
    return () => previous?.focus();
  }, []);

  function submit() {
    if (!onOk) onClose();
    else if (!okDisabled) onOk();
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    // App shortcuts (Del, Ctrl+Z, F, A...) must not act on the scene behind a modal.
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement) && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
    } else if (e.key === "Tab" && panelRef.current) {
      trapTab(e, panelRef.current);
    }
  }

  return (
    <div
      className={backdropClassName ? `dlg-backdrop ${backdropClassName}` : "dlg-backdrop"}
      // Close only when the press also started on the backdrop, so a text selection dragged out of an input does not close it.
      onPointerDown={(e) => {
        pressedBackdrop.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        if (pressedBackdrop.current && e.target === e.currentTarget) onClose();
      }}
      onKeyDown={onKeyDown}
    >
      <div ref={panelRef} className={className ? `dlg ${className}` : "dlg"} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
        <h2 id={titleId} className="dlg-title">
          {title}
        </h2>
        <div className="dlg-body">{children}</div>
        <div className="dlg-footer">
          {onOk ? (
            <>
              <button type="button" className="dlg-btn" onClick={secondary?.onClick ?? onClose}>
                {secondary?.label ?? "Cancel"}
              </button>
              <button type="button" className="dlg-btn dlg-btn-primary" disabled={okDisabled} onClick={onOk}>
                {okLabel}
              </button>
            </>
          ) : (
            <button type="button" className="dlg-btn dlg-btn-primary" onClick={onClose}>
              Close
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
