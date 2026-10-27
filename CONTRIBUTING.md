# Contributing to tricky3D

Issues and pull requests are welcome. For a bug, open an issue with the steps to reproduce it and, if you can share it, the file that shows the problem. For a larger change, open an issue first so we can agree on the approach before you write the code.

## Adding a feature

Every ribbon button and menu item comes from `src/features.ts`. Most planned features are already listed there with `status: "soon"`, which shows a "Soon" badge. To ship one, give its entry a `run()` and set `status: "ready"`. If the feature isn't listed yet, add an entry.

## Architecture rule

All code that reads or changes mesh geometry (repair, edits, booleans, cuts, analysis) lives in Rust (`src-tauri/src`), runs off the UI thread with progress and Cancel, and has tests. The TypeScript frontend only handles the interface: buttons, panels, selection display, and calling Rust commands.

## Ground rules

- **No network access.** tricky3D makes no network connections and collects nothing: no telemetry, analytics, crash reports, accounts, update checks, CDN fonts or remote assets. Don't add network permissions to `src-tauri/capabilities/` or loosen the content security policy in `src-tauri/tauri.conf.json`. On Windows, the switches in the main window's `additionalBrowserArgs` turn off WebView2's own background networking; keep them when you change that line.
- **GPL-3.0-compatible dependencies only.** After adding or updating a dependency, run `node scripts/third-party.mjs` to regenerate the full list in `THIRD_PARTY.md`. For a new direct dependency, also add a row with its license and what it's used for to the tables at the top of that file. CI rejects licenses that are not on the allow list in `deny.toml`.
- **Your own work.** Don't copy code, icons or artwork from Netfabb or other closed-source tools. Icons come from [Lucide](https://lucide.dev).
- **Millimetres.** tricky3D works in mm and reads unitless formats (STL, OBJ, PLY, OFF) as mm.
- **Only synthetic or public test models.** Committed tests and fixtures use shapes built in code (boxes, cylinders, tori) or small hand-written files. Don't commit model files you aren't allowed to share, or geometry taken from them.
- **Heavy work stays polite.** Mesh jobs run below normal priority and leave one CPU core free (`src-tauri/src/workers.rs`). New commands that read or change meshes should go through `heavy()` in `src-tauri/src/commands.rs`.

## Tests on your own models

Some tests are skipped unless you point them at model files on your computer, so you can check a change against real models without committing them. The paths only ever come from environment variables, for example:

```sh
TRICKY3D_MODELS="a.stl|b.3mf" cargo test --release --manifest-path src-tauri/Cargo.toml --test wrap_overlap -- --ignored --nocapture extended_repair_on_model_files
```

Each of these tests says in its doc comment which variable it reads. `scripts/corpus/repair-corpus.ps1` runs Extended repair over a whole folder of models, such as the [Thingi10K](https://ten-thousand-models.appspot.com/) dataset; pass the folder with `-Models` or the `TRICKY3D_CORPUS` variable. Point its `-OutDir` at `corpus-out/`, which git ignores, or at a folder outside the repository.

## Before you open a pull request

Run the checks CI runs:

```sh
npm run build      # type-check and build the frontend (the Rust build needs dist/)
npm test           # TypeScript unit tests
npm run clippy     # Rust lints, warnings are errors
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
node scripts/third-party.mjs --check
cargo deny --manifest-path src-tauri/Cargo.toml check licenses   # install once: cargo install --locked cargo-deny
```

Use [Conventional Commits](https://www.conventionalcommits.org) for commit messages, for example `fix: keep part names when splitting shells`.

By contributing, you agree that your work is licensed under the GPL-3.0-only license that covers tricky3D.
