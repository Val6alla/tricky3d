import {
  FEATURES,
  activate,
  blockedReason,
  effectiveStatus,
  hoverHintFor,
  isHidden,
  tooltipFor,
  type Feature,
} from "../features";
import { useRepair } from "../repair/store";
import { useStore } from "../store";
import { useUi } from "../ui";
import "../repair/repair.css";

/** The strip's entries: the "quick" tab's top-level features, without the Soon ones while those are hidden. */
export function quickFeatures(features: readonly Feature[], hideSoon: boolean): Feature[] {
  return features.filter((f) => f.tab === "quick" && !f.parent && !isHidden(f, hideSoon));
}

/**
 * The quick-action strip at the bottom of the right panel (normal mode): every "quick" feature as an icon button with
 * its tooltip. Soon ones are greyed and only say they are coming, as everywhere else.
 */
export function QuickActions() {
  const hideSoon = useStore((s) => s.hideSoon);
  const features = quickFeatures(FEATURES, hideSoon);
  if (features.length === 0) return null;
  return (
    <div className="quick-actions" role="group" aria-label="Quick actions">
      {features.map((f) => (
        <QuickButton key={f.id} f={f} />
      ))}
    </div>
  );
}

function QuickButton({ f }: { f: Feature }) {
  const blocked = useRepair((r) => blockedReason(f, r));
  const soon = effectiveStatus(f) === "soon";
  const Icon = f.icon;
  return (
    <button
      type="button"
      className={`quick-btn${soon || blocked !== null ? " is-soon" : ""}`}
      aria-label={f.label}
      aria-disabled={blocked !== null ? true : undefined}
      title={tooltipFor(f, blocked)}
      onClick={() => activate(f)}
      onMouseEnter={() => useUi.getState().setHoverHint(hoverHintFor(f, blocked))}
      onMouseLeave={() => useUi.getState().setHoverHint(null)}
    >
      <Icon size={18} strokeWidth={1.75} aria-hidden="true" />
    </button>
  );
}
