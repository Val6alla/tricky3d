//! Smoothing: Taubin's λ|μ smoothing (each step pulls every point towards the average of its
//! neighbours, then pushes it back a little further), which flattens roughness without the
//! shrinking plain averaging gives. It shares the shell tidying's move check (src/ops/collapse.rs
//! `try_move`: a point only moves when its triangles keep their shape).
//! - Keep sharp edges: an edge whose triangles meet at more than the angle stays sharp. A point
//!   on two such edges (along a crease) only moves along them; a point on one, or three or more
//!   (a corner), stays.
//! - Selected triangles only: only points whose triangles are all selected move.
//! - Keep the volume: a closed part's volume is put back at the end by moving the points that
//!   moved along their normals, all by the same distance.
//!
//! Points on a hole's border or a non-manifold edge stay where they are.

use rayon::prelude::*;
use serde::Deserialize;

use super::{Modified, Soundness, inside, mask, on_open_edges};
use crate::mesh::{Mesh, fmt_count};
use crate::ops::collapse::{self, Work, normal, rounded};
use crate::repair::exact::{self, P3, add, dot, scale, sub};
use crate::repair::topology::{Edges, side_ends};
use crate::repair::{Control, RepairError, RepairResult};

/// `SmoothSettings` in src/modify/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmoothSettings {
    /// λ, 0 to 1: how far each step pulls a point towards its neighbours' average.
    pub strength: f64,
    pub iterations: u32,
    pub keep_edges: bool,
    /// Degrees between the two triangles' normals from which an edge is sharp.
    pub edge_angle: f64,
    pub selected_only: bool,
    pub keep_volume: bool,
}

pub const MAX_ITERATIONS: u32 = 200;

impl SmoothSettings {
    pub fn problem(&self) -> Option<String> {
        if !(self.strength.is_finite() && self.strength > 0.0 && self.strength <= 1.0) {
            return Some("Enter a strength greater than 0 and at most 1.".into());
        }
        if !(1..=MAX_ITERATIONS).contains(&self.iterations) {
            return Some(format!("Enter between 1 and {MAX_ITERATIONS} iterations."));
        }
        if !(self.edge_angle.is_finite() && (0.0..=180.0).contains(&self.edge_angle)) {
            return Some("Enter an edge angle between 0° and 180°.".into());
        }
        None
    }
}

/// The pass band of Taubin's filter: μ = λ / (kPB·λ − 1).
const PASS_BAND: f64 = 0.1;

/// How a point may move.
#[derive(Clone, Debug, PartialEq)]
enum Freedom {
    Fixed,
    /// Towards the average of all its neighbours.
    Free,
    /// Along a crease, towards the average of its two neighbours on it.
    Along([u32; 2]),
}

/// Smooths `mesh`; where the result has triangles crossing (and the part had none), it runs again
/// with the points round them left where they were, up to three times.
pub fn smooth(
    mesh: &Mesh,
    selection: &[u32],
    s: &SmoothSettings,
    before: Soundness,
    control: &mut Control,
) -> RepairResult<Modified> {
    let n = mesh.triangle_count();
    let chosen = s.selected_only.then(|| mask(selection, n));
    if chosen.as_ref().is_some_and(|c| !c.contains(&true)) {
        return Err(RepairError::Failed(
            "Select the triangles to smooth first, or switch off Selected triangles only.".into(),
        ));
    }
    let tries = super::remesh::TRIES;
    let mut held = vec![false; mesh.vertex_count()];
    let mut last = None;
    for attempt in 0..tries {
        let span = 1.0 / tries as f64;
        let mut part = control.sub(attempt as f64 * span, (attempt + 1) as f64 * span);
        let out = smooth_once(mesh, chosen.as_deref(), s, &held, &mut part.sub(0.0, 0.9))?;
        if !out.changed || before.crossings > 0 {
            return Ok(out);
        }
        part.stage("Checking the result");
        let pairs = crate::repair::tritri::intersecting_pairs(&out.mesh, &mut part.sub(0.9, 1.0))?;
        if pairs.is_empty() {
            return Ok(out);
        }
        let size = pairs
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .map(|t| collapse::longest_side(&exact::corners(&out.mesh, t)))
            .fold(0.0f64, f64::max);
        collapse::lock_round(mesh, &out.mesh, &pairs, 2.0 * size, &mut held);
        last = Some(out);
    }
    // Still crossing: `modify::run` refuses it with its sentence.
    Ok(last.unwrap_or_else(|| Modified::unchanged(mesh, "No point could be smoothed.")))
}

fn smooth_once(
    mesh: &Mesh,
    chosen: Option<&[bool]>,
    s: &SmoothSettings,
    held: &[bool],
    control: &mut Control,
) -> RepairResult<Modified> {
    control.stage("Smoothing");
    let mut freedom = freedoms(mesh, chosen, s);
    for (f, &h) in freedom.iter_mut().zip(held) {
        if h {
            *f = Freedom::Fixed;
        }
    }
    control.check()?;
    let mut work = Work::new(mesh, f64::INFINITY);
    let closed = !on_open_edges(mesh).contains(&true);
    let volume0 = volume(&work);
    let lambda = s.strength;
    let mu = lambda / (PASS_BAND * lambda - 1.0);
    let steps = 2 * s.iterations as usize;
    for step in 0..steps {
        control.check()?;
        control.progress(0.9 * step as f64 / steps as f64);
        let factor = if step % 2 == 0 { lambda } else { mu };
        let w: &Work = &work;
        let targets: Vec<Option<P3>> = (0..w.pos.len() as u32)
            .into_par_iter()
            .map_init(Vec::new, |ring, v| {
                let towards = match &freedom[v as usize] {
                    Freedom::Fixed => return None,
                    Freedom::Free => {
                        w.neighbours(v, ring);
                        if ring.is_empty() {
                            return None;
                        }
                        let sum = ring
                            .iter()
                            .fold([0.0; 3], |m, &u| add(m, w.pos[u as usize]));
                        scale(sum, 1.0 / ring.len() as f64)
                    }
                    Freedom::Along([a, b]) => {
                        scale(add(w.pos[*a as usize], w.pos[*b as usize]), 0.5)
                    }
                };
                let p = w.pos[v as usize];
                Some(rounded(add(p, scale(sub(towards, p), factor))))
            })
            .collect();
        for (v, x) in targets.into_iter().enumerate() {
            if let Some(x) = x {
                collapse::try_move(&mut work, v as u32, x);
            }
        }
    }
    let mut warnings = Vec::new();
    if s.keep_volume {
        if closed && volume0.abs() > 0.0 {
            restore_volume(&mut work, &freedom, volume0);
        } else {
            warnings.push(
                "The part is not closed, so its volume could not be kept; it was smoothed all \
                 the same."
                    .into(),
            );
        }
    }
    control.progress(1.0);
    let positions: Vec<f32> = work.pos.iter().flatten().map(|&x| x as f32).collect();
    let moved = positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(mesh.positions.as_chunks::<3>().0)
        .filter(|(a, b)| a != b)
        .count();
    if moved == 0 {
        return Ok(Modified::unchanged(mesh, "No point could be smoothed."));
    }
    let mut out = Modified::moved(
        Mesh {
            positions,
            indices: mesh.indices.clone(),
        },
        format!(
            "Smoothed {} {}.",
            fmt_count(moved as u64),
            if moved == 1 { "point" } else { "points" }
        ),
    );
    out.warnings = warnings;
    Ok(out)
}

/// Each vertex's freedom (module comment).
fn freedoms(mesh: &Mesh, chosen: Option<&[bool]>, s: &SmoothSettings) -> Vec<Freedom> {
    let movable = inside(mesh, chosen);
    let edges = Edges::new(mesh);
    let cos_limit = s.edge_angle.to_radians().cos();
    let normals: Vec<P3> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| normal(&t.map(|v| exact::vertex(mesh, v))))
        .collect();
    let mut sharp: Vec<Vec<u32>> = vec![Vec::new(); mesh.vertex_count()];
    let mut fixed = vec![false; mesh.vertex_count()];
    for e in 0..edges.len() as u32 {
        let [a, b] = edges.ends(e);
        if edges.degree(e) != 2 {
            fixed[a as usize] = true;
            fixed[b as usize] = true;
            continue;
        }
        if !s.keep_edges {
            continue;
        }
        let [x, y] = [0, 1].map(|i| edges.sides(e)[i] / 3);
        let (nx, ny) = (normals[x as usize], normals[y as usize]);
        let (lx, ly) = (exact::length(nx), exact::length(ny));
        let creased = lx == 0.0 || ly == 0.0 || dot(nx, ny) < cos_limit * lx * ly;
        if creased {
            let (p, q) = side_ends(mesh, edges.sides(e)[0]);
            sharp[p as usize].push(q);
            sharp[q as usize].push(p);
        }
    }
    (0..mesh.vertex_count())
        .map(|v| {
            if !movable[v] || fixed[v] {
                return Freedom::Fixed;
            }
            match sharp[v].as_slice() {
                [] => Freedom::Free,
                &[a, b] => Freedom::Along([a, b]),
                _ => Freedom::Fixed,
            }
        })
        .collect()
}

/// Signed volume of the alive triangles.
fn volume(work: &Work) -> f64 {
    work.tris
        .par_iter()
        .zip(&work.alive)
        .filter(|(_, a)| **a)
        .map(|(t, _)| {
            let [a, b, c] = t.map(|v| work.pos[v as usize]);
            dot(a, exact::cross(b, c)) / 6.0
        })
        .sum()
}

/// Moves the points that may move along their normals, all by one distance, until the volume is
/// `target` again (first order: the distance is the volume missing over their share of the area;
/// three rounds settle it to rounding).
fn restore_volume(work: &mut Work, freedom: &[Freedom], target: f64) {
    for _ in 0..3 {
        let now = volume(work);
        let missing = target - now;
        let (normals, area): (Vec<P3>, f64) = {
            let mut normals = vec![[0.0; 3]; work.pos.len()];
            let mut area = 0.0;
            for (t, tri) in work.tris.iter().enumerate() {
                if !work.alive[t] {
                    continue;
                }
                let n = normal(&work.corners(t as u32));
                for &v in tri {
                    if freedom[v as usize] == Freedom::Free {
                        normals[v as usize] = add(normals[v as usize], n);
                        area += exact::length(n) / 6.0;
                    }
                }
            }
            (normals, area)
        };
        if area <= 0.0 || missing.abs() <= 1e-9 * target.abs() {
            return;
        }
        let d = missing / area;
        for v in 0..work.pos.len() {
            if freedom[v] != Freedom::Free {
                continue;
            }
            let n = normals[v];
            let l = exact::length(n);
            if l > 0.0 {
                let x = rounded(add(work.pos[v], scale(n, d / l)));
                collapse::try_move(work, v as u32, x);
            }
        }
    }
}
