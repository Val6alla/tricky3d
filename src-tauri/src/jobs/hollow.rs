//! Hollow (src/hollow/ipc.ts): the cavity preview, the drain holes' checks and places, and Apply,
//! off the UI thread with progress and Cancel. The preview's part and cavity stay here (`HollowCache`)
//! under the preview's key, so the holes and Apply work on exactly what the window shows without
//! sending it back.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{
    CANCELLED, PROGRESS_INTERVAL, RepairJobs, encode_results, field, run_error, world_meshes,
};
use crate::bundle::{self, Bundle, OutMesh};
use crate::mesh::{Mesh, stats};
use crate::ops::hollow::{self, Hole, HoleCheck, HollowSettings, Scene};
use crate::repair::{Control, status};

pub const HOLLOW_PROGRESS: &str = "hollow-progress";

/// `HollowProgress` in src/hollow/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HollowProgress {
    pub job: u64,
    pub stage: Option<String>,
    /// 0..1, never decreasing.
    pub fraction: f64,
}

/// The last preview: its key, the part (world space), its cavity and what checking holes needs.
pub struct Preview {
    pub key: u64,
    pub part: Mesh,
    pub cavity: Option<Mesh>,
    pub settings: HollowSettings,
    pub scene: Scene,
}

#[derive(Default)]
pub struct HollowCache(Mutex<Option<Arc<Preview>>>);

impl HollowCache {
    fn get(&self, key: u64) -> Result<Arc<Preview>, String> {
        let kept = self.0.lock().unwrap_or_else(|p| p.into_inner()).clone();
        kept.filter(|p| p.key == key).ok_or_else(|| {
            "The cavity changed meanwhile: wait for the preview to finish, then try again.".into()
        })
    }

    fn put(&self, preview: Preview) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(preview));
    }

    /// Forgets the preview (the panel closed).
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// `hollow_preview`: meta {job, key, settings: HollowSettings}; one mesh, the part in world space.
/// A part that is not closed and oriented is refused: meta {refused: {closed, oriented, holes,
/// invertedTriangles}}, no mesh. Otherwise meta {key, partVolume, cavityVolume, thin: {share,
/// bounds, message} or null, warnings} and one mesh, the cavity (empty when none fits), facing out
/// of the cavity. Kept under `key` for `hollow_holes` and `hollow_apply`.
pub fn hollow_preview(
    bundle: Bundle,
    jobs: &RepairJobs,
    cache: &HollowCache,
    emit: &mut dyn FnMut(&HollowProgress),
) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not hollow the part";
    let damaged = || format!("{ACTION}: the request from the window is damaged.");
    let job = bundle
        .meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let key = bundle
        .meta
        .get("key")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let settings: HollowSettings = field(&bundle.meta, "settings").ok_or_else(damaged)?;
    if let Some(problem) = settings.problem() {
        return Err(problem);
    }
    let mut meshes = world_meshes(bundle, ACTION, true)?.into_iter();
    let (Some(part), None) = (meshes.next(), meshes.next()) else {
        return Err(damaged());
    };
    let running = jobs.start(job);
    let full_speed = crate::power::FullSpeed::start();
    let cancel = running.cancel_flag();
    let events = RefCell::new(Events::new(job, emit));
    let mut control = Control::new(cancel, |f| events.borrow_mut().progress(f))
        .with_stages(|name| events.borrow_mut().stage(name));
    control.stage("Checking the part");
    let s = status::analyze(&part, &mut control.sub(0.0, 0.05))
        .map_err(run_error)?
        .status;
    if !(s.closed && s.oriented) {
        let mut meta = Map::new();
        meta.insert(
            "refused".into(),
            json!({"closed": s.closed, "oriented": s.oriented, "holes": s.holes,
                   "invertedTriangles": s.inverted_triangles}),
        );
        return Ok(bundle::encode(&meta, &[]));
    }
    let made = hollow::cavity(&part, &settings, &mut control.sub(0.05, 0.95)).map_err(run_error)?;
    control.stage("Measuring");
    let scene = Scene::new(&part, made.mesh.as_ref(), settings);
    control.check().map_err(run_error)?;
    drop(control);
    let part_volume = stats(&part).volume;
    let cavity_volume = made.mesh.as_ref().map_or(0.0, |m| stats(m).volume);
    let mut meta = Map::new();
    meta.insert("key".into(), json!(key));
    meta.insert("partVolume".into(), json!(part_volume));
    meta.insert("cavityVolume".into(), json!(cavity_volume));
    meta.insert(
        "thin".into(),
        match &made.thin {
            Some((thin, message)) => json!({"share": thin.share, "bounds": thin.bounds,
                                            "centre": thin.centre, "message": message}),
            None => Value::Null,
        },
    );
    meta.insert("warnings".into(), json!(made.warnings));
    let empty = Mesh::default();
    let cavity = made.mesh.as_ref().unwrap_or(&empty);
    let answer = bundle::encode(
        &meta,
        &[OutMesh {
            info: Map::new(),
            positions: &cavity.positions,
            indices: &cavity.indices,
            edges: None,
        }],
    );
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    cache.put(Preview {
        key,
        part,
        cavity: made.mesh,
        settings,
        scene,
    });
    drop(full_speed);
    drop(running);
    Ok(answer)
}

/// `AutoHoles` in src/hollow/types.ts: how many holes Auto holes places, and their shape.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct AutoHoles {
    pub count: usize,
    pub template: Hole,
}

/// `hollow_holes`: meta {key, holes: Hole[], auto: AutoHoles or null}. Answers meta {holes (with the
/// ones Auto holes placed after them), checks: HoleCheck[]} and one mesh per hole, its solid for the
/// view (a refused hole's as deep as twice the wall).
pub fn hollow_holes(bundle: Bundle, cache: &HollowCache) -> Result<Vec<u8>, String> {
    let damaged =
        || "Could not place the holes: the request from the window is damaged.".to_string();
    let key = bundle
        .meta
        .get("key")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let mut holes: Vec<Hole> = field(&bundle.meta, "holes").ok_or_else(damaged)?;
    let auto: Option<AutoHoles> = bundle
        .meta
        .get("auto")
        .filter(|v| !v.is_null())
        .map(|v| serde_json::from_value(v.clone()).map_err(|_| damaged()))
        .transpose()?;
    let preview = cache.get(key)?;
    if let Some(auto) = auto {
        if let Some(problem) = auto.template.problem() {
            return Err(problem);
        }
        let placed = hollow::auto_holes(&preview.scene, auto.count.min(32), &auto.template);
        holes.extend(placed);
    }
    let checks: Vec<HoleCheck> = (0..holes.len())
        .map(|i| hollow::check_hole(&preview.scene, &holes, i))
        .collect();
    let t = preview.settings.thickness;
    let solids: Vec<Mesh> = holes
        .iter()
        .zip(&checks)
        .map(|(h, c)| {
            if h.problem().is_some() {
                return Mesh::default();
            }
            hollow::hole_solid(h, c.depth.unwrap_or(2.0 * t), c.room.unwrap_or(t), 0.0, 0.0)
        })
        .collect();
    let mut meta = Map::new();
    meta.insert("holes".into(), json!(holes));
    meta.insert("checks".into(), json!(checks));
    let out: Vec<OutMesh> = solids
        .iter()
        .map(|m| OutMesh {
            info: Map::new(),
            positions: &m.positions,
            indices: &m.indices,
            edges: None,
        })
        .collect();
    Ok(bundle::encode(&meta, &out))
}

/// `hollow_apply`: meta {job, key, holes: Hole[], plugs: clearance (mm) or null}. Answers meta
/// {skipped: [{index, reason}], warnings, plugs: count} and the hollow part (world space, measured
/// as results are, with its problem edges) followed by one mesh per plug. Cancel fails it with
/// CANCELLED.
pub fn hollow_apply(
    bundle: Bundle,
    jobs: &RepairJobs,
    cache: &HollowCache,
    emit: &mut dyn FnMut(&HollowProgress),
) -> Result<Vec<u8>, String> {
    let damaged =
        || "Could not hollow the part: the request from the window is damaged.".to_string();
    let job = bundle
        .meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let key = bundle
        .meta
        .get("key")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let holes: Vec<Hole> = field(&bundle.meta, "holes").ok_or_else(damaged)?;
    let plugs: Option<f64> = match bundle.meta.get("plugs") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|c| c.is_finite() && (0.0..=2.0).contains(c))
                .ok_or_else(|| "Enter a plug clearance from 0 to 2 mm.".to_string())?,
        ),
    };
    let preview = cache.get(key)?;
    let Some(cavity) = &preview.cavity else {
        return Err(
            "No cavity fits in this part: every wall is thinner than twice the wall thickness."
                .into(),
        );
    };
    let running = jobs.start(job);
    let full_speed = crate::power::FullSpeed::start();
    let cancel = running.cancel_flag();
    let events = RefCell::new(Events::new(job, emit));
    let mut control = Control::new(cancel, |f| events.borrow_mut().progress(f))
        .with_stages(|name| events.borrow_mut().stage(name));
    let applied = hollow::apply(
        &preview.part,
        cavity,
        &preview.settings,
        &holes,
        plugs,
        &mut control,
    )
    .map_err(run_error)?;
    drop(control);
    let mut meta = Map::new();
    meta.insert(
        "skipped".into(),
        json!(
            applied
                .skipped
                .iter()
                .map(|(i, why)| json!({"index": i, "reason": why}))
                .collect::<Vec<_>>()
        ),
    );
    meta.insert("warnings".into(), json!(applied.warnings));
    meta.insert("plugs".into(), json!(applied.plugs.len()));
    let mut results = vec![(Map::new(), applied.mesh)];
    results.extend(applied.plugs.into_iter().map(|p| (Map::new(), p)));
    let answer = encode_results(&meta, &results);
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    drop(full_speed);
    drop(running);
    Ok(answer)
}

/// A run's progress as `hollow-progress` events: a new step at once, the rest at most once per
/// PROGRESS_INTERVAL.
struct Events<'a> {
    job: u64,
    emit: &'a mut dyn FnMut(&HollowProgress),
    last: Option<Instant>,
    sent: f64,
    stage: Option<&'static str>,
}

impl<'a> Events<'a> {
    fn new(job: u64, emit: &'a mut dyn FnMut(&HollowProgress)) -> Events<'a> {
        Events {
            job,
            emit,
            last: None,
            sent: 0.0,
            stage: None,
        }
    }

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
        (self.emit)(&HollowProgress {
            job: self.job,
            stage: self.stage.map(str::to_string),
            fraction: self.sent,
        });
    }
}
