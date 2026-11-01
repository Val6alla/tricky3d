//! Tidying the offset surface marching tetrahedra built: smoothing it (Smooth result) and reducing
//! its triangles by edge collapses. The surface must stay closed, oriented and free of crossings:
//! - a collapse keeps the mesh a closed manifold (the link condition, and no vertex left with
//!   fewer than three neighbours);
//! - no triangle turns over, against its old self or against the area it covers, or becomes a
//!   sliver it was not already;
//! - the surface stays on the offset: the exact distance to the part, at each moved vertex and at
//!   the centre of each changed triangle longer than two grid steps, stays within a small share of
//!   the step of the thickness, and no edge grows longer than `longest`;
//! - where another sheet of the surface lies within reach of a vertex along its normal (a thin
//!   cavity, a narrow gap, a thin wall), the vertex stays where it is: there a collapse could cut
//!   across the other sheet;
//! - at the end, the exact triangle test of Detect self-intersections looks for crossings. Where it
//!   finds some, the tidying runs again from the start with the vertices round them left where they
//!   were; after three tries the surface is left as marching tetrahedra built it.
//!
//! The collapses run cheapest first by quadric error (Garland and Heckbert), one at a time
//! (src/ops/collapse.rs, whose checks Triangle reduction shares). Their quadrics come from the mesh's own
//! triangles, which keeps its shape; with `features` they come from the tangent planes of the true
//! offset surface at each vertex instead, so a collapse across an edge that marching tetrahedra
//! cut off puts the vertex back on the sharp edge or corner. Those collapses (the two ends see
//! different tangent planes) all run before any other, so the flat parts round an edge are not
//! reduced first into triangles too long to fold back onto it.

use super::field::{Offset, Surface, level};
use crate::mesh::Mesh;
pub(super) use crate::ops::collapse::near_other_sheets;
use crate::ops::collapse::{
    self, Plan, Quadric, Rules, Scratch, Work, centre, lock_round, longest_side, moved, rounded,
    shape_fits,
};
use crate::repair::exact::{self, P3, add, scale, sub};
use crate::repair::{Control, RepairResult, tritri};

pub(crate) struct Settings {
    /// The grid step.
    pub cell: f64,
    pub offset: Offset,
    /// Rebuild sharp edges and corners from the offset surface's tangent planes.
    pub features: bool,
    pub smooth: bool,
    /// Longest edge a collapse may make (Hollow part keeps it under the thickness, so the cavity
    /// never reaches the part's own surface).
    pub longest: f64,
}

/// Distance from the offset a moved vertex may have, and the centre of a changed triangle longer
/// than `CHECKED` steps, in steps.
const VERTEX_SLACK: f64 = 0.05;
const CENTRE_SLACK: f64 = 0.1;
const CHECKED: f64 = 2.0;
/// How far smoothing may take a vertex from the offset, in steps.
const SMOOTH_SLACK: f64 = 0.5;
/// Longest edge, in steps, whatever `longest` allows.
const MAX_EDGE: f64 = 6.0;
/// The quadric error a collapse may cost: mean squared distance, in steps squared.
const MAX_COST: f64 = 0.05 * 0.05;
/// Taubin smoothing: the shrinking and the inflating step, and how many pairs of them.
const LAMBDA: f64 = 0.5;
const MU: f64 = -0.53;
const SMOOTH_PAIRS: usize = 4;
/// How many times the tidying may start again round crossings, and the most crossing pairs worth
/// trying to keep clear of.
const TRIES: usize = 3;
const MAX_CROSSINGS: usize = 500;

/// The tidied surface; `untouched` when it had to be left as built.
pub(crate) struct Tidied {
    pub mesh: Mesh,
    pub untouched: bool,
}

pub(crate) fn tidy(
    mesh: Mesh,
    surface: &Surface,
    s: &Settings,
    control: &mut Control,
) -> RepairResult<Tidied> {
    let mut locked = near_other_sheets(
        &mesh,
        s.longest.min(MAX_EDGE * s.cell),
        control.cancel_flag(),
    );
    control.check()?;
    for attempt in 0..TRIES {
        let span = 1.0 / TRIES as f64;
        let mut part = control.sub(attempt as f64 * span, (attempt + 1) as f64 * span);
        let mut work = Work::new(&mesh, s.longest.min(MAX_EDGE * s.cell));
        if s.smooth {
            part.stage("Smoothing");
            smooth(&mut work, surface, s, &locked, &mut part.sub(0.0, 0.3))?;
        }
        part.stage("Reducing triangles");
        reduce(&mut work, surface, s, &locked, &mut part.sub(0.3, 0.8))?;
        let tidied = work.finish();
        part.stage("Checking the tidied mesh");
        let pairs = tritri::intersecting_pairs(&tidied, &mut part.sub(0.8, 1.0))?;
        if pairs.is_empty() {
            return Ok(Tidied {
                mesh: tidied,
                untouched: false,
            });
        }
        if pairs.len() > MAX_CROSSINGS {
            break;
        }
        let reach = s.longest.min(MAX_EDGE * s.cell);
        lock_round(&mesh, &tidied, &pairs, reach, &mut locked);
    }
    Ok(Tidied {
        mesh,
        untouched: true,
    })
}

// ---------------------------------------------------------------------------------------------
// Checks against the offset

/// How far `p` is from the offset surface (signed, as `field::level`).
fn off(surface: &Surface, s: &Settings, p: P3) -> f64 {
    level(surface, s.offset, p, 2.0 * s.cell)
}

// ---------------------------------------------------------------------------------------------
// Smoothing

/// Taubin's λ|μ smoothing over the one-ring averages, vertex by vertex: a vertex moves when its
/// triangles keep their shape and it ends at most SMOOTH_SLACK steps from the offset.
fn smooth(
    work: &mut Work,
    surface: &Surface,
    s: &Settings,
    locked: &[bool],
    control: &mut Control,
) -> RepairResult<()> {
    use rayon::prelude::*;
    let n = work.pos.len();
    let steps = 2 * SMOOTH_PAIRS;
    for step in 0..steps {
        control.check()?;
        control.progress(step as f64 / steps as f64);
        let factor = if step % 2 == 0 { LAMBDA } else { MU };
        // Every target from the positions before this step, and its distance from the offset.
        let w: &Work = work;
        let targets: Vec<Option<P3>> = (0..n as u32)
            .into_par_iter()
            .map(|v| {
                if locked[v as usize] || w.around[v as usize].is_empty() {
                    return None;
                }
                let mut ring = Vec::new();
                w.neighbours(v, &mut ring);
                let mut mean = [0.0; 3];
                for &u in &ring {
                    mean = add(mean, w.pos[u as usize]);
                }
                let mean = scale(mean, 1.0 / ring.len() as f64);
                let p = w.pos[v as usize];
                let x = rounded(add(p, scale(sub(mean, p), factor)));
                (off(surface, s, x).abs() <= SMOOTH_SLACK * s.cell).then_some(x)
            })
            .collect();
        for (v, x) in targets.into_iter().enumerate() {
            if let Some(x) = x {
                collapse::try_move(work, v as u32, x);
            }
        }
    }
    control.progress(1.0);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Reducing

fn normal(c: &[P3; 3]) -> P3 {
    collapse::normal(c)
}

/// Each vertex's quadric: its triangles' planes (area weighted), or with `features` the offset
/// surface's tangent plane where the vertex lies (weighted by its share of the area).
fn quadrics(work: &Work, surface: &Surface, s: &Settings) -> Vec<Quadric> {
    use rayon::prelude::*;
    if !s.features {
        return collapse::plane_quadrics(work);
    }
    let t = s.offset.thickness();
    let reach = t + 2.0 * s.cell;
    (0..work.pos.len())
        .into_par_iter()
        .map(|v| {
            let p = work.pos[v];
            let share: f64 = work.around[v]
                .iter()
                .map(|&tri| exact::length(normal(&work.corners(tri))) / 6.0)
                .sum();
            match surface.nearest(p, reach * reach) {
                Some((d, near)) if d > 0.0 => {
                    // From the part's surface toward the vertex, and the offset's outward normal.
                    let away = scale(sub(p, near), 1.0 / d.sqrt());
                    let n = match s.offset {
                        Offset::Outer(_) => away,
                        Offset::Inner(_) => scale(away, -1.0),
                    };
                    Quadric::plane(n, add(near, scale(away, t)), share.max(1e-12))
                }
                _ => Quadric::default(),
            }
        })
        .collect()
}

/// What deciding a collapse needs: the offset to stay on, the vertices left alone, the dearest
/// collapse allowed, and whether only edges between different planes count (the first round with
/// `features`).
struct TidyRules<'a> {
    surface: &'a Surface,
    s: &'a Settings,
    locked: &'a [bool],
    max_cost: f64,
    edges_only: bool,
}

impl Rules for TidyRules<'_> {
    /// Priced at the middle; the optimum (an eigen solve) only where the middle is too dear or
    /// edges alone count.
    fn price(&self, work: &Work, q: &[Quadric], u: u32, v: u32) -> Option<f64> {
        if self.locked[u as usize] || self.locked[v as usize] {
            return None;
        }
        let both = q[u as usize].plus(&q[v as usize]);
        let mid = scale(add(work.pos[u as usize], work.pos[v as usize]), 0.5);
        let mut cost = both.cost(mid);
        if self.edges_only || cost > self.max_cost {
            let (x, rank) = both.optimum(mid);
            if self.edges_only && rank < 2 {
                return None;
            }
            cost = cost.min(both.cost(x));
        }
        (cost <= self.max_cost).then_some(cost)
    }

    /// The collapse of edge u–v at its best position that passes the checks, if any: the optimum
    /// when it stays near the edge, else the middle.
    fn plan(&self, work: &Work, q: &[Quadric], u: u32, v: u32, k: &mut Scratch) -> Option<Plan> {
        if !collapse::linked(work, u, v, k) {
            return None;
        }
        let both = q[u as usize].plus(&q[v as usize]);
        let (pu, pv) = (work.pos[u as usize], work.pos[v as usize]);
        let mid = scale(add(pu, pv), 0.5);
        let (best, _) = both.optimum(mid);
        let reach = exact::length(sub(pu, pv)).max(0.5 * self.s.cell);
        let near = exact::length(sub(best, mid)) <= reach;
        [best, mid]
            .into_iter()
            .skip(if near { 0 } else { 1 })
            .map(rounded)
            .map(|x| (x, both.cost(x)))
            .find(|&(x, cost)| cost <= self.max_cost && self.fits(work, u, v, x, k))
            .map(|(x, _)| Plan { u, v, x })
    }
}

impl TidyRules<'_> {
    /// True when the star round a linked edge u–v, with u and v at `x`, keeps its shape and the
    /// surface stays on the offset (cheapest checks first).
    fn fits(&self, work: &Work, u: u32, v: u32, x: P3, k: &Scratch) -> bool {
        let (surface, s) = (self.surface, self.s);
        if !shape_fits(work, u, v, x, k) || off(surface, s, x).abs() > VERTEX_SLACK * s.cell {
            return false;
        }
        k.star.iter().all(|&t| {
            let new = moved(work, t, u, v, x);
            longest_side(&new) <= CHECKED * s.cell
                || off(surface, s, centre(&new)).abs() <= CENTRE_SLACK * s.cell
        })
    }
}

fn reduce(
    work: &mut Work,
    surface: &Surface,
    s: &Settings,
    locked: &[bool],
    control: &mut Control,
) -> RepairResult<()> {
    let mut q = quadrics(work, surface, s);
    control.check()?;
    let rounds = if s.features { 2 } else { 1 };
    for round in 0..rounds {
        let span = 1.0 / rounds as f64;
        let mut rules = TidyRules {
            surface,
            s,
            locked,
            max_cost: MAX_COST * s.cell * s.cell,
            edges_only: round + 1 < rounds,
        };
        collapse::collapse_serial(
            work,
            &mut q,
            &mut rules,
            &mut control.sub(round as f64 * span, (round + 1) as f64 * span),
        )?;
    }
    Ok(())
}
