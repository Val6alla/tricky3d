import { useState } from "react";
import { ChevronDown } from "lucide-react";
import { fileMenuEntries } from "../menus/entries";
import { Menu } from "../menus/Menu";

/** The "File" button at the far left of the tab row and its dropdown. */
export function FileMenu() {
  const [menuAt, setMenuAt] = useState<HTMLElement | null>(null);
  return (
    <>
      <button
        type="button"
        className={menuAt ? "rb-file is-open" : "rb-file"}
        aria-haspopup="menu"
        aria-expanded={menuAt !== null}
        onClick={(e) => {
          const el = e.currentTarget;
          setMenuAt((open) => (open ? null : el));
        }}
      >
        File
        <ChevronDown size={12} aria-hidden />
      </button>
      {menuAt && <Menu entries={fileMenuEntries()} anchor={{ el: menuAt }} label="File" onClose={() => setMenuAt(null)} />}
    </>
  );
}
