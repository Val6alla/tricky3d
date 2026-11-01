//! Arrange (src/arrange/ipc.ts): Align, Duplicate, Lay flat, Rotate to smallest box, Orient and
//! Pack. One command for all of them; the answer is new matrices, never meshes.

use std::path::Path;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{CANCELLED, PROGRESS_INTERVAL, RepairJobs, checked_part, field, run_error, write_text};
use crate::arrange::align::{self, Align};
use crate::arrange::collide::{self, Body};
use crate::arrange::duplicate::{self, Duplicate};
use crate::arrange::orient::{self, BoxAxes, Goal};
use crate::arrange::pack::{self, Item, Method, Rotation};
use crate::arrange::{Aabb, V3, World, translated, union_all};
use crate::bundle::{self, Bundle};
use crate::formats;
use crate::mesh::Matrix;
use crate::repair::Control;

/// A part's packing priority when it has none of its own (0 to 10, higher packs first).
pub const DEFAULT_PRIORITY: i32 = 5;

/// The Tauri event an arrange run reports its progress with.
pub const ARRANGE_PROGRESS: &str = "arrange-progress";

/// `ArrangeProgress` in src/arrange/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArrangeProgress {
    pub job: u64,
    /// 0..1, never decreasing.
    pub fraction: f64,
}

/// Sends progress at most once per PROGRESS_INTERVAL, and the end at once.
struct Events<'a> {
    job: u64,
    emit: &'a mut dyn FnMut(&ArrangeProgress),
    last: Option<Instant>,
    sent: f64,
}

impl Events<'_> {
    fn send(&mut self, fraction: f64) {
        let fraction = fraction.clamp(0.0, 1.0).max(self.sent);
        let now = Instant::now();
        if fraction < 1.0
            && self
                .last
                .is_some_and(|t| now.duration_since(t) < PROGRESS_INTERVAL)
        {
            return;
        }
        self.last = Some(now);
        self.sent = fraction;
        (self.emit)(&ArrangeProgress {
            job: self.job,
            fraction,
        });
    }
}

/// A part of the request: its world geometry, its matrix, and whether it is one the command acts
/// on (else it is only in the way, for Duplicate's Fill and for packing).
struct Part {
    name: String,
    world: World,
    matrix: Matrix,
    target: bool,
    priority: i32,
    rotation: Rotation,
}

fn matrix_json(m: &Matrix) -> Value {
    json!(m.to_vec())
}

/// `arrange`: meta {job, action, platform: [x, y, z], and the action's settings}; one bundle mesh
/// per part with info {name, matrix, target: bool (default true), and for packing priority (0 to
/// 10) and rotation ("default", "none", "z", "free")}. Answers a bundle with no meshes whose meta
/// depends on the action:
/// - "align" {align}: {matrices} for the targets, in order;
/// - "duplicate" {duplicate}: {matrices} (the targets, moved for Fill), {copies: [{source,
///   matrix}]}, each copy of the group in turn;
/// - "lay-flat" {point, normal}: {matrices} for the one target;
/// - "lay-flat-auto", "min-box" {axes}: {matrices};
/// - "orient" {goal}: {now: scores, candidates: [{matrix, supportArea, height, footprint}]};
/// - "pack" {pack: Method}: {matrices, fits: [bool], height, volume, density}, or a failure when
///   the packed parts' meshes collide (checked on the meshes, not their boxes).
///
/// Cancel (`repair_cancel` with the job) fails it with CANCELLED.
pub fn arrange(
    bundle: Bundle,
    jobs: &RepairJobs,
    emit: &mut dyn FnMut(&ArrangeProgress),
) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not arrange the parts";
    let damaged = || format!("{ACTION}: the request from the window is damaged.");
    let job = bundle
        .meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let action = bundle
        .meta
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(damaged)?
        .to_string();
    let platform: V3 = field(&bundle.meta, "platform").ok_or_else(damaged)?;
    if !platform.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(damaged());
    }
    let meta = bundle.meta;
    let parts = bundle
        .meshes
        .into_iter()
        .enumerate()
        .map(|(n, m)| {
            let (name, matrix) = checked_part(&m, n).map_err(|r| format!("{ACTION}: {r}."))?;
            let target = m
                .info
                .get("target")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let priority = m
                .info
                .get("priority")
                .and_then(Value::as_i64)
                .map_or(DEFAULT_PRIORITY, |p| p.clamp(0, 10) as i32);
            let rotation = m
                .info
                .get("rotation")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(Rotation::Default);
            Ok(Part {
                name,
                world: World::new(&m.mesh, &matrix),
                matrix,
                target,
                priority,
                rotation,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let running = jobs.start(job);
    let cancel = running.cancel_flag();
    let mut events = Events {
        job,
        emit,
        last: None,
        sent: 0.0,
    };
    let mut control = Control::new(cancel, |f| events.send(f));
    let mut out = Map::new();
    let targets: Vec<&Part> = parts.iter().filter(|p| p.target).collect();
    let boxes = |list: &[&Part]| -> Result<Vec<Aabb>, String> {
        list.iter()
            .map(|p| {
                p.world
                    .bounds()
                    .ok_or_else(|| format!("{ACTION}: a part has no geometry."))
            })
            .collect()
    };
    match action.as_str() {
        "align" => {
            let how: Align = field(&meta, "align").ok_or_else(damaged)?;
            let moves = align::align(&boxes(&targets)?, how, platform);
            let matrices: Vec<Value> = targets
                .iter()
                .zip(moves)
                .map(|(p, d)| matrix_json(&translated(&p.matrix, d)))
                .collect();
            out.insert("matrices".into(), json!(matrices));
        }
        "duplicate" => {
            let how: Duplicate = field(&meta, "duplicate").ok_or_else(damaged)?;
            let group = union_all(&boxes(&targets)?).ok_or_else(damaged)?;
            let others: Vec<&Part> = parts.iter().filter(|p| !p.target).collect();
            let others = boxes(&others)?;
            let copies = duplicate::duplicate(&group, &others, platform, &how)?;
            let matrices: Vec<Value> = targets
                .iter()
                .map(|p| matrix_json(&translated(&p.matrix, copies.group)))
                .collect();
            let mut made = Vec::with_capacity(copies.offsets.len() * targets.len());
            for offset in &copies.offsets {
                control.check().map_err(run_error)?;
                for (i, p) in targets.iter().enumerate() {
                    made.push(json!({ "source": i, "matrix": matrix_json(&translated(&p.matrix, *offset)) }));
                }
            }
            out.insert("matrices".into(), json!(matrices));
            out.insert("copies".into(), json!(made));
        }
        "lay-flat" => {
            let point: V3 = field(&meta, "point").ok_or_else(damaged)?;
            let normal: V3 = field(&meta, "normal").ok_or_else(damaged)?;
            let [part] = targets.as_slice() else {
                return Err(damaged());
            };
            let turn = orient::lay_flat_at(&part.world, point, normal)
                .ok_or_else(|| format!("{ACTION}: the part has no geometry."))?;
            out.insert(
                "matrices".into(),
                json!([matrix_json(&turn.apply(&part.matrix))]),
            );
        }
        "lay-flat-auto" | "min-box" => {
            let axes: Option<BoxAxes> = field(&meta, "axes");
            if action == "min-box" && axes.is_none() {
                return Err(damaged());
            }
            let mut matrices = Vec::with_capacity(targets.len());
            for (i, p) in targets.iter().enumerate() {
                control.check().map_err(run_error)?;
                let turn = match axes {
                    Some(axes) => orient::min_box(&p.world, axes),
                    None => orient::lay_flat_auto(&p.world),
                }
                .ok_or_else(|| format!("{ACTION}: a part has no geometry."))?;
                matrices.push(matrix_json(&turn.apply(&p.matrix)));
                control.progress((i + 1) as f64 / targets.len() as f64);
            }
            out.insert("matrices".into(), json!(matrices));
        }
        "orient" => {
            let goal: Goal = field(&meta, "goal").ok_or_else(damaged)?;
            let [part] = targets.as_slice() else {
                return Err(damaged());
            };
            let (now, candidates) =
                orient::orient(&part.world, goal, &mut control).map_err(run_error)?;
            let list: Vec<Value> = candidates
                .iter()
                .map(|c| {
                    json!({
                        "matrix": matrix_json(&c.turn.apply(&part.matrix)),
                        "supportArea": c.scores.support_area,
                        "height": c.scores.height,
                        "footprint": c.scores.footprint,
                    })
                })
                .collect();
            out.insert("now".into(), json!(now));
            out.insert("candidates".into(), json!(list));
        }
        "pack" => {
            let method: Method = field(&meta, "pack").ok_or_else(damaged)?;
            pack_parts(&parts, &method, platform, &mut control, &mut out)?;
        }
        _ => return Err(damaged()),
    }
    control.progress(1.0);
    drop(control);
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    drop(running);
    Ok(bundle::encode(&out, &[]))
}

/// Packs the targets round the other parts, checks the meshes of the result for collisions, and
/// fills `out`. The pack takes the first 85 % of the progress, the check the rest.
fn pack_parts(
    parts: &[Part],
    method: &Method,
    platform: V3,
    control: &mut Control,
    out: &mut Map<String, Value>,
) -> Result<(), String> {
    let targets: Vec<&Part> = parts.iter().filter(|p| p.target).collect();
    if targets.is_empty() {
        return Err("Could not pack the parts: there are no parts to pack.".into());
    }
    let items: Vec<Item> = targets
        .iter()
        .map(|p| Item {
            world: p.world.clone(),
            priority: p.priority,
            rotation: p.rotation,
        })
        .collect();
    let obstacles: Vec<World> = parts
        .iter()
        .filter(|p| !p.target)
        .map(|p| p.world.clone())
        .collect();
    let outcome = pack::pack(
        &items,
        &obstacles,
        platform,
        method,
        &mut control.sub(0.0, 0.85),
    )
    .map_err(run_error)??;
    // The check, on the meshes where they now are.
    let mut bodies = Vec::with_capacity(parts.len());
    let mut moved = Vec::with_capacity(parts.len());
    let mut names: Vec<&str> = targets.iter().map(|p| p.name.as_str()).collect();
    for (item, turn) in items.iter().zip(&outcome.turns) {
        bodies.push(Body::new(orient::moved(&item.world, turn)));
        moved.push(true);
    }
    for p in parts.iter().filter(|p| !p.target) {
        bodies.push(Body::new(p.world.clone()));
        moved.push(false);
        names.push(p.name.as_str());
    }
    let hits =
        collide::collisions(&bodies, &moved, &mut control.sub(0.85, 1.0)).map_err(run_error)?;
    if let Some(&(a, b)) = hits.first() {
        let more = match hits.len() {
            1 => String::new(),
            n => format!(" ({n} pairs in all)"),
        };
        return Err(format!(
            "Packing failed: \"{}\" and \"{}\" would collide{more}. Nothing was changed.",
            names[a], names[b]
        ));
    }
    let volume: f64 = items
        .iter()
        .zip(&outcome.fits)
        .filter(|(_, fits)| **fits)
        .map(|(item, _)| item.world.volume().abs())
        .sum();
    let density = if outcome.height > 0.0 {
        volume / (outcome.height * platform[0] * platform[1])
    } else {
        0.0
    };
    let matrices: Vec<Value> = targets
        .iter()
        .zip(&outcome.turns)
        .map(|(p, t)| matrix_json(&t.apply(&p.matrix)))
        .collect();
    out.insert("matrices".into(), json!(matrices));
    out.insert("fits".into(), json!(outcome.fits));
    out.insert("height".into(), json!(outcome.height));
    out.insert("volume".into(), json!(volume));
    out.insert("density".into(), json!(density));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Packing presets

/// The user's packing presets, in the app's config folder: by method, then by name, each the
/// settings the window saved. Never sent anywhere.
pub const PRESETS_FILE: &str = "packing-presets.json";

/// The other presets files, by the group the window names (`settings_presets_load`): the file
/// and what the messages call its contents. Each holds presets by scope (an analysis, a mesh
/// tool), then by name, as the packing presets do by method.
pub fn preset_file(group: &str) -> Option<(&'static str, &'static str)> {
    match group {
        "analysis" => Some(("analysis-presets.json", "analysis presets")),
        "modify" => Some(("mesh-tool-presets.json", "mesh tool presets")),
        _ => None,
    }
}

/// `packing_presets_load`: the presets file in `dir`; empty before anything was saved there.
pub fn load_presets(dir: &Path) -> Result<Map<String, Value>, String> {
    load_preset_file(dir, PRESETS_FILE, "packing presets")
}

/// `packing_presets_save`: see `save_preset_file`.
pub fn save_presets(dir: &Path, presets: &Map<String, Value>) -> Result<(), String> {
    save_preset_file(dir, PRESETS_FILE, "packing presets", presets)
}

/// The presets file `file` in `dir`; empty before anything was saved there. `what` names its
/// contents in messages ("packing presets").
pub fn load_preset_file(dir: &Path, file: &str, what: &str) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(dir.join(file)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => {
            return Err(format!(
                "Could not read your {what}: {}.",
                formats::read_error(&e)
            ));
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(all)) if all.values().all(Value::is_object) => Ok(all),
        _ => Err(format!(
            "Could not read your {what}: {file} in the settings folder is damaged."
        )),
    }
}

/// Writes the presets to `file` in `dir` (created when missing) through a temporary file renamed
/// into place, so a crash never leaves half a file.
pub fn save_preset_file(
    dir: &Path,
    file: &str,
    what: &str,
    presets: &Map<String, Value>,
) -> Result<(), String> {
    let fail = |reason: String| format!("Could not save your {what}: {reason}.");
    if !presets.values().all(Value::is_object) {
        return Err(fail("they arrived in the wrong form".into()));
    }
    let mut text = serde_json::to_string_pretty(presets).map_err(|e| fail(e.to_string()))?;
    text.push('\n');
    std::fs::create_dir_all(dir).map_err(|e| fail(formats::write_error(&e)))?;
    write_text(&dir.join(file), &text).map_err(|e| fail(formats::write_error(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_saved_and_read_back_and_a_damaged_file_says_so() {
        let dir = crate::test_dir::temp_dir("packing-presets");
        assert_eq!(load_presets(&dir), Ok(Map::new()));
        let mut presets = Map::new();
        presets.insert(
            "monte-carlo".into(),
            json!({"Tight": {"performance": 2, "distance": 0.5}}),
        );
        save_presets(&dir.join("new"), &presets).unwrap();
        assert_eq!(load_presets(&dir.join("new")), Ok(presets.clone()));
        // Saving again replaces the file.
        presets.insert("bounding-box".into(), json!({}));
        save_presets(&dir.join("new"), &presets).unwrap();
        assert_eq!(load_presets(&dir.join("new")).unwrap().len(), 2);
        std::fs::write(dir.join(PRESETS_FILE), "{\"x\": 3").unwrap();
        assert!(load_presets(&dir).unwrap_err().contains("damaged"));
        let mut bad = Map::new();
        bad.insert("x".into(), json!(3));
        assert!(save_presets(&dir, &bad).is_err());
    }

    #[test]
    fn analysis_and_tool_presets_have_files_of_their_own() {
        let dir = crate::test_dir::temp_dir("settings-presets");
        let (file, what) = preset_file("analysis").unwrap();
        assert_ne!(file, PRESETS_FILE);
        assert_ne!(Some((file, what)), preset_file("modify"));
        assert_eq!(preset_file("scripts"), None);
        let mut presets = Map::new();
        presets.insert("wall".into(), json!({"Thin": {"threshold": 0.8}}));
        save_preset_file(&dir, file, what, &presets).unwrap();
        assert_eq!(load_preset_file(&dir, file, what), Ok(presets));
        // The packing presets are untouched.
        assert_eq!(load_presets(&dir), Ok(Map::new()));
        std::fs::write(dir.join(file), "[]").unwrap();
        let err = load_preset_file(&dir, file, what).unwrap_err();
        assert!(
            err.starts_with("Could not read your analysis presets"),
            "{err}"
        );
    }
}
