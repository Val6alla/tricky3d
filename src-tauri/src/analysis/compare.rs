//! Compare two meshes: for every point of one part's surface, its signed distance to the other
//! part's surface (positive outside it, negative inside). Both parts are copied, the copies refined
//! until their triangles are small enough to show the distances across each face, and optionally
//! the comparison copy is fitted onto the reference first (centred, turned along matching
//! principal axes, then refined by iterative closest points). The user's parts never change.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::arrange::{Rot, V3, World, add, cross, dot, len, scale, sub};
use crate::mesh::Welder;
use crate::repair::bvh::{Bvh, boxes_of};
use crate::repair::{Control, RepairError, RepairResult};

/// `CompareDirection` in src/analysis/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Both,
    /// The reference's points against the comparison's surface.
    ReferenceToComparison,
    ComparisonToReference,
}

/// `CompareSettings` in src/analysis/types.ts (the parts themselves travel as the request's meshes).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompareSettings {
    pub direction: Direction,
    pub align: bool,
}

/// One direction's distances: `SideResult` in src/analysis/types.ts. mm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SideResult {
    pub max_positive: f64,
    /// The most negative distance (≤ 0).
    pub max_negative: f64,
    pub average_signed: f64,
    pub max_unsigned: f64,
    pub average_unsigned: f64,
    pub points: u64,
}

/// The fit found by Align: the comparison copy was turned by `rotation` (rows) and then moved by
/// `translation`, both in world space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    pub rotation: Rot,
    pub translation: V3,
    /// Degrees turned, about any axis.
    pub angle: f64,
    /// Root-mean-square distance of the fitted points, mm.
    pub rms: f64,
}

/// A refined copy of one part with a distance per vertex, in world space.
pub struct Surface {
    pub points: Vec<f32>,
    pub triangles: Vec<u32>,
    pub distances: Vec<f32>,
}

pub struct Comparison {
    /// The reference's points against the comparison, and the other way.
    pub forward: Option<(SideResult, Surface)>,
    pub backward: Option<(SideResult, Surface)>,
    pub fit: Option<Fit>,
}

/// Triangles each refined copy may have.
pub const REFINED_TRIANGLES: usize = 400_000;

// ---------------------------------------------------------------------------------------------
// Geometry

/// The closest point to `p` on triangle abc (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_on_triangle(p: V3, a: V3, b: V3, c: V3) -> V3 {
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
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

/// A part's surface ready for nearest-point queries.
pub struct Target<'a> {
    world: &'a World,
    bvh: Bvh,
    normals: Vec<V3>,
}

impl<'a> Target<'a> {
    pub fn new(world: &'a World) -> Target<'a> {
        let boxes = boxes_of(&world.points, &world.triangles);
        let normals = world
            .triangles
            .iter()
            .map(|&t| world.normal_area(t).0)
            .collect();
        Target {
            world,
            bvh: Bvh::new(&boxes),
            normals,
        }
    }

    /// The nearest surface point to `p` and the triangle it is on.
    fn nearest(&self, p: V3) -> Option<(V3, u32)> {
        let (i, _) = self.bvh.nearest(p, f64::INFINITY, |i| {
            let [a, b, c] = self.world.corners(self.world.triangles[i as usize]);
            let d = sub(p, closest_on_triangle(p, a, b, c));
            dot(d, d)
        })?;
        let [a, b, c] = self.world.corners(self.world.triangles[i as usize]);
        Some((closest_on_triangle(p, a, b, c), i))
    }

    /// The signed distance from `p` to the surface: positive outside (on the side the surface
    /// faces), negative inside.
    pub fn signed_distance(&self, p: V3) -> Option<f64> {
        let (q, i) = self.nearest(p)?;
        let d = sub(p, q);
        let l = len(d);
        if l == 0.0 {
            return Some(0.0);
        }
        // Where the nearest point is an edge or a corner, several triangles are as near; the one
        // that faces the point most squarely tells the side.
        let dir = scale(d, 1.0 / l);
        let mut side = dot(dir, self.normals[i as usize]);
        let reach = l * (1.0 + 1e-9) + 1e-12;
        let around = [sub(p, [reach; 3]), add(p, [reach; 3])];
        self.bvh.query(&around, |j| {
            let [a, b, c] = self.world.corners(self.world.triangles[j as usize]);
            if len(sub(p, closest_on_triangle(p, a, b, c))) <= reach {
                let s = dot(dir, self.normals[j as usize]);
                if s.abs() > side.abs() {
                    side = s;
                }
            }
        });
        Some(if side < 0.0 { -l } else { l })
    }
}

/// The part's triangles cut into smaller ones (each into k² by splitting its sides in k), so that
/// all of them together stay within `budget`; the corners are welded again.
pub fn refine(world: &World, budget: usize) -> (Vec<f32>, Vec<u32>) {
    let longest: Vec<f64> = world
        .triangles
        .iter()
        .map(|&t| {
            let [a, b, c] = world.corners(t);
            len(sub(b, a)).max(len(sub(c, b))).max(len(sub(a, c)))
        })
        .collect();
    let count = |step: f64| -> usize {
        longest
            .iter()
            .map(|&l| {
                let k = (l / step).ceil().max(1.0) as usize;
                k * k
            })
            .sum()
    };
    let longest_edge = longest.iter().fold(0.0f64, |m, &l| m.max(l));
    // The shortest step that keeps within the budget, found by halving the range.
    let mut step = longest_edge.max(1e-9);
    if count(step) <= budget {
        let (mut lo, mut hi) = (longest_edge / 4096.0, step);
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if count(mid) <= budget {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        step = hi;
    }
    let mut w = Welder::new(budget.min(4 * REFINED_TRIANGLES) * 3);
    let f = |p: V3| [p[0] as f32, p[1] as f32, p[2] as f32];
    for (&t, &l) in world.triangles.iter().zip(&longest) {
        let [a, b, c] = world.corners(t);
        let k = (l / step).ceil().max(1.0) as usize;
        let at = |i: usize, j: usize| {
            // i steps along ab, j along ac.
            let (u, v) = (i as f64 / k as f64, j as f64 / k as f64);
            f(add(a, add(scale(sub(b, a), u), scale(sub(c, a), v))))
        };
        for i in 0..k {
            for j in 0..k - i {
                w.triangle(at(i, j), at(i + 1, j), at(i, j + 1));
                if i + j + 1 < k {
                    w.triangle(at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                }
            }
        }
    }
    let mesh = w.finish().unwrap_or_default();
    (mesh.positions, mesh.indices)
}

// ---------------------------------------------------------------------------------------------
// Alignment

/// Eigenvalues and eigenvectors (columns of the returned matrix, by rows) of a symmetric n×n
/// matrix, by Jacobi rotations; n is 3 or 4.
fn jacobi<const N: usize>(mut a: [[f64; N]; N]) -> ([f64; N], [[f64; N]; N]) {
    let mut v = [[0.0; N]; N];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..100 {
        let mut off = 0.0;
        for (i, row) in a.iter().enumerate() {
            for (j, x) in row.iter().enumerate() {
                if i != j {
                    off += x * x;
                }
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..N {
            for q in p + 1..N {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                // A = Jᵀ A J: columns p and q, then rows p and q.
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (row_p, row_q) = (a[p], a[q]);
                for (k, (apk, aqk)) in row_p.into_iter().zip(row_q).enumerate() {
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut values = [0.0; N];
    for (i, x) in values.iter_mut().enumerate() {
        *x = a[i][i];
    }
    (values, v)
}

fn apply_rot(r: &Rot, p: V3) -> V3 {
    [dot(r[0], p), dot(r[1], p), dot(r[2], p)]
}

fn mul_rot(a: &Rot, b: &Rot) -> Rot {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn transpose(r: &Rot) -> Rot {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = r[j][i];
        }
    }
    out
}

/// Area-weighted sample points of a surface (triangle centres) with their weights.
fn samples(world: &World, most: usize) -> Vec<(V3, f64)> {
    let all: Vec<(V3, f64)> = world
        .triangles
        .iter()
        .map(|&t| {
            let [a, b, c] = world.corners(t);
            (scale(add(add(a, b), c), 1.0 / 3.0), world.normal_area(t).1)
        })
        .filter(|(_, w)| *w > 0.0)
        .collect();
    let stride = all.len().div_ceil(most.max(1)).max(1);
    all.into_iter().step_by(stride).collect()
}

/// The weighted centre and principal axes (rows, largest spread first, right-handed).
fn principal(points: &[(V3, f64)]) -> (V3, Rot) {
    let total: f64 = points
        .iter()
        .map(|p| p.1)
        .sum::<f64>()
        .max(f64::MIN_POSITIVE);
    let centre = scale(
        points
            .iter()
            .fold([0.0; 3], |s, (p, w)| add(s, scale(*p, *w))),
        1.0 / total,
    );
    let mut cov = [[0.0; 3]; 3];
    for (p, w) in points {
        let d = sub(*p, centre);
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x += w * d[i] * d[j];
            }
        }
    }
    let (values, vectors) = jacobi(cov);
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| values[b].total_cmp(&values[a]));
    let column = |k: usize| [vectors[0][k], vectors[1][k], vectors[2][k]];
    let (e0, e1) = (column(order[0]), column(order[1]));
    (centre, [e0, e1, cross(e0, e1)])
}

/// The rotation and translation that best move `from` onto `to` (Horn's quaternion method).
fn best_rigid(pairs: &[(V3, V3, f64)]) -> (Rot, V3) {
    let total: f64 = pairs
        .iter()
        .map(|p| p.2)
        .sum::<f64>()
        .max(f64::MIN_POSITIVE);
    let cf = scale(
        pairs.iter().fold([0.0; 3], |s, p| add(s, scale(p.0, p.2))),
        1.0 / total,
    );
    let ct = scale(
        pairs.iter().fold([0.0; 3], |s, p| add(s, scale(p.1, p.2))),
        1.0 / total,
    );
    let mut m = [[0.0; 3]; 3];
    for (f, t, w) in pairs {
        let (a, b) = (sub(*f, cf), sub(*t, ct));
        for (i, row) in m.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x += w * a[i] * b[j];
            }
        }
    }
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = m;
    let n = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let (values, vectors) = jacobi(n);
    let best = (0..4)
        .max_by(|&a, &b| values[a].total_cmp(&values[b]))
        .unwrap_or(0);
    let [w, x, y, z] = [
        vectors[0][best],
        vectors[1][best],
        vectors[2][best],
        vectors[3][best],
    ];
    let r = [
        [
            w * w + x * x - y * y - z * z,
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            w * w - x * x + y * y - z * z,
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            w * w - x * x - y * y + z * z,
        ],
    ];
    (r, sub(ct, apply_rot(&r, cf)))
}

/// Iterative closest points from the transform (r, t): returns the improved transform and its
/// root-mean-square distance.
fn icp(
    target: &Target,
    points: &[(V3, f64)],
    mut r: Rot,
    mut t: V3,
    rounds: usize,
    cancel: &AtomicBool,
) -> RepairResult<(Rot, V3, f64)> {
    let mut rms = f64::INFINITY;
    for _ in 0..rounds {
        if cancel.load(Ordering::Relaxed) {
            return Err(RepairError::Cancelled);
        }
        let pairs: Vec<(V3, V3, f64)> = points
            .par_iter()
            .filter_map(|&(p, w)| {
                let moved = add(apply_rot(&r, p), t);
                target.nearest(moved).map(|(q, _)| (moved, q, w))
            })
            .collect();
        let total: f64 = pairs
            .iter()
            .map(|p| p.2)
            .sum::<f64>()
            .max(f64::MIN_POSITIVE);
        let now = (pairs
            .iter()
            .map(|(a, b, w)| w * dot(sub(*a, *b), sub(*a, *b)))
            .sum::<f64>()
            / total)
            .sqrt();
        let (dr, dt) = best_rigid(&pairs);
        r = mul_rot(&dr, &r);
        t = add(apply_rot(&dr, t), dt);
        let settled = (rms - now).abs() <= 1e-12 * (1.0 + rms.abs());
        rms = now;
        if settled {
            break;
        }
    }
    Ok((r, t, rms))
}

/// The rigid transform that fits `moving` onto `fixed` best.
pub fn align(fixed: &World, moving: &World, control: &mut Control) -> RepairResult<Fit> {
    let cancel = control.cancel_flag();
    let target = Target::new(fixed);
    let fixed_samples = samples(fixed, 20_000);
    let moving_samples = samples(moving, 4_000);
    let (cf, af) = principal(&fixed_samples);
    let (cm, am) = principal(&moving_samples);
    // Candidates: moved centre on centre, unturned; and the principal axes matched in the four
    // right-handed ways.
    let mut candidates: Vec<Rot> = vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]];
    for (s0, s1) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let flipped = [scale(am[0], s0), scale(am[1], s1), scale(am[2], s0 * s1)];
        // Rows of `flipped` map world into the moving part's axes; af's rows back out of them.
        candidates.push(mul_rot(&transpose(&af), &flipped));
    }
    let mut best: Option<(Rot, V3, f64)> = None;
    for (k, r) in candidates.iter().enumerate() {
        control.check()?;
        let t = sub(cf, apply_rot(r, cm));
        let found = icp(&target, &moving_samples, *r, t, 15, cancel)?;
        if best.is_none_or(|b| found.2 < b.2) {
            best = Some(found);
        }
        control.progress((k + 1) as f64 / (candidates.len() + 1) as f64);
    }
    let (r, t, _) = best.unwrap_or((
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
        0.0,
    ));
    let (r, t, rms) = icp(&target, &moving_samples, r, t, 60, cancel)?;
    control.progress(1.0);
    let trace = r[0][0] + r[1][1] + r[2][2];
    Ok(Fit {
        rotation: r,
        translation: t,
        angle: ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees(),
        rms,
    })
}

// ---------------------------------------------------------------------------------------------
// The comparison

/// The distance of every vertex of `from`'s refined copy to `to`'s surface.
fn side(from: &World, to: &World, control: &mut Control) -> RepairResult<(SideResult, Surface)> {
    let cancel = control.cancel_flag();
    let (points, triangles) = refine(from, REFINED_TRIANGLES);
    control.progress(0.2);
    control.check()?;
    let target = Target::new(to);
    let count = points.len() / 3;
    let mut distances = vec![0f32; count];
    const BATCH: usize = 65_536;
    let mut done = 0;
    while done < count {
        control.check()?;
        let end = (done + BATCH).min(count);
        distances[done..end]
            .par_iter_mut()
            .enumerate()
            .for_each(|(k, d)| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let i = (done + k) * 3;
                let p = [points[i] as f64, points[i + 1] as f64, points[i + 2] as f64];
                *d = target.signed_distance(p).unwrap_or(0.0) as f32;
            });
        done = end;
        control.progress(0.2 + 0.8 * done as f64 / count.max(1) as f64);
    }
    control.check()?;
    let mut r = SideResult {
        points: count as u64,
        ..SideResult::default()
    };
    let (mut signed, mut unsigned) = (0.0, 0.0);
    for &d in &distances {
        let d = d as f64;
        r.max_positive = r.max_positive.max(d);
        r.max_negative = r.max_negative.min(d);
        r.max_unsigned = r.max_unsigned.max(d.abs());
        signed += d;
        unsigned += d.abs();
    }
    if count > 0 {
        r.average_signed = signed / count as f64;
        r.average_unsigned = unsigned / count as f64;
    }
    Ok((
        r,
        Surface {
            points,
            triangles,
            distances,
        },
    ))
}

/// `World` moved by the fit.
fn fitted(world: &World, fit: &Fit) -> World {
    World {
        points: world
            .points
            .iter()
            .map(|&p| add(apply_rot(&fit.rotation, p), fit.translation))
            .collect(),
        triangles: world.triangles.clone(),
    }
}

pub fn compare(
    reference: &World,
    comparison: &World,
    s: &CompareSettings,
    control: &mut Control,
) -> RepairResult<Comparison> {
    let (fit, moved);
    let comparison = if s.align {
        let found = align(reference, comparison, &mut control.sub(0.0, 0.3))?;
        moved = fitted(comparison, &found);
        fit = Some(found);
        &moved
    } else {
        fit = None;
        comparison
    };
    let start = if s.align { 0.3 } else { 0.0 };
    let (forward, backward) = match s.direction {
        Direction::ReferenceToComparison => (
            Some(side(reference, comparison, &mut control.sub(start, 1.0))?),
            None,
        ),
        Direction::ComparisonToReference => (
            None,
            Some(side(comparison, reference, &mut control.sub(start, 1.0))?),
        ),
        Direction::Both => {
            let mid = (start + 1.0) / 2.0;
            let f = side(reference, comparison, &mut control.sub(start, mid))?;
            let b = side(comparison, reference, &mut control.sub(mid, 1.0))?;
            (Some(f), Some(b))
        }
    };
    Ok(Comparison {
        forward,
        backward,
        fit,
    })
}
