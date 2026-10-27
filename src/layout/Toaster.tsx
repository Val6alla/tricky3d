import { CircleAlert, TriangleAlert } from "lucide-react";
import { useUi, type ToastItem } from "../ui";
import "./layout.css";

function ToastIcon({ kind }: { kind: ToastItem["kind"] }) {
  if (kind === "warn") return <TriangleAlert size={15} className="toast-icon" />;
  if (kind === "error") return <CircleAlert size={15} className="toast-icon" />;
  return null;
}

export function Toaster() {
  const toasts = useUi((s) => s.toasts);
  const dismiss = useUi((s) => s.dismissToast);

  // The live region stays mounted so screen readers announce toasts added to it.
  return (
    <div className="toaster" aria-live="polite">
      {toasts.map((t) => (
        <button
          key={t.id}
          type="button"
          className={`toast toast-${t.kind}`}
          title="Click to dismiss"
          onClick={() => dismiss(t.id)}
        >
          <ToastIcon kind={t.kind} />
          <span className="toast-message">{t.message}</span>
        </button>
      ))}
    </div>
  );
}
