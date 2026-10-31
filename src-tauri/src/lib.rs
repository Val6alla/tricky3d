pub mod analysis;
pub mod arrange;
pub mod bundle;
mod commands;
pub mod formats;
pub mod grants;
pub mod jobs;
pub mod label;
pub mod library;
pub mod measure;
pub mod mesh;
pub mod migrate;
mod open_dialog;
pub mod ops;
mod power;
pub mod progress;
pub mod project;
pub mod repair;
mod save_dialog;
pub mod stats;
#[doc(hidden)]
pub mod test_dir;
pub mod workers;

use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::{AppHandle, DragDropEvent, Manager, Runtime, Window, WindowEvent};

use crate::grants::Grants;
use crate::jobs::{CutCache, RepairJobs};

const QUIT_ID: &str = "tricky3d-quit";

/// Tauri's default macOS menu with its Quit item swapped for one that closes the main window. The
/// built-in Quit terminates the app without a close event, so the window's unsaved-changes prompt
/// would never run. Built on every platform so it is type-checked everywhere, installed on macOS only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    let quit = MenuItem::with_id(
        app,
        QUIT_ID,
        format!("Quit {}", app.package_info().name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    // The first submenu is the app menu; its built-in Quit is a predefined item titled "Quit …".
    if let Some(app_menu) = menu.items()?.first().and_then(|item| item.as_submenu()) {
        for item in app_menu.items()? {
            if let Some(builtin) = item.as_predefined_menuitem()
                && builtin.text()?.starts_with("Quit")
            {
                app_menu.remove(builtin)?;
            }
        }
        app_menu.append(&quit)?;
    }
    Ok(menu)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    if event.id() == QUIT_ID
        && let Some(window) = app.get_webview_window("main")
    {
        // The close goes through the window's close-requested handler, which asks about unsaved changes.
        if let Err(err) = window.close() {
            log_error(&format!("could not close the window: {err}"));
            app.exit(0);
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn log_error(message: &str) {
    eprintln!("tricky3d: {message}");
}

/// Files dropped on the window may be read (src/grants.rs). Tauri sends the drop to the page before it
/// calls this, but both run in the same main-thread callback, and the page's import request is only
/// handled on the main thread after it returns, so the grant is always in place first.
fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
        let grants = window.state::<Grants>();
        for path in paths.iter().filter(|p| p.is_file()) {
            grants.allow_read(path.clone());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    workers::init();
    // Dialog and fs only: the app must never get network, shell or updater plugins.
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(Grants::default())
        .manage(RepairJobs::default())
        .manage(CutCache::default())
        .manage(measure::MeasureCache::default())
        .manage(jobs::Fonts::default())
        .on_window_event(on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::import_mesh,
            commands::open_project,
            commands::export_mesh,
            commands::export_each,
            commands::save_project,
            commands::mesh_stats,
            commands::split_shells,
            commands::merge_meshes,
            commands::cut_meshes,
            commands::analyze_meshes,
            commands::pick_files,
            commands::pick_open_file,
            commands::pick_save_file,
            commands::pick_export_file,
            commands::repair_catalog,
            commands::repair_run,
            commands::repair_cancel,
            commands::arrange,
            commands::analyze_part,
            commands::packing_presets_load,
            commands::packing_presets_save,
            commands::repair_check,
            commands::repair_shells,
            commands::repair_cut_path,
            commands::repair_cut_reset,
            commands::repair_scripts_load,
            commands::repair_scripts_save,
            commands::repair_script_import,
            commands::repair_script_export,
            commands::library_catalog,
            commands::library_build,
            commands::library_pick_image,
            commands::label_fonts,
            commands::label_shape,
            commands::label_fit,
            commands::label_apply,
            commands::label_pick_csv,
            commands::measure_pick,
            commands::measure_fit,
            commands::measure_evaluate,
            commands::measure_reset,
            commands::platform_stats,
            commands::shell_generate,
            commands::shell_estimate,
            commands::export_text,
        ]);
    #[cfg(target_os = "macos")]
    let builder = builder.menu(macos_menu).on_menu_event(on_menu_event);
    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    // The window opens once the app runs, so the old app's settings are in place before it reads them.
    migrate::from_old_name(app.handle());
    app.run(|_, _| {});
}
