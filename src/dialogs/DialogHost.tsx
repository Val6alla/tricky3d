import { lazy, Suspense, type ComponentType } from "react";
import { useUi, type Dialog } from "../ui";

/**
 * A dialog component loaded the first time it opens, so the app's start parses less code. Loading takes a few
 * milliseconds from the app's own files; until then nothing shows.
 */
function onDemand<P extends object>(load: () => Promise<ComponentType<P>>) {
  return lazy(async () => ({ default: await load() }));
}

const PlatformDialog = onDemand(() => import("./PlatformDialog").then((m) => m.PlatformDialog));
const RenameDialog = onDemand(() => import("./RenameDialog").then((m) => m.RenameDialog));
const SettingsDialog = onDemand(() => import("./SettingsDialog").then((m) => m.SettingsDialog));
const AboutDialog = onDemand(() => import("./AboutDialog").then((m) => m.AboutDialog));
const FormatsDialog = onDemand(() => import("./FormatsDialog").then((m) => m.FormatsDialog));
const TransformDialog = onDemand(() => import("./TransformDialog").then((m) => m.TransformDialog));
const CutDialog = onDemand(() => import("./CutDialog").then((m) => m.CutDialog));
const PlatformStatsDialog = onDemand(() => import("../stats/PlatformStatsDialog").then((m) => m.PlatformStatsDialog));
const GenerateShellDialog = onDemand(() => import("../shell/GenerateShellDialog").then((m) => m.GenerateShellDialog));
const DuplicateDialog = onDemand(() => import("../arrange/DuplicateDialog").then((m) => m.DuplicateDialog));
const MinBoxDialog = onDemand(() => import("../arrange/MinBoxDialog").then((m) => m.MinBoxDialog));
const OrientDialog = onDemand(() => import("../arrange/OrientDialog").then((m) => m.OrientDialog));
const PackDialog = onDemand(() => import("../arrange/PackDialog").then((m) => m.PackDialog));
const PackingSettingsDialog = onDemand(() => import("../arrange/PackingSettingsDialog").then((m) => m.PackingSettingsDialog));
const PlacePromptDialog = onDemand(() => import("./PlacePromptDialog").then((m) => m.PlacePromptDialog));
const ImportObjectsDialog = onDemand(() => import("./ImportObjectsDialog").then((m) => m.ImportObjectsDialog));
const ScaleMetresDialog = onDemand(() => import("./ScaleMetresDialog").then((m) => m.ScaleMetresDialog));
const SplitManyDialog = onDemand(() => import("./SplitManyDialog").then((m) => m.SplitManyDialog));
const ExportOptionsDialog = onDemand(() => import("./ExportOptionsDialog").then((m) => m.ExportOptionsDialog));
const ChoosePartDialog = onDemand(() => import("../repair/dialogs/ChoosePartDialog").then((m) => m.ChoosePartDialog));
const ApplyRepairDialog = onDemand(() => import("../repair/dialogs/ApplyRepairDialog").then((m) => m.ApplyRepairDialog));
const ScriptPickerDialog = onDemand(() => import("../repair/dialogs/ScriptPickerDialog").then((m) => m.ScriptPickerDialog));
const RunProgressDialog = onDemand(() => import("../repair/dialogs/RunProgressDialog").then((m) => m.RunProgressDialog));
const ScriptSettingsDialog = onDemand(() =>
  import("../repair/dialogs/ScriptSettingsDialog").then((m) => m.ScriptSettingsDialog),
);

/** Renders the dialog named by useUi().dialog, or nothing. */
export function DialogHost() {
  const dialog = useUi((s) => s.dialog);
  if (!dialog) return null;
  return (
    <Suspense fallback={null}>
      <OpenDialog dialog={dialog} />
    </Suspense>
  );
}

function OpenDialog({ dialog }: { dialog: Exclude<Dialog, null> }) {
  const close = useUi((s) => s.closeDialog);
  switch (dialog.kind) {
    case "platform":
      return <PlatformDialog onClose={close} />;
    case "rename":
      // Keyed so opening it for another part starts from that part's name.
      return <RenameDialog key={dialog.partId} partId={dialog.partId} onClose={close} />;
    case "settings":
      return <SettingsDialog onClose={close} />;
    case "about":
      return <AboutDialog onClose={close} />;
    case "formats":
      return <FormatsDialog onClose={close} />;
    case "transform":
      return <TransformDialog key={dialog.mode} mode={dialog.mode} onClose={close} />;
    case "cut":
      return <CutDialog onClose={close} />;
    case "platform-stats":
      return <PlatformStatsDialog onClose={close} />;
    case "generate-shell":
      return <GenerateShellDialog onClose={close} />;
    case "duplicate":
      // Keyed so switching from one way of duplicating to another starts afresh.
      return <DuplicateDialog key={dialog.mode} mode={dialog.mode} onClose={close} />;
    case "min-box":
      return <MinBoxDialog onClose={close} />;
    case "orient":
      return <OrientDialog key={dialog.partId} partId={dialog.partId} onClose={close} />;
    case "pack":
      return <PackDialog key={dialog.method} method={dialog.method} onClose={close} />;
    case "packing-settings":
      return <PackingSettingsDialog key={dialog.ids.join()} ids={dialog.ids} onClose={close} />;
    case "place-on-import":
      return <PlacePromptDialog key={dialog.fileName} fileName={dialog.fileName} resolve={dialog.resolve} onClose={close} />;
    case "import-objects":
      return <ImportObjectsDialog key={dialog.fileName} fileName={dialog.fileName} count={dialog.count} resolve={dialog.resolve} onClose={close} />;
    case "scale-on-import":
      return <ScaleMetresDialog key={dialog.fileName} fileName={dialog.fileName} size={dialog.size} resolve={dialog.resolve} onClose={close} />;
    case "split-many":
      return <SplitManyDialog count={dialog.count} partName={dialog.partName} resolve={dialog.resolve} onClose={close} />;
    case "export-options":
      return <ExportOptionsDialog count={dialog.count} options={dialog.options} resolve={dialog.resolve} onClose={close} />;
    case "repair-choose-part":
      // Keyed so opening it for other parts starts with the first of them picked.
      return <ChoosePartDialog key={dialog.ids.join()} ids={dialog.ids} onClose={close} />;
    case "repair-apply":
      return <ApplyRepairDialog onClose={close} />;
    case "repair-script-picker":
      return <ScriptPickerDialog onClose={close} />;
    case "repair-progress":
      return <RunProgressDialog onClose={close} />;
    case "repair-script-settings":
      // Keyed so opening it for another script starts from that script's name.
      return <ScriptSettingsDialog key={dialog.scriptId ?? ""} scriptId={dialog.scriptId} onClose={close} />;
  }
}
