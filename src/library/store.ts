// The Part library mode's state: the catalog from Rust, the open session (category, primitive, name and the values
// typed into the form), a part being made, and the tile thumbnails. src/library/session.ts is the only code that
// changes it.
import { create } from "zustand";
import type { LibraryPrimitive } from "./ipc";

/** The library's categories; only Primitive has parts yet. */
export type LibraryCategory = "primitive" | "design" | "mathematical" | "quality";

export const CATEGORIES: { id: LibraryCategory; label: string; ready: boolean }[] = [
  { id: "primitive", label: "Primitive", ready: true },
  { id: "design", label: "Design", ready: false },
  { id: "mathematical", label: "Mathematical", ready: false },
  { id: "quality", label: "Quality", ready: false },
];

export interface LibrarySession {
  category: LibraryCategory;
  /** Id of the primitive picked in the tiles. */
  primitive: string;
  /** The new part's name. */
  name: string;
  /** True once the user typed a name: picking another primitive then keeps it. */
  nameEdited: boolean;
  /** The form's text per parameter key, as typed. */
  values: Record<string, string>;
}

/** Create part in progress. */
export interface LibraryBusy {
  job: number;
  cancelling: boolean;
}

export interface LibraryState {
  /** Loaded once, the first time the library opens. */
  catalog: LibraryPrimitive[] | null;
  /** Null outside the mode. */
  session: LibrarySession | null;
  busy: LibraryBusy | null;
  /** Tile pictures as image URLs, by primitive id. The picked tile's follows the form. */
  thumbs: Record<string, string>;
}

export const useLibrary = create<LibraryState>()(() => ({
  catalog: null,
  session: null,
  busy: null,
  thumbs: {},
}));
