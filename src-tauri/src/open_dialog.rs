//! The open dialog behind File › Open, File › Import part and repair mode's Import script. Rust
//! sets its format menu: for meshes "All supported files" first, which the dialog starts on, then
//! one entry per format, then "All files" (`mesh_filters`); for scripts the same shape
//! (`script_filters`). Extensions match in any letter case (".STL", ".Stl").
//!
//! macOS gets its own panel (macos.rs). The dialog plugin hands AppKit the extensions as
//! `allowedFileTypes`; AppKit maps each to a Uniform Type Identifier and enables only files whose
//! type conforms to one. A file is greyed out when macOS types its extension differently from the
//! filter (another app declaring its own type for .stl is enough), and the plugin shows no format
//! menu there at all.

#[cfg(target_os = "macos")]
mod macos;

use std::path::{Path, PathBuf};

use tauri::Window;

use crate::formats::ImportFormat;

/// An entry of the dialog's format menu.
pub struct Filter {
    pub name: String,
    /// Lower-case extensions without the dot; "*" matches every file.
    pub extensions: Vec<&'static str>,
}

impl Filter {
    /// True when the filter shows `path`: its extension is one of the filter's, in any letter case.
    /// The macOS panel asks this for every file; the other platforms' dialogs match on their own.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn accepts(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|e| e.to_str());
        self.extensions
            .iter()
            .any(|&want| want == "*" || ext.is_some_and(|ext| ext.eq_ignore_ascii_case(want)))
    }
}

/// The mesh format menu, in order. The first entry is the one the dialog starts on.
pub fn mesh_filters() -> Vec<Filter> {
    let ids = ImportFormat::ALL.map(ImportFormat::id);
    let mut out = vec![Filter {
        name: "All supported files".into(),
        extensions: ids.to_vec(),
    }];
    out.extend(ids.iter().map(|&id| Filter {
        name: format!("{} (*.{id})", id.to_ascii_uppercase()),
        extensions: vec![id],
    }));
    out.push(Filter {
        name: "All files".into(),
        extensions: vec!["*"],
    });
    out
}

/// The repair script menu: our own JSON scripts and the reference tool's `.repair` lists.
pub fn script_filters() -> Vec<Filter> {
    [
        ("Repair scripts", vec!["json", "repair"]),
        ("tricky3D script (*.json)", vec!["json"]),
        ("Repair list (*.repair)", vec!["repair"]),
        ("All files", vec!["*"]),
    ]
    .into_iter()
    .map(|(name, extensions)| Filter {
        name: name.into(),
        extensions,
    })
    .collect()
}

/// Shows the open dialog over `window` with the format menu `filters` and waits until it closes.
/// Returns the files picked, none when it was cancelled. Blocks, so call it off the main thread.
pub fn pick(
    window: &Window,
    title: &str,
    filters: Vec<Filter>,
    multiple: bool,
) -> Result<Vec<PathBuf>, String> {
    #[cfg(target_os = "macos")]
    return macos::pick(window, title, filters, multiple);
    #[cfg(not(target_os = "macos"))]
    return Ok(plugin_pick(window, title, filters, multiple));
}

/// Windows and Linux: the dialog plugin, which shows the menu with its first entry selected.
#[cfg(not(target_os = "macos"))]
fn plugin_pick(window: &Window, title: &str, filters: Vec<Filter>, multiple: bool) -> Vec<PathBuf> {
    use tauri_plugin_dialog::DialogExt;

    let mut dialog = window.dialog().file().set_parent(window).set_title(title);
    for filter in filters {
        // Windows matches "*.stl" in any case on its own; GTK matches patterns case-sensitively.
        let patterns: Vec<String> = filter
            .extensions
            .iter()
            .map(|&ext| {
                if cfg!(windows) {
                    ext.into()
                } else {
                    any_case(ext)
                }
            })
            .collect();
        let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(filter.name, &patterns);
    }
    let picked = if multiple {
        dialog.blocking_pick_files().unwrap_or_default()
    } else {
        dialog.blocking_pick_file().into_iter().collect()
    };
    picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect()
}

/// A GTK glob for `ext` in any letter case: "stl" becomes "[sS][tT][lL]" (the dialog adds "*.").
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn any_case(ext: &str) -> String {
    ext.chars()
        .map(|c| match c {
            c if c.is_ascii_alphabetic() => {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            }
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_starts_on_all_supported_files_then_lists_each_format() {
        let names: Vec<String> = mesh_filters().into_iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            [
                "All supported files",
                "STL (*.stl)",
                "OBJ (*.obj)",
                "PLY (*.ply)",
                "3MF (*.3mf)",
                "AMF (*.amf)",
                "OFF (*.off)",
                "All files",
            ]
        );
    }

    #[test]
    fn extensions_match_in_any_letter_case() {
        let all = &mesh_filters()[0];
        for name in [
            "part.stl",
            "PART.STL",
            "Part.Stl",
            "plate.3MF",
            "a.Obj",
            "b.PLY",
            "c.aMf",
            "d.OFF",
            "/Users/пользователь/Модели 3D/деталь й.STL",
        ] {
            assert!(all.accepts(Path::new(name)), "{name}");
        }
        for name in ["part.step", "part", "part.stl.bak", "stl", ".stl"] {
            assert!(!all.accepts(Path::new(name)), "{name}");
        }
        let stl = &mesh_filters()[1];
        assert!(stl.accepts(Path::new("x.sTL")));
        assert!(!stl.accepts(Path::new("x.obj")));
        let any = mesh_filters().pop().unwrap();
        assert!(any.accepts(Path::new("notes.txt")));
        assert!(any.accepts(Path::new("Makefile")));
    }

    #[test]
    fn the_script_menu_starts_on_both_script_kinds() {
        let filters = script_filters();
        let names: Vec<&str> = filters.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Repair scripts",
                "tricky3D script (*.json)",
                "Repair list (*.repair)",
                "All files",
            ]
        );
        let scripts = &filters[0];
        for name in ["Extended repair.repair", "mine.JSON", "C:/x/y.Repair"] {
            assert!(scripts.accepts(Path::new(name)), "{name}");
        }
        for name in ["part.stl", "list.repair.bak", "json"] {
            assert!(!scripts.accepts(Path::new(name)), "{name}");
        }
        assert!(filters[1].accepts(Path::new("a.json")));
        assert!(!filters[1].accepts(Path::new("a.repair")));
        assert!(filters[2].accepts(Path::new("a.REPAIR")));
        assert!(filters[3].accepts(Path::new("anything")));
    }

    #[test]
    fn gtk_patterns_match_both_cases() {
        assert_eq!(any_case("stl"), "[sS][tT][lL]");
        assert_eq!(any_case("3mf"), "3[mM][fF]");
        assert_eq!(any_case("*"), "*");
    }
}
