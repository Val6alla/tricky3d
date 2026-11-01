//! Modify's mesh tools (src/modify/ipc.ts): one tool on one working copy, off the UI thread, with
//! progress and Cancel. The window sends the part baked into world space (as repair does) and
//! keeps the result as a preview until OK; nothing about a run is kept here.

use std::cell::RefCell;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{CANCELLED, PROGRESS_INTERVAL, RepairJobs, run_error};
use crate::bundle::{self, Blob, BlobRef, Bundle, Extra, OutMesh};
use crate::mesh::analyze_unless_cancelled;
use crate::modify::{self, Tool};
use crate::repair::Control;

pub const MODIFY_PROGRESS: &str = "modify-progress";

/// `ModifyProgress` in src/modify/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModifyProgress {
    pub job: u64,
    /// The step it is on ("Reducing triangles").
    pub stage: Option<String>,
    /// 0..1, never decreasing.
    pub fraction: f64,
}

/// `modify_run`: meta {job, tool: ToolSettings (src/modify/types.ts)}; one mesh, the working copy
/// in world space, with `extra.selection` (sorted triangle ids, may be empty). Answers meta
/// {changed, message, warnings, before, after} (triangle counts) and one mesh: when changed, the
/// result with info {stats}, its problem edges and `extra.origin` (the input triangle of each
/// triangle, or none when the triangles are the input's in order); when not, empty arrays. A
/// result that would open, turn or cross a sound part fails with the tool's sentence. Cancel
/// (`repair_cancel` with the job) fails it with CANCELLED.
pub fn modify_run(
    bundle: Bundle,
    jobs: &RepairJobs,
    emit: &mut dyn FnMut(&ModifyProgress),
) -> Result<Vec<u8>, String> {
    let damaged =
        || "Could not run the mesh tool: the request from the window is damaged.".to_string();
    let Bundle { meta, meshes } = bundle;
    let job = meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let tool: Tool = meta
        .get("tool")
        .and_then(|t| serde_json::from_value(t.clone()).ok())
        .ok_or_else(damaged)?;
    if let Some(problem) = tool.problem() {
        return Err(problem);
    }
    let mut meshes = meshes.into_iter();
    let (Some(input), None) = (meshes.next(), meshes.next()) else {
        return Err(damaged());
    };
    let label = tool.label();
    input
        .mesh
        .check()
        .map_err(|reason| format!("Could not run {label}: the part is damaged ({reason})."))?;
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
                "Could not run {label}: the selection does not fit the part."
            ));
        }
    };
    let running = jobs.start(job);
    let full_speed = crate::power::FullSpeed::start();
    let cancel = running.cancel_flag();
    let events = RefCell::new(Events {
        job,
        emit,
        last: None,
        sent: 0.0,
        stage: None,
    });
    let mut control = Control::new(cancel, |f| events.borrow_mut().progress(f))
        .with_stages(|name| events.borrow_mut().stage(name));
    let out = modify::run(&tool, &input.mesh, &selection, &mut control).map_err(run_error)?;
    drop(control);
    let mut meta = Map::new();
    meta.insert("changed".into(), json!(out.changed));
    meta.insert("message".into(), json!(out.message));
    meta.insert("warnings".into(), json!(out.warnings));
    meta.insert("before".into(), json!(triangles));
    meta.insert("after".into(), json!(out.mesh.triangle_count()));
    let answer = if out.changed {
        let analysis = analyze_unless_cancelled(&out.mesh, cancel).ok_or(CANCELLED)?;
        let mut info = Map::new();
        info.insert("stats".into(), json!(analysis.stats));
        let mesh = OutMesh {
            info,
            positions: &out.mesh.positions,
            indices: &out.mesh.indices,
            edges: Some(&analysis.problem_edges),
        };
        let mut extra: Extra = Vec::new();
        if let Some(origin) = &out.origin {
            extra.push(("origin", BlobRef::U32(origin)));
        }
        bundle::encode_with_extra(&meta, &[mesh], &[extra])
    } else {
        let empty = OutMesh {
            info: Map::new(),
            positions: &[],
            indices: &[],
            edges: None,
        };
        bundle::encode(&meta, &[empty])
    };
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    drop(full_speed);
    drop(running);
    Ok(answer)
}

/// A run's progress as `modify-progress` events: a new step at once, the rest at most once per
/// PROGRESS_INTERVAL.
struct Events<'a> {
    job: u64,
    emit: &'a mut dyn FnMut(&ModifyProgress),
    last: Option<Instant>,
    sent: f64,
    stage: Option<&'static str>,
}

impl Events<'_> {
    fn progress(&mut self, f: f64) {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < PROGRESS_INTERVAL)
        {
            return;
        }
        self.last = Some(now);
        self.sent = self.sent.max(f.clamp(0.0, 1.0));
        self.send();
    }

    fn stage(&mut self, name: &'static str) {
        if self.stage != Some(name) {
            self.stage = Some(name);
            self.send();
        }
    }

    fn send(&mut self) {
        (self.emit)(&ModifyProgress {
            job: self.job,
            stage: self.stage.map(str::to_string),
            fraction: self.sent,
        });
    }
}
