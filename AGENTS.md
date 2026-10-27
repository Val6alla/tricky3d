# Architecture rule

All code that reads or changes mesh geometry (repair, edits, booleans, cuts, analysis) lives in Rust (`src-tauri/src`), runs off the UI thread with progress and Cancel, and has tests. The TypeScript frontend only handles the interface: buttons, panels, selection display, and calling Rust commands.
