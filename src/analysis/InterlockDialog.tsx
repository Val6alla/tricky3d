// The interlocking test's answer: the parts that cannot be lifted out upwards, or the order in which all of them can.
import { Dialog } from "../dialogs/Dialog";
import { useStore } from "../store";
import "./analysis.css";

export function InterlockDialog({ rounds, trapped, onClose }: { rounds: string[][]; trapped: string[]; onClose(): void }) {
  const parts = useStore((s) => s.parts);
  const name = (id: string) => parts.find((p) => p.id === id)?.name ?? "a deleted part";
  const tested = rounds.flat().length + trapped.length;
  const select = () => {
    useStore.getState().select(trapped.filter((id) => parts.some((p) => p.id === id)), "replace");
    onClose();
  };
  return (
    <Dialog
      title="Interlocking test"
      onClose={onClose}
      closeLabel="Close"
      {...(trapped.length > 0 ? { onOk: select, okLabel: "Select them" } : {})}
      className="dlg-analysis"
    >
      {trapped.length === 0 ? (
        <>
          <p className={`an-verdict is-pass`}>No parts are interlocked</p>
          <p className="dlg-note">All {tested} parts can be lifted out upwards, in this order (parts in one step together):</p>
        </>
      ) : (
        <>
          <p className={`an-verdict is-fail`}>{trapped.length === 1 ? "1 part cannot be lifted out" : `${trapped.length} parts cannot be lifted out`}</p>
          <p className="dlg-note">Lifted straight up, each of these would hit another part that is still there:</p>
          <ul className="an-pairs">
            {trapped.map((id) => (
              <li key={id}>
                <span className="an-pair-names">{name(id)}</span>
              </li>
            ))}
          </ul>
          {rounds.length > 0 && <p className="dlg-note">The others lift out in this order:</p>}
        </>
      )}
      {rounds.length > 0 && (
        <ol className="an-rounds">
          {rounds.map((round, i) => (
            <li key={i}>{round.map(name).join(", ")}</li>
          ))}
        </ol>
      )}
    </Dialog>
  );
}
