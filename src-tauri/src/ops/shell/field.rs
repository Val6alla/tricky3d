//! The part's distance field near the offset surface, sampled on the grid Wrap part surface's last
//! resort uses (src/repair/levelset.rs). The sign comes from the grid's inside map (winding
//! numbers); the distance is exact, to the nearest triangle, and only measured where it decides
//! something: the grid is cut into blocks of 8 × 8 × 8 points, and one distance from a block's
//! centre tells whether all of the block lies nearer than the thickness or further than it. Only
//! the blocks the offset surface passes through are measured point by point (the narrow band).

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::mesh::Mesh;
use crate::repair::bvh::{Bvh, boxes_of};
use crate::repair::exact::{self, P3, add, dot, scale, sub};
use crate::repair::levelset::{Grid, Inside};
use crate::repair::{Control, RepairResult};

/// Points along each side of a block.
pub(crate) const BLOCK: usize = 8;

/// Which offset surface: at `thickness` outside the part, or inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Offset {
    Outer(f64),
    Inner(f64),
}

impl Offset {
    pub(crate) fn thickness(self) -> f64 {
        match self {
            Offset::Outer(t) | Offset::Inner(t) => t,
        }
    }
}

/// The input part's triangles, for distance queries.
pub(crate) struct Surface {
    points: Vec<P3>,
    triangles: Vec<[u32; 3]>,
    bvh: Bvh,
}

impl Surface {
    pub(crate) fn new(mesh: &Mesh) -> Surface {
        let points: Vec<P3> = (0..mesh.vertex_count() as u32)
            .map(|v| exact::vertex(mesh, v))
            .collect();
        let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let bvh = Bvh::new(&boxes_of(&points, &triangles));
        Surface {
            points,
            triangles,
            bvh,
        }
    }

    pub(crate) fn points(&self) -> &[P3] {
        &self.points
    }

    pub(crate) fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// The squared distance from `p` to the nearest triangle and the nearest point on it, among
    /// the triangles nearer than `limit` (squared); None when none is.
    pub(crate) fn nearest(&self, p: P3, limit: f64) -> Option<(f64, P3)> {
        let found = self.bvh.nearest(p, limit, |t| {
            let [a, b, c] = self.triangles[t as usize].map(|v| self.points[v as usize]);
            let q = closest_on_triangle(p, a, b, c);
            let d = sub(q, p);
            dot(d, d)
        })?;
        let [a, b, c] = self.triangles[found.0 as usize].map(|v| self.points[v as usize]);
        Some((found.1, closest_on_triangle(p, a, b, c)))
    }

    /// The triangle nearest `p` among those nearer than `limit` (squared), with its normal (not
    /// unit length).
    pub(crate) fn nearest_normal(&self, p: P3, limit: f64) -> Option<P3> {
        let (t, _) = self.bvh.nearest(p, limit, |t| {
            let [a, b, c] = self.triangles[t as usize].map(|v| self.points[v as usize]);
            let q = closest_on_triangle(p, a, b, c);
            let d = sub(q, p);
            dot(d, d)
        })?;
        let [a, b, c] = self.triangles[t as usize].map(|v| self.points[v as usize]);
        Some(exact::cross(sub(b, a), sub(c, a)))
    }

    /// The nearest point of the triangles facing less than a right angle away from `n`, among
    /// those nearer than `limit` (squared): in a thin wall, the near side's, not the far side's.
    pub(crate) fn nearest_facing(&self, p: P3, n: P3, limit: f64) -> Option<P3> {
        let (t, _) = self.bvh.nearest(p, limit, |t| {
            let [a, b, c] = self.triangles[t as usize].map(|v| self.points[v as usize]);
            if dot(exact::cross(sub(b, a), sub(c, a)), n) <= 0.0 {
                return f64::INFINITY;
            }
            let q = closest_on_triangle(p, a, b, c);
            let d = sub(q, p);
            dot(d, d)
        })?;
        let [a, b, c] = self.triangles[t as usize].map(|v| self.points[v as usize]);
        Some(closest_on_triangle(p, a, b, c))
    }

    /// The distance from `p` to the surface, or `cap` when it is at least that.
    pub(crate) fn distance(&self, p: P3, cap: f64) -> f64 {
        self.nearest(p, cap * cap).map_or(cap, |(d, _)| d.sqrt())
    }

    /// Where the segment from `a` to `b` last meets the surface, as a fraction of the way (None
    /// when it does not meet it).
    pub(crate) fn last_hit(&self, a: P3, b: P3) -> Option<f64> {
        let dir = sub(b, a);
        let mut last: Option<f64> = None;
        self.bvh.segment(a, b, |t| {
            let [x, y, z] = self.triangles[t as usize].map(|v| self.points[v as usize]);
            if let Some(s) = crate::repair::levelset::segment_hit(a, dir, x, y, z)
                && last.is_none_or(|l| s > l)
            {
                last = Some(s);
            }
        });
        last
    }
}

/// How far a point on the right side of the part lies from the offset surface: negative in the
/// offset solid, positive outside it, `cap` beyond the thickness counted at most. Right side: outside
/// the part for an outer offset, inside it for an inner one (every grid edge the offset surface
/// crosses is, when the grid step is at most half the thickness).
pub(crate) fn level(surface: &Surface, offset: Offset, p: P3, cap: f64) -> f64 {
    let t = offset.thickness();
    let d = surface.distance(p, t + cap);
    match offset {
        Offset::Outer(_) => d - t,
        Offset::Inner(_) => t - d,
    }
}

/// The offset solid on the grid: per point, whether it lies nearer to the part than the thickness
/// or inside it (outer offset), or inside the part and at least the thickness from its surface
/// (inner offset). Points beyond the grid's border rows stay outside.
pub(crate) fn solid(
    grid: &Grid,
    inside: &Inside,
    surface: &Surface,
    offset: Offset,
    control: &mut Control,
) -> RepairResult<Inside> {
    let [nx, ny, nz] = grid.n;
    let nb = grid.n.map(|n| n.div_ceil(BLOCK));
    let mut slabs = vec![vec![0u64; grid.row_words * ny]; nz];
    if surface.is_empty() {
        return Ok(Inside { slabs });
    }
    let cancel = control.cancel_flag();
    // A few dozen rounds of block layers, so progress and cancel are seen while it runs.
    let step = nb[2].div_ceil(40).max(1);
    let mut first = 0;
    while first < nb[2] {
        control.check()?;
        let last = (first + step).min(nb[2]);
        let blocks: Vec<[usize; 3]> = (first..last)
            .flat_map(|bz| (0..nb[1]).flat_map(move |by| (0..nb[0]).map(move |bx| [bx, by, bz])))
            .collect();
        let rows: Vec<([usize; 3], [u8; BLOCK * BLOCK])> = blocks
            .par_iter()
            .filter_map(|&b| block_bits(grid, inside, surface, offset, b, cancel).map(|r| (b, r)))
            .collect();
        for ([bx, by, bz], bits) in rows {
            let i0 = bx * BLOCK;
            for dk in 0..BLOCK {
                let k = bz * BLOCK + dk;
                if k >= nz {
                    break;
                }
                for dj in 0..BLOCK {
                    let j = by * BLOCK + dj;
                    if j >= ny {
                        break;
                    }
                    let byte = bits[dk * BLOCK + dj] as u64;
                    // BLOCK divides 64, so a block's row never straddles two words.
                    slabs[k][j * grid.row_words + i0 / 64] |= byte << (i0 % 64);
                }
            }
        }
        first = last;
        control.progress(first as f64 / nb[2] as f64);
    }
    control.check()?;
    // The border points stay outside, so the surface closes inside the grid.
    let clear = |bits: &mut Vec<u64>, k: usize| {
        if k == 0 || k == nz - 1 {
            bits.fill(0);
        }
        for j in [0, ny - 1] {
            bits[j * grid.row_words..(j + 1) * grid.row_words].fill(0);
        }
        for j in 0..ny {
            for i in [0, nx - 1] {
                bits[j * grid.row_words + i / 64] &= !(1 << (i % 64));
            }
        }
    };
    slabs
        .par_iter_mut()
        .enumerate()
        .for_each(|(k, bits)| clear(bits, k));
    Ok(Inside { slabs })
}

/// One block's points in the offset solid: a byte per row of x (bit i for the block's i-th point),
/// rows by y then z. None when no point is.
fn block_bits(
    grid: &Grid,
    inside: &Inside,
    surface: &Surface,
    offset: Offset,
    [bx, by, bz]: [usize; 3],
    cancel: &AtomicBool,
) -> Option<[u8; BLOCK * BLOCK]> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let lo = [bx, by, bz].map(|b| b * BLOCK);
    let hi = [0, 1, 2].map(|a| (lo[a] + BLOCK).min(grid.n[a]));
    // Which points are on the side of the part the distance decides: outside it for an outer
    // offset (the inside is in the solid anyway), inside it for an inner one (the outside never is).
    let outer = matches!(offset, Offset::Outer(_));
    let mut part = [0u8; BLOCK * BLOCK];
    let mut decide = 0usize;
    for k in lo[2]..hi[2] {
        for j in lo[1]..hi[1] {
            let mut byte = 0u8;
            for i in lo[0]..hi[0] {
                if grid.at(inside, [i, j, k]) {
                    byte |= 1 << (i - lo[0]);
                }
            }
            let row = (k - lo[2]) * BLOCK + (j - lo[1]);
            let width = hi[0] - lo[0];
            let all = if width == 8 { 0xff } else { (1u8 << width) - 1 };
            part[row] = byte;
            decide += if outer {
                (!byte & all).count_ones()
            } else {
                byte.count_ones()
            } as usize;
        }
    }
    let t = offset.thickness();
    // The inside of the part is in an outer offset's solid; with nothing to decide that is all.
    let fill = |near: bool| -> [u8; BLOCK * BLOCK] {
        let mut out = [0u8; BLOCK * BLOCK];
        for k in lo[2]..hi[2] {
            for j in lo[1]..hi[1] {
                let row = (k - lo[2]) * BLOCK + (j - lo[1]);
                let width = hi[0] - lo[0];
                let all = if width == 8 { 0xff } else { (1u8 << width) - 1 };
                out[row] = match (outer, near) {
                    // Outer: the inside, and the outside within the thickness.
                    (true, true) => all,
                    (true, false) => part[row],
                    // Inner: the inside deeper than the thickness.
                    (false, true) => 0,
                    (false, false) => part[row],
                };
            }
        }
        out
    };
    let result = if decide == 0 {
        fill(!outer)
    } else {
        let centre = [0, 1, 2].map(|a| (grid.coord(a, lo[a]) + grid.coord(a, hi[a] - 1)) / 2.0);
        let half = [0, 1, 2].map(|a| (grid.coord(a, hi[a] - 1) - grid.coord(a, lo[a])) / 2.0);
        let radius = exact::length(half);
        let reach = t + radius;
        match surface.nearest(centre, reach * reach) {
            // Every point of the block is further than the thickness from the surface.
            None => fill(false),
            Some((d, _)) if d.sqrt() - radius > t => fill(false),
            // Every point is nearer than the thickness.
            Some((d, _)) if d.sqrt() + radius < t => fill(true),
            Some(_) => {
                let mut out = [0u8; BLOCK * BLOCK];
                for k in lo[2]..hi[2] {
                    for j in lo[1]..hi[1] {
                        let row = (k - lo[2]) * BLOCK + (j - lo[1]);
                        let mut byte = 0u8;
                        for i in lo[0]..hi[0] {
                            let bit = 1u8 << (i - lo[0]);
                            let is_inside = part[row] & bit != 0;
                            let solid = if outer {
                                is_inside || surface.nearest(grid.point([i, j, k]), t * t).is_some()
                            } else {
                                is_inside && surface.nearest(grid.point([i, j, k]), t * t).is_none()
                            };
                            if solid {
                                byte |= bit;
                            }
                        }
                        out[row] = byte;
                    }
                }
                out
            }
        }
    };
    result.iter().any(|&b| b != 0).then_some(result)
}

/// The solid without its parts thinner than about two grid steps: a point stays when the 3 × 3 × 3
/// cube of points round it, or round one of its 26 neighbours, lies wholly in the solid (an
/// opening by that cube). What is left is part of the solid, and every box-shaped region three or
/// more points across stays whole.
pub(crate) fn open(grid: &Grid, solid: &Inside, cancel: &AtomicBool) -> Inside {
    let [_, ny, nz] = grid.n;
    let w = grid.row_words;
    // Each bit with its x neighbours combined: all three set (`and`), or any of them (`or`).
    let along_x = |src: &[u64], out: &mut [u64], and: bool| {
        for x in 0..w {
            let left = (src[x] << 1) | if x > 0 { src[x - 1] >> 63 } else { 0 };
            let right = (src[x] >> 1) | if x + 1 < w { src[x + 1] << 63 } else { 0 };
            out[x] = if and {
                src[x] & left & right
            } else {
                src[x] | left | right
            };
        }
    };
    let combine = |src: &Inside, and: bool| -> Inside {
        let lines: Vec<Vec<u64>> = (0..nz)
            .into_par_iter()
            .map(|k| {
                let mut slab = vec![0u64; w * ny];
                if cancel.load(Ordering::Relaxed) {
                    return slab;
                }
                for j in 0..ny {
                    along_x(row(src, w, j, k), &mut slab[j * w..(j + 1) * w], and);
                }
                slab
            })
            .collect();
        let lines = Inside { slabs: lines };
        let slabs: Vec<Vec<u64>> = (0..nz)
            .into_par_iter()
            .map(|k| {
                let mut slab = vec![if and { !0u64 } else { 0 }; w * ny];
                for j in 0..ny {
                    let out = &mut slab[j * w..(j + 1) * w];
                    for dk in [-1i64, 0, 1] {
                        for dj in [-1i64, 0, 1] {
                            let (jj, kk) = (j as i64 + dj, k as i64 + dk);
                            let outside = jj < 0 || kk < 0 || jj >= ny as i64 || kk >= nz as i64;
                            match (outside, and) {
                                (true, true) => out.fill(0),
                                (true, false) => {}
                                (false, _) => {
                                    let src = row(&lines, w, jj as usize, kk as usize);
                                    for (o, &v) in out.iter_mut().zip(src) {
                                        if and {
                                            *o &= v;
                                        } else {
                                            *o |= v;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                slab
            })
            .collect();
        Inside { slabs }
    };
    let eroded = combine(solid, true);
    let mut opened = combine(&eroded, false);
    for (o, s) in opened.slabs.iter_mut().zip(&solid.slabs) {
        for (a, b) in o.iter_mut().zip(s) {
            *a &= b;
        }
    }
    opened
}

/// Row `j` of slab `k`, `w` words long.
fn row(s: &Inside, w: usize, j: usize, k: usize) -> &[u64] {
    &s.slabs[k][j * w..(j + 1) * w]
}

/// The point of triangle `a`, `b`, `c` nearest to `p` (Ericson, Real-Time Collision Detection,
/// 5.1.5); for a triangle without area, the nearest point on its sides.
pub(crate) fn closest_on_triangle(p: P3, a: P3, b: P3, c: P3) -> P3 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), w));
    }
    let sum = va + vb + vc;
    if !(sum.is_finite() && sum > 0.0) {
        return [(a, b), (b, c), (c, a)]
            .into_iter()
            .map(|(u, v)| closest_on_segment(p, u, v))
            .min_by(|x, y| {
                let (dx, dy) = (sub(*x, p), sub(*y, p));
                dot(dx, dx).total_cmp(&dot(dy, dy))
            })
            .unwrap_or(a);
    }
    let (v, w) = (vb / sum, vc / sum);
    add(a, add(scale(ab, v), scale(ac, w)))
}

fn closest_on_segment(p: P3, a: P3, b: P3) -> P3 {
    let ab = sub(b, a);
    let len = dot(ab, ab);
    if len <= 0.0 {
        return a;
    }
    add(a, scale(ab, (dot(sub(p, a), ab) / len).clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repair::exact::testing::Rng;

    /// Against a dense search over the triangle's barycentric grid.
    #[test]
    fn closest_point_on_a_triangle_is_nearest() {
        let mut rng = Rng(5);
        for _ in 0..400 {
            let mut pt = || [0, 1, 2].map(|_| rng.unit() * 10.0 - 5.0);
            let (a, b, c, p) = (pt(), pt(), pt(), pt());
            let q = closest_on_triangle(p, a, b, c);
            let dq = exact::length(sub(q, p));
            let n = 60;
            for i in 0..=n {
                for j in 0..=n - i {
                    let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                    let x = add(a, add(scale(sub(b, a), u), scale(sub(c, a), v)));
                    assert!(dq <= exact::length(sub(x, p)) + 1e-9, "{p:?}");
                }
            }
        }
        // Without area: the nearest point on its sides.
        let q = closest_on_triangle([1.0, 1.0, 0.0], [0.0; 3], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]);
        assert_eq!(q, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn distances_to_a_box() {
        let s = Surface::new(&crate::mesh::make_box([0.0; 3], [10.0; 3]));
        assert!((s.distance([5.0, 5.0, 5.0], 100.0) - 5.0).abs() < 1e-12);
        assert!((s.distance([13.0, 14.0, 5.0], 100.0) - 5.0).abs() < 1e-12);
        assert_eq!(s.distance([50.0, 5.0, 5.0], 3.0), 3.0);
    }
}
