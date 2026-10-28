//! Tauri commands: the IPC contract in src/io/ipc.ts and src/repair/ipc.ts. Each command moves its
//! work onto a blocking thread (a sync command would run on the main thread and freeze the window;
//! only a few instant commands run in place) and leaves the work itself to src/jobs.rs; mesh work
//! runs below normal priority on at most all cores but one (src/workers.rs). The file commands
//! first check the path against src/grants.rs.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::ipc::{Channel, InvokeBody, Request, Response};
use tauri::{AppHandle, Emitter, Manager, State, Window};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder};

use crate::bundle::{self, Bundle};
use crate::formats::{ExportFormat, ImportFormat};
use crate::grants::Grants;
use crate::jobs::{self, CutCache, RepairCheck, RepairJobs};
use crate::measure::{self, MeasureCache};
use crate::mesh::MeshStats;
use crate::open_dialog;
use crate::progress::Progress;
use crate::repair::defaults::Catalog;
use crate::repair::scripts::{Imported, Script, StoredScripts};
use crate::workers;

/// `ImportProgress` in src/io/ipc.ts.
#[derive(Clone, Serialize)]
pub struct ImportProgress {
    stage: &'static str,
    fraction: f64,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The background task stopped unexpectedly ({e})."))?
}

/// A job that reads or changes meshes: on a thread of its own below normal priority, with its
/// parallel loops on the worker pool (src/workers.rs).
async fn heavy<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    blocking(move || workers::run(work)).await
}

fn progress_to(channel: Channel<ImportProgress>) -> Progress<'static> {
    Progress::new(move |stage, fraction| {
        // A closed window drops the channel; the import still finishes.
        let _ = channel.send(ImportProgress {
            stage: stage.name(),
            fraction,
        });
    })
}

/// A raw-body request (`invoke(cmd, ArrayBuffer)`) decoded as a bundle.
fn bundle_of(request: &Request<'_>, action: &str) -> Result<Bundle, String> {
    let wrong_form = || format!("Could not {action}: the mesh data arrived in the wrong form.");
    let fallback;
    let bytes: &[u8] = match request.body() {
        InvokeBody::Raw(bytes) => bytes,
        // If the webview blocks Tauri's ipc: protocol, Tauri falls back to postMessage, which
        // sends an ArrayBuffer as a JSON array of byte values.
        InvokeBody::Json(serde_json::Value::Array(values)) => {
            fallback = values
                .iter()
                .map(|v| v.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(wrong_form)?;
            &fallback
        }
        InvokeBody::Json(_) => return Err(wrong_form()),
    };
    bundle::decode(bytes).map_err(|reason| format!("Could not {action}: {reason}."))
}

/// The bundle's target path, for the grant check. A missing or blank path passes here and fails in
/// the job with its own message.
fn target_path(meta: &Map<String, Value>) -> Option<&Path> {
    meta.get("path").and_then(Value::as_str).map(Path::new)
}

#[tauri::command]
pub async fn import_mesh(
    grants: State<'_, Grants>,
    path: String,
    merge: bool,
    on_progress: Channel<ImportProgress>,
) -> Result<Response, String> {
    grants.check_read(Path::new(&path))?;
    heavy(move || jobs::import_mesh(Path::new(&path), merge, &mut progress_to(on_progress)))
        .await
        .map(Response::new)
}

#[tauri::command]
pub async fn open_project(
    grants: State<'_, Grants>,
    path: String,
    on_progress: Channel<ImportProgress>,
) -> Result<Response, String> {
    let file = PathBuf::from(&path);
    grants.check_read(&file)?;
    let bytes =
        heavy(move || jobs::open_project(Path::new(&path), &mut progress_to(on_progress))).await?;
    // The opened project is the document now, so Save may write it back, provided the user picked
    // it in the File › Open dialog: an imported or dropped 3MF opens too, but stays read-only.
    grants.adopt_project(&file);
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn export_mesh(grants: State<'_, Grants>, request: Request<'_>) -> Result<(), String> {
    let bundle = bundle_of(&request, "export")?;
    if let Some(path) = target_path(&bundle.meta) {
        let extension = bundle
            .meta
            .get("format")
            .and_then(Value::as_str)
            .and_then(ExportFormat::parse)
            .map_or("", ExportFormat::extension);
        grants.check_write(path, extension, "write")?;
    }
    heavy(move || jobs::export_mesh(bundle)).await
}

#[tauri::command]
pub async fn save_project(grants: State<'_, Grants>, request: Request<'_>) -> Result<(), String> {
    let bundle = bundle_of(&request, "save the project")?;
    if let Some(path) = target_path(&bundle.meta) {
        grants.check_write(path, "3mf", "save")?;
    }
    heavy(move || jobs::save_project(bundle)).await
}

#[tauri::command]
pub async fn mesh_stats(request: Request<'_>) -> Result<Vec<MeshStats>, String> {
    let bundle = bundle_of(&request, "measure the part")?;
    heavy(move || jobs::mesh_stats(bundle)).await
}

// ---------------------------------------------------------------------------------------------
// Mesh operations: bundles in, bundles out (src/io/ipc.ts, "Mesh operations").

#[tauri::command]
pub async fn split_shells(request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "split the part")?;
    heavy(move || jobs::split_shells(bundle))
        .await
        .map(Response::new)
}

#[tauri::command]
pub async fn merge_meshes(request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "merge the parts")?;
    heavy(move || jobs::merge_meshes(bundle))
        .await
        .map(Response::new)
}

#[tauri::command]
pub async fn cut_meshes(request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "cut the part")?;
    heavy(move || jobs::cut_meshes(bundle))
        .await
        .map(Response::new)
}

#[tauri::command]
pub async fn analyze_meshes(request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "measure the part")?;
    heavy(move || jobs::analyze_meshes(bundle))
        .await
        .map(Response::new)
}

// ---------------------------------------------------------------------------------------------
// File dialogs. They run here rather than through the dialog plugin's own commands so that the
// paths the user picks are granted (src/grants.rs) by code the webview cannot imitate.

/// `FileFilter` in src/io/ipc.ts, for save dialogs. Open dialogs use src/open_dialog.rs's own list.
#[derive(Deserialize)]
pub struct FileFilter {
    name: String,
    extensions: Vec<String>,
}

fn file_dialog(
    window: &Window,
    title: String,
    filters: &[FileFilter],
) -> FileDialogBuilder<tauri::Wry> {
    let mut dialog = window.dialog().file().set_parent(window).set_title(title);
    for filter in filters {
        let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(&filter.name, &extensions);
    }
    dialog
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The open dialog with every format tricky3D reads (src/open_dialog.rs). Each file picked may be
/// read; none when the dialog is cancelled.
#[tauri::command]
pub async fn pick_files(
    window: Window,
    title: String,
    multiple: bool,
) -> Result<Vec<String>, String> {
    // The dialog shows on the main thread and is waited for here.
    blocking(move || {
        let picked = open_dialog::pick(&window, &title, open_dialog::mesh_filters(), multiple)?;
        let grants = window.state::<Grants>();
        let mut out = Vec::with_capacity(picked.len());
        for path in picked {
            out.push(path_text(&path));
            grants.allow_read(path);
        }
        Ok(out)
    })
    .await
}

/// The File › Open dialog: the same formats, with a title Rust sets, so the window cannot dress an
/// import pick up as an Open pick. A 3MF picked here opens as a project and may be written once it
/// has (Grants::allow_project); any other mesh file may only be read, as the part of a new project.
#[tauri::command]
pub async fn pick_open_file(window: Window) -> Result<Option<String>, String> {
    blocking(move || {
        let Some(path) =
            open_dialog::pick(&window, "Open", open_dialog::mesh_filters(), false)?.pop()
        else {
            return Ok(None);
        };
        let text = path_text(&path);
        let grants = window.state::<Grants>();
        if ImportFormat::from_path(&path) == Some(ImportFormat::ThreeMf) {
            grants.allow_project(path);
        } else {
            grants.allow_read(path);
        }
        Ok(Some(text))
    })
    .await
}

/// The save dialog, opened at `default_path`: a file name, or a path beside a file the user already
/// picked or dropped. The path picked may be written; null when the dialog is cancelled.
#[tauri::command]
pub async fn pick_save_file(
    window: Window,
    grants: State<'_, Grants>,
    title: String,
    default_path: Option<String>,
    filters: Vec<FileFilter>,
) -> Result<Option<String>, String> {
    let mut dialog = file_dialog(&window, title, &filters);
    if let Some(default_path) = default_path.filter(|p| !p.trim().is_empty()) {
        // Rebuilt from its components so a path the window wrote with "/" uses "\" on Windows.
        let path: PathBuf = Path::new(&default_path).components().collect();
        // The folder is only used when a granted file lives there. Nothing about any other folder is
        // looked up: even checking whether a network path exists reaches out to that host.
        if let Some(parent) = path.parent()
            && parent.components().count() > 0
            && grants.knows_folder(parent)
        {
            dialog = dialog.set_directory(parent);
        }
        if let Some(name) = path.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
    }
    let app = window.app_handle().clone();
    blocking(move || {
        let Some(path) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else {
            return Ok(None);
        };
        let text = path_text(&path);
        app.state::<Grants>().allow_write(path);
        Ok(Some(text))
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Repair mode (src/repair/ipc.ts). Script files are read and written right here in Rust, from
// paths picked in dialogs Rust shows: the window never names a path, so nothing is granted.

/// Built in memory in an instant, so it runs right here.
#[tauri::command]
pub fn repair_catalog() -> Catalog {
    jobs::repair_catalog()
}

#[tauri::command]
pub async fn repair_run(window: Window, request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "run the repair")?;
    heavy(move || {
        let registry = window.state::<RepairJobs>();
        jobs::repair_run(bundle, &registry, &mut |progress| {
            // A closed window only misses the event; the run still finishes.
            let _ = window.emit(jobs::REPAIR_PROGRESS, progress);
        })
    })
    .await
    .map(Response::new)
}

/// Only sets a flag, so it runs right here.
#[tauri::command]
pub fn repair_cancel(registry: State<'_, RepairJobs>, job: u64) {
    registry.cancel(job);
}

#[tauri::command]
pub async fn repair_check(request: Request<'_>) -> Result<Vec<RepairCheck>, String> {
    let bundle = bundle_of(&request, "check the part")?;
    heavy(move || jobs::repair_check(bundle)).await
}

#[tauri::command]
pub async fn repair_shells(request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "list the shells")?;
    heavy(move || jobs::repair_shells(bundle))
        .await
        .map(Response::new)
}

#[tauri::command]
pub async fn repair_cut_path(window: Window, request: Request<'_>) -> Result<Response, String> {
    let bundle = bundle_of(&request, "show the cut line")?;
    heavy(move || jobs::repair_cut_path(bundle, &window.state::<CutCache>()))
        .await
        .map(Response::new)
}

/// Only drops a cached mesh, so it runs right here.
#[tauri::command]
pub fn repair_cut_reset(cache: State<'_, CutCache>) {
    cache.clear();
}

// ---------------------------------------------------------------------------------------------
// Part library (src/library/ipc.ts).

/// Built in memory in an instant, so it runs right here.
#[tauri::command]
pub fn library_catalog() -> Vec<crate::library::Primitive> {
    crate::library::catalog()
}

#[tauri::command]
pub async fn library_primitive(
    window: Window,
    id: String,
    params: Map<String, Value>,
    job: u64,
) -> Result<Response, String> {
    heavy(move || jobs::library_primitive(&id, &params, job, &window.state::<RepairJobs>()))
        .await
        .map(Response::new)
}

// ---------------------------------------------------------------------------------------------
// Measure mode (src/measure/ipc.ts).

#[tauri::command]
pub async fn measure_pick(window: Window, request: Request<'_>) -> Result<jobs::PickReply, String> {
    let bundle = bundle_of(&request, "measure there")?;
    heavy(move || jobs::measure_pick(bundle, &window.state::<MeasureCache>())).await
}

/// A few numbers, so it runs right here.
#[tauri::command]
pub fn measure_fit(
    kind: String,
    points: Vec<measure::Feature>,
) -> Result<measure::Feature, String> {
    jobs::measure_fit(&kind, &points)
}

/// A few numbers, so it runs right here.
#[tauri::command]
pub fn measure_evaluate(
    op: measure::Op,
    features: Vec<measure::Feature>,
    matrix: Vec<f64>,
) -> Result<measure::Evaluation, String> {
    let matrix: crate::mesh::Matrix = matrix
        .try_into()
        .map_err(|_| "Could not measure: the part's matrix is not 16 numbers.".to_string())?;
    measure::evaluate(op, &features, &matrix)
}

/// Only drops the cached meshes, so it runs right here.
#[tauri::command]
pub fn measure_reset(cache: State<'_, MeasureCache>) {
    cache.clear();
}

// ---------------------------------------------------------------------------------------------
// Platform statistics (src/stats/ipc.ts).

#[tauri::command]
pub async fn platform_stats(
    window: Window,
    request: Request<'_>,
) -> Result<jobs::StatsReply, String> {
    let bundle = bundle_of(&request, "work out the platform statistics")?;
    heavy(move || {
        let registry = window.state::<RepairJobs>();
        jobs::platform_stats(bundle, &registry, &mut |progress| {
            let _ = window.emit(jobs::STATS_PROGRESS, progress);
        })
    })
    .await
}

/// Export to CSV: `path` must be a file picked in the save dialog (pick_save_file).
#[tauri::command]
pub async fn export_text(
    grants: State<'_, Grants>,
    path: String,
    text: String,
) -> Result<(), String> {
    let path = PathBuf::from(path);
    grants.check_write(&path, "csv", "write")?;
    blocking(move || jobs::export_text(&path, &text)).await
}

/// The app's config folder, which holds the user's repair scripts. `action` starts the message.
fn config_dir(app: &AppHandle, action: &str) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("{action}: the settings folder cannot be found ({e})."))
}

#[tauri::command]
pub async fn repair_scripts_load(app: AppHandle) -> Result<Option<StoredScripts>, String> {
    let dir = config_dir(&app, "Could not read your repair scripts")?;
    blocking(move || jobs::load_scripts(&dir)).await
}

#[tauri::command]
pub async fn repair_scripts_save(app: AppHandle, data: StoredScripts) -> Result<(), String> {
    let dir = config_dir(&app, "Could not save your repair scripts")?;
    blocking(move || jobs::save_scripts(&dir, &data)).await
}

/// The open dialog for a script file; the file is read here. Null when the dialog is cancelled.
#[tauri::command]
pub async fn repair_script_import(window: Window) -> Result<Option<Imported>, String> {
    blocking(move || {
        let filters = open_dialog::script_filters();
        let Some(path) = open_dialog::pick(&window, "Import repair script", filters, false)?.pop()
        else {
            return Ok(None);
        };
        jobs::import_script(&path).map(Some)
    })
    .await
}

/// The save dialog for exporting `script`, which is written here. False when it is cancelled.
#[tauri::command]
pub async fn repair_script_export(window: Window, script: Script) -> Result<bool, String> {
    let dialog = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Export repair script")
        .add_filter("tricky3D script (*.json)", &["json"])
        .set_file_name(jobs::script_file_name(&script.name));
    blocking(move || {
        let Some(path) = dialog.blocking_save_file().and_then(|p| p.into_path().ok()) else {
            return Ok(false);
        };
        jobs::export_script(&path, &script).map(|()| true)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::formats::{ExportFormat, NamedMesh, write_export};
    use crate::grants::Grants;
    use crate::jobs;
    use crate::mesh::{IDENTITY, make_box};
    use crate::progress::Progress;
    use crate::project::{self, PartOut};

    use super::path_text;

    /// What a path picked in a dialog goes through: `path_text` sends it to the window as JSON, and
    /// the window sends the same text back as the file command's `path` argument.
    fn through_the_window(picked: &Path) -> String {
        let sent = serde_json::to_string(&path_text(picked)).unwrap();
        serde_json::from_str(&sent).unwrap()
    }

    #[test]
    fn picked_files_with_spaces_and_non_ascii_names_pass_the_grant_check_and_open() {
        let root = std::env::temp_dir()
            .join("tricky3d-tests")
            .join(format!("picked-{}", std::process::id()));
        // "й" spelled as macOS file names often store it: "и" plus a combining breve.
        let folder = root
            .join("Модели 3D")
            .join("Мои\u{438}\u{306} файлы (копия)");
        std::fs::create_dir_all(&folder).unwrap();
        let mesh = make_box([0.0; 3], [10.0; 3]);

        let stl = folder.join("деталь номер 1.STL");
        let bytes = write_export(
            std::io::Cursor::new(Vec::new()),
            ExportFormat::StlBinary,
            "деталь",
            &[NamedMesh {
                name: "деталь",
                mesh: &mesh,
            }],
        )
        .unwrap()
        .into_inner();
        std::fs::write(&stl, bytes).unwrap();

        let project_file = folder.join("плита и стол.3MF");
        let part = PartOut {
            name: "деталь",
            matrix: IDENTITY,
            color: None,
            visible: true,
            transparent: false,
            locked: false,
            measurements: None,
            mesh: &mesh,
        };
        project::save_file(&project_file, [200.0; 3], &[part]).unwrap();

        let grants = Grants::default();
        // Picked in Import part or File › Open (a mesh): readable.
        grants.allow_read(stl.clone());
        let arg = through_the_window(&stl);
        assert_eq!(PathBuf::from(&arg), stl);
        grants.check_read(Path::new(&arg)).unwrap();
        jobs::import_mesh(Path::new(&arg), false, &mut Progress::silent()).unwrap();

        // Picked in File › Open (a 3MF): readable, and writable once it opened as a project.
        grants.allow_project(project_file.clone());
        let arg = through_the_window(&project_file);
        grants.check_read(Path::new(&arg)).unwrap();
        jobs::open_project(Path::new(&arg), &mut Progress::silent()).unwrap();
        grants.adopt_project(Path::new(&arg));
        grants.check_write(Path::new(&arg), "3mf", "save").unwrap();

        let _ = std::fs::remove_dir_all(&root);
    }
}
