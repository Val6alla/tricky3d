//! The work behind each Tauri command as plain functions. src/commands.rs only moves it onto a
//! background thread and converts IPC types, so tests run exactly what the app runs.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::bundle::{self, Blob, BlobRef, Bundle, Extra, InMesh, OutMesh};
use crate::formats::{self, ExportFormat, NamedMesh, display_name};
use crate::measure;
use crate::mesh::{
    Analysis, IDENTITY, Matrix, Mesh, MeshStats, analyze, analyze_unless_cancelled, bake,
    join_coincident, stats,
};
use crate::ops;
use crate::progress::{Progress, Stage};
use crate::project::{self, PartOut};
use crate::repair::scripts::{self, Imported, Script, Step, StoredScripts};
use crate::repair::status::{self, StatusDetail};
use crate::repair::{
    self, Action, ActionId, Control, RepairError, ScriptOutcome, defaults, shells,
};
use crate::stats;

/// `import_mesh`: reads, welds and measures a file; returns the bundle described in src/io/ipc.ts.
pub fn import_mesh(path: &Path, merge: bool, progress: &mut Progress) -> Result<Vec<u8>, String> {
    let imported = formats::import_file(path, merge, progress)?;
    let analyses = analyze_all(imported.parts.iter().map(|p| &p.mesh), progress);
    let mut meta = Map::new();
    meta.insert("format".into(), json!(imported.format.id()));
    meta.insert("objectCount".into(), json!(imported.object_count));
    meta.insert("splittable".into(), json!(imported.splittable));
    meta.insert("warnings".into(), json!(imported.warnings));
    let meshes: Vec<OutMesh> = imported
        .parts
        .iter()
        .zip(&analyses)
        .map(|(part, analysis)| {
            let mut info = Map::new();
            info.insert("name".into(), json!(part.name));
            if let Some(matrix) = &part.matrix {
                info.insert("matrix".into(), json!(matrix.to_vec()));
            }
            info.insert("stats".into(), json!(analysis.stats));
            out_mesh(info, &part.mesh, analysis)
        })
        .collect();
    Ok(bundle::encode(&meta, &meshes))
}

/// `open_project`: reads a project 3MF with its parts' display fields and the platform size.
pub fn open_project(path: &Path, progress: &mut Progress) -> Result<Vec<u8>, String> {
    let project = project::open_file(path, progress)?;
    let analyses = analyze_all(project.parts.iter().map(|p| &p.mesh), progress);
    let mut meta = Map::new();
    meta.insert("platform".into(), json!(project.platform));
    meta.insert("warnings".into(), json!(project.warnings));
    let meshes: Vec<OutMesh> = project
        .parts
        .iter()
        .zip(&analyses)
        .map(|(part, analysis)| {
            let mut info = Map::new();
            info.insert("name".into(), json!(part.name));
            info.insert("matrix".into(), json!(part.matrix.to_vec()));
            if let Some(color) = &part.color {
                info.insert("color".into(), json!(color));
            }
            for (key, value) in [
                ("visible", part.visible),
                ("transparent", part.transparent),
                ("locked", part.locked),
            ] {
                if let Some(value) = value {
                    info.insert(key.into(), json!(value));
                }
            }
            if let Some(measurements) = &part.measurements {
                info.insert("measurements".into(), json!(measurements));
            }
            info.insert("stats".into(), json!(analysis.stats));
            out_mesh(info, &part.mesh, analysis)
        })
        .collect();
    Ok(bundle::encode(&meta, &meshes))
}

fn out_mesh<'a>(info: Map<String, Value>, mesh: &'a Mesh, analysis: &'a Analysis) -> OutMesh<'a> {
    OutMesh {
        info,
        positions: &mesh.positions,
        indices: &mesh.indices,
        edges: Some(&analysis.problem_edges),
    }
}

/// Measures every mesh, reporting the analyze stage weighted by triangle count.
fn analyze_all<'a>(
    meshes: impl Iterator<Item = &'a Mesh>,
    progress: &mut Progress,
) -> Vec<Analysis> {
    let meshes: Vec<&Mesh> = meshes.collect();
    let total = meshes
        .iter()
        .map(|m| m.triangle_count())
        .sum::<usize>()
        .max(1) as f64;
    let mut done = 0.0;
    let mut out = Vec::with_capacity(meshes.len());
    for mesh in meshes {
        let share = mesh.triangle_count() as f64 / total;
        out.push(analyze(mesh, &mut |f| {
            progress.report(Stage::Analyze, done + f * share)
        }));
        done += share;
    }
    progress.report(Stage::Analyze, 1.0);
    out
}

/// `export_mesh`: bakes each part's matrix into its vertices and writes one file.
pub fn export_mesh(bundle: Bundle) -> Result<(), String> {
    let path = path_field(&bundle.meta)?;
    let file = display_name(&path);
    let format = bundle
        .meta
        .get("format")
        .and_then(Value::as_str)
        .and_then(ExportFormat::parse)
        .ok_or_else(|| format!("Could not write \"{file}\": the export format is unknown."))?;
    let parts = parts_from(&bundle, &file)?;
    let baked: Vec<Mesh> = parts
        .iter()
        .map(|(_, matrix, mesh)| bake(mesh, matrix))
        .collect();
    let named: Vec<NamedMesh> = parts
        .iter()
        .zip(&baked)
        .map(|((name, _, _), mesh)| NamedMesh { name, mesh })
        .collect();
    formats::export_file(&path, format, &named)
}

/// `save_project`: writes a project 3MF with each part's matrix kept as its build transform.
pub fn save_project(bundle: Bundle) -> Result<(), String> {
    let path = path_field(&bundle.meta)?;
    let file = display_name(&path);
    let platform = bundle
        .meta
        .get("platform")
        .and_then(Value::as_array)
        .and_then(|a| {
            match a
                .iter()
                .map(Value::as_f64)
                .collect::<Option<Vec<f64>>>()?
                .as_slice()
            {
                &[x, y, z] if [x, y, z].iter().all(|v| v.is_finite() && *v > 0.0) => {
                    Some([x, y, z])
                }
                _ => None,
            }
        })
        .ok_or_else(|| format!("Could not save \"{file}\": the platform size is not valid."))?;
    let parts = parts_from(&bundle, &file)?;
    let out: Vec<PartOut> = parts
        .iter()
        .zip(&bundle.meshes)
        .map(|((name, matrix, mesh), m)| PartOut {
            name,
            matrix: *matrix,
            color: m.info.get("color").and_then(Value::as_str),
            visible: m
                .info
                .get("visible")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            transparent: m
                .info
                .get("transparent")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            locked: m
                .info
                .get("locked")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            measurements: m.info.get("measurements").and_then(Value::as_str),
            mesh,
        })
        .collect();
    project::save_file(&path, platform, &out)
}

/// `mesh_stats`: measures each mesh in order.
pub fn mesh_stats(bundle: Bundle) -> Result<Vec<MeshStats>, String> {
    bundle
        .meshes
        .iter()
        .map(|m| {
            m.mesh
                .check()
                .map_err(|reason| format!("Could not measure the part: {reason}."))?;
            Ok(stats(&m.mesh))
        })
        .collect()
}

fn path_field(meta: &Map<String, Value>) -> Result<PathBuf, String> {
    match meta.get("path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => Ok(PathBuf::from(path)),
        _ => Err("Could not save: no file was chosen.".into()),
    }
}

/// (name, matrix, mesh) per part of a bundle, each mesh checked.
fn parts_from<'b>(
    bundle: &'b Bundle,
    file: &str,
) -> Result<Vec<(String, Matrix, &'b Mesh)>, String> {
    bundle
        .meshes
        .iter()
        .enumerate()
        .map(|(n, m)| {
            let (name, matrix) = checked_part(m, n)
                .map_err(|reason| format!("Could not write \"{file}\": {reason}."))?;
            Ok((name, matrix, &m.mesh))
        })
        .collect()
}

/// A bundle mesh's part name and matrix (missing = identity), after `Mesh::check`. The error is a
/// reason clause naming the part, for the caller's sentence.
fn checked_part(m: &InMesh, n: usize) -> Result<(String, Matrix), String> {
    let name = formats::clean_name(m.info.get("name").and_then(Value::as_str))
        .unwrap_or_else(|| format!("Part {}", n + 1));
    let damaged = |reason: &str| format!("the data for \"{name}\" is damaged ({reason})");
    m.mesh.check().map_err(|reason| damaged(&reason))?;
    let matrix = match m.info.get("matrix") {
        None | Some(Value::Null) => IDENTITY,
        Some(Value::Array(values)) => {
            let values: Option<Vec<f64>> = values
                .iter()
                .map(|v| v.as_f64().filter(|v| v.is_finite()))
                .collect();
            values
                .and_then(|v| Matrix::try_from(v.as_slice()).ok())
                .ok_or_else(|| damaged("its matrix is not 16 numbers"))?
        }
        Some(_) => return Err(damaged("its matrix is not 16 numbers")),
    };
    Ok((name, matrix))
}

// ---------------------------------------------------------------------------------------------
// Mesh operations. Each takes one bundle mesh per part with info
// { "name", "matrix" }, works in world space, and returns results with an identity matrix.

/// Every mesh of an operation's bundle in world space: checked, then its matrix baked in.
/// `action` starts the error sentence, such as "Could not split the part".
fn world_meshes(bundle: Bundle, action: &str, join: bool) -> Result<Vec<Mesh>, String> {
    bundle
        .meshes
        .into_iter()
        .enumerate()
        .map(|(n, m)| {
            let (_, matrix) =
                checked_part(&m, n).map_err(|reason| format!("{action}: {reason}."))?;
            // The window's arrays already are world space for an identity matrix: no copy.
            if matrix == IDENTITY {
                return Ok(m.mesh);
            }
            let baked = bake(&m.mesh, &matrix);
            Ok(match join.then(|| join_coincident(&baked)).flatten() {
                Some(joined) => joined,
                None => baked,
            })
        })
        .collect()
}

/// `split_shells`: meta out {"counts": [shells per input]}; meshes out: the shells of every input
/// with two or more, in input order then shell order (largest first), info {"source": i, "stats"}
/// plus problem edges. An input with one shell returns none.
pub fn split_shells(bundle: Bundle) -> Result<Vec<u8>, String> {
    let meshes = world_meshes(bundle, "Could not split the part", false)?;
    let mut counts = Vec::with_capacity(meshes.len());
    let mut results = Vec::new();
    for (i, mesh) in meshes.iter().enumerate() {
        let found = ops::shells::split_shells(mesh);
        counts.push(found.len());
        if found.len() >= 2 {
            results.extend(found.into_iter().map(|m| (source_info(i), m)));
        }
    }
    let mut meta = Map::new();
    meta.insert("counts".into(), json!(counts));
    Ok(encode_results(&meta, &results))
}

/// `merge_meshes`: Merge parts. Two or more meshes in; one mesh out, the baked inputs concatenated
/// in the given order without welding one to another, info {"stats"} plus problem edges. Within
/// an input, vertices its bake moved onto one position are joined (`join_coincident`), so the
/// result holds each part as import would have read it from a file in world space.
pub fn merge_meshes(bundle: Bundle) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not merge the parts";
    if bundle.meshes.len() < 2 {
        return Err(format!("{ACTION}: at least two parts are needed."));
    }
    let meshes = world_meshes(bundle, ACTION, true)?;
    let refs: Vec<&Mesh> = meshes.iter().collect();
    let merged = ops::concat(&refs).map_err(|reason| format!("{ACTION}: {reason}."))?;
    drop(meshes); // Before measuring and encoding: a big merge would otherwise be held twice.
    Ok(encode_results(&Map::new(), &[(Map::new(), merged)]))
}

/// `cut_meshes`: meta in {"normal": [x, y, z], "offset", "cap": bool, "gap": mm >= 0 (optional)};
/// the plane is dot(normal, p) = offset, normalised here. Out meta {"openOutline": [bool per
/// input]}; meshes: for each input the plane crosses, its piece below the plane, then the piece
/// above (moved gap mm along the normal), info {"source": i, "side": "below" | "above", "stats"}
/// plus problem edges. An input with all of it on one side returns no pieces: it stays as it is.
pub fn cut_meshes(bundle: Bundle) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not cut the part";
    let (plane, cap, gap) = cut_settings(&bundle.meta)
        .ok_or_else(|| format!("{ACTION}: the cut settings are not valid."))?;
    let meshes = world_meshes(bundle, ACTION, false)?;
    let mut open = Vec::with_capacity(meshes.len());
    let mut results = Vec::new();
    for (i, mesh) in meshes.iter().enumerate() {
        let pieces = ops::cut::cut(mesh, &plane, cap);
        open.push(pieces.open_outline);
        // A part the plane does not cross stays as it is, so its one side (the whole part) would
        // only be measured and sent back for nothing.
        let (Some(below), Some(above)) = (pieces.below, pieces.above) else {
            continue;
        };
        for (side, piece) in [("below", below), ("above", above)] {
            let mut piece = piece;
            if side == "above" && gap > 0.0 {
                ops::translate(&mut piece, plane.normal.map(|v| v * gap));
            }
            let mut info = source_info(i);
            info.insert("side".into(), json!(side));
            results.push((info, piece));
        }
    }
    let mut meta = Map::new();
    meta.insert("openOutline".into(), json!(open));
    Ok(encode_results(&meta, &results))
}

/// The plane, cap flag and gap of a cut request; None when any of them is missing or unusable.
fn cut_settings(meta: &Map<String, Value>) -> Option<(ops::cut::Plane, bool, f64)> {
    let normal: Vec<f64> = meta
        .get("normal")?
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect::<Option<_>>()?;
    let &[x, y, z] = normal.as_slice() else {
        return None;
    };
    let offset = meta.get("offset")?.as_f64()?;
    let cap = meta.get("cap")?.as_bool()?;
    let gap = match meta.get("gap") {
        None | Some(Value::Null) => 0.0,
        Some(v) => v.as_f64()?,
    };
    let length = (x * x + y * y + z * z).sqrt();
    if !(length.is_finite() && length > 0.0 && offset.is_finite() && gap.is_finite() && gap >= 0.0)
    {
        return None;
    }
    // Scaling the normal to unit length scales the offset with it: the same plane.
    let plane = ops::cut::Plane {
        normal: [x / length, y / length, z / length],
        offset: offset / length,
    };
    Some((plane, cap, gap))
}

/// `analyze_meshes`: stats and problem edges for meshes that already are world space (any matrix
/// is ignored), such as a boolean's result. Per input info {"stats"} plus problem edges, with
/// empty positions and indices: the window keeps its own arrays.
pub fn analyze_meshes(bundle: Bundle) -> Result<Vec<u8>, String> {
    for (n, m) in bundle.meshes.iter().enumerate() {
        checked_part(m, n).map_err(|reason| format!("Could not measure the part: {reason}."))?;
    }
    let analyses = analyze_all(
        bundle.meshes.iter().map(|m| &m.mesh),
        &mut Progress::silent(),
    );
    let out: Vec<OutMesh> = analyses
        .iter()
        .map(|a| {
            let mut info = Map::new();
            info.insert("stats".into(), json!(a.stats));
            OutMesh {
                info,
                positions: &[],
                indices: &[],
                edges: Some(&a.problem_edges),
            }
        })
        .collect();
    Ok(bundle::encode(&Map::new(), &out))
}

fn source_info(i: usize) -> Map<String, Value> {
    let mut info = Map::new();
    info.insert("source".into(), json!(i));
    info
}

/// Encodes result meshes with their stats and problem edges added, like import.
fn encode_results(meta: &Map<String, Value>, results: &[(Map<String, Value>, Mesh)]) -> Vec<u8> {
    let analyses = analyze_all(results.iter().map(|(_, m)| m), &mut Progress::silent());
    let out: Vec<OutMesh> = results
        .iter()
        .zip(&analyses)
        .map(|((info, mesh), analysis)| {
            let mut info = info.clone();
            info.insert("stats".into(), json!(analysis.stats));
            out_mesh(info, mesh, analysis)
        })
        .collect();
    bundle::encode(meta, &out)
}

// ---------------------------------------------------------------------------------------------
// Repair mode (src/repair/ipc.ts). The window keeps the working copy and sends it with
// every request; nothing about a repair session stays here but the cancel flags of running jobs.

/// The Tauri event a repair run reports its progress with.
pub const REPAIR_PROGRESS: &str = "repair-progress";

/// What a cancelled repair run fails with, exactly: the window takes it as a cancel, not an error.
pub const CANCELLED: &str = "cancelled";

/// Shortest time between two progress events of a run (about 30 a second); each action's start
/// is sent regardless.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(33);

/// `RepairProgress` in src/repair/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RepairProgress {
    pub job: u64,
    /// 0-based index of the running action, and the run's action count.
    pub step: usize,
    pub steps: usize,
    /// The running action's label.
    pub label: String,
    /// The step the running action is on ("Finding intersections"), for actions that name theirs.
    pub detail: Option<String>,
    /// 0..1 over the running action alone.
    pub local: f64,
    /// 0..1 over the whole run, never decreasing.
    pub fraction: f64,
}

/// `RepairCheck` in src/repair/types.ts: what a part's damage badge needs.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairCheck {
    pub closed: bool,
    pub oriented: bool,
    pub holes: u64,
    pub border_edges: u64,
    pub non_manifold_edges: u64,
    pub inverted_triangles: u64,
}

/// The cancel flags of the repair runs in progress, by job number (Tauri managed state). A cancel
/// can arrive before its run starts, while a big working copy is still on its way: a job number
/// above every one started so far is then remembered as cancelled until that job starts.
#[derive(Default)]
pub struct RepairJobs {
    table: Mutex<JobTable>,
}

#[derive(Default)]
struct JobTable {
    running: HashMap<u64, Arc<AtomicBool>>,
    /// Jobs cancelled before they started.
    early: HashSet<u64>,
    /// The highest job number started so far.
    highest: Option<u64>,
}

/// A registered run: its cancel flag, unregistered when this is dropped (the run ended, however).
pub struct RunningJob<'a> {
    jobs: &'a RepairJobs,
    job: u64,
    cancel: Arc<AtomicBool>,
}

impl RepairJobs {
    fn table(&self) -> MutexGuard<'_, JobTable> {
        // The table stays valid even if a thread panicked while holding the lock.
        self.table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Registers the run `job`, already cancelled when its cancel came first.
    pub fn start(&self, job: u64) -> RunningJob<'_> {
        let mut table = self.table();
        table.highest = Some(table.highest.map_or(job, |h| h.max(job)));
        let early = table.early.remove(&job);
        // Early cancels of lower numbers belong to runs that never started.
        table.early.retain(|&j| j > job);
        let cancel = table.running.entry(job).or_default().clone();
        if early {
            cancel.store(true, Ordering::Relaxed);
        }
        RunningJob {
            jobs: self,
            job,
            cancel,
        }
    }

    /// `repair_cancel`: asks the run `job` to stop. A job that already ended is ignored.
    pub fn cancel(&self, job: u64) {
        let mut table = self.table();
        if let Some(flag) = table.running.get(&job) {
            flag.store(true, Ordering::Relaxed);
        } else if table.highest.is_none_or(|h| job > h) {
            table.early.insert(job);
        }
    }

    /// True when no run is registered and no early cancel waits.
    pub fn is_idle(&self) -> bool {
        let table = self.table();
        table.running.is_empty() && table.early.is_empty()
    }
}

impl RunningJob<'_> {
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }
}

impl Drop for RunningJob<'_> {
    fn drop(&mut self) {
        self.jobs.table().running.remove(&self.job);
    }
}

/// `repair_catalog`: every action with its parameters, and the built-in scripts.
pub fn repair_catalog() -> defaults::Catalog {
    defaults::catalog()
}

/// A repair_run request, checked.
struct RunRequest {
    job: u64,
    label: String,
    /// The enabled steps as actions, each with its label.
    actions: Vec<(Action, String)>,
    with_status: bool,
    mesh: Mesh,
    selection: Vec<u32>,
}

/// Reads and checks a repair_run request: meta {job, label, actions, withStatus}, one mesh with
/// `extra.selection`. Every enabled step is parsed here, so a bad one fails the run before
/// anything runs.
fn run_request(bundle: Bundle) -> Result<RunRequest, String> {
    let damaged =
        || "Could not run the repair: the request from the window is damaged.".to_string();
    let Bundle { meta, meshes } = bundle;
    let job = meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let label = meta
        .get("label")
        .and_then(Value::as_str)
        .and_then(|l| formats::clean_name(Some(l)))
        .unwrap_or_else(|| "the repair".into());
    let with_status = match meta.get("withStatus") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or_else(damaged)?,
    };
    let list = match meta.get("actions") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(list)) => list.as_slice(),
        Some(_) => return Err(damaged()),
    };
    let mut steps: Vec<Step> = Vec::with_capacity(list.len());
    for (n, value) in list.iter().enumerate() {
        let step = Step::deserialize(value).map_err(|_| {
            let name = value.get("type").and_then(Value::as_str);
            match name.filter(|&name| ActionId::deserialize(&json!(name)).is_err()) {
                Some(name) => format!(
                    "Could not run {label}: step {} is \"{name}\", which tricky3D does not know.",
                    n + 1
                ),
                None => format!("Could not run {label}: step {} is damaged.", n + 1),
            }
        })?;
        steps.push(step);
    }
    let mut meshes = meshes.into_iter();
    let (Some(input), None) = (meshes.next(), meshes.next()) else {
        return Err(damaged());
    };
    input.mesh.check().map_err(|reason| {
        format!("Could not run {label}: the working copy is damaged ({reason}).")
    })?;
    let triangles = input.mesh.triangle_count();
    let selection = match input.extra("selection") {
        None => Vec::new(),
        Some(Blob::U32(ids))
            if ids.windows(2).all(|w| w[0] < w[1])
                && ids.last().is_none_or(|&t| (t as usize) < triangles) =>
        {
            ids.clone()
        }
        Some(_) => {
            return Err(format!(
                "Could not run {label}: the selection does not fit the working copy."
            ));
        }
    };
    let mut actions = Vec::with_capacity(steps.len());
    for (n, step) in steps.iter().enumerate().filter(|(_, s)| s.enabled) {
        let action = scripts::parse_action(step).map_err(|reason| {
            if steps.len() == 1 {
                format!("{reason}.")
            } else {
                format!("{reason} (step {} of {label}).", n + 1)
            }
        })?;
        actions.push((action, defaults::label_of(step.action).to_string()));
    }
    Ok(RunRequest {
        job,
        label,
        actions,
        with_status,
        mesh: input.mesh,
        selection,
    })
}

/// `repair_run`: runs the request's enabled actions on its working copy (a script, one action, or
/// none for the Update button) and answers with the bundle src/repair/ipc.ts reads: meta {changed,
/// selected, reports, status, warnings}; mesh 0 with the new arrays, stats and problem edges when
/// changed, empty arrays when not; `extra.selection` carried through the run (set by an action
/// when `selected`), the status highlights
/// (border, nonManifold, inverted, degenerate) when asked for, and `extra.intersecting` when an
/// action left a highlight. `emit` receives the progress events. A cancelled run fails with
/// exactly CANCELLED; the window keeps the working copy it sent.
pub fn repair_run(
    bundle: Bundle,
    jobs: &RepairJobs,
    emit: &mut dyn FnMut(&RepairProgress),
) -> Result<Vec<u8>, String> {
    let request = run_request(bundle)?;
    let running = jobs.start(request.job);
    // Windows would slow the run down as soon as the user switches to another app.
    let full_speed = crate::power::FullSpeed::start();
    let cancel = running.cancel_flag();
    let outcome = if request.actions.is_empty() {
        ScriptOutcome {
            mesh: request.mesh,
            changed: false,
            selection: request.selection,
            selected: false,
            reports: Vec::new(),
            highlight: None,
        }
    } else {
        let events = RefCell::new(ProgressEvents::new(request.job, &request.actions, emit));
        repair::run_script(
            &request.actions,
            &request.mesh,
            &request.selection,
            cancel,
            |step, _| events.borrow_mut().started(step),
            |step, fraction| events.borrow_mut().progress(step, fraction),
            |step, name| events.borrow_mut().stage(step, name),
        )
        .map_err(run_error)?
    };
    if outcome.changed {
        // A broken action must not hand the window indices it would choke on.
        outcome.mesh.check().map_err(|reason| {
            format!(
                "Could not run {}: a repair action produced a damaged mesh ({reason}).",
                request.label
            )
        })?;
    }
    let detail = if request.with_status {
        let mut control = Control::new(cancel, |_| {});
        Some(status::analyze(&outcome.mesh, &mut control).map_err(run_error)?)
    } else {
        None
    };
    // The run stays registered until its answer is built, so a cancel still reaches it.
    let answer = run_response(&outcome, detail.as_ref(), cancel);
    drop(full_speed);
    drop(running);
    answer
}

/// The command's error for a run that did not finish: CANCELLED, or the action's own sentence.
fn run_error(e: RepairError) -> String {
    match e {
        RepairError::Cancelled => CANCELLED.into(),
        RepairError::Failed(message) => message,
    }
}

/// The answer to a run, or CANCELLED when `cancel` is set while a changed working copy is measured
/// for it: on a big result that takes longer than many actions do.
fn run_response(
    outcome: &ScriptOutcome,
    detail: Option<&StatusDetail>,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    // Every warning of the run once, in order; the reports carry them per action too.
    let mut warnings: Vec<&str> = Vec::new();
    for w in outcome.reports.iter().flat_map(|r| &r.warnings) {
        if !warnings.contains(&w.as_str()) {
            warnings.push(w);
        }
    }
    let mut meta = Map::new();
    meta.insert("changed".into(), json!(outcome.changed));
    meta.insert("selected".into(), json!(outcome.selected));
    meta.insert("reports".into(), json!(outcome.reports));
    meta.insert(
        "status".into(),
        detail.map_or(Value::Null, |d| json!(d.status)),
    );
    meta.insert("warnings".into(), json!(warnings));
    // An unchanged working copy does not go back: the window keeps its own arrays.
    let analysis = match outcome.changed {
        true => Some(analyze_unless_cancelled(&outcome.mesh, cancel).ok_or(CANCELLED)?),
        false => None,
    };
    let mut info = Map::new();
    let out = match &analysis {
        Some(a) => {
            info.insert("stats".into(), json!(a.stats));
            OutMesh {
                info,
                positions: &outcome.mesh.positions,
                indices: &outcome.mesh.indices,
                edges: Some(&a.problem_edges),
            }
        }
        None => OutMesh {
            info,
            positions: &[],
            indices: &[],
            edges: None,
        },
    };
    let mut extra: Extra = vec![("selection", BlobRef::U32(&outcome.selection))];
    if let Some(d) = detail {
        extra.push(("border", BlobRef::F32(&d.border)));
        extra.push(("nonManifold", BlobRef::F32(&d.non_manifold)));
        extra.push(("inverted", BlobRef::U32(&d.inverted)));
        extra.push(("degenerate", BlobRef::U32(&d.degenerate)));
    }
    if let Some(highlight) = &outcome.highlight {
        extra.push(("intersecting", BlobRef::U32(highlight)));
    }
    Ok(bundle::encode_with_extra(&meta, &[out], &[extra]))
}

/// Turns run_script's per-action progress into `repair-progress` events over the whole run: one
/// when each action starts, otherwise at most one per PROGRESS_INTERVAL.
struct ProgressEvents<'a> {
    job: u64,
    labels: Vec<&'a str>,
    emit: &'a mut dyn FnMut(&RepairProgress),
    last: Option<Instant>,
    sent: f64,
    /// The running action's step, and its local progress when the step started.
    detail: Option<&'static str>,
    local: f64,
}

impl<'a> ProgressEvents<'a> {
    fn new(
        job: u64,
        actions: &'a [(Action, String)],
        emit: &'a mut dyn FnMut(&RepairProgress),
    ) -> ProgressEvents<'a> {
        ProgressEvents {
            job,
            labels: actions.iter().map(|(_, label)| label.as_str()).collect(),
            emit,
            last: None,
            sent: 0.0,
            detail: None,
            local: 0.0,
        }
    }

    fn started(&mut self, step: usize) {
        self.detail = None;
        self.local = 0.0;
        self.send(step, 0.0, true);
    }

    fn progress(&mut self, step: usize, fraction: f64) {
        self.local = fraction;
        self.send(step, fraction, false);
    }

    /// A new step inside the action is sent right away, so the dialog never names an old one.
    fn stage(&mut self, step: usize, name: &'static str) {
        if self.detail != Some(name) {
            self.detail = Some(name);
            self.send(step, self.local, true);
        }
    }

    fn send(&mut self, step: usize, local: f64, always: bool) {
        let now = Instant::now();
        if !always
            && self
                .last
                .is_some_and(|t| now.duration_since(t) < PROGRESS_INTERVAL)
        {
            return;
        }
        let steps = self.labels.len();
        let Some(label) = self.labels.get(step) else {
            return;
        };
        self.last = Some(now);
        let overall = (step as f64 + local.clamp(0.0, 1.0)) / steps as f64;
        self.sent = self.sent.max(overall.clamp(0.0, 1.0));
        (self.emit)(&RepairProgress {
            job: self.job,
            step,
            steps,
            label: label.to_string(),
            detail: self.detail.map(str::to_string),
            local: local.clamp(0.0, 1.0),
            fraction: self.sent,
        });
    }
}

/// `repair_check`: whether each mesh is closed and oriented, with the counts its damage badge
/// names, in order.
pub fn repair_check(bundle: Bundle) -> Result<Vec<RepairCheck>, String> {
    bundle
        .meshes
        .iter()
        .map(|m| {
            m.mesh.check().map_err(|reason| {
                format!("Could not check the part: its data is damaged ({reason}).")
            })?;
            let s = repair::silent(|c| status::analyze(&m.mesh, c))
                .map_err(run_error)?
                .status;
            Ok(RepairCheck {
                closed: s.closed,
                oriented: s.oriented,
                holes: s.holes,
                border_edges: s.border_edges,
                non_manifold_edges: s.non_manifold_edges,
                inverted_triangles: s.inverted_triangles,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Part library (src/library/ipc.ts)

/// `library_primitive`: the mesh of a Part library primitive (`id` with `params`, key → number) in
/// a bundle like import's, with its stats and problem edges. Cancel (`repair_cancel` with `job`)
/// makes it fail with CANCELLED: a fine mesh takes a moment to build and to measure.
pub fn library_primitive(
    id: &str,
    params: &Map<String, Value>,
    job: u64,
    jobs: &RepairJobs,
) -> Result<Vec<u8>, String> {
    let running = jobs.start(job);
    let mesh = crate::library::generate(id, params)?;
    let analysis = analyze_unless_cancelled(&mesh, running.cancel_flag())
        .ok_or_else(|| CANCELLED.to_string())?;
    let mut info = Map::new();
    info.insert("stats".into(), json!(analysis.stats));
    Ok(bundle::encode(
        &Map::new(),
        &[out_mesh(info, &mesh, &analysis)],
    ))
}

// ---------------------------------------------------------------------------------------------
// Measure mode (src/measure/ipc.ts)

/// `PickReply` in src/measure/ipc.ts: the feature in the owning part's frame, the same in world
/// space, and a surface's outline (world, x, y, z, x, y, z per segment).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PickReply {
    pub feature: measure::Feature,
    pub world: measure::Feature,
    pub outline: Vec<f64>,
}

fn matrix_field(meta: &Map<String, Value>, key: &str) -> Option<Matrix> {
    let values: Vec<f64> = meta
        .get(key)?
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect::<Option<_>>()?;
    Matrix::try_from(values.as_slice())
        .ok()
        .filter(|m| m.iter().all(|v| v.is_finite()))
}

fn field<T: serde::de::DeserializeOwned>(meta: &Map<String, Value>, key: &str) -> Option<T> {
    serde_json::from_value(meta.get(key)?.clone()).ok()
}

/// What a squashed-flat part (no inverse) gives the measure commands.
const FLAT: &str = "The part is squashed flat, so nothing on it can be measured.";

/// `measure_pick`: what a click picks. Meta {meshKey, kind, triangle, hit: world [x, y, z], matrix:
/// the clicked part's, owner: the matrix of the part the measurement belongs to, snap, tolerance:
/// world mm, clip: world planes}; one mesh, the clicked part's own (part-local), kept in `cache`
/// under `meshKey`, or one without triangles to use the one kept (MESH_NEEDED when there is none).
pub fn measure_pick(bundle: Bundle, cache: &measure::MeasureCache) -> Result<PickReply, String> {
    const ACTION: &str = "Could not measure there";
    let damaged = || format!("{ACTION}: the request from the window is damaged.");
    let Bundle { meta, meshes } = bundle;
    let mut meshes = meshes.into_iter();
    let (Some(input), None) = (meshes.next(), meshes.next()) else {
        return Err(damaged());
    };
    let key = meta
        .get("meshKey")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let index = if input.mesh.triangle_count() > 0 {
        input
            .mesh
            .check()
            .map_err(|reason| format!("{ACTION}: the part is damaged ({reason})."))?;
        let index = Arc::new(measure::MeshIndex::new(input.mesh));
        cache.put(key, index.clone());
        index
    } else {
        cache.get(key).ok_or_else(|| MESH_NEEDED.to_string())?
    };
    let kind: measure::PickKind = field(&meta, "kind").ok_or_else(damaged)?;
    let snap: measure::Snap = field(&meta, "snap").ok_or_else(damaged)?;
    let triangle = meta
        .get("triangle")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)? as u32;
    let hit: [f64; 3] = field(&meta, "hit").ok_or_else(damaged)?;
    let tolerance = meta
        .get("tolerance")
        .and_then(Value::as_f64)
        .filter(|t| t.is_finite() && *t >= 0.0)
        .ok_or_else(damaged)?;
    let clip: Vec<measure::ClipPlane> = field(&meta, "clip").ok_or_else(damaged)?;
    let matrix = matrix_field(&meta, "matrix").ok_or_else(damaged)?;
    let owner = matrix_field(&meta, "owner").ok_or_else(damaged)?;
    let to_local = measure::invert(&matrix).ok_or(FLAT)?;
    let to_owner = crate::mesh::multiply(&measure::invert(&owner).ok_or(FLAT)?, &matrix);
    let scale = crate::mesh::linear_determinant(&matrix)
        .abs()
        .cbrt()
        .max(1e-300);
    // A world plane n.x + c >= 0 is (A^T n).x_local + (n.t + c) >= 0 in the part's frame.
    let local_clip: Vec<measure::ClipPlane> = clip
        .iter()
        .map(|p| measure::ClipPlane {
            normal: [0, 1, 2].map(|c| (0..3).map(|r| matrix[c * 4 + r] * p.normal[r]).sum()),
            constant: (0..3).map(|r| p.normal[r] * matrix[12 + r]).sum::<f64>() + p.constant,
        })
        .collect();
    let click = measure::Click {
        triangle,
        hit: crate::mesh::transform_point(&to_local, hit),
        snap,
        tolerance: tolerance / scale,
        clip: &local_clip,
    };
    let (feature, outline) = measure::pick(&index, kind, &click)?;
    let outline = outline
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| crate::mesh::transform_point(&matrix, *p))
        .collect();
    Ok(PickReply {
        world: measure::transform(&feature, &matrix)?,
        feature: measure::transform(&feature, &to_owner)?,
        outline,
    })
}

/// `measure_fit`: the 3-point circle or 4-point sphere through points (each a point feature).
pub fn measure_fit(kind: &str, points: &[measure::Feature]) -> Result<measure::Feature, String> {
    let at: Vec<[f64; 3]> = points
        .iter()
        .map(|f| match f {
            measure::Feature::Point { p } => Ok(*p),
            _ => Err("A circle or sphere is fitted to points only.".to_string()),
        })
        .collect::<Result<_, _>>()?;
    match (kind, at.as_slice()) {
        ("circle3", &[a, b, c]) => measure::circle_through([a, b, c]),
        ("sphere4", &[a, b, c, d]) => measure::sphere_through([a, b, c, d]),
        _ => Err("Pick three points for a circle, or four for a sphere.".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// Platform statistics (src/stats/ipc.ts)

/// The Tauri event a platform statistics run reports its progress with: {job, fraction}.
pub const STATS_PROGRESS: &str = "platform-stats-progress";

/// `StatsProgress` in src/stats/ipc.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatsProgress {
    pub job: u64,
    /// 0..1 over the whole run.
    pub fraction: f64,
}

/// `StatsReply` in src/stats/ipc.ts: a row per part in the order sent, and the Information section
/// for every part and for the selected ones.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatsReply {
    pub parts: Vec<stats::PartStats>,
    pub all: stats::Summary,
    pub selected: stats::Summary,
}

/// `platform_stats`: meta {"job", "platform": [x, y, z], "selected": [part index]}, one bundle mesh
/// per part with info {"name", "matrix"}. Each part is measured in its own frame and its matrix
/// applied to the numbers, as the Information panel does. Cancel (`repair_cancel` with the job)
/// stops it between and inside parts with CANCELLED.
pub fn platform_stats(
    bundle: Bundle,
    jobs: &RepairJobs,
    emit: &mut dyn FnMut(&StatsProgress),
) -> Result<StatsReply, String> {
    const ACTION: &str = "Could not work out the platform statistics";
    let damaged = || format!("{ACTION}: the request from the window is damaged.");
    let job = bundle
        .meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let platform = bundle
        .meta
        .get("platform")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_f64).collect::<Option<Vec<f64>>>())
        .and_then(|v| <[f64; 3]>::try_from(v).ok())
        .filter(|p| p.iter().all(|v| v.is_finite() && *v > 0.0))
        .ok_or_else(damaged)?;
    let selected: Vec<usize> = bundle
        .meta
        .get("selected")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(|v| v.as_u64().map(|i| i as usize)).collect())
        .ok_or_else(damaged)?;
    let mut parts = Vec::with_capacity(bundle.meshes.len());
    for (n, m) in bundle.meshes.iter().enumerate() {
        let (_, matrix) = checked_part(m, n).map_err(|reason| format!("{ACTION}: {reason}."))?;
        parts.push((matrix, &m.mesh));
    }
    let running = jobs.start(job);
    let total = parts
        .iter()
        .map(|(_, m)| m.triangle_count())
        .sum::<usize>()
        .max(1) as f64;
    let mut done = 0.0;
    let mut last = Instant::now();
    let mut rows = Vec::with_capacity(parts.len());
    for (matrix, mesh) in &parts {
        let share = mesh.triangle_count() as f64 / total;
        let start = done;
        let mut control = Control::new(running.cancel_flag(), |f| {
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                emit(&StatsProgress {
                    job,
                    fraction: start + f * share,
                });
            }
        });
        let row = stats::part_stats(mesh, matrix, &mut control).map_err(run_error)?;
        drop(control);
        rows.push(row);
        done += share;
    }
    emit(&StatsProgress { job, fraction: 1.0 });
    let every: Vec<usize> = (0..rows.len()).collect();
    Ok(StatsReply {
        all: stats::summary(&rows, &every, platform),
        selected: stats::summary(&rows, &selected, platform),
        parts: rows,
    })
}

/// `export_text`: writes `text` (UTF-8) to `path`, a file the user picked in a save dialog: the
/// platform statistics' Export to CSV. The grant is checked by the command.
pub fn export_text(path: &Path, text: &str) -> Result<(), String> {
    let file = display_name(path);
    formats::write_atomic(path, |mut w| {
        w.write_all(text.as_bytes())?;
        Ok(w)
    })
    .map_err(|e| format!("Could not write \"{file}\": {}.", formats::write_error(&e)))
}

/// `repair_shells`: the Shells tab of one working copy. Meta {rows}; mesh 0 has empty arrays and
/// `extra.shellOf`, the row of each triangle.
pub fn repair_shells(bundle: Bundle) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not list the shells";
    let [input] = bundle.meshes.as_slice() else {
        return Err(format!("{ACTION}: the request from the window is damaged."));
    };
    input
        .mesh
        .check()
        .map_err(|reason| format!("{ACTION}: the working copy is damaged ({reason})."))?;
    let (rows, shell_of) = shells::shell_table(&input.mesh);
    let mut meta = Map::new();
    meta.insert("rows".into(), json!(rows));
    let out = OutMesh {
        info: Map::new(),
        positions: &[],
        indices: &[],
        edges: None,
    };
    Ok(bundle::encode_with_extra(
        &meta,
        &[out],
        &[vec![("shellOf", BlobRef::U32(&shell_of))]],
    ))
}

/// The working copy Cut surfaces draws its line on, kept between `repair_cut_path` calls: the
/// window sends it once, not with every click (12 MB for 700k triangles, which took longer to send
/// than the line takes to route). One at a time, under the number the window gave that working
/// copy; the window empties it when the cutting mode ends (`repair_cut_reset`). Only a cache: the
/// cut itself goes through `repair_run` with the working copy, like every repair run.
#[derive(Default)]
pub struct CutCache(Mutex<Option<(u64, Arc<Mesh>)>>);

impl CutCache {
    fn get(&self, key: u64) -> Option<Arc<Mesh>> {
        let held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, m)| m.clone())
    }

    fn put(&self, key: u64, mesh: Arc<Mesh>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((key, mesh));
    }

    /// `repair_cut_reset`: lets go of the working copy.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// What `repair_cut_path` fails with when it was sent no mesh and holds none under the key: the
/// window then sends the working copy.
pub const MESH_NEEDED: &str = "mesh needed";

/// `repair_cut_path`: the cut line Cut surfaces would cut along, for the window to draw. Meta
/// {meshKey, points, smooth, step, snap, closed}, the last five as the `cut_surface` action takes
/// them, and one mesh: the working copy, kept in `cache` under `meshKey`, or one without
/// triangles to use the working copy kept under it (MESH_NEEDED when there is none). Answers meta
/// {pieces: the point count of each piece, null for one that found no way along the surface;
/// spans: the clicked points each piece joins; anchors: where each clicked point went, [x, y, z]};
/// mesh 0 has empty arrays and `extra.path`, every piece's points (x, y, z each) one after the
/// other.
pub fn repair_cut_path(bundle: Bundle, cache: &CutCache) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not show the cut line";
    let Bundle { meta, meshes } = bundle;
    let mut meshes = meshes.into_iter();
    let (Some(input), None) = (meshes.next(), meshes.next()) else {
        return Err(format!("{ACTION}: the request from the window is damaged."));
    };
    let key = meta.get("meshKey").and_then(Value::as_u64);
    let mesh = if input.mesh.triangle_count() > 0 {
        input
            .mesh
            .check()
            .map_err(|reason| format!("{ACTION}: the working copy is damaged ({reason})."))?;
        let mesh = Arc::new(input.mesh);
        if let Some(key) = key {
            cache.put(key, mesh.clone());
        }
        mesh
    } else {
        key.and_then(|k| cache.get(k))
            .ok_or_else(|| MESH_NEEDED.to_string())?
    };
    let step = Step {
        action: ActionId::CutSurface,
        enabled: true,
        params: meta,
    };
    let Action::CutSurface(line) = scripts::parse_action(&step).map_err(|r| format!("{r}."))?
    else {
        return Err(format!("{ACTION}: the request from the window is damaged."));
    };
    let preview =
        repair::silent(|c| repair::surface_cut::preview(&mesh, &line, c)).map_err(run_error)?;
    let mut path: Vec<f32> = Vec::new();
    let pieces: Vec<Value> = preview
        .pieces
        .iter()
        .map(|piece| match piece {
            Some(points) => {
                path.extend(points.iter().flat_map(|p| p.map(|x| x as f32)));
                json!(points.len())
            }
            None => Value::Null,
        })
        .collect();
    let mut out = Map::new();
    out.insert("pieces".into(), json!(pieces));
    out.insert("spans".into(), json!(preview.spans));
    out.insert("anchors".into(), json!(preview.anchors));
    let mesh = OutMesh {
        info: Map::new(),
        positions: &[],
        indices: &[],
        edges: None,
    };
    Ok(bundle::encode_with_extra(
        &out,
        &[mesh],
        &[vec![("path", BlobRef::F32(&path))]],
    ))
}

/// The user's repair scripts and picker order, in the app's config folder. Never sent anywhere.
pub const SCRIPTS_FILE: &str = "repair-scripts.json";

/// `repair_scripts_load`: the scripts file in `dir`; None before anything was saved there.
pub fn load_scripts(dir: &Path) -> Result<Option<StoredScripts>, String> {
    let text = match std::fs::read_to_string(dir.join(SCRIPTS_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Could not read your repair scripts: {}.",
                formats::read_error(&e)
            ));
        }
    };
    serde_json::from_str(&text).map(Some).map_err(|e| {
        format!("Could not read your repair scripts: {SCRIPTS_FILE} in the settings folder is damaged ({e}).")
    })
}

/// `repair_scripts_save`: writes the user's scripts to `dir` (created when missing) through a
/// temporary file renamed into place, so a crash never leaves half a file. Built-in scripts are
/// never stored.
pub fn save_scripts(dir: &Path, data: &StoredScripts) -> Result<(), String> {
    let fail = |reason: String| format!("Could not save your repair scripts: {reason}.");
    let own = StoredScripts {
        scripts: data
            .scripts
            .iter()
            .filter(|s| !s.built_in)
            .cloned()
            .collect(),
        order: data.order.clone(),
    };
    let mut text = serde_json::to_string_pretty(&own).map_err(|e| fail(e.to_string()))?;
    text.push('\n');
    std::fs::create_dir_all(dir).map_err(|e| fail(formats::write_error(&e)))?;
    write_text(&dir.join(SCRIPTS_FILE), &text).map_err(|e| fail(formats::write_error(&e)))
}

fn write_text(path: &Path, text: &str) -> std::io::Result<()> {
    formats::write_atomic(path, |mut w| {
        w.write_all(text.as_bytes())?;
        Ok(w)
    })
}

/// Largest script file tricky3D reads; real ones are a few kilobytes.
const MAX_SCRIPT_FILE: u64 = 16 << 20;

/// `repair_script_import`: reads a script file picked in the open dialog, the reference tool's
/// `.repair` XML or our own JSON, told apart by the extension, or else by the first character.
pub fn import_script(path: &Path) -> Result<Imported, String> {
    let file = display_name(path);
    let fail = |reason: String| format!("Could not import \"{file}\": {reason}.");
    let bytes = read_script_file(path).map_err(fail)?;
    let text = script_text(&bytes).map_err(fail)?;
    let xml = if formats::has_extension(path, "repair") {
        true
    } else if formats::has_extension(path, "json") {
        false
    } else {
        match text
            .trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n'])
            .chars()
            .next()
        {
            Some('<') => true,
            Some('{') => false,
            _ => return Err(fail("it is not a repair script".into())),
        }
    };
    let imported = if xml {
        scripts::import_repair_xml(&text)
    } else {
        scripts::import_json(&text)
    };
    imported.map_err(fail)
}

fn read_script_file(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| formats::read_error(&e))?;
    let mut bytes = Vec::new();
    file.take(MAX_SCRIPT_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| formats::read_error(&e))?;
    if bytes.len() as u64 > MAX_SCRIPT_FILE {
        return Err("it is too large to be a repair script".into());
    }
    Ok(bytes)
}

/// A script file's text: UTF-8, with or without a byte order mark, or UTF-16 with one (as some
/// Windows programs write XML).
fn script_text(bytes: &[u8]) -> Result<String, String> {
    let not_text = || "it is not a text file".to_string();
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let (units, tail) = rest.as_chunks::<2>();
        if !tail.is_empty() {
            return Err(not_text());
        }
        let units: Vec<u16> = units.iter().map(|&pair| unit(pair)).collect();
        String::from_utf16(&units).map_err(|_| not_text())
    };
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        String::from_utf8(rest.to_vec()).map_err(|_| not_text())
    } else if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        utf16(rest, u16::from_be_bytes)
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| not_text())
    }
}

/// `repair_script_export`: writes `script` in our JSON format to `path`, picked in the save dialog.
pub fn export_script(path: &Path, script: &Script) -> Result<(), String> {
    write_text(path, &scripts::export_json(script)).map_err(|e| {
        format!(
            "Could not export \"{}\": {}.",
            display_name(path),
            formats::write_error(&e)
        )
    })
}

/// The file name the export dialog suggests for the script `name`: characters file systems refuse
/// replaced by "_", cut to a safe length, never a name Windows reserves, and ".json" added.
pub fn script_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut stem = String::new();
    for c in cleaned.trim().chars() {
        // Bytes, not characters: file systems limit names to 255 bytes.
        if stem.len() + c.len_utf8() > 120 {
            break;
        }
        stem.push(c);
    }
    // Windows drops trailing dots and spaces from file names.
    let mut stem = stem.trim_end_matches(['.', ' ']).to_string();
    if stem.is_empty() {
        stem = "Repair script".into();
    }
    // Windows reserves device names before any extension ("con.backup" too): mark the device part.
    let upper = stem.to_ascii_uppercase();
    let device = upper.split('.').next().unwrap_or("");
    let numbered = (device.starts_with("COM") || device.starts_with("LPT"))
        && device.len() == 4
        && matches!(device.as_bytes()[3], b'1'..=b'9');
    if matches!(device, "CON" | "PRN" | "AUX" | "NUL") || numbered {
        stem.insert(device.len(), '_');
    }
    format!("{stem}.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repair::SanitizeParams;

    fn actions(n: usize) -> Vec<(Action, String)> {
        (0..n)
            .map(|k| {
                (
                    Action::SanitizeDuplicateNodes(SanitizeParams { tolerance: 0.0 }),
                    format!("Action {k}"),
                )
            })
            .collect()
    }

    #[test]
    fn progress_events_are_throttled_but_every_start_is_sent() {
        let actions = actions(3);
        let mut seen = Vec::new();
        let started = Instant::now();
        {
            let mut emit = |p: &RepairProgress| seen.push(p.clone());
            let mut events = ProgressEvents::new(5, &actions, &mut emit);
            for step in 0..3 {
                events.started(step);
                for i in 0..=100_000 {
                    events.progress(step, i as f64 / 100_000.0);
                }
            }
            // Past the end, or a step that does not exist: nothing.
            events.progress(7, 0.5);
        }
        let elapsed = started.elapsed().as_secs_f64();
        let budget = 3 + (elapsed / PROGRESS_INTERVAL.as_secs_f64()).ceil() as usize + 1;
        assert!(seen.len() <= budget, "{} events in {elapsed} s", seen.len());
        assert!(seen.windows(2).all(|w| w[1].fraction >= w[0].fraction));
        for step in 0..3 {
            let first = seen.iter().find(|p| p.step == step).unwrap();
            assert_eq!(first.fraction, step as f64 / 3.0);
            assert_eq!(first.label, format!("Action {step}"));
            assert_eq!((first.job, first.steps), (5, 3));
        }
    }

    #[test]
    fn progress_never_goes_back_or_past_the_end() {
        let actions = actions(2);
        let mut seen = Vec::new();
        {
            let mut emit = |p: &RepairProgress| seen.push(p.fraction);
            let mut events = ProgressEvents::new(1, &actions, &mut emit);
            events.started(0);
            std::thread::sleep(PROGRESS_INTERVAL);
            events.progress(0, 0.8);
            std::thread::sleep(PROGRESS_INTERVAL);
            // An action that reports less than before, or more than all of it.
            events.progress(0, 0.3);
            std::thread::sleep(PROGRESS_INTERVAL);
            events.progress(0, 7.0);
            events.started(1);
            std::thread::sleep(PROGRESS_INTERVAL);
            events.progress(1, f64::INFINITY);
        }
        assert_eq!(seen, [0.0, 0.4, 0.4, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn jobs_register_cancel_and_unregister() {
        let jobs = RepairJobs::default();
        {
            let run = jobs.start(1);
            assert!(!run.cancel_flag().load(Ordering::Relaxed));
            assert!(!jobs.is_idle());
            jobs.cancel(1);
            assert!(run.cancel_flag().load(Ordering::Relaxed));
        }
        assert!(jobs.is_idle());
        // A cancel for a job not started yet waits for it; one for an older job is ignored.
        jobs.cancel(3);
        jobs.cancel(1);
        {
            let run = jobs.start(3);
            assert!(run.cancel_flag().load(Ordering::Relaxed));
        }
        assert!(jobs.is_idle());
        {
            let run = jobs.start(2);
            assert!(!run.cancel_flag().load(Ordering::Relaxed));
        }
        assert!(jobs.is_idle());
    }

    #[test]
    fn script_text_reads_utf8_and_utf16() {
        assert_eq!(script_text(b"<a/>").unwrap(), "<a/>");
        assert_eq!(script_text(b"\xEF\xBB\xBF{}").unwrap(), "{}");
        assert_eq!(script_text(b"\xFF\xFE<\0a\0/\0>\0").unwrap(), "<a/>");
        assert_eq!(script_text(b"\xFE\xFF\0<\0a\0/\0>").unwrap(), "<a/>");
        assert_eq!(script_text("Ü".as_bytes()).unwrap(), "Ü");
        assert!(script_text(b"\xFF\xFE<").is_err());
        assert!(script_text(b"\xC3(").is_err());
        // An unpaired surrogate is not text.
        assert!(script_text(b"\xFF\xFE\x00\xD8").is_err());
    }

    #[test]
    fn runs_fail_with_exactly_cancelled() {
        assert_eq!(run_error(RepairError::Cancelled), "cancelled");
        assert_eq!(
            run_error(RepairError::Failed(
                "Wrap part surface ran out of memory.".into()
            )),
            "Wrap part surface ran out of memory."
        );
    }
}
