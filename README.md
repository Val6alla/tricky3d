# tricky3D

tricky3D is a free, open-source desktop app that prepares and repairs meshes for 3D printing. It runs natively on Apple Silicon Macs, Windows and Linux, and it works without an internet connection.

tricky3D is early and unfinished. Many buttons are placeholders with a "Soon" badge, and the tools that work still have rough edges. Read [What works today](#what-works-today) and [Known limitations](#known-limitations) before you rely on it for a print.

tricky3D is an independent project. It is not affiliated with or endorsed by Autodesk. Netfabb is a trademark of Autodesk, Inc.

## Privacy

tricky3D works fully offline. It has no accounts, telemetry, analytics, crash reports or update checks, and its code makes no network connections. Your files never leave your computer.

Apart from the files you pick in its file dialogs or drop on its window, it reads and writes only two things of its own:

- its view settings and panel widths, kept in its app data folder
- the repair scripts you save, in a `repair-scripts.json` file in its config folder

### One caveat on Windows

On Windows, tricky3D's window is drawn by Microsoft's WebView2 runtime, the engine of Microsoft Edge. It runs as `msedgewebview2.exe` processes under tricky3D, and it can connect to Microsoft servers on its own. tricky3D starts it with its background networking, component updates and similar services switched off (`additionalBrowserArgs` in `src-tauri/tauri.conf.json`). In our last test on Windows 11, the runtime still opened two HTTPS connections to a Microsoft Office 365 address soon after start. They come from the runtime, not from tricky3D's code, and we haven't found a switch that stops them. The macOS and Linux builds use the system's WebKit instead, and we haven't checked them the same way yet.

### How to check this yourself

Three files in this repository decide what the app can reach:

- `src-tauri/tauri.conf.json` holds the content security policy (`app.security.csp`). `default-src 'self'` lets the window load only the files bundled with the app. `connect-src` lets it talk only to the app itself and to the app's Rust backend (`ipc:` and `http://ipc.localhost`, which is how Tauri passes messages on Windows).
- `src-tauri/capabilities/default.json` lists every permission the window has. There is no network, shell, updater or process permission, and the window has no file access of its own.
- `src-tauri/src/grants.rs` is the check that lets the Rust backend open only the files you picked or dropped.

You can also watch the running app from outside and list its connections while you use it: Resource Monitor's Network tab on Windows (include the `msedgewebview2.exe` processes), Little Snitch or `nettop` on macOS, `ss -tp` on Linux.

## What works today

| Feature | Status |
| --- | --- |
| Workspace: ribbon, parts list, 3D view, standard views, wireframe, rulers | Ready |
| Import and export (see [Supported formats](#supported-formats)), including drag and drop | Ready |
| Projects: save and open all parts with their positions as a 3MF file | Ready |
| Move, rotate and scale with the gizmo or typed values; place and centre parts on the platform | Ready |
| Clip planes along X, Y, Z or any direction | Ready |
| Information panel: size, volume, area, triangles, shells, open edges | Ready |
| Split shells into parts, merge parts, cut parts along a plane with or without closing the cut | Ready |
| Boolean union, difference and intersection, with a preview | Ready |
| Undo and redo, copy and paste | Ready |
| Repair: status and damage warnings, close holes, stitch open edges, fix flipped triangles, remove self-intersections and degenerate triangles, wrap the part surface | Ready |
| Repair scripts: Simple, Default and Extended repair, your own scripts, and `.repair` script files saved by Netfabb | Ready |
| Triangle selection and editing: select by triangle, surface, shell, brush or criteria; delete and flip triangles; move points; cut the surface open along a drawn line | Ready |
| Repair of many parts at once | Ready |
| Part library: box, cylinder, sphere, torus, helix and 8 more solids, made to the sizes you type | Ready |
| Platform statistics: a sortable table of every part, the platform's packing density, export to CSV | Ready |
| Measuring: distances, angles, radii, positions and wall thickness between points, edges, circles, spheres and surfaces, saved with the project | Ready |
| Labels: text, a picture, numbered or listed copies, and breakaway tags, raised from or sunk into a part's surface, in a built-in font or one installed on your computer | Ready |
| Bridging a gap between two open edges | Coming soon |
| Overhang, skin, centre of gravity and other analysis | Coming soon |
| Mirror, hollow, simplify, smooth and remesh | Coming soon |
| Auto-arrange, packing and copies | Coming soon |
| Recent files and autosave | Coming soon |
| Screenshots and reports | Coming soon |

Buttons for features that aren't ready yet carry a "Soon" badge. To hide them, turn on **Hide unavailable features** in File › Settings.

## Known limitations

- **Unsigned builds.** The installers aren't signed with a paid Apple or Microsoft certificate, so macOS and Windows warn you the first time. [INSTALL.md](INSTALL.md) shows how to allow the app.
- **Platforms.** Macs need macOS 13 or later; one app runs on both Apple Silicon and Intel Macs. Linux builds are for x86-64 only. Most testing happens on Windows, and the macOS and Linux builds get less of it.
- **Units.** STL, OBJ, PLY and OFF files don't say which unit they use, so tricky3D reads them as millimetres. A model drawn in inches needs scaling after import. When a model looks like it was drawn in metres, tricky3D asks.
- **Extended repair can fill hollow parts.** Wrap part surface rebuilds the outside of a part. On a part with an inner cavity, such as a hollowed resin print, it can fill the cavity. In the last full test on the [Thingi10K](https://ten-thousand-models.appspot.com/) models, this happened on 21 of 5,050 models that needed no repair. Repair a copy and check the result before you print it.
- **Repair speed.** Extended repair on large, messy models (hundreds of thousands of triangles in many overlapping shells) can take from a few seconds to over a minute. In the same test, it repaired 4,939 of Thingi10K's 4,947 broken models. Seven did not finish within 90 seconds, and one file could not be read.
- **Results differ from other tools.** During development, repair results are compared with Netfabb's. tricky3D's code is its own, and its results are not identical.
- **Download size.** The Windows installers are about 210 MB because they carry Microsoft's WebView2 installer, so installing never needs a download.

## Supported formats

STL, OBJ, PLY and OFF files have no units, so tricky3D reads them as millimetres. 3MF and AMF files say which unit they use, and tricky3D converts them to millimetres.

| Format | Import | Export |
| --- | --- | --- |
| STL | Binary and ASCII | Binary or ASCII, all selected parts in one mesh |
| OBJ | Polygons are split into triangles; files with several objects can become one part each | One object per part |
| PLY | ASCII and binary | Binary, all selected parts in one mesh |
| 3MF | Objects, placement and units | One object per part, in mm |
| AMF | Plain or zipped, with units | One object per part, in mm |
| OFF | Text; polygons are split into triangles | Text, all selected parts in one mesh |

Export (Ctrl+E) writes each part where it sits on the platform. It asks whether STL should be binary or ASCII, and, for several parts, whether they go into one file or into one file per part, each named after its part.

tricky3D saves projects as 3MF files, which other 3MF programs can open too.

## Installing

Download the installer for your computer from the [Releases](../../releases) page: a `.dmg` for Macs with Apple or Intel chips (macOS 13 or later), a `-setup.exe` for Windows, and an `.AppImage` or `.deb` for Linux. The installers aren't signed, so macOS and Windows warn you before the first start. [INSTALL.md](INSTALL.md) shows, step by step, how to install tricky3D and get past that warning on each system.

## Building from source

You need:

- Node.js 22 (22.12 or later) or Node.js 24. Node 20 is too old for the test runner.
- Rust, stable toolchain.
- The [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform: Xcode Command Line Tools on macOS, the Microsoft C++ Build Tools and WebView2 on Windows, and the WebKitGTK development packages on Linux.

Then run these from the repository root:

```sh
npm install                                          # install the JavaScript dependencies
npm run tauri dev                                    # start the app in development mode
npm run tauri build                                  # build installers into src-tauri/target/release/bundle/
```

To run the same checks as CI:

```sh
npm run build                                        # type-check and build the frontend (the Rust build needs dist/)
npm test                                             # run the TypeScript unit tests
npm run clippy                                       # lint the Rust code; warnings are errors
cargo fmt --manifest-path src-tauri/Cargo.toml --check   # check the Rust formatting
cargo test --manifest-path src-tauri/Cargo.toml      # run the Rust tests, including the format round trips
node scripts/third-party.mjs --check                 # check THIRD_PARTY.md and the npm licenses
cargo deny --manifest-path src-tauri/Cargo.toml check licenses   # check the Rust licenses (install once: cargo install --locked cargo-deny)
```

GitHub Actions builds the macOS, Windows and Linux installers (`.github/workflows/release.yml`) after running the same checks as CI. Run the workflow by hand from the Actions tab to get test builds as downloads on the run page, or push a tag such as `v0.2.0` to create a draft release with the installers attached. The tag must match the app's version, which `scripts/bump-version.sh` (or `scripts/bump-version.ps1` on Windows) sets in every file that holds it.

## Contributing

Issues and pull requests are welcome. Every ribbon button and menu item comes from `src/features.ts`, so new features start there. See [CONTRIBUTING.md](CONTRIBUTING.md) for the details, and [SECURITY.md](SECURITY.md) to report a security problem privately.

## License

Copyright (C) 2026 the tricky3D contributors.

tricky3D is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License, version 3 only (GPL-3.0-only). See [LICENSE](LICENSE). The licenses of the libraries it uses are listed in [THIRD_PARTY.md](THIRD_PARTY.md).

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

## Trademarks

tricky3D is an independent project. It is not affiliated with, endorsed by or sponsored by Autodesk, Inc. Netfabb is a trademark of Autodesk, Inc. Other product names are trademarks of their owners. This README names them only to describe file compatibility or to compare results.
