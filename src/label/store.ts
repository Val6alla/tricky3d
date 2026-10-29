// Label mode's state: the part being labelled, the labels placed on it (not yet applied), the selected one, the tool,
// the mode's own undo steps, an Apply in progress, the shapes Rust drew and the fonts. src/label/session.ts is the
// only code that changes it.
import { create } from "zustand";
import type { ShapeReply } from "./ipc";
import type { FontInfo, LabelKind, PlacedLabel } from "./types";

/** Select picks and moves labels; Place adds a label of `kind` where the part is clicked next. */
export type LabelTool = "select" | "place";

/** A state of the labels the mode's Undo and Redo go back and forth between. */
export interface LabelStep {
  labels: PlacedLabel[];
  selected: string | null;
  label: string;
}

export interface LabelSession {
  partId: string;
  labels: PlacedLabel[];
  selected: string | null;
  tool: LabelTool;
  /** The kind Place label adds. */
  kind: LabelKind;
  /** Undo and Redo inside the mode: the labels before each change, and after each change undone. */
  past: LabelStep[];
  future: LabelStep[];
}

/** Apply in progress. */
export interface LabelBusy {
  job: number;
  cancelling: boolean;
  /** 0..1, from Rust's progress events. */
  fraction: number;
}

/** A label's shape as Rust drew it, or why it could not. */
export type ShapeEntry = { status: "loading" } | { status: "ready"; reply: ShapeReply } | { status: "error"; message: string };

/** A question the panel shows over the window, with Yes and No. */
export interface LabelQuestion {
  title: string;
  text: string;
}

export interface LabelState {
  session: LabelSession | null;
  busy: LabelBusy | null;
  question: LabelQuestion | null;
  /** By shape key (src/label/session.ts shapeKey): what the drawing and Keep aspect ratio use. */
  shapes: Record<string, ShapeEntry>;
  /** Null until first asked for. */
  fonts: FontInfo[] | null;
}

export const useLabel = create<LabelState>()(() => ({ session: null, busy: null, question: null, shapes: {}, fonts: null }));
