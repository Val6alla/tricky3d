//! Triangle reduction: fewer, larger triangles within a deviation from the original surface. It
//! collapses edges with Generate shell's engine (src/ops/collapse.rs: quadric costs, the manifold
//! and shape checks, parallel rounds) and its own rule for where a vertex may go:
//! - every original vertex stays within Max. deformation of the reduced surface: each one is kept
//!   on the reduced triangle nearest it, and a collapse must find each point of the triangles it
//!   changes such a triangle within the limit;
//! - each changed triangle's corners, centre, edge midpoints and three inner points stay within
//!   90 % of the limit of the original surface;
//! - no edge grows past Max. edge length; with Selected triangles only, no vertex of an
//!   unselected triangle moves; the borders of holes stay as they are.
//!
//! The result is checked for crossing triangles as the shell's tidying does: where some appear,
//! the reduction runs again with the vertices round them left alone.

use serde::Deserialize;

use super::{Modified, Soundness, inside, mask, on_open_edges, triangles};
use crate::mesh::Mesh;
use crate::ops::collapse::{
    self, Plan, Quadric, Rounds, Rules, Scratch, Work, centre, lock_round, moved,
    near_other_sheets, rounded, shape_fits,
};
use crate::ops::shell::field::{Surface, closest_on_triangle};
use crate::repair::exact::{self, P3, add, dot, scale, sub};
use crate::repair::{Control, RepairError, RepairResult, tritri};

/// `ReduceSettings` in src/modify/types.ts. Lengths in mm.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReduceSettings {
    /// Triangles the part should end with (ignored with `lowest`).
    pub target: u64,
    /// Set target as low as possible: reduce while the limits allow.
    pub lowest: bool,
    /// Max. deformation: how far the surface may move.
    pub max_deviation: f64,
    /// Max. edge length: no edge of the result is longer.
    pub max_edge: f64,
    pub selected_only: bool,
    /// Fast mode: in mesh order and stop at the target, not cheapest first all over.
    pub fast: bool,
}

/// Smallest Max. deformation, mm: below it f32 coordinates alone use it up.
pub const MIN_DEVIATION: f64 = 1e-4;

impl ReduceSettings {
    pub fn problem(&self) -> Option<String> {
        if !(self.max_deviation.is_finite() && self.max_deviation >= MIN_DEVIATION) {
            return Some(format!(
                "Enter a Max. deformation of at least {MIN_DEVIATION} mm."
            ));
        }
        if !(self.max_edge.is_finite() && self.max_edge > 0.0) {
            return Some("Enter a Max. edge length greater than 0 mm.".into());
        }
        if !self.lowest && self.target < 4 {
            return Some("Enter a target of at least 4 triangles.".into());
        }
        None
    }
}

/// Share of Max. deformation a changed triangle's samples may lie from the original surface.
const SAMPLED: f64 = 0.9;
/// The nearest original triangle may lean away from a new one by up to this much past a right
/// angle (cosine): enough for a sample near an edge or corner, not for the far side of a wall.
const FACING: f64 = 0.3;

/// How many times the reduction may start again round crossings.
const TRIES: usize = 4;

/// Reduces `mesh` (module comment). `before` is the input's soundness from `modify::run`.
pub fn reduce(
    mesh: &Mesh,
    selection: &[u32],
    s: &ReduceSettings,
    before: Soundness,
    control: &mut Control,
) -> RepairResult<Modified> {
    let n = mesh.triangle_count();
    let target = if s.lowest { 0 } else { s.target as usize };
    if !s.lowest && target >= n {
        return Ok(Modified::unchanged(
            mesh,
            format!(
                "The part already has {}, no more than the target.",
                triangles(n)
            ),
        ));
    }
    let chosen = s.selected_only.then(|| mask(selection, n));
    if chosen.as_ref().is_some_and(|c| !c.contains(&true)) {
        return Err(RepairError::Failed(
            "Select the triangles to reduce first, or switch off Selected triangles only.".into(),
        ));
    }
    control.stage("Preparing");
    let surface = Surface::new(mesh);
    let movable = inside(mesh, chosen.as_deref());
    let open = on_open_edges(mesh);
    let reach = (3.0 * s.max_deviation).max(1e-3);
    let near = near_other_sheets(mesh, reach, control.cancel_flag());
    control.check()?;
    let mut locked: Vec<bool> = (0..mesh.vertex_count())
        .map(|v| !movable[v] || open[v] || near[v])
        .collect();
    let points: Vec<P3> = (0..mesh.vertex_count() as u32)
        .map(|v| exact::vertex(mesh, v))
        .collect();
    let tol = s.max_deviation;
    for attempt in 0..TRIES {
        let span = 1.0 / TRIES as f64;
        let mut part = control.sub(attempt as f64 * span, (attempt + 1) as f64 * span);
        part.stage("Reducing triangles");
        let mut work = Work::new(mesh, s.max_edge);
        work.relaxed = true;
        let mut q = collapse::plane_quadrics(&work);
        let mut rules = ReduceRules {
            surface: &surface,
            tol,
            locked: &locked,
            points: &points,
            on_face: assign_points(&work),
        };
        collapse::collapse_rounds(
            &mut work,
            &mut q,
            &mut rules,
            Rounds {
                ordered: !s.fast,
                target,
            },
            &mut part.sub(0.0, 0.85),
        )?;
        let reduced = work.finish();
        if reduced.triangle_count() == n {
            return Ok(Modified::unchanged(
                mesh,
                "No triangles could be merged within the Max. deformation and Max. edge length.",
            ));
        }
        if before.crossings == 0 {
            part.stage("Checking the reduced mesh");
            let pairs = tritri::intersecting_pairs(&reduced, &mut part.sub(0.85, 1.0))?;
            if !pairs.is_empty() {
                // Leave alone every original point within the size of the crossing triangles.
                let size = pairs
                    .iter()
                    .flat_map(|&(a, b)| [a, b])
                    .map(|t| collapse::longest_side(&exact::corners(&reduced, t)))
                    .fold(0.0f64, f64::max);
                lock_round(mesh, &reduced, &pairs, size.min(s.max_edge), &mut locked);
                continue;
            }
        }
        let left = reduced.triangle_count();
        let mut out = Modified {
            changed: true,
            origin: Some(work.survivors()),
            message: format!("Reduced {} to {}.", triangles(n), fmt(left)),
            warnings: Vec::new(),
            mesh: reduced,
        };
        if !s.lowest && left > target {
            out.warnings.push(format!(
                "Stopped at {}: fewer would move the surface by more than {} mm or make \
                 edges longer than {} mm.",
                triangles(left),
                mm(s.max_deviation),
                mm(s.max_edge)
            ));
        }
        part.progress(1.0);
        return Ok(out);
    }
    Err(RepairError::Failed(
        "Triangle reduction could not reduce the part without triangles crossing each other, so \
         the part was left as it was. Try a smaller Max. deformation."
            .into(),
    ))
}

fn fmt(n: usize) -> String {
    crate::mesh::fmt_count(n as u64)
}

/// Millimetres for a message: at most four decimals, no trailing zeros.
fn mm(x: f64) -> String {
    let s = format!("{x:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Each original vertex on one triangle round it: the triangle it is kept on.
fn assign_points(work: &Work) -> Vec<Vec<u32>> {
    let mut on_face = vec![Vec::new(); work.tris.len()];
    for (v, around) in work.around.iter().enumerate() {
        if let Some(&t) = around.first() {
            on_face[t as usize].push(v as u32);
        }
    }
    on_face
}

struct ReduceRules<'a> {
    surface: &'a Surface,
    tol: f64,
    locked: &'a [bool],
    /// The original vertices.
    points: &'a [P3],
    /// The original vertices kept on each triangle.
    on_face: Vec<Vec<u32>>,
}

fn distance2(p: P3, c: &[P3; 3]) -> f64 {
    let d = sub(closest_on_triangle(p, c[0], c[1], c[2]), p);
    dot(d, d)
}

impl ReduceRules<'_> {
    /// True when `p` lies within the limit of the original surface.
    /// Its samples are held to SAMPLED of the limit: the deepest point of a triangle can lie
    /// between them (on a sphere, its centre of the circle through its corners). The original
    /// triangle nearest a sample must not face against the new triangle `n`: in a thin wall the
    /// nearest surface can be the far side, and a triangle laid onto it would cross the wall.
    fn on_original(&self, p: P3, n: P3) -> bool {
        let limit = SAMPLED * self.tol;
        self.surface
            .nearest_normal(p, limit * limit)
            .is_some_and(|m| {
                let (lm, ln) = (exact::length(m), exact::length(n));
                lm == 0.0 || ln == 0.0 || dot(m, n) > -FACING * lm * ln
            })
    }

    /// True when the star of linked edge u–v, with u and v at `x`, stays within the limits.
    fn fits(&self, work: &Work, u: u32, v: u32, x: P3, k: &Scratch) -> bool {
        if !shape_fits(work, u, v, x, k) {
            return false;
        }
        let new: Vec<[P3; 3]> = k.star.iter().map(|&t| moved(work, t, u, v, x)).collect();
        let mean = new
            .iter()
            .fold([0.0; 3], |m, c| add(m, collapse::normal(c)));
        if !self.on_original(x, mean) {
            return false;
        }
        for c in &new {
            let n = collapse::normal(c);
            let g = centre(c);
            let mids = [0, 1, 2].map(|i| scale(add(c[i], c[(i + 1) % 3]), 0.5));
            let inner = [0, 1, 2].map(|i| scale(add(c[i], g), 0.5));
            if !self.on_original(g, n)
                || !mids.iter().chain(&inner).all(|&m| self.on_original(m, n))
            {
                return false;
            }
        }
        let tol2 = self.tol * self.tol;
        k.star
            .iter()
            .chain(&k.shared)
            .flat_map(|&t| &self.on_face[t as usize])
            .all(|&p| {
                let at = self.points[p as usize];
                new.iter().any(|c| distance2(at, c) <= tol2)
            })
    }
}

impl Rules for ReduceRules<'_> {
    /// The quadric error at the cheapest of the edge's ends, its middle and the optimum.
    fn price(&self, work: &Work, q: &[Quadric], u: u32, v: u32) -> Option<f64> {
        if self.locked[u as usize] || self.locked[v as usize] {
            return None;
        }
        let both = q[u as usize].plus(&q[v as usize]);
        let (pu, pv) = (work.pos[u as usize], work.pos[v as usize]);
        let mid = scale(add(pu, pv), 0.5);
        let mut cost = both.cost(mid).min(both.cost(pu)).min(both.cost(pv));
        let max_cost = self.tol * self.tol;
        if cost > max_cost {
            cost = cost.min(both.cost(both.optimum(mid).0));
        }
        (cost <= max_cost).then_some(cost)
    }

    fn plan(&self, work: &Work, q: &[Quadric], u: u32, v: u32, k: &mut Scratch) -> Option<Plan> {
        if !collapse::linked(work, u, v, k) {
            return None;
        }
        let both = q[u as usize].plus(&q[v as usize]);
        let (pu, pv) = (work.pos[u as usize], work.pos[v as usize]);
        let mid = scale(add(pu, pv), 0.5);
        let max_cost = self.tol * self.tol;
        let (best, _) = both.optimum(mid);
        let near = exact::length(sub(best, mid)) <= exact::length(sub(pu, pv));
        let mut options: Vec<(P3, f64)> = [near.then_some(best), Some(mid), Some(pv), Some(pu)]
            .into_iter()
            .flatten()
            .map(rounded)
            .map(|x| (x, both.cost(x)))
            .filter(|&(_, c)| c <= max_cost)
            .collect();
        options.sort_by(|a, b| a.1.total_cmp(&b.1));
        options
            .into_iter()
            .find(|&(x, _)| self.fits(work, u, v, x, k))
            .map(|(x, _)| Plan { u, v, x })
    }

    /// Puts the original vertices kept on each changed triangle onto the nearest triangle of the
    /// collapse's new star.
    fn before_apply(&mut self, work: &Work, plans: &[Plan]) {
        use rayon::prelude::*;
        let moves: Vec<Vec<(u32, Vec<u32>)>> = plans
            .par_iter()
            .map_init(Scratch::default, |k, p| {
                if !collapse::linked(work, p.u, p.v, k) {
                    return Vec::new();
                }
                let new: Vec<[P3; 3]> = k
                    .star
                    .iter()
                    .map(|&t| moved(work, t, p.u, p.v, p.x))
                    .collect();
                let mut lists: Vec<(u32, Vec<u32>)> =
                    k.star.iter().map(|&t| (t, Vec::new())).collect();
                for &t in k.star.iter().chain(&k.shared) {
                    for &pt in &self.on_face[t as usize] {
                        let at = self.points[pt as usize];
                        let best = (0..new.len())
                            .min_by(|&a, &b| {
                                distance2(at, &new[a]).total_cmp(&distance2(at, &new[b]))
                            })
                            .unwrap_or(0);
                        lists[best].1.push(pt);
                    }
                }
                for &t in &k.shared {
                    lists.push((t, Vec::new()));
                }
                lists
            })
            .collect();
        for list in moves {
            for (t, points) in list {
                self.on_face[t as usize] = points;
            }
        }
    }
}
