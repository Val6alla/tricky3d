//! Part analyses (src/analysis/ipc.ts): one command for every kind. The answer is numbers for the
//! panel plus what the view draws: a colour class per triangle of the part, shapes in world space
//! (triangles and line segments) and markers in the result's own fields.

use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{CANCELLED, PROGRESS_INTERVAL, RepairJobs, checked_part, field, run_error};
use crate::analysis::compare::{CompareSettings, Surface, compare};
use crate::analysis::gravity::centre_of_gravity;
use crate::analysis::lift::{Solid, interlock, removability};
use crate::analysis::shadow::{union_area, upward};
use crate::analysis::skin::{MAX_ANGLE, SkinSettings, skins};
use crate::analysis::standard;
use crate::analysis::support::{SupportSettings, support_volume};
use crate::analysis::wall::{WallSettings, wall_thickness};
use crate::arrange::World;
use crate::arrange::collide::{Body, closer_than, collide, crossing_triangles};
use crate::bundle::{self, BlobRef, Bundle, Extra, OutMesh};
use crate::mesh::{Matrix, Mesh};
use crate::repair::topology::Edges;
use crate::repair::{Control, status};

/// The Tauri event an analysis reports its progress with.
pub const ANALYSIS_PROGRESS: &str = "analysis-progress";

/// `AnalysisProgress` in src/analysis/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnalysisProgress {
    pub job: u64,
    /// 0..1, never decreasing.
    pub fraction: f64,
}

/// Sends progress at most once per PROGRESS_INTERVAL, and the end at once.
struct Events<'a> {
    job: u64,
    emit: &'a mut dyn FnMut(&AnalysisProgress),
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
        (self.emit)(&AnalysisProgress {
            job: self.job,
            fraction,
        });
    }
}

/// What an analysis draws besides its numbers.
#[derive(Default)]
struct Drawing {
    /// A colour class per triangle of the part (the meaning depends on the kind).
    classes: Option<Vec<u32>>,
    /// World triangles, x, y, z per corner.
    triangles: Vec<f32>,
    /// World line segments, x, y, z per end.
    lines: Vec<f32>,
    /// A value per triangle of the part (wall thickness; NaN where none).
    values: Option<Vec<f32>>,
    /// Refined world copies with a value per vertex (mesh compare), each with its role.
    surfaces: Vec<(&'static str, Surface)>,
}

/// A part of the request: its own mesh and its matrix.
struct Placed {
    name: String,
    mesh: Mesh,
    matrix: Matrix,
}

/// `analyze_part`: meta {job, kind, settings}; one bundle mesh per part with info {name, matrix}
/// (one part for the part analyses). Answers a bundle whose meta is {result} and whose one mesh
/// carries the drawing: its positions are world triangles (indices 0, 1, 2, ...), with the extra
/// arrays "classes" (u32 per triangle of the part, when the kind colours it), "values" (f32 per
/// triangle of the part) and "lines" (f32, world segments). Mesh compare adds a mesh per refined
/// copy, info {surface: "reference" | "comparison"}, with a distance per vertex in "values". Kinds
/// and their settings:
/// - "standard", "gravity", "shadow": none;
/// - "support": SupportSettings; "skin": SkinSettings; "wall": WallSettings;
/// - "compare": CompareSettings, with two parts, the reference first;
/// - "removability": none; "interlock": none, with two parts or more;
/// - "collisions": {minDistance} (mm), with every part to check: pairs as indices.
///
/// Cancel (`repair_cancel` with the job) fails it with CANCELLED.
pub fn analyze_part(
    bundle: Bundle,
    jobs: &RepairJobs,
    emit: &mut dyn FnMut(&AnalysisProgress),
) -> Result<Vec<u8>, String> {
    const ACTION: &str = "Could not analyze the part";
    let damaged = || format!("{ACTION}: the request from the window is damaged.");
    let job = bundle
        .meta
        .get("job")
        .and_then(Value::as_u64)
        .ok_or_else(damaged)?;
    let kind = bundle
        .meta
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(damaged)?
        .to_string();
    let meta = bundle.meta;
    let parts = bundle
        .meshes
        .into_iter()
        .enumerate()
        .map(|(n, m)| {
            let (name, matrix) = checked_part(&m, n).map_err(|r| format!("{ACTION}: {r}."))?;
            Ok(Placed {
                name,
                mesh: m.mesh,
                matrix,
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
    let mut drawing = Drawing::default();
    let one = || match parts.as_slice() {
        [part] => Ok(part),
        _ => Err(damaged()),
    };
    let result: Value = match kind.as_str() {
        "standard" => {
            let part = one()?;
            let a = standard::analyze(&part.mesh, &part.matrix, &mut control).map_err(run_error)?;
            json!(a)
        }
        "gravity" => {
            let part = one()?;
            let world = World::new(&part.mesh, &part.matrix);
            let g = centre_of_gravity(&world, &mut control)
                .map_err(run_error)?
                .ok_or_else(|| {
                    format!(
                        "\"{}\" encloses no volume, so it has no centre of gravity. Close its holes first.",
                        part.name
                    )
                })?;
            json!(g)
        }
        "shadow" => {
            let part = one()?;
            let detail =
                status::analyze(&part.mesh, &mut control.sub(0.0, 0.2)).map_err(run_error)?;
            if let Some(why) = unsound(&detail.status) {
                return Err(format!(
                    "Shadow area needs a closed, sound mesh, and \"{}\" {why}. Repair the part first.",
                    part.name
                ));
            }
            let world = World::new(&part.mesh, &part.matrix);
            let (flats, up) = upward(&world);
            let area = union_area(&flats, &mut control.sub(0.2, 0.95)).map_err(run_error)?;
            for t in &flats {
                for p in t {
                    drawing
                        .triangles
                        .extend_from_slice(&[p[0] as f32, p[1] as f32, 0.0]);
                }
            }
            drawing.lines = outline(&part.mesh, &world, &up);
            json!({ "area": area })
        }
        "support" => {
            let part = one()?;
            let s: SupportSettings = field(&meta, "settings").ok_or_else(damaged)?;
            if !(0.0..=90.0).contains(&s.angle) {
                return Err(damaged());
            }
            let world = World::new(&part.mesh, &part.matrix);
            let detail = support_volume(&part.mesh, &world, &s, &mut control).map_err(run_error)?;
            drawing.classes = Some(detail.classes.iter().map(|&c| c as u32).collect());
            drawing.triangles = detail.blocks;
            json!(detail.support)
        }
        "skin" => {
            let part = one()?;
            let s: SkinSettings = field(&meta, "settings").ok_or_else(damaged)?;
            let angle_ok = |a: f64| (0.0..=MAX_ANGLE).contains(&a);
            if !(angle_ok(s.up_angle)
                && angle_ok(s.down_angle)
                && s.min_component.is_finite()
                && s.min_component >= 0.0)
            {
                return Err(damaged());
            }
            let world = World::new(&part.mesh, &part.matrix);
            let found = skins(&part.mesh, &world, &s, &mut control).map_err(run_error)?;
            drawing.classes = Some(found.classes.iter().map(|&c| c as u32).collect());
            json!({ "up": found.up, "down": found.down })
        }
        "wall" => {
            let part = one()?;
            let s: WallSettings = field(&meta, "settings").ok_or_else(damaged)?;
            if !(s.threshold.is_finite()
                && s.threshold > 0.0
                && (0.0..=100.0).contains(&s.acceptance))
            {
                return Err(damaged());
            }
            let world = World::new(&part.mesh, &part.matrix);
            let found = wall_thickness(&world, &s, &mut control).map_err(run_error)?;
            drawing.values = Some(found.thickness);
            json!(found.result)
        }
        "compare" => {
            let [reference, comparison] = parts.as_slice() else {
                return Err(damaged());
            };
            let s: CompareSettings = field(&meta, "settings").ok_or_else(damaged)?;
            let found = compare(
                &World::new(&reference.mesh, &reference.matrix),
                &World::new(&comparison.mesh, &comparison.matrix),
                &s,
                &mut control,
            )
            .map_err(run_error)?;
            let forward = found.forward.map(|(r, surface)| {
                drawing.surfaces.push(("reference", surface));
                r
            });
            let backward = found.backward.map(|(r, surface)| {
                drawing.surfaces.push(("comparison", surface));
                r
            });
            json!({ "forward": forward, "backward": backward, "fit": found.fit })
        }
        "removability" => {
            let part = one()?;
            let world = World::new(&part.mesh, &part.matrix);
            let (found, classes) =
                removability(&part.mesh, &world, &mut control).map_err(run_error)?;
            drawing.classes = Some(classes.iter().map(|&c| c as u32).collect());
            json!(found)
        }
        "interlock" => {
            if parts.len() < 2 {
                return Err(damaged());
            }
            let solids: Vec<Solid> = parts
                .par_iter()
                .map(|p| Solid::new(World::new(&p.mesh, &p.matrix)))
                .collect();
            json!(interlock(&solids, &mut control).map_err(run_error)?)
        }
        "collisions" => {
            let limit = meta
                .get("settings")
                .and_then(|s| s.get("minDistance"))
                .and_then(Value::as_f64)
                .ok_or_else(damaged)?;
            if !(0.0..=MAX_MIN_DISTANCE).contains(&limit) {
                return Err(damaged());
            }
            collisions(&parts, limit, &mut control, &mut drawing).map_err(run_error)?
        }
        _ => return Err(damaged()),
    };
    control.progress(1.0);
    drop(control);
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    drop(running);
    let mut out = Map::new();
    out.insert("result".into(), result);
    Ok(encode(&out, &drawing))
}

/// Largest minimum distance the collision check takes (mm).
pub const MAX_MIN_DISTANCE: f64 = 100.0;

/// Most triangles the collision drawing shows; the rest of a big crossing is left out.
const MAX_DRAWN: usize = 300_000;

/// Every pair of parts that collide (as Packing checks them) or come closer than `limit`, with the
/// crossing triangles to draw and a line between the closest points of a pair too close.
fn collisions(
    parts: &[Placed],
    limit: f64,
    control: &mut Control,
    drawing: &mut Drawing,
) -> crate::repair::RepairResult<Value> {
    let bodies: Vec<Body> = parts
        .par_iter()
        .map(|p| Body::new(World::new(&p.mesh, &p.matrix)))
        .collect();
    control.progress(0.1);
    let boxes: Vec<Option<crate::arrange::Aabb>> =
        bodies.iter().map(|b| b.world().bounds()).collect();
    let near = |a: &crate::arrange::Aabb, b: &crate::arrange::Aabb| {
        (0..3).all(|k| a[0][k] - limit <= b[1][k] && b[0][k] - limit <= a[1][k])
    };
    let mut pairs = Vec::new();
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            if let (Some(a), Some(b)) = (boxes[i], boxes[j])
                && near(&a, &b)
            {
                pairs.push((i, j));
            }
        }
    }
    let mut found = Vec::new();
    let mut drawn = 0;
    for (k, &(i, j)) in pairs.iter().enumerate() {
        control.check()?;
        let (a, b) = (&bodies[i], &bodies[j]);
        if collide(a, b) {
            found.push(json!({ "a": i, "b": j, "kind": "collide", "distance": 0.0 }));
            let (mine, theirs) = crossing_triangles(a, b);
            for (body, list) in [(a, mine), (b, theirs)] {
                for t in list {
                    if drawn >= MAX_DRAWN {
                        break;
                    }
                    drawn += 1;
                    for p in body.world().corners(body.world().triangles[t as usize]) {
                        drawing.triangles.extend_from_slice(&[
                            p[0] as f32,
                            p[1] as f32,
                            p[2] as f32,
                        ]);
                    }
                }
            }
        } else if limit > 0.0
            && let Some((d, p, q)) = closer_than(a, b, limit)
        {
            found.push(json!({ "a": i, "b": j, "kind": "near", "distance": d }));
            for c in [p, q] {
                drawing
                    .lines
                    .extend_from_slice(&[c[0] as f32, c[1] as f32, c[2] as f32]);
            }
        }
        control.progress(0.1 + 0.9 * (k + 1) as f64 / pairs.len() as f64);
    }
    Ok(json!({ "pairs": found, "parts": parts.len(), "minDistance": limit }))
}

/// Why a mesh is not closed and sound, as the end of a sentence; None when it is.
fn unsound(s: &status::RepairStatus) -> Option<String> {
    let plural = |n: u64, one: &str, many: &str| {
        format!(
            "{} {}",
            crate::mesh::fmt_count(n),
            if n == 1 { one } else { many }
        )
    };
    if s.triangles == 0 {
        Some("has no triangles".into())
    } else if s.border_edges > 0 {
        Some(format!("has {}", plural(s.holes.max(1), "hole", "holes")))
    } else if s.non_manifold_edges > 0 {
        Some(format!(
            "has {}",
            plural(
                s.non_manifold_edges,
                "edge shared by more than two triangles",
                "edges shared by more than two triangles"
            )
        ))
    } else if s.inverted_triangles > 0 {
        Some(format!(
            "has {}",
            plural(
                s.inverted_triangles,
                "flipped triangle",
                "flipped triangles"
            )
        ))
    } else if !s.oriented {
        Some("cannot be oriented".into())
    } else {
        None
    }
}

/// The shadow's outline on the platform: edges between a triangle that faces up and one that
/// does not, flattened to Z = 0.
fn outline(mesh: &Mesh, world: &World, up: &[bool]) -> Vec<f32> {
    let edges = Edges::new(mesh);
    let mut lines = Vec::new();
    for e in 0..edges.len() as u32 {
        let sides = edges.sides(e);
        let ups = sides.iter().filter(|&&s| up[(s / 3) as usize]).count();
        if ups == 0 || ups == sides.len() {
            continue;
        }
        for v in edges.ends(e) {
            let p = world.points[v as usize];
            lines.extend_from_slice(&[p[0] as f32, p[1] as f32, 0.0]);
        }
    }
    lines
}

fn encode(meta: &Map<String, Value>, drawing: &Drawing) -> Vec<u8> {
    let indices: Vec<u32> = (0..(drawing.triangles.len() / 3) as u32).collect();
    let mut extra: Extra = Vec::new();
    if let Some(classes) = &drawing.classes {
        extra.push(("classes", BlobRef::U32(classes)));
    }
    if !drawing.lines.is_empty() {
        extra.push(("lines", BlobRef::F32(&drawing.lines)));
    }
    if let Some(values) = &drawing.values {
        extra.push(("values", BlobRef::F32(values)));
    }
    let mut meshes = vec![OutMesh {
        info: Map::new(),
        positions: &drawing.triangles,
        indices: &indices,
        edges: None,
    }];
    let mut extras = vec![extra];
    for (role, surface) in &drawing.surfaces {
        let mut info = Map::new();
        info.insert("surface".into(), json!(role));
        meshes.push(OutMesh {
            info,
            positions: &surface.points,
            indices: &surface.triangles,
            edges: None,
        });
        extras.push(vec![("values", BlobRef::F32(&surface.distances))]);
    }
    bundle::encode_with_extra(meta, &meshes, &extras)
}
