// Generate shell's dialog: thickness, kind, output, accuracy with an estimate of the work, smoothing and whether the
// original goes. OK runs it in Rust with a progress bar and Cancel in the same dialog; a part that is not closed and
// oriented is named, with Repair part.
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Dialog } from "../dialogs/Dialog";
import "../dialogs/transform.css";
import { cancelRepair } from "../repair/ipc";
import { useStore } from "../store";
import { toast } from "../ui";
import { estimateShells } from "./ipc";
import { repairRefused, runGenerateShell, shellTargets, type Refused } from "./session";
import { checkFields, estimateParts, estimateText, heavyWarning, kindChoice, KINDS, loadFields, type ShellFields } from "./settings";
import "./shell.css";
import type { ShellEstimate, ShellKind, ShellProgress } from "./types";
import { nextJob } from "../jobs";

type Phase =
  | { kind: "form" }
  | { kind: "running"; job: number; progress: ShellProgress | null; cancelling: boolean }
  | { kind: "refused"; parts: Refused[] }
  | { kind: "warnings"; made: number; warnings: string[] }
  | { kind: "failed"; message: string };

export function GenerateShellDialog({ onClose }: { onClose(): void }) {
  const parts = useStore((s) => s.parts);
  const selection = useStore((s) => s.selection);
  // shellTargets reads the store itself; parts and selection are listed so the targets follow it.
  const targets = useMemo(() => shellTargets(), [parts, selection]);
  const [fields, setFields] = useState<ShellFields>(loadFields);
  const [phase, setPhase] = useState<Phase>({ kind: "form" });
  const job = phase.kind === "running" ? phase.job : null;
  // Set by Cancel; read when the run ends, so a cancel too late (or too early) for Rust still adds nothing.
  const cancelled = useRef(false);

  // Closed while running (however): Rust stops too, and nothing is added.
  useEffect(() => {
    if (job === null) return;
    return () => {
      cancelRepair(job).catch(() => {});
    };
  }, [job]);

  async function run() {
    const settings = checkFields(fields).settings;
    if (!settings) return;
    const job = nextJob();
    cancelled.current = false;
    setPhase({ kind: "running", job, progress: null, cancelling: false });
    const outcome = await runGenerateShell(
      settings,
      fields.removeOriginal,
      job,
      (progress) => setPhase((p) => (p.kind === "running" && p.job === job ? { ...p, progress } : p)),
      () => cancelled.current,
    );
    switch (outcome.kind) {
      case "done":
        if (outcome.warnings.length) {
          setPhase({ kind: "warnings", made: outcome.made, warnings: outcome.warnings });
        } else {
          onClose();
          toast(outcome.made === 1 ? "Made 1 new part." : `Made ${outcome.made} new parts.`);
        }
        break;
      case "cancelled":
        onClose();
        toast("Generate shell was cancelled. Nothing changed.");
        break;
      case "refused":
        setPhase({ kind: "refused", parts: outcome.parts });
        break;
      case "failed":
        setPhase({ kind: "failed", message: outcome.message });
        break;
    }
  }

  function cancel() {
    if (phase.kind !== "running" || phase.cancelling) return;
    cancelled.current = true;
    setPhase({ ...phase, cancelling: true });
    cancelRepair(phase.job).catch(() => {});
  }

  switch (phase.kind) {
    case "form":
      return <ShellForm fields={fields} setFields={setFields} targets={targets.length} onRun={() => void run()} onClose={onClose} />;
    case "running":
      return (
        <Dialog title="Generate shell" onClose={cancel} closeLabel={phase.cancelling ? "Cancelling…" : "Cancel"} closeDisabled={phase.cancelling}>
          <Running progress={phase.progress} />
        </Dialog>
      );
    case "refused":
      return (
        <Dialog
          title="Generate shell"
          onClose={onClose}
          onOk={() => {
            onClose();
            repairRefused(phase.parts.map((p) => p.id));
          }}
          okLabel={phase.parts.length === 1 ? "Repair part" : "Repair a part"}
          secondary={{ label: "Close", onClick: onClose }}
        >
          {phase.parts.map((p) => (
            <p key={p.id} className="dlg-text">
              {p.text}
            </p>
          ))}
          <p className="dlg-text">Generate shell needs a closed part whose triangles all face the same way. Repair can make it so; nothing was changed.</p>
        </Dialog>
      );
    case "warnings":
      return <ShellWarnings made={phase.made} warnings={phase.warnings} onClose={onClose} />;
    case "failed":
      return (
        <Dialog title="Generate shell" onClose={onClose}>
          <p className="dlg-error" role="alert">
            {phase.message}
          </p>
        </Dialog>
      );
  }
}

/**
 * A run that finished with warnings. When no part was made, the warnings say so for each part ("No part was made."),
 * so the count line is left out rather than saying it twice.
 */
export function ShellWarnings({ made, warnings, onClose }: { made: number; warnings: string[]; onClose(): void }) {
  return (
    <Dialog title="Generate shell" onClose={onClose}>
      {made > 0 && <p className="dlg-text">{made === 1 ? "Made 1 new part." : `Made ${made} new parts.`}</p>}
      <ul className="shell-warnings" role="alert">
        {warnings.map((w, i) => (
          <li key={i}>{w}</li>
        ))}
      </ul>
    </Dialog>
  );
}

function Running({ progress }: { progress: ShellProgress | null }) {
  const percent = Math.round(Math.min(1, Math.max(0, progress?.fraction ?? 0)) * 100);
  const step = progress?.stage ?? "Starting";
  const where = progress && progress.parts > 1 ? `Part ${progress.part + 1} of ${progress.parts}: ` : "";
  return (
    <div className="shell-running" role="status">
      <p className="dlg-text">
        {where}
        {step}…
      </p>
      <div className="shell-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
        <div className="shell-progress-fill" style={{ width: `${percent}%` }} />
      </div>
      <p className="dlg-note">Cancel stops it at once and leaves the project as it was.</p>
    </div>
  );
}

export interface ShellFormProps {
  fields: ShellFields;
  setFields(update: (f: ShellFields) => ShellFields): void;
  /** How many parts the run would act on. */
  targets: number;
  onRun(): void;
  onClose(): void;
}

export function ShellForm({ fields, setFields, targets, onRun, onClose }: ShellFormProps) {
  const check = checkFields(fields);
  const [estimate, setEstimate] = useState<ShellEstimate | null>(null);
  const thicknessId = useId();
  const kindId = useId();
  const kindHelpId = useId();
  const accuracyId = useId();
  const estimateId = useId();
  const outputName = useId();
  const problemId = useId();
  const smoothHelpId = useId();
  const removeHelpId = useId();
  const edit = (patch: Partial<ShellFields>) => setFields((f) => ({ ...f, ...patch }));

  // The estimate follows the settings; Rust only does arithmetic, so it comes back at once.
  const settings = check.settings;
  const key = settings ? `${settings.kind} ${settings.thickness} ${settings.accuracy}` : "";
  useEffect(() => {
    if (!settings) {
      setEstimate(null);
      return;
    }
    let live = true;
    estimateShells(estimateParts(shellTargets()), settings)
      .then((e) => live && setEstimate(e))
      .catch(() => live && setEstimate(null));
    return () => {
      live = false;
    };
    // `key` stands for the settings, which checkFields builds anew on every render.
  }, [key, targets]);

  const refused = estimate?.refused ?? null;
  const warning = estimate && !refused ? heavyWarning(estimate) : null;
  const noTargets = targets === 0 ? "Select a shown, unlocked part first." : null;
  const problem = check.problem ?? refused ?? noTargets;
  const describe = (field: "thickness" | "accuracy") => (check.bad === field ? problemId : undefined);

  return (
    <Dialog
      title="Generate shell"
      onClose={onClose}
      onOk={onRun}
      okLabel={warning ? "Run anyway" : "OK"}
      okDisabled={!settings || refused !== null || targets === 0}
      className="dlg-shell"
    >
      <div className="tf-row">
        <label className="dlg-label" htmlFor={thicknessId}>
          Shell thickness <span className="dlg-unit">mm</span>
        </label>
        <input
          id={thicknessId}
          className="dlg-input tf-num"
          type="text"
          inputMode="decimal"
          spellCheck={false}
          autoComplete="off"
          value={fields.thickness}
          aria-invalid={check.bad === "thickness" || undefined}
          aria-describedby={describe("thickness")}
          onChange={(e) => edit({ thickness: e.target.value })}
        />
      </div>
      <div className="tf-row">
        <label className="dlg-label" htmlFor={kindId}>
          Shell type
        </label>
        <select
          id={kindId}
          className="dlg-input dlg-select"
          value={fields.kind}
          aria-describedby={kindHelpId}
          onChange={(e) => edit({ kind: kindChoice(e.target.value as ShellKind).value })}
        >
          {KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
      </div>
      <p id={kindHelpId} className="dlg-note shell-kind-help">
        {kindChoice(fields.kind).help}
      </p>
      <section className="dlg-section" aria-label="Output">
        <h3 className="dlg-section-title">Output</h3>
        <div className="shell-outputs" role="radiogroup" aria-label="Output">
          <label className="dlg-check">
            <input type="radio" name={outputName} checked readOnly />
            <span className="dlg-check-label">Create part</span>
          </label>
          <label className="dlg-check shell-soon" title="Create slice is coming in a future version.">
            <input type="radio" name={outputName} disabled />
            <span className="dlg-check-label">
              Create slice <span className="soon-pill">Soon</span>
            </span>
          </label>
        </div>
        <div className="shell-part-options">
          <div className="tf-row">
            <label className="dlg-label" htmlFor={accuracyId}>
              Accuracy <span className="dlg-unit">mm</span>
            </label>
            <input
              id={accuracyId}
              className="dlg-input tf-num"
              type="text"
              inputMode="decimal"
              spellCheck={false}
              autoComplete="off"
              value={fields.accuracy}
              aria-invalid={check.bad === "accuracy" || undefined}
              aria-describedby={[describe("accuracy"), estimateId].filter(Boolean).join(" ")}
              onChange={(e) => edit({ accuracy: e.target.value })}
            />
          </div>
          <p id={estimateId} className="dlg-note">
            Smaller is finer and slower. {estimate && !refused ? estimateText(estimate) : ""}
          </p>
          <label className="dlg-check">
            <input type="checkbox" checked={fields.smooth} aria-describedby={smoothHelpId} onChange={(e) => edit({ smooth: e.target.checked })} />
            <span>
              <span className="dlg-check-label">Smooth result</span>
              <span id={smoothHelpId} className="dlg-help">
                Round off the new surface's corners for an even mesh.
              </span>
            </span>
          </label>
          <label className="dlg-check">
            <input
              type="checkbox"
              checked={fields.removeOriginal}
              aria-describedby={removeHelpId}
              onChange={(e) => edit({ removeOriginal: e.target.checked })}
            />
            <span>
              <span className="dlg-check-label">Remove original part</span>
              <span id={removeHelpId} className="dlg-help">
                The new part takes the original's place in the tree.
              </span>
            </span>
          </label>
        </div>
      </section>
      {problem && (
        <p id={problemId} className={check.bad || refused ? "dlg-error" : "dlg-note"} role="alert">
          {problem}
        </p>
      )}
      {warning && !problem && (
        <p className="shell-heavy" role="alert">
          {warning}
        </p>
      )}
    </Dialog>
  );
}
