//! The save dialog behind Export. Its type list holds every format tricky3D writes, always in the
//! same order (`EXPORT_TYPES`), and the window names the type to select: the format of the file
//! the parts came from, else STL.
//!
//! On Windows Rust shows the system dialog itself, since the dialog plugin always selects the
//! first type. A default extension makes the dialog add the selected type's extension to a name
//! typed without one, and swap it when another type is picked, so the name the dialog returns is
//! the file written; the system asks before replacing a file that is there, unless told not to
//! (`confirm_replace`). Elsewhere the plugin shows it: the list is in the same order, but its first
//! type is selected, the file name's extension decides, and the system always asks.

use std::path::{Path, PathBuf};

use tauri::Window;

use crate::open_dialog::Filter;

/// Every format Export writes, in the order the dialog lists them: its extension and its name.
pub const EXPORT_TYPES: [(&str, &str); 6] = [
    ("stl", "STL"),
    ("3mf", "3MF"),
    ("obj", "OBJ"),
    ("ply", "PLY"),
    ("amf", "AMF"),
    ("off", "OFF"),
];

/// What the dialog shows.
pub struct SaveRequest {
    pub title: String,
    /// The folder it opens in; None leaves it to the system.
    pub folder: Option<PathBuf>,
    pub file_name: Option<String>,
    pub types: Vec<Filter>,
    /// The type selected when it opens, an index into `types`.
    pub selected: usize,
    /// Let the system ask before the file picked replaces one that is there. Only the Windows dialog
    /// can leave the question out; the plugin's dialogs on macOS and Linux always ask.
    #[cfg(windows)]
    pub confirm_replace: bool,
}

/// The file picked and the type selected when the dialog closed (an index into the request's types).
pub struct Saved {
    pub path: PathBuf,
    pub type_index: usize,
}

/// The index in `EXPORT_TYPES` of the extension `id` (any case), else STL's.
pub fn type_index(id: &str) -> usize {
    EXPORT_TYPES
        .iter()
        .position(|(ext, _)| ext.eq_ignore_ascii_case(id))
        .unwrap_or(0)
}

/// The Export dialog for parts whose format is `format` (an extension), asking before it replaces
/// a file (commands::pick_export_file leaves that out for one file per part).
pub fn export_request(
    title: String,
    folder: Option<PathBuf>,
    file_name: Option<String>,
    format: &str,
) -> SaveRequest {
    SaveRequest {
        title,
        folder,
        file_name,
        types: EXPORT_TYPES
            .iter()
            .map(|&(ext, name)| Filter {
                name: format!("{name} (*.{ext})"),
                extensions: vec![ext],
            })
            .collect(),
        selected: type_index(format),
        #[cfg(windows)]
        confirm_replace: true,
    }
}

/// The type of the file picked: the one its extension names (any case), else the one selected.
fn type_of(path: &Path, types: &[Filter], selected: usize) -> usize {
    let ext = path.extension().and_then(|e| e.to_str());
    types
        .iter()
        .position(|f| {
            f.extensions
                .iter()
                .any(|want| ext.is_some_and(|ext| ext.eq_ignore_ascii_case(want)))
        })
        .unwrap_or(selected)
}

/// Shows the dialog over `window` and waits until it closes: the file picked, or None when it was
/// cancelled. Blocks, so call it off the main thread.
pub fn show(window: &Window, request: &SaveRequest) -> Result<Option<Saved>, String> {
    let selected = request.selected.min(request.types.len().saturating_sub(1));
    #[cfg(windows)]
    let picked = windows_dialog::show(window, request, selected)?;
    #[cfg(not(windows))]
    let picked = plugin_show(window, request);
    Ok(picked.map(|(path, chosen)| {
        let chosen = chosen.filter(|&i| i < request.types.len());
        let type_index = chosen.unwrap_or_else(|| type_of(&path, &request.types, selected));
        Saved { path, type_index }
    }))
}

/// macOS and Linux: the dialog plugin, which selects its first type and reports none.
#[cfg(not(windows))]
fn plugin_show(window: &Window, request: &SaveRequest) -> Option<(PathBuf, Option<usize>)> {
    use tauri_plugin_dialog::DialogExt;

    let mut dialog = window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(&request.title);
    for filter in &request.types {
        dialog = dialog.add_filter(&filter.name, &filter.extensions);
    }
    if let Some(folder) = &request.folder {
        dialog = dialog.set_directory(folder);
    }
    if let Some(name) = &request.file_name {
        dialog = dialog.set_file_name(name);
    }
    let path = dialog.blocking_save_file()?.into_path().ok()?;
    Some((path, None))
}

#[cfg(windows)]
mod windows_dialog {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    use tauri::Window;
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
        CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileSaveDialog, IFileSaveDialog, IShellItem,
        SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::{HSTRING, PCWSTR};

    use super::SaveRequest;

    /// Balances a successful CoInitializeEx on this thread.
    pub(super) struct Apartment;

    impl Apartment {
        /// The single-threaded apartment the dialog needs; None when the thread already joined
        /// another kind, which then stays as it is.
        pub(super) fn enter() -> Option<Apartment> {
            // SAFETY: plain COM initialization of the calling thread; balanced in Drop.
            let hr =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            hr.is_ok().then_some(Apartment)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: matches the successful CoInitializeEx in `enter`, on the same thread.
            unsafe { CoUninitialize() }
        }
    }

    fn failed(e: windows::core::Error) -> String {
        format!("Could not show the save dialog ({}).", e.message())
    }

    /// The dialog with everything in `request` set, not shown yet. Call it inside an Apartment.
    pub(super) fn prepare(
        request: &SaveRequest,
        selected: usize,
    ) -> windows::core::Result<IFileSaveDialog> {
        let names: Vec<HSTRING> = request
            .types
            .iter()
            .map(|f| HSTRING::from(f.name.as_str()))
            .collect();
        let specs: Vec<HSTRING> = request
            .types
            .iter()
            .map(|f| {
                let patterns: Vec<String> = f.extensions.iter().map(|e| format!("*.{e}")).collect();
                HSTRING::from(patterns.join(";"))
            })
            .collect();
        let filters: Vec<COMDLG_FILTERSPEC> = names
            .iter()
            .zip(&specs)
            .map(|(name, spec)| COMDLG_FILTERSPEC {
                pszName: PCWSTR(name.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            })
            .collect();
        // SAFETY: COM calls on an interface this thread created; the strings outlive the calls,
        // and the dialog copies them.
        unsafe {
            let dialog: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetFileTypes(&filters)?;
            // The list counts from 1.
            dialog.SetFileTypeIndex(selected as u32 + 1)?;
            if let Some(ext) = request
                .types
                .get(selected)
                .and_then(|f| f.extensions.first())
            {
                dialog.SetDefaultExtension(&HSTRING::from(*ext))?;
            }
            dialog.SetTitle(&HSTRING::from(request.title.as_str()))?;
            if let Some(name) = &request.file_name {
                dialog.SetFileName(&HSTRING::from(name.as_str()))?;
            }
            if let Some(folder) = &request.folder {
                // A folder that cannot be resolved leaves the system's choice.
                let item: windows::core::Result<IShellItem> =
                    SHCreateItemFromParsingName(&HSTRING::from(folder.as_os_str()), None);
                if let Ok(item) = item {
                    dialog.SetFolder(&item)?;
                }
            }
            let options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
            dialog.SetOptions(if request.confirm_replace {
                options | FOS_OVERWRITEPROMPT
            } else {
                options & !FOS_OVERWRITEPROMPT
            })?;
            Ok(dialog)
        }
    }

    /// The file picked and the type selected (an index), or None when cancelled.
    pub(super) fn show(
        window: &Window,
        request: &SaveRequest,
        selected: usize,
    ) -> Result<Option<(PathBuf, Option<usize>)>, String> {
        let _apartment = Apartment::enter();
        let dialog = prepare(request, selected).map_err(failed)?;
        let owner = window.hwnd().ok();
        // SAFETY: COM calls on the dialog made above, on this thread; the path string the item
        // hands out is freed once copied.
        unsafe {
            if let Err(e) = dialog.Show(owner) {
                return if e.code() == ERROR_CANCELLED.to_hresult() {
                    Ok(None)
                } else {
                    Err(failed(e))
                };
            }
            let item = dialog.GetResult().map_err(failed)?;
            let index = dialog.GetFileTypeIndex().map_err(failed)?;
            let text = item.GetDisplayName(SIGDN_FILESYSPATH).map_err(failed)?;
            let path = PathBuf::from(OsString::from_wide(text.as_wide()));
            CoTaskMemFree(Some(text.0 as *const _));
            Ok(Some((path, (index as usize).checked_sub(1))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_list_is_stl_3mf_obj_ply_amf_off() {
        let request = export_request("Export".into(), None, None, "obj");
        let names: Vec<&str> = request.types.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "STL (*.stl)",
                "3MF (*.3mf)",
                "OBJ (*.obj)",
                "PLY (*.ply)",
                "AMF (*.amf)",
                "OFF (*.off)"
            ]
        );
        let exts: Vec<&str> = request.types.iter().map(|f| f.extensions[0]).collect();
        assert_eq!(exts, EXPORT_TYPES.map(|(ext, _)| ext));
    }

    #[test]
    fn the_parts_own_format_is_selected_else_stl() {
        for (format, want) in [
            ("stl", 0),
            ("3mf", 1),
            ("OBJ", 2),
            ("ply", 3),
            ("amf", 4),
            ("off", 5),
            ("step", 0),
            ("", 0),
        ] {
            assert_eq!(
                export_request("Export".into(), None, None, format).selected,
                want,
                "{format}"
            );
        }
    }

    #[test]
    fn the_dialog_opens_on_the_source_files_name_folder_and_format() {
        let folder = PathBuf::from(r"C:\models");
        let request = export_request(
            "Export 1 part".into(),
            Some(folder.clone()),
            Some("gun.stl".into()),
            "stl",
        );
        assert_eq!(request.folder.as_deref(), Some(folder.as_path()));
        assert_eq!(request.file_name.as_deref(), Some("gun.stl"));
        assert_eq!(request.selected, 0);
        // So saving it as it is replaces the source file, which the system asks about.
        #[cfg(windows)]
        assert!(request.confirm_replace);
    }

    #[test]
    fn the_extension_picked_decides_the_type_else_the_one_selected() {
        let types = export_request("Export".into(), None, None, "stl").types;
        assert_eq!(type_of(Path::new(r"C:\o\gun.OBJ"), &types, 0), 2);
        assert_eq!(type_of(Path::new("/o/gun.3mf"), &types, 0), 1);
        assert_eq!(type_of(Path::new("/o/gun"), &types, 4), 4);
        assert_eq!(type_of(Path::new("/o/gun.txt"), &types, 5), 5);
    }

    /// The real Windows dialog, set up but not shown: the type the part came in is selected, the
    /// name and folder are filled in, and the system asks before replacing (one file) or leaves it
    /// to the window (one file per part).
    #[cfg(windows)]
    #[test]
    fn the_windows_dialog_selects_the_parts_format_and_asks_before_replacing() {
        use windows::Win32::UI::Shell::{FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH};

        let folder = std::env::temp_dir()
            .join("tricky3d-tests")
            .join(format!("save-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let _apartment = windows_dialog::Apartment::enter();
        for (format, each, index) in [("obj", false, 3u32), ("stl", false, 1), ("off", true, 6)] {
            // As commands::pick_export_file asks for it.
            let request = SaveRequest {
                confirm_replace: !each,
                ..export_request(
                    "Export".into(),
                    Some(folder.clone()),
                    Some(format!("gun.{format}")),
                    format,
                )
            };
            let dialog = windows_dialog::prepare(&request, request.selected).unwrap();
            // SAFETY: reads back state from the dialog made above, on this thread.
            unsafe {
                assert_eq!(dialog.GetFileTypeIndex().unwrap(), index, "{format}");
                let replace = (dialog.GetOptions().unwrap() & FOS_OVERWRITEPROMPT).0 != 0;
                assert_eq!(replace, !each, "{format}");
                let name = dialog.GetFileName().unwrap();
                assert_eq!(name.to_string().unwrap(), format!("gun.{format}"));
                windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as *const _));
                let shown = dialog.GetFolder().unwrap();
                let text = shown.GetDisplayName(SIGDN_FILESYSPATH).unwrap();
                assert_eq!(PathBuf::from(text.to_string().unwrap()), folder);
                windows::Win32::System::Com::CoTaskMemFree(Some(text.0 as *const _));
            }
        }
        let _ = std::fs::remove_dir_all(&folder);
    }
}
