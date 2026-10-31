//! Shadow area: the area the part covers when it is projected straight down onto the X/Y plane,
//! the union of its triangles' shadows (where they overlap it counts once). For a closed part
//! whose triangles face out, the triangles facing up cover exactly that shadow.
//!
//! The union is measured exactly with a sweep along Y. Between two consecutive corner heights
//! every triangle crosses a horizontal line in an interval whose ends move linearly, so the
//! length the intervals cover is piecewise linear; each such slab is integrated by halving it
//! until every piece is linear (three samples agree with the straight line through its ends).

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::arrange::World;
use crate::repair::{Control, RepairError, RepairResult};

/// A triangle in the X/Y plane.
pub type Flat = [[f64; 2]; 3];

/// The shadows of the part's triangles that face up (positive area seen from above).
pub fn upward(world: &World) -> (Vec<Flat>, Vec<bool>) {
    let mut up = vec![false; world.triangles.len()];
    let flats = world
        .triangles
        .iter()
        .enumerate()
        .filter_map(|(i, &t)| {
            let [a, b, c] = world.corners(t);
            let twice = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            (twice > 0.0).then(|| {
                up[i] = true;
                [[a[0], a[1]], [b[0], b[1]], [c[0], c[1]]]
            })
        })
        .collect();
    (flats, up)
}

fn y_range(t: &Flat) -> (f64, f64) {
    let lo = t[0][1].min(t[1][1]).min(t[2][1]);
    let hi = t[0][1].max(t[1][1]).max(t[2][1]);
    (lo, hi)
}

/// Where the line at height `y` crosses the triangle, as an X interval.
fn interval(t: &Flat, y: f64) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (p, q) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
        let (y0, y1) = (p[1].min(q[1]), p[1].max(q[1]));
        if y < y0 || y > y1 {
            continue;
        }
        if p[1] == q[1] {
            lo = lo.min(p[0].min(q[0]));
            hi = hi.max(p[0].max(q[0]));
        } else {
            let s = (y - p[1]) / (q[1] - p[1]);
            let x = p[0] + s * (q[0] - p[0]);
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// The length of X the active triangles cover at height `y`.
fn covered(tris: &[Flat], active: &[u32], y: f64, scratch: &mut Vec<(f64, f64)>) -> f64 {
    scratch.clear();
    scratch.extend(
        active
            .iter()
            .filter_map(|&i| interval(&tris[i as usize], y)),
    );
    scratch.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let mut run: Option<(f64, f64)> = None;
    for &(lo, hi) in scratch.iter() {
        match run {
            Some((a, b)) if lo <= b => run = Some((a, b.max(hi))),
            Some((a, b)) => {
                total += b - a;
                run = Some((lo, hi));
            }
            None => run = Some((lo, hi)),
        }
    }
    if let Some((a, b)) = run {
        total += b - a;
    }
    total
}

struct Slab<'a> {
    tris: &'a [Flat],
    active: &'a [u32],
    tol: f64,
    scratch: Vec<(f64, f64)>,
}

impl Slab<'_> {
    fn at(&mut self, y: f64) -> f64 {
        covered(self.tris, self.active, y, &mut self.scratch)
    }

    /// The area between y0 and y1, where the covered length is l0 and l1.
    fn area(&mut self, y0: f64, y1: f64, l0: f64, l1: f64, depth: u32) -> f64 {
        let h = y1 - y0;
        let mid = self.at(y0 + h / 2.0);
        let q1 = self.at(y0 + h / 4.0);
        let q3 = self.at(y0 + 3.0 * h / 4.0);
        let tol = self.tol;
        let linear = (mid - (l0 + l1) / 2.0).abs() <= tol
            && (q1 - (3.0 * l0 + l1) / 4.0).abs() <= tol
            && (q3 - (l0 + 3.0 * l1) / 4.0).abs() <= tol;
        if linear {
            return (l0 + l1) / 2.0 * h;
        }
        if depth >= 40 {
            // Simpson's rule on what is left: far below anything a print could show.
            return h / 6.0 * (l0 + 4.0 * mid + l1);
        }
        let ym = y0 + h / 2.0;
        self.area(y0, ym, l0, mid, depth + 1) + self.area(ym, y1, mid, l1, depth + 1)
    }
}

/// The area of the union of `tris` (mm²).
pub fn union_area(tris: &[Flat], control: &mut Control) -> RepairResult<f64> {
    if tris.is_empty() {
        return Ok(0.0);
    }
    let mut ys: Vec<f64> = tris.iter().flat_map(|t| t.iter().map(|p| p[1])).collect();
    ys.sort_unstable_by(f64::total_cmp);
    ys.dedup();
    let extent = tris
        .iter()
        .flatten()
        .fold(0.0f64, |m, p| m.max(p[0].abs()).max(p[1].abs()))
        .max(1e-9);
    let tol = extent * 1e-9;
    let mut order: Vec<u32> = (0..tris.len() as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        y_range(&tris[a as usize])
            .0
            .total_cmp(&y_range(&tris[b as usize]).0)
    });
    let slabs = ys.len() - 1;
    let chunks = (slabs / 256).clamp(1, 256);
    let per = slabs.div_ceil(chunks).max(1);
    let cancel: &AtomicBool = control.cancel_flag();
    let batch = rayon::current_num_threads().max(1) * 2;
    let mut total = 0.0;
    let starts: Vec<usize> = (0..slabs).step_by(per).collect();
    let mut done = 0;
    for group in starts.chunks(batch) {
        control.check()?;
        let parts: Vec<Option<f64>> = group
            .par_iter()
            .map(|&s0| {
                let s1 = (s0 + per).min(slabs);
                chunk_area(tris, &order, &ys, s0, s1, tol, cancel)
            })
            .collect();
        for p in parts {
            total += p.ok_or(RepairError::Cancelled)?;
        }
        done += group.len();
        control.progress(done as f64 / starts.len() as f64);
    }
    Ok(total)
}

/// The area of slabs [s0, s1); None once cancelled.
fn chunk_area(
    tris: &[Flat],
    order: &[u32],
    ys: &[f64],
    s0: usize,
    s1: usize,
    tol: f64,
    cancel: &AtomicBool,
) -> Option<f64> {
    let (bottom, top) = (ys[s0], ys[s1]);
    // Every triangle that reaches into the chunk, by lowest corner.
    let end = order.partition_point(|&i| y_range(&tris[i as usize]).0 < top);
    let candidates: Vec<u32> = order[..end]
        .iter()
        .copied()
        .filter(|&i| y_range(&tris[i as usize]).1 > bottom)
        .collect();
    let mut next = 0;
    let mut active: Vec<u32> = Vec::new();
    let mut total = 0.0;
    for s in s0..s1 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (y0, y1) = (ys[s], ys[s + 1]);
        while next < candidates.len() && y_range(&tris[candidates[next] as usize]).0 < y1 {
            active.push(candidates[next]);
            next += 1;
        }
        active.retain(|&i| y_range(&tris[i as usize]).1 > y0);
        if active.is_empty() {
            continue;
        }
        let mut slab = Slab {
            tris,
            active: &active,
            tol,
            scratch: Vec::with_capacity(active.len()),
        };
        let l0 = slab.at(y0);
        let l1 = slab.at(y1);
        total += slab.area(y0, y1, l0, l1, 0);
    }
    Some(total)
}
