//! The exact mesh arrangement behind Wrap part surface: every triangle split along the curves where
//! other triangles cross or touch it, so that afterwards triangles meet only along shared edges and
//! at shared corners, and overlapping coplanar triangles become one layer of faces with a coverage.
//!
//! Nothing is perturbed or snapped. Points that intersections create stay implicit, defined by what
//! made them: where an input edge crosses a triangle's plane, where two input edges cross, where the
//! planes of three triangles meet. Every decision is an exact predicate on such points: orientation
//! in a plane, order along a line, the side of a plane. A predicate first evaluates in floating
//! point with a rigorous error bound and falls back to exact expansion arithmetic (Shewchuk) only
//! when the bound cannot decide, which in practice means exactly degenerate cases; most of those
//! (points along one crossing segment or one edge) the points' construction settles without any
//! arithmetic (`Input::collinear_by_construction`). So the contacts
//! CAD models are full of (faces in one plane, a corner on another part's edge, an edge lying in
//! another part's face, three faces through one point) come out as what they are: shared points and
//! edges, never slivers.
//!
//! `build` in stages:
//! 1. `contacts`: each pair of triangles whose boxes touch, classified exactly from their corners:
//!    the segment where they cross, the points where they touch, or an overlap in a shared plane
//!    (the two then belong to one cluster and are triangulated together).
//! 2. The points inside each input edge, sorted along it (`edge_chain`).
//! 3. Per unit (a triangle, or a cluster of coplanar overlapping triangles): the crossings among its
//!    segments and the points lying on them (`unit_points`); then per segment the points both of its
//!    triangles found, sorted along it (`segment_chain`).
//! 4. Per unit: a constrained triangulation of its plane (`Cdt`) with every chain as a constraint,
//!    and the net coverage of each face: +1 per triangle over it facing one way, −1 per triangle
//!    facing the other. A face whose coverage cancels is no surface at all and goes.
//! 5. Points found to coincide on the way are merged, and the faces assembled.
//!
//! Coordinates must be of ordinary size (as for `exact`): products of up to about twenty coordinate
//! differences must neither overflow nor underflow f64, which holds for any model in millimetres.

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering as Atomic};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use super::bvh::{self, Aabb, Bvh};
use super::exact::{self, P2, P3};
use super::topology::Edges;
use super::{Control, RepairResult};
use crate::mesh::Mesh;

/// Why an arrangement could not be built; the caller keeps the mesh it had.
pub type Failure = &'static str;

const NONE: u32 = u32::MAX;

// ---------------------------------------------------------------------------------------------
// Exact arithmetic

/// Shewchuk's floating-point expansions: a number held exactly as a sum of f64 components that do
/// not overlap, in increasing magnitude, so sums and products are exact. The predicates fall back
/// to these when floating point cannot decide.
pub(super) mod exp {
    /// An expansion. Never empty: zero is `[0.0]`, and the last component carries the sign.
    pub type Exp = Vec<f64>;

    /// 2^27 + 1, for splitting a double into two halves of 26 bits.
    const SPLITTER: f64 = 134_217_729.0;

    #[inline]
    fn two_sum(a: f64, b: f64) -> (f64, f64) {
        let x = a + b;
        let bv = x - a;
        let av = x - bv;
        (x, (a - av) + (b - bv))
    }

    #[inline]
    fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
        let x = a + b;
        (x, b - (x - a))
    }

    #[inline]
    fn split(a: f64) -> (f64, f64) {
        let c = SPLITTER * a;
        let hi = c - (c - a);
        (hi, a - hi)
    }

    /// The exact value a − b.
    pub fn diff(a: f64, b: f64) -> Exp {
        let x = a - b;
        let bv = a - x;
        let av = x + bv;
        let y = (a - av) + (bv - b);
        if y == 0.0 { vec![x] } else { vec![y, x] }
    }

    pub fn from(x: f64) -> Exp {
        vec![x]
    }

    /// e + f, zero components dropped (fast_expansion_sum_zeroelim).
    pub fn add(e: &[f64], f: &[f64]) -> Exp {
        let mut h = Vec::with_capacity(e.len() + f.len());
        let (mut i, mut j) = (0, 0);
        // Components in increasing magnitude, merged from both.
        let mut next = || match (e.get(i), f.get(j)) {
            (Some(&x), Some(&y)) => {
                if (y > x) == (y > -x) {
                    i += 1;
                    Some(x)
                } else {
                    j += 1;
                    Some(y)
                }
            }
            (Some(&x), None) => {
                i += 1;
                Some(x)
            }
            (None, Some(&y)) => {
                j += 1;
                Some(y)
            }
            (None, None) => None,
        };
        let mut q = next().unwrap_or(0.0);
        while let Some(x) = next() {
            let (s, small) = two_sum(q, x);
            if small != 0.0 {
                h.push(small);
            }
            q = s;
        }
        if q != 0.0 || h.is_empty() {
            h.push(q);
        }
        h
    }

    pub fn neg(e: &[f64]) -> Exp {
        e.iter().map(|&x| -x).collect()
    }

    pub fn sub(e: &[f64], f: &[f64]) -> Exp {
        add(e, &neg(f))
    }

    /// e · b, zero components dropped (scale_expansion_zeroelim).
    pub fn scale(e: &[f64], b: f64) -> Exp {
        let mut h = Vec::with_capacity(e.len() * 2);
        let (bh, bl) = split(b);
        let product = |a: f64| {
            let x = a * b;
            let (ah, al) = split(a);
            let err = ((x - ah * bh) - al * bh) - ah * bl;
            (x, al * bl - err)
        };
        let (mut q, small) = product(e[0]);
        if small != 0.0 {
            h.push(small);
        }
        for &a in &e[1..] {
            let (big, low) = product(a);
            let (sum, small) = two_sum(q, low);
            if small != 0.0 {
                h.push(small);
            }
            let (s, small) = fast_two_sum(big, sum);
            if small != 0.0 {
                h.push(small);
            }
            q = s;
        }
        if q != 0.0 || h.is_empty() {
            h.push(q);
        }
        h
    }

    /// e · f, compressed.
    pub fn mul(e: &[f64], f: &[f64]) -> Exp {
        let (e, f) = if e.len() >= f.len() { (e, f) } else { (f, e) };
        let mut sum = scale(e, f[0]);
        for &b in &f[1..] {
            sum = add(&sum, &scale(e, b));
        }
        if sum.len() > 4 { compress(&sum) } else { sum }
    }

    /// The same value in as few components as possible (Shewchuk's compress).
    pub fn compress(e: &[f64]) -> Exp {
        let n = e.len();
        let mut g = e.to_vec();
        let mut bottom = n - 1;
        let mut q = e[n - 1];
        for i in (0..n - 1).rev() {
            let (big, small) = fast_two_sum(q, e[i]);
            if small != 0.0 {
                g[bottom] = big;
                bottom -= 1;
                q = small;
            } else {
                q = big;
            }
        }
        let mut h = Vec::with_capacity(n - bottom);
        for &x in &g[bottom + 1..] {
            let (big, small) = fast_two_sum(x, q);
            if small != 0.0 {
                h.push(small);
            }
            q = big;
        }
        h.push(q);
        h
    }

    pub fn sign(e: &[f64]) -> i32 {
        let x = e[e.len() - 1];
        (x > 0.0) as i32 - (x < 0.0) as i32
    }

    /// An approximation of the value (the components summed, smallest first).
    pub fn estimate(e: &[f64]) -> f64 {
        e.iter().sum()
    }
}

use exp::Exp;

/// Unit roundoff of f64.
const U: f64 = f64::EPSILON / 2.0;

/// A floating-point value and a bound on how far it may lie from the exact value it stands for.
/// The filters evaluate predicates with it and trust a sign only when the bound allows.
#[derive(Clone, Copy, Debug)]
struct Fx {
    v: f64,
    e: f64,
}

impl Fx {
    fn new(v: f64, e: f64) -> Fx {
        Fx { v, e }
    }

    fn exact(v: f64) -> Fx {
        Fx { v, e: 0.0 }
    }

    fn add(self, o: Fx) -> Fx {
        let v = self.v + o.v;
        Fx {
            v,
            e: self.e + o.e + v.abs() * U,
        }
    }

    fn sub(self, o: Fx) -> Fx {
        let v = self.v - o.v;
        Fx {
            v,
            e: self.e + o.e + v.abs() * U,
        }
    }

    fn mul(self, o: Fx) -> Fx {
        let v = self.v * o.v;
        Fx {
            v,
            e: self.v.abs() * o.e + o.v.abs() * self.e + self.e * o.e + v.abs() * U,
        }
    }

    /// None when the divisor's sign is not certain.
    fn div(self, o: Fx) -> Option<Fx> {
        let low = o.v.abs() - o.e;
        if low.is_nan() || low <= 0.0 {
            return None;
        }
        let v = self.v / o.v;
        Some(Fx {
            v,
            e: (self.e * o.v.abs() + self.v.abs() * o.e) / (low * o.v.abs()) + v.abs() * U,
        })
    }

    /// The sign, when the bound decides it. The margin covers the rounding of the bound itself.
    fn sign(self) -> Option<i32> {
        let m = self.e * (1.0 + 1e-9) + f64::MIN_POSITIVE;
        if self.v > m {
            Some(1)
        } else if self.v < -m {
            Some(-1)
        } else {
            None
        }
    }
}

type F3 = [Fx; 3];
pub(super) type E3 = [Exp; 3];

fn fx3(p: P3, e: f64) -> F3 {
    p.map(|x| Fx::new(x, e))
}

fn sub_f(a: F3, b: F3) -> F3 {
    [a[0].sub(b[0]), a[1].sub(b[1]), a[2].sub(b[2])]
}

fn cross_f(a: F3, b: F3) -> F3 {
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}

fn dot_f(a: F3, b: F3) -> Fx {
    a[0].mul(b[0]).add(a[1].mul(b[1])).add(a[2].mul(b[2]))
}

pub(super) fn sub_e(a: P3, b: P3) -> E3 {
    [0, 1, 2].map(|k| exp::diff(a[k], b[k]))
}

pub(super) fn cross_e(a: &E3, b: &E3) -> E3 {
    [0, 1, 2].map(|k| {
        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
        exp::sub(&exp::mul(&a[i], &b[j]), &exp::mul(&a[j], &b[i]))
    })
}

pub(super) fn dot_e(a: &E3, b: &E3) -> Exp {
    exp::add(
        &exp::add(&exp::mul(&a[0], &b[0]), &exp::mul(&a[1], &b[1])),
        &exp::mul(&a[2], &b[2]),
    )
}

/// (b − a) × (c − a), exactly.
pub(super) fn normal_e(a: P3, b: P3, c: P3) -> E3 {
    cross_e(&sub_e(b, a), &sub_e(c, a))
}

/// An estimate of an exact value with a bound on its error.
fn estimate_fx(e: &[f64]) -> Fx {
    let v = exp::estimate(e);
    let size: f64 = e.iter().map(|x| x.abs()).sum();
    Fx::new(v, size * U * (e.len() as f64 + 1.0))
}

// ---------------------------------------------------------------------------------------------
// Points

/// The triangles an arrangement is built over: welded (one vertex per position, so corners are
/// shared exactly when their indices are), every triangle with area, no two over the same corners
/// facing the same way.
pub struct Input {
    pub points: Vec<P3>,
    pub tris: Vec<[u32; 3]>,
    edges: Edges,
    /// The triangles round each vertex: those of vertex v are around[start[v]..start[v + 1]].
    start: Vec<u32>,
    around: Vec<u32>,
}

impl Input {
    pub fn new(mesh: &Mesh) -> Input {
        let tris: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let count = mesh.vertex_count();
        let mut start = vec![0u32; count + 1];
        for &v in tris.iter().flatten() {
            start[v as usize + 1] += 1;
        }
        for v in 0..count {
            start[v + 1] += start[v];
        }
        let mut fill = start.clone();
        let mut around = vec![0u32; tris.len() * 3];
        for (t, tri) in tris.iter().enumerate() {
            for &v in tri {
                around[fill[v as usize] as usize] = t as u32;
                fill[v as usize] += 1;
            }
        }
        Input {
            points: (0..count as u32).map(|v| mesh.point(v)).collect(),
            tris,
            edges: Edges::new(mesh),
            start,
            around,
        }
    }

    /// The triangles round vertex v.
    fn around(&self, v: u32) -> &[u32] {
        &self.around[self.start[v as usize] as usize..self.start[v as usize + 1] as usize]
    }

    /// What the point lies on by how it was made (not necessarily all of it): the input triangles
    /// whose planes contain it, and the input edges whose lines do.
    fn supports(&self, g: &Geom, planes: &mut Vec<u32>, lines: &mut Vec<u32>) {
        let sides = |e: u32| self.edges.sides(e).iter().map(|&s| s / 3);
        match *g {
            Geom::V(v) => {
                planes.extend_from_slice(self.around(v));
                for &t in self.around(v) {
                    let k = (0..3).find(|&k| self.tris[t as usize][k] == v).unwrap_or(0);
                    lines.push(self.side(t, k));
                    lines.push(self.side(t, (k + 2) % 3));
                }
            }
            Geom::Lpi(e, plane) => {
                planes.extend(sides(e));
                lines.push(e);
                match plane {
                    Plane::Tri(f) => planes.push(f),
                    Plane::Edge(f, _) => {
                        planes.extend(sides(f));
                        lines.push(f);
                    }
                }
            }
            Geom::Tpi(ts) => planes.extend_from_slice(&ts),
            Geom::Probe(..) | Geom::Flat(_) => {}
        }
    }

    /// True when the three points lie, by how they were made, on one input edge's line or on the
    /// planes of two input triangles that are not coplanar (so on the line where those meet): their
    /// orientation in any projection is zero, with no arithmetic on the points. False says nothing.
    fn collinear_by_construction(&self, pts: [&Pt; 3]) -> bool {
        let (mut planes, mut lines) = (Vec::new(), Vec::new());
        self.supports(&pts[0].g, &mut planes, &mut lines);
        let (mut other_planes, mut other_lines) = (Vec::new(), Vec::new());
        for p in &pts[1..] {
            other_planes.clear();
            other_lines.clear();
            self.supports(&p.g, &mut other_planes, &mut other_lines);
            planes.retain(|t| other_planes.contains(t));
            lines.retain(|e| other_lines.contains(e));
            if planes.len() < 2 && lines.is_empty() {
                return false;
            }
        }
        if !lines.is_empty() {
            return true;
        }
        let [a, b, c] = self.corners(planes[0]);
        planes[1..].iter().any(|&t| {
            self.corners(t)
                .iter()
                .any(|&x| exact::orient3d(a, b, c, x) != 0)
        })
    }

    /// The edge of side k of triangle t (from corner k to corner k + 1).
    fn side(&self, t: u32, k: usize) -> u32 {
        self.edges.edge_of(t * 3 + k as u32)
    }

    /// The edge of triangle t between corners u and v.
    fn edge_between(&self, t: u32, u: usize, v: usize) -> u32 {
        if v == (u + 1) % 3 {
            self.side(t, u)
        } else {
            self.side(t, v)
        }
    }

    pub fn corners(&self, t: u32) -> [P3; 3] {
        self.tris[t as usize].map(|v| self.points[v as usize])
    }

    /// The lower and higher vertex of edge e, as points.
    fn edge_points(&self, e: u32) -> [P3; 2] {
        self.edges.ends(e).map(|v| self.points[v as usize])
    }

    /// The three points that span `plane`.
    fn plane_points(&self, plane: Plane) -> [P3; 3] {
        match plane {
            Plane::Tri(t) => self.corners(t),
            Plane::Edge(e, axis) => {
                let [p, q] = self.edge_points(e);
                let mut r = p;
                let k = axis as usize;
                r[k] = p[k] + offset(p[k]);
                [p, q, r]
            }
        }
    }

    /// How to compute with the point `key` stands for.
    fn geom(&self, key: Key) -> Geom {
        match key {
            Key::V(v) => Geom::V(v),
            Key::EF(e, f) => Geom::Lpi(e, Plane::Tri(f)),
            Key::EE(e, f) => Geom::Lpi(e, Plane::Edge(f, self.across_axis(e, f))),
            Key::TPI(a, b, c) => Geom::Tpi([a, b, c]),
        }
    }

    /// For two crossing edges e and f: the axis along which the normal of the plane they both lie
    /// in points most, so the plane through f along that axis does not contain e.
    fn across_axis(&self, e: u32, f: u32) -> u8 {
        let [p, q] = self.edge_points(e);
        let [r, s] = self.edge_points(f);
        let n = cross_e(&sub_e(q, p), &sub_e(s, r));
        (0..3)
            .max_by(|&x, &y| {
                exp::estimate(&n[x])
                    .abs()
                    .total_cmp(&exp::estimate(&n[y]).abs())
            })
            .unwrap_or(0) as u8
    }

    /// Exact homogeneous coordinates.
    fn hom(&self, g: &Geom) -> Hom {
        match *g {
            Geom::V(v) => Hom::of(self.points[v as usize]),
            Geom::Flat(p) => Hom::of([p[0], p[1], 0.0]),
            Geom::Lpi(e, plane) => lpi_hom(self.edge_points(e), self.plane_points(plane)),
            Geom::Probe(line, t) => lpi_hom(line, self.corners(t)),
            Geom::Tpi(ts) => tpi_hom(ts.map(|t| self.corners(t))),
        }
    }

    /// Approximate coordinates and a bound on their error (the same for all three).
    fn approx(&self, g: &Geom) -> (P3, f64) {
        let fast = match *g {
            Geom::V(v) => return (self.points[v as usize], 0.0),
            Geom::Flat(p) => return ([p[0], p[1], 0.0], 0.0),
            Geom::Lpi(e, plane) => lpi_fast(self.edge_points(e), self.plane_points(plane)),
            Geom::Probe(line, t) => lpi_fast(line, self.corners(t)),
            Geom::Tpi(ts) => tpi_fast(ts.map(|t| self.corners(t))),
        };
        if let Some((p, e)) = fast {
            let size = p.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            if e <= 1e-13 * size {
                return (p, e);
            }
        }
        approx_hom(&self.hom(g))
    }

    fn pt(&self, g: Geom) -> Pt {
        let (p, e) = self.approx(&g);
        Pt { g, p, e }
    }
}

/// A power of two that `x` (at most 24 significant bits, as every coordinate here) can be moved by
/// exactly: at least |x|, so the sum needs at most 26 bits.
fn offset(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let exponent = ((x.abs().to_bits() >> 52) & 0x7ff) as i32 - 1023;
    2f64.powi(exponent + 1)
}

/// A point of the arrangement by how it was made; the arrangement's vertices are keyed by these.
/// Ordered with input vertices first, so a group of coincident points keeps an input vertex if it
/// has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    /// An input vertex.
    V(u32),
    /// Where input edges e and f (e < f) cross, inside both.
    EE(u32, u32),
    /// Where input edge e crosses the inside of triangle f.
    EF(u32, u32),
    /// Where the planes of three triangles (sorted) meet, inside all three.
    TPI(u32, u32, u32),
}

/// How to compute with a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Geom {
    /// An input vertex.
    V(u32),
    /// Where the line through input edge e crosses a plane.
    Lpi(u32, Plane),
    /// Where the planes of three input triangles meet.
    Tpi([u32; 3]),
    /// Where the line through two explicit points crosses the plane of input triangle t: a point
    /// on a face that a ray starts from.
    Probe([P3; 2], u32),
    /// A point of a unit's projection only (the enclosing triangle's corners).
    Flat(P2),
}

/// A plane an implicit point is made with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plane {
    /// The plane of input triangle t.
    Tri(u32),
    /// The plane through input edge e that contains the direction of the given axis.
    Edge(u32, u8),
}

/// Exact homogeneous coordinates: the point is x / w, with w > 0.
struct Hom {
    x: E3,
    w: Exp,
}

impl Hom {
    fn of(p: P3) -> Hom {
        Hom {
            x: p.map(exp::from),
            w: exp::from(1.0),
        }
    }

    fn positive(x: E3, w: Exp) -> Hom {
        if exp::sign(&w) < 0 {
            Hom {
                x: x.map(|v| exp::neg(&v)),
                w: exp::neg(&w),
            }
        } else {
            Hom { x, w }
        }
    }
}

/// The line through p and q meets the plane through a, b, c at (q dp − p dq) / (dp − dq), with dp
/// and dq six times the signed volumes of the tetrahedra (a, b, c, p) and (a, b, c, q).
fn lpi_hom([p, q]: [P3; 2], [a, b, c]: [P3; 3]) -> Hom {
    let n = normal_e(a, b, c);
    let dp = dot_e(&n, &sub_e(p, a));
    let dq = dot_e(&n, &sub_e(q, a));
    let w = exp::sub(&dp, &dq);
    let x = [0, 1, 2].map(|k| exp::sub(&exp::scale(&dp, q[k]), &exp::scale(&dq, p[k])));
    Hom::positive(x, w)
}

/// Three planes n_i · x = d_i meet at (d1 n2 × n3 + d2 n3 × n1 + d3 n1 × n2) / (n1 · n2 × n3).
fn tpi_hom(planes: [[P3; 3]; 3]) -> Hom {
    let n = planes.map(|[a, b, c]| normal_e(a, b, c));
    let d: [Exp; 3] = [0, 1, 2].map(|i| {
        let a = planes[i][0];
        exp::add(
            &exp::add(&exp::scale(&n[i][0], a[0]), &exp::scale(&n[i][1], a[1])),
            &exp::scale(&n[i][2], a[2]),
        )
    });
    let c23 = cross_e(&n[1], &n[2]);
    let c31 = cross_e(&n[2], &n[0]);
    let c12 = cross_e(&n[0], &n[1]);
    let w = dot_e(&n[0], &c23);
    let x = [0, 1, 2].map(|k| {
        exp::add(
            &exp::add(&exp::mul(&d[0], &c23[k]), &exp::mul(&d[1], &c31[k])),
            &exp::mul(&d[2], &c12[k]),
        )
    });
    Hom::positive(x, w)
}

/// `lpi_hom` in floating point, as p + t (q − p) so the point stays accurate next to either end.
fn lpi_fast([p, q]: [P3; 2], [a, b, c]: [P3; 3]) -> Option<(P3, f64)> {
    let ex = |x: P3| fx3(x, 0.0);
    let n = cross_f(sub_f(ex(b), ex(a)), sub_f(ex(c), ex(a)));
    let dp = dot_f(n, sub_f(ex(p), ex(a)));
    let dq = dot_f(n, sub_f(ex(q), ex(a)));
    let t = dp.div(dp.sub(dq))?;
    let mut out = [0.0; 3];
    let mut err = 0.0f64;
    for k in 0..3 {
        let x = Fx::exact(p[k]).add(t.mul(Fx::exact(q[k]).sub(Fx::exact(p[k]))));
        out[k] = x.v;
        err = err.max(x.e);
    }
    Some((out, err))
}

/// `tpi_hom` in floating point, relative to the first plane's first point.
fn tpi_fast(planes: [[P3; 3]; 3]) -> Option<(P3, f64)> {
    let o = planes[0][0];
    let ex = |x: P3| fx3(x, 0.0);
    let n = planes.map(|[a, b, c]| cross_f(sub_f(ex(b), ex(a)), sub_f(ex(c), ex(a))));
    let d = [0, 1, 2].map(|i| dot_f(n[i], sub_f(ex(planes[i][0]), ex(o))));
    let c23 = cross_f(n[1], n[2]);
    let c31 = cross_f(n[2], n[0]);
    let c12 = cross_f(n[0], n[1]);
    let w = dot_f(n[0], c23);
    let mut out = [0.0; 3];
    let mut err = 0.0f64;
    for k in 0..3 {
        let x = d[0]
            .mul(c23[k])
            .add(d[1].mul(c31[k]))
            .add(d[2].mul(c12[k]))
            .div(w)?;
        let x = Fx::exact(o[k]).add(x);
        out[k] = x.v;
        err = err.max(x.e);
    }
    Some((out, err))
}

fn approx_hom(h: &Hom) -> (P3, f64) {
    let w = estimate_fx(&h.w);
    let mut out = [0.0; 3];
    let mut err = 0.0f64;
    for k in 0..3 {
        match estimate_fx(&h.x[k]).div(w) {
            Some(x) => {
                out[k] = x.v;
                err = err.max(x.e);
            }
            None => return (out, f64::INFINITY),
        }
    }
    (out, err)
}

/// A point ready for predicates: how it was made, its approximate position and the error bound.
#[derive(Clone, Copy, Debug)]
pub struct Pt {
    pub g: Geom,
    pub p: P3,
    pub e: f64,
}

fn hom_of(input: &Input, p: &Pt) -> Hom {
    if p.e == 0.0 {
        Hom::of(p.p)
    } else {
        input.hom(&p.g)
    }
}

/// The point seen along `axis` (Flat points are already in the projection).
fn flat(p: &Pt, axis: usize) -> P2 {
    match p.g {
        Geom::Flat(q) => q,
        _ => exact::project(p.p, axis),
    }
}

fn flat_hom(input: &Input, p: &Pt, axis: usize) -> [Exp; 3] {
    if p.e == 0.0 {
        let q = flat(p, axis);
        return [exp::from(q[0]), exp::from(q[1]), exp::from(1.0)];
    }
    let h = input.hom(&p.g);
    let [x, y, z] = h.x;
    let xs = [x, y, z];
    [xs[(axis + 1) % 3].clone(), xs[(axis + 2) % 3].clone(), h.w]
}

/// exact::orient2d of three points seen along `axis`.
pub fn orient2d(input: &Input, axis: usize, a: &Pt, b: &Pt, c: &Pt) -> i32 {
    let (fa, fb, fc) = (flat(a, axis), flat(b, axis), flat(c, axis));
    if a.e == 0.0 && b.e == 0.0 && c.e == 0.0 {
        return exact::orient2d(fa, fb, fc);
    }
    let f = |q: P2, e: f64| [Fx::new(q[0], e), Fx::new(q[1], e)];
    let (pa, pb, pc) = (f(fa, a.e), f(fb, b.e), f(fc, c.e));
    let det = pb[0]
        .sub(pa[0])
        .mul(pc[1].sub(pa[1]))
        .sub(pb[1].sub(pa[1]).mul(pc[0].sub(pa[0])));
    if let Some(s) = det.sign() {
        return s;
    }
    // Floating point cannot decide almost only when the points are exactly collinear, which
    // their construction usually shows: points along one crossing segment or one edge.
    if input.collinear_by_construction([a, b, c]) {
        return 0;
    }
    let [ha, hb, hc] = [a, b, c].map(|p| flat_hom(input, p, axis));
    let minor = |i: usize, j: usize| exp::sub(&exp::mul(&hb[i], &hc[j]), &exp::mul(&hb[j], &hc[i]));
    let t1 = exp::mul(&ha[0], &minor(1, 2));
    let t2 = exp::mul(&ha[1], &minor(0, 2));
    let t3 = exp::mul(&ha[2], &minor(0, 1));
    exp::sign(&exp::add(&exp::sub(&t1, &t2), &t3))
}

/// The order of two points' coordinate k.
pub fn cmp_coord(input: &Input, a: &Pt, b: &Pt, k: usize) -> Ordering {
    if a.e == 0.0 && b.e == 0.0 {
        return a.p[k].partial_cmp(&b.p[k]).unwrap_or(Ordering::Equal);
    }
    if let Some(s) = Fx::new(a.p[k], a.e).sub(Fx::new(b.p[k], b.e)).sign() {
        return s.cmp(&0);
    }
    let (ha, hb) = (hom_of(input, a), hom_of(input, b));
    exp::sign(&exp::sub(
        &exp::mul(&ha.x[k], &hb.w),
        &exp::mul(&hb.x[k], &ha.w),
    ))
    .cmp(&0)
}

/// True when the two points coincide. Each point's exact coordinates are made once, for all
/// three axes: they are the expensive part.
pub fn same(input: &Input, a: &Pt, b: &Pt) -> bool {
    if a.e == 0.0 && b.e == 0.0 {
        return a.p == b.p;
    }
    // The filter tells coordinates apart, never that they are equal.
    if (0..3).any(|k| {
        Fx::new(a.p[k], a.e)
            .sub(Fx::new(b.p[k], b.e))
            .sign()
            .is_some()
    }) {
        return false;
    }
    let (ha, hb) = (hom_of(input, a), hom_of(input, b));
    (0..3).all(|k| {
        exp::sign(&exp::sub(
            &exp::mul(&ha.x[k], &hb.w),
            &exp::mul(&hb.x[k], &ha.w),
        )) == 0
    })
}

/// exact::orient3d of an explicit plane and a point.
pub fn orient3d_plane(input: &Input, [a, b, c]: [P3; 3], x: &Pt) -> i32 {
    if x.e == 0.0 {
        return exact::orient3d(a, b, c, x.p);
    }
    let ex = |p: P3| fx3(p, 0.0);
    let n = cross_f(sub_f(ex(b), ex(a)), sub_f(ex(c), ex(a)));
    if let Some(s) = dot_f(n, sub_f(fx3(x.p, x.e), ex(a))).sign() {
        return s;
    }
    let h = input.hom(&x.g);
    let n = normal_e(a, b, c);
    let rel = [0, 1, 2].map(|k| exp::sub(&h.x[k], &exp::scale(&h.w, a[k])));
    exp::sign(&dot_e(&n, &rel))
}

/// The sign of d · (q − p) for an exact direction d.
pub fn dir_sign(input: &Input, d: &E3, p: &Pt, q: &Pt) -> i32 {
    let dv = [0, 1, 2].map(|k| estimate_fx(&d[k]));
    if let Some(s) = dot_f(dv, sub_f(fx3(q.p, q.e), fx3(p.p, p.e))).sign() {
        return s;
    }
    let (hp, hq) = (hom_of(input, p), hom_of(input, q));
    let rel: E3 =
        [0, 1, 2].map(|k| exp::sub(&exp::mul(&hq.x[k], &hp.w), &exp::mul(&hp.x[k], &hq.w)));
    exp::sign(&dot_e(d, &rel))
}

/// The axis to look at triangle t along: the one its normal points along most (exact).
fn plane_axis(input: &Input, t: u32) -> Result<usize, Failure> {
    let [a, b, c] = input.corners(t);
    exact::dominant_axis(a, b, c).ok_or("a triangle has no area")
}

// ---------------------------------------------------------------------------------------------
// Stage 1: contacts between pairs of triangles

/// What a pair of triangles contributes to the arrangement.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Contact {
    /// The two triangles overlap in a shared plane: one cluster.
    Union(u32, u32),
    /// A point strictly inside an input edge.
    OnEdge(u32, Key),
    /// A point strictly inside a triangle.
    Inside(u32, Key),
    /// A crossing segment inside triangle `tri` (its ends may lie on the triangle's border).
    Segment(Seg),
}

/// Where two triangles cross, as seen from one of them (`tri`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seg {
    tri: u32,
    other: u32,
    line: Line,
    ends: [Key; 2],
}

impl Seg {
    /// The segment's identity, the same from both triangles.
    fn pair(&self) -> (u32, u32) {
        (self.tri.min(self.other), self.tri.max(self.other))
    }
}

/// The line a segment lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Line {
    /// Where the planes of two triangles (sorted) meet.
    Planes(u32, u32),
    /// The line of an input edge.
    Edge(u32),
}

/// How a triangle meets the other triangle's plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// In a segment through its inside.
    Cut,
    /// Along one of its edges.
    Edge,
    /// At one corner.
    Corner,
}

/// One end of a triangle's piece of the line where the two planes meet: where the edge from
/// corner u (below the other plane, or in it) to corner v (above it, or in it) meets the other
/// plane; `corner` when that is one of the triangle's corners.
#[derive(Clone, Copy, Debug)]
struct End {
    u: usize,
    v: usize,
    corner: Option<usize>,
}

/// A triangle's piece of the line: its ends (one for a corner), how it meets the plane, and
/// whether the second end comes after the first along (own normal) × (other normal).
#[derive(Clone, Copy, Debug)]
struct Piece {
    ends: [End; 2],
    count: usize,
    kind: Kind,
    forward: bool,
}

/// The piece from the sides `s` of the triangle's corners (not all zero, not all strictly on one
/// side). The order along the line follows from which corners lie on the positive side of the
/// other plane and the orientation of the triangle (see the tests).
fn piece(s: [i32; 3]) -> Piece {
    let cross = |u, v| End { u, v, corner: None };
    let corner = |u, v, k| End {
        u,
        v,
        corner: Some(k),
    };
    match s.iter().filter(|&&x| x == 0).count() {
        0 => {
            // One corner alone on its side; the plane crosses its two edges.
            let l = if s[1] == s[2] {
                0
            } else if s[0] == s[2] {
                1
            } else {
                2
            };
            let (m, n) = ((l + 1) % 3, (l + 2) % 3);
            let ends = if s[l] < 0 {
                [cross(l, m), cross(l, n)]
            } else {
                [cross(m, l), cross(n, l)]
            };
            Piece {
                ends,
                count: 2,
                kind: Kind::Cut,
                forward: s[l] < 0,
            }
        }
        1 => {
            let z = s.iter().position(|&x| x == 0).unwrap_or(0);
            let (m, n) = ((z + 1) % 3, (z + 2) % 3);
            if s[m] != s[n] {
                // From corner z through the opposite edge.
                let (neg, pos) = if s[m] < 0 { (m, n) } else { (n, m) };
                Piece {
                    ends: [corner(z, pos, z), cross(neg, pos)],
                    count: 2,
                    kind: Kind::Cut,
                    forward: s[m] > 0,
                }
            } else {
                let end = if s[m] > 0 {
                    corner(z, m, z)
                } else {
                    corner(m, z, z)
                };
                Piece {
                    ends: [end, end],
                    count: 1,
                    kind: Kind::Corner,
                    forward: true,
                }
            }
        }
        _ => {
            // The edge between the two corners in the plane.
            let e = s.iter().position(|&x| x != 0).unwrap_or(0);
            let (y, z) = ((e + 1) % 3, (e + 2) % 3);
            let ends = if s[e] > 0 {
                [corner(y, e, y), corner(z, e, z)]
            } else {
                [corner(e, y, y), corner(e, z, z)]
            };
            Piece {
                ends,
                count: 2,
                kind: Kind::Edge,
                forward: s[e] < 0,
            }
        }
    }
}

/// A point where the two pieces' ranges begin or end: an end of A's piece, of B's, or both (then
/// they coincide).
#[derive(Clone, Copy, Debug)]
struct Cand {
    a: Option<End>,
    b: Option<End>,
}

fn one_side(s: [i32; 3]) -> bool {
    s[0] != 0 && s[0] == s[1] && s[1] == s[2]
}

/// Appends what triangles a and b contribute: segments where they cross, points where they touch
/// (a corner on the other's edge or inside it, crossing edges), or a union when they overlap in a
/// shared plane. Only exact predicates on their corners.
fn contacts(input: &Input, a: u32, b: u32, out: &mut Vec<Contact>) {
    let (ta, tb) = (input.tris[a as usize], input.tris[b as usize]);
    let (pa, pb) = (input.corners(a), input.corners(b));
    let shared_a = ta.map(|v| tb.contains(&v));
    let shared_b = tb.map(|v| ta.contains(&v));
    let shared = shared_a.iter().filter(|&&s| s).count();
    if shared == 3 {
        out.push(Contact::Union(a, b));
        return;
    }
    let side = |t: [P3; 3], u: [P3; 3], skip: [bool; 3]| {
        [0, 1, 2].map(|k| {
            if skip[k] {
                0
            } else {
                exact::orient3d(t[0], t[1], t[2], u[k])
            }
        })
    };
    let sb = side(pa, pb, shared_b);
    if one_side(sb) {
        return;
    }
    if sb == [0; 3] {
        coplanar(input, a, b, &pa, &pb, shared_a, shared_b, out);
        return;
    }
    if shared == 2 {
        // Triangles in different planes that share an edge meet only along it.
        return;
    }
    let sa = side(pb, pa, shared_a);
    if one_side(sa) || sa == [0; 3] {
        return;
    }
    let (ka, kb) = (piece(sa), piece(sb));
    // Order along the line d = nA × nB where the planes meet: with A's end on edge u → v and B's on
    // edge x → y (both towards the other plane's positive side), orient3d(a_u, a_v, b_x, b_y) is
    // positive exactly when B's end comes after A's (see tritri::across).
    let after = |x: End, y: End| exact::orient3d(pa[x.u], pa[x.v], pb[y.u], pb[y.v]);
    let (a1, a2) = if ka.forward {
        (ka.ends[0], ka.ends[1])
    } else {
        (ka.ends[1], ka.ends[0])
    };
    // B's own order runs along nB × nA = −d.
    let (b1, b2) = if kb.forward {
        (kb.ends[1], kb.ends[0])
    } else {
        (kb.ends[0], kb.ends[1])
    };
    let start = match after(a1, b1) {
        1 => Cand {
            a: None,
            b: Some(b1),
        },
        -1 => Cand {
            a: Some(a1),
            b: None,
        },
        _ => Cand {
            a: Some(a1),
            b: Some(b1),
        },
    };
    let end = match after(a2, b2) {
        1 => Cand {
            a: Some(a2),
            b: None,
        },
        -1 => Cand {
            a: None,
            b: Some(b2),
        },
        _ => Cand {
            a: Some(a2),
            b: Some(b2),
        },
    };
    let order = if start.a.is_some() && end.a.is_some() {
        if ka.count == 1 {
            Ordering::Equal
        } else {
            Ordering::Less
        }
    } else if start.b.is_some() && end.b.is_some() {
        if kb.count == 1 {
            Ordering::Equal
        } else {
            Ordering::Less
        }
    } else if let (Some(x), Some(y)) = (start.a, end.b) {
        match after(x, y) {
            1 => Ordering::Less,
            0 => Ordering::Equal,
            _ => Ordering::Greater,
        }
    } else if let (Some(y), Some(x)) = (start.b, end.a) {
        match after(x, y) {
            1 => Ordering::Greater,
            0 => Ordering::Equal,
            _ => Ordering::Less,
        }
    } else {
        Ordering::Greater
    };
    let edge_a = |e: End| input.edge_between(a, e.u, e.v);
    let edge_b = |e: End| input.edge_between(b, e.u, e.v);
    // For a piece along an edge: that edge.
    let along = |t: u32, k: &Piece| {
        let (u, v) = (k.ends[0].corner.unwrap_or(0), k.ends[1].corner.unwrap_or(1));
        input.edge_between(t, u, v)
    };
    let key = |c: Cand| -> Key {
        if let Some(k) = c.a.and_then(|e| e.corner) {
            return Key::V(ta[k]);
        }
        if let Some(k) = c.b.and_then(|e| e.corner) {
            return Key::V(tb[k]);
        }
        match (c.a, c.b) {
            (Some(x), Some(y)) => ee(edge_a(x), edge_b(y)),
            (Some(x), None) => match kb.kind {
                Kind::Edge => ee(edge_a(x), along(b, &kb)),
                _ => Key::EF(edge_a(x), b),
            },
            (None, Some(y)) => match ka.kind {
                Kind::Edge => ee(edge_b(y), along(a, &ka)),
                _ => Key::EF(edge_b(y), a),
            },
            (None, None) => Key::V(ta[0]),
        }
    };
    // Where a point lies in one triangle: a corner (nothing to add), on an edge, or inside.
    let place = |c: Cand, key: Key, out: &mut Vec<Contact>| {
        for (end, t, k, edge) in [
            (c.a, a, &ka, &edge_a as &dyn Fn(End) -> u32),
            (c.b, b, &kb, &edge_b as &dyn Fn(End) -> u32),
        ] {
            match end {
                Some(e) if e.corner.is_some() => {}
                Some(e) => out.push(Contact::OnEdge(edge(e), key)),
                None => match k.kind {
                    Kind::Cut => out.push(Contact::Inside(t, key)),
                    Kind::Edge => out.push(Contact::OnEdge(along(t, k), key)),
                    Kind::Corner => {}
                },
            }
        }
    };
    match order {
        Ordering::Greater => {}
        Ordering::Equal => {
            let c = Cand {
                a: start.a.or(end.a),
                b: start.b.or(end.b),
            };
            place(c, key(c), out);
        }
        Ordering::Less => {
            let ends = [key(start), key(end)];
            let planes = Line::Planes(a.min(b), a.max(b));
            if ka.kind == Kind::Cut {
                out.push(Contact::Segment(Seg {
                    tri: a,
                    other: b,
                    line: if kb.kind == Kind::Edge {
                        Line::Edge(along(b, &kb))
                    } else {
                        planes
                    },
                    ends,
                }));
            }
            if kb.kind == Kind::Cut {
                out.push(Contact::Segment(Seg {
                    tri: b,
                    other: a,
                    line: if ka.kind == Kind::Edge {
                        Line::Edge(along(a, &ka))
                    } else {
                        planes
                    },
                    ends,
                }));
            }
            place(start, ends[0], out);
            place(end, ends[1], out);
        }
    }
}

fn ee(e: u32, f: u32) -> Key {
    Key::EE(e.min(f), e.max(f))
}

/// True when p lies on segment st, strictly between its ends (2D, exact).
fn on_segment(s: P2, t: P2, p: P2) -> bool {
    if exact::orient2d(s, t, p) != 0 {
        return false;
    }
    let k = if (t[0] - s[0]).abs() >= (t[1] - s[1]).abs() {
        0
    } else {
        1
    };
    (s[k] < p[k] && p[k] < t[k]) || (t[k] < p[k] && p[k] < s[k])
}

/// True when segments ab and cd cross at a point inside both (2D, exact).
fn proper_cross(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (o1, o2) = (exact::orient2d(a, b, c), exact::orient2d(a, b, d));
    let (o3, o4) = (exact::orient2d(c, d, a), exact::orient2d(c, d, b));
    o1 * o2 < 0 && o3 * o4 < 0
}

/// True when the line through some edge of `t` has all of `u` on the side away from `t` or on it.
fn separated(t: &[P2; 3], u: &[P2; 3]) -> bool {
    let turn = exact::orient2d(t[0], t[1], t[2]);
    (0..3).any(|i| {
        let (p, q) = (t[i], t[(i + 1) % 3]);
        u.iter().all(|&r| exact::orient2d(p, q, r) * turn <= 0)
    })
}

/// Two triangles in one plane: corners of one strictly inside the other's edges, edges crossing,
/// and a union when they overlap with positive area.
#[allow(clippy::too_many_arguments)]
fn coplanar(
    input: &Input,
    a: u32,
    b: u32,
    pa: &[P3; 3],
    pb: &[P3; 3],
    shared_a: [bool; 3],
    shared_b: [bool; 3],
    out: &mut Vec<Contact>,
) {
    let Some(axis) = exact::dominant_axis(pa[0], pa[1], pa[2]) else {
        return;
    };
    let (ta, tb) = (input.tris[a as usize], input.tris[b as usize]);
    let qa = pa.map(|p| exact::project(p, axis));
    let qb = pb.map(|p| exact::project(p, axis));
    for (t, q, tv, shared, other, qo) in [
        (a, &qa, &tb, &shared_b, b, &qb),
        (b, &qb, &ta, &shared_a, a, &qa),
    ] {
        let _ = other;
        for j in 0..3 {
            if shared[j] {
                continue;
            }
            for i in 0..3 {
                if on_segment(q[i], q[(i + 1) % 3], qo[j]) {
                    out.push(Contact::OnEdge(input.side(t, i), Key::V(tv[j])));
                }
            }
        }
    }
    for i in 0..3 {
        for j in 0..3 {
            let (u, v) = (ta[i], ta[(i + 1) % 3]);
            let (w, x) = (tb[j], tb[(j + 1) % 3]);
            if u == w || u == x || v == w || v == x {
                continue;
            }
            if proper_cross(qa[i], qa[(i + 1) % 3], qb[j], qb[(j + 1) % 3]) {
                let (e, f) = (input.side(a, i), input.side(b, j));
                let key = ee(e, f);
                out.push(Contact::OnEdge(e, key));
                out.push(Contact::OnEdge(f, key));
            }
        }
    }
    let shared = shared_a.iter().filter(|&&s| s).count();
    let overlap = if shared == 2 {
        // They overlap exactly when the third corners lie on the same side of the shared edge.
        let i = shared_a.iter().position(|&s| !s).unwrap_or(0);
        let j = shared_b.iter().position(|&s| !s).unwrap_or(0);
        let (s, t) = (qa[(i + 1) % 3], qa[(i + 2) % 3]);
        exact::orient2d(s, t, qa[i]) == exact::orient2d(s, t, qb[j])
    } else {
        !separated(&qa, &qb) && !separated(&qb, &qa)
    };
    if overlap {
        out.push(Contact::Union(a, b));
    }
}

// ---------------------------------------------------------------------------------------------
// The table of points

/// Every point the arrangement uses, by key (sorted), with its geometry and approximate position.
struct Table {
    keys: Vec<Key>,
    geoms: Vec<Geom>,
    approx: Vec<(P3, f64)>,
}

impl Table {
    fn new(input: &Input, mut keys: Vec<Key>) -> Table {
        keys.par_sort_unstable();
        keys.dedup();
        let geoms: Vec<Geom> = keys.par_iter().map(|&k| input.geom(k)).collect();
        let approx = geoms.par_iter().map(|g| input.approx(g)).collect();
        Table {
            keys,
            geoms,
            approx,
        }
    }

    fn get(&self, k: Key) -> Option<usize> {
        self.keys.binary_search(&k).ok()
    }

    fn id(&self, k: Key) -> usize {
        self.get(k)
            .expect("every point of the arrangement is in its table")
    }

    fn pt(&self, id: usize) -> Pt {
        let (p, e) = self.approx[id];
        Pt {
            g: self.geoms[id],
            p,
            e,
        }
    }

    fn key_pt(&self, k: Key) -> Pt {
        self.pt(self.id(k))
    }

    /// Adds points made later (sorted in; ones already known are skipped).
    fn add(&mut self, mut more: Vec<(Key, Geom, (P3, f64))>) {
        more.sort_unstable_by_key(|m| m.0);
        more.dedup_by_key(|m| m.0);
        more.retain(|m| self.get(m.0).is_none());
        if more.is_empty() {
            return;
        }
        let total = self.keys.len() + more.len();
        let mut keys = Vec::with_capacity(total);
        let mut geoms = Vec::with_capacity(total);
        let mut approx = Vec::with_capacity(total);
        let mut fresh = more.into_iter().peekable();
        for i in 0..self.keys.len() {
            while fresh.peek().is_some_and(|m| m.0 < self.keys[i]) {
                let (k, g, a) = fresh.next().unwrap_or_else(|| unreachable!());
                keys.push(k);
                geoms.push(g);
                approx.push(a);
            }
            keys.push(self.keys[i]);
            geoms.push(self.geoms[i]);
            approx.push(self.approx[i]);
        }
        for (k, g, a) in fresh {
            keys.push(k);
            geoms.push(g);
            approx.push(a);
        }
        *self = Table {
            keys,
            geoms,
            approx,
        };
    }
}

/// Union-find over table ids; the smallest id (the smallest key) of a group represents it.
struct Groups {
    parent: Vec<u32>,
}

impl Groups {
    fn new(n: usize) -> Groups {
        Groups {
            parent: (0..n as u32).collect(),
        }
    }

    fn root(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let up = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = up;
            x = up;
        }
        x
    }

    fn join(&mut self, a: u32, b: u32) -> bool {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra == rb {
            return false;
        }
        self.parent[ra.max(rb) as usize] = ra.min(rb);
        true
    }

    /// Every id's representative.
    fn roots(&mut self) -> Vec<u32> {
        (0..self.parent.len() as u32)
            .map(|x| self.root(x))
            .collect()
    }
}

/// The root of `x` in a local alias list (path halving).
fn alias_root(alias: &mut [usize], mut x: usize) -> usize {
    while alias[x] != x {
        let up = alias[alias[x]];
        alias[x] = up;
        x = up;
    }
    x
}

// ---------------------------------------------------------------------------------------------
// Stage 2: points along input edges

/// The points strictly inside each input edge, from its lower vertex to its higher one.
struct Chains {
    start: Vec<u32>,
    keys: Vec<Key>,
}

impl Chains {
    fn of(&self, e: u32) -> &[Key] {
        &self.keys[self.start[e as usize] as usize..self.start[e as usize + 1] as usize]
    }
}

/// The keys on edge e sorted along it; coincident ones keep the first and are reported as merges.
fn edge_chain(input: &Input, table: &Table, e: u32, keys: &[Key]) -> (Vec<Key>, Vec<(Key, Key)>) {
    let [p, q] = input.edge_points(e);
    let k = (0..3)
        .max_by(|&x, &y| (q[x] - p[x]).abs().total_cmp(&(q[y] - p[y]).abs()))
        .unwrap_or(0);
    let up = q[k] > p[k];
    let mut items: Vec<(Key, Pt)> = keys.iter().map(|&key| (key, table.key_pt(key))).collect();
    let dir = |o: Ordering| if up { o } else { o.reverse() };
    items.sort_by(|x, y| dir(cmp_coord(input, &x.1, &y.1, k)).then(x.0.cmp(&y.0)));
    let mut chain: Vec<Key> = Vec::with_capacity(items.len());
    let mut merges = Vec::new();
    for i in 0..items.len() {
        if i > 0 && cmp_coord(input, &items[i - 1].1, &items[i].1, k) == Ordering::Equal {
            merges.push((items[i - 1].0, items[i].0));
            continue;
        }
        chain.push(items[i].0);
    }
    (chain, merges)
}

/// The keys of a member side's chain from corner k to corner k + 1, both corners included.
fn side_chain(input: &Input, chains: &Chains, t: u32, k: usize) -> Vec<Key> {
    let tri = input.tris[t as usize];
    let (from, to) = (tri[k], tri[(k + 1) % 3]);
    let inner = chains.of(input.side(t, k));
    let mut out = Vec::with_capacity(inner.len() + 2);
    out.push(Key::V(from));
    if from < to {
        out.extend_from_slice(inner);
    } else {
        out.extend(inner.iter().rev());
    }
    out.push(Key::V(to));
    out
}

// ---------------------------------------------------------------------------------------------
// Stage 3: points on segments

/// What `unit_points` found in one unit.
#[derive(Default)]
struct Found {
    /// (segment, a point strictly inside it).
    on: Vec<((u32, u32), Key)>,
    merges: Vec<(Key, Key)>,
    /// Crossing points made here, with their geometry and approximate position.
    fresh: Vec<(Key, Geom, (P3, f64))>,
}

/// Below this many items a unit compares all pairs instead of building a tree.
const SMALL: usize = 24;

/// A segment of a unit with its ends as local point indices.
struct LocalSeg {
    seg: Seg,
    ends: [usize; 2],
}

/// The point where two segments of one unit cross, by the lines they lie on.
fn crossing_key(s: &Seg, r: &Seg) -> Key {
    match (s.line, r.line) {
        (Line::Planes(..), Line::Planes(..)) => {
            let mut k = [s.tri.min(r.tri), s.other, r.other];
            k.sort_unstable();
            Key::TPI(k[0], k[1], k[2])
        }
        (Line::Planes(..), Line::Edge(e)) => Key::EF(e, s.other),
        (Line::Edge(e), Line::Planes(..)) => Key::EF(e, r.other),
        (Line::Edge(e), Line::Edge(f)) => ee(e, f),
    }
}

/// A box (z = 0) around a point seen along `axis`, grown by its error bound.
fn flat_box(p: &Pt, axis: usize) -> Aabb {
    let q = flat(p, axis);
    let e = p.e * 2.0 + f64::MIN_POSITIVE;
    [[q[0] - e, q[1] - e, 0.0], [q[0] + e, q[1] + e, 0.0]]
}

/// Pairs of points (index pairs, lower first) that join the `alive` points into groups that
/// coincide exactly; a pair already known to be in one group is not reported. Points are compared
/// only when their error boxes touch (a sweep along one axis would compare every pair within the
/// widest error bound of all), and not when they are already grouped: many crossings through one
/// spot would otherwise take an exact test per pair.
fn coincident(
    input: &Input,
    pts: &[Pt],
    alive: &[bool],
    cancel: &AtomicBool,
) -> Vec<(usize, usize)> {
    let order: Vec<usize> = (0..pts.len()).filter(|&i| alive[i]).collect();
    let reach = |i: usize| pts[i].e * 2.0 + f64::MIN_POSITIVE;
    let boxes: Vec<Aabb> = order
        .iter()
        .map(|&i| {
            let (p, r) = (pts[i].p, reach(i));
            [p.map(|x| x - r), p.map(|x| x + r)]
        })
        .collect();
    let mut group: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::new();
    let mut found = |i: usize, j: usize| {
        let near = (0..3).all(|k| {
            (pts[j].p[k] - pts[i].p[k]).abs() <= (pts[i].e + pts[j].e) * 2.0 + f64::MIN_POSITIVE
        });
        let (gi, gj) = (alias_root(&mut group, i), alias_root(&mut group, j));
        if gi != gj && near && !cancel.load(Atomic::Relaxed) && same(input, &pts[i], &pts[j]) {
            group[gi.max(gj)] = gi.min(gj);
            out.push((i.min(j), i.max(j)));
        }
    };
    if boxes.len() <= SMALL {
        for a in 0..boxes.len() {
            for b in a + 1..boxes.len() {
                if bvh::overlap(&boxes[a], &boxes[b]) {
                    found(order[a], order[b]);
                }
            }
        }
    } else {
        Bvh::new(&boxes).self_pairs(|a, b| found(order[a as usize], order[b as usize]));
    }
    out.sort_unstable();
    out
}

/// True when `p` lies strictly between a and b, the three being on one line (exact).
fn strictly_between(input: &Input, axis: usize, a: &Pt, p: &Pt, b: &Pt) -> bool {
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    let axes = if (a.p[i] - b.p[i]).abs() >= (a.p[j] - b.p[j]).abs() {
        [i, j]
    } else {
        [j, i]
    };
    for k in axes {
        let ab = cmp_coord(input, a, b, k);
        if ab != Ordering::Equal {
            return cmp_coord(input, a, p, k) == ab && cmp_coord(input, p, b, k) == ab;
        }
    }
    false
}

/// In one unit: the points where its segments cross, the points that lie on its segments, and the
/// points that coincide. Once `cancel` is set it stops early with what it has (the caller drops
/// it).
fn unit_points(
    input: &Input,
    table: &Table,
    chains: &Chains,
    members: &[u32],
    segs: &[Seg],
    inside: &[Key],
    cancel: &AtomicBool,
) -> Result<Found, Failure> {
    let axis = plane_axis(input, members[0])?;
    let mut keys: Vec<Key> = Vec::new();
    for &t in members {
        keys.extend(input.tris[t as usize].map(Key::V));
        for k in 0..3 {
            keys.extend_from_slice(chains.of(input.side(t, k)));
        }
    }
    keys.extend_from_slice(inside);
    for s in segs {
        keys.extend(s.ends);
    }
    keys.sort_unstable();
    keys.dedup();
    let mut pts: Vec<Pt> = keys.iter().map(|&k| table.key_pt(k)).collect();
    let mut index: HashMap<Key, usize> = keys.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    let mut found = Found::default();
    let mut alias: Vec<usize> = (0..pts.len()).collect();
    let merge = |alias: &mut Vec<usize>, keys: &[Key], i: usize, j: usize, found: &mut Found| {
        let (ri, rj) = (alias_root(alias, i), alias_root(alias, j));
        if ri != rj {
            let (keep, gone) = if keys[ri] < keys[rj] {
                (ri, rj)
            } else {
                (rj, ri)
            };
            alias[gone] = keep;
            found.merges.push((keys[keep], keys[gone]));
        }
    };
    let alive = vec![true; pts.len()];
    for (i, j) in coincident(input, &pts, &alive, cancel) {
        merge(&mut alias, &keys, i, j, &mut found);
    }
    let local: Vec<LocalSeg> = segs
        .iter()
        .filter_map(|s| {
            let ends = s.ends.map(|k| alias_root(&mut alias, index[&k]));
            (ends[0] != ends[1]).then_some(LocalSeg { seg: *s, ends })
        })
        .collect();
    let mut on: Vec<Vec<usize>> = vec![Vec::new(); local.len()];
    let seg_box = |pts: &[Pt], s: &LocalSeg| {
        let mut b = flat_box(&pts[s.ends[0]], axis);
        bvh::grow(&mut b, &flat_box(&pts[s.ends[1]], axis));
        b
    };
    // Crossings, among segments whose boxes touch (all pairs when there are few).
    let boxes: Vec<Aabb> = local.iter().map(|s| seg_box(&pts, s)).collect();
    let mut candidates = Vec::new();
    if boxes.len() <= SMALL {
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                if bvh::overlap(&boxes[i], &boxes[j]) {
                    candidates.push((i, j));
                }
            }
        }
    } else {
        Bvh::new(&boxes).self_pairs(|i, j| candidates.push((i as usize, j as usize)));
        candidates.sort_unstable();
    }
    for (n, (i, j)) in candidates.into_iter().enumerate() {
        if n.is_multiple_of(CHECK_EVERY) && cancel.load(Atomic::Relaxed) {
            return Ok(found);
        }
        let (s, r) = (&local[i], &local[j]);
        if s.ends.iter().any(|e| r.ends.contains(e)) {
            continue;
        }
        let [a, b] = s.ends;
        let [c, d] = r.ends;
        let o = |x: usize, y: usize, z: usize| orient2d(input, axis, &pts[x], &pts[y], &pts[z]);
        let (o1, o2) = (o(a, b, c), o(a, b, d));
        if o1 == 0 || o2 == 0 || o1 == o2 {
            continue;
        }
        let (o3, o4) = (o(c, d, a), o(c, d, b));
        if o3 == 0 || o4 == 0 || o3 == o4 {
            continue;
        }
        let key = crossing_key(&s.seg, &r.seg);
        let id = match index.get(&key) {
            Some(&id) => id,
            None => {
                let geom = input.geom(key);
                let approx = match table.get(key) {
                    Some(t) => table.approx[t],
                    None => {
                        let approx = input.approx(&geom);
                        found.fresh.push((key, geom, approx));
                        approx
                    }
                };
                pts.push(Pt {
                    g: geom,
                    p: approx.0,
                    e: approx.1,
                });
                keys.push(key);
                alias.push(pts.len() - 1);
                index.insert(key, pts.len() - 1);
                pts.len() - 1
            }
        };
        on[i].push(id);
        on[j].push(id);
    }
    // Crossings may coincide with other points (several segments through one point).
    if pts.len() > alive.len() {
        let alive: Vec<bool> = (0..pts.len())
            .map(|i| alias_root(&mut alias, i) == i)
            .collect();
        for (i, j) in coincident(input, &pts, &alive, cancel) {
            merge(&mut alias, &keys, i, j, &mut found);
        }
    }
    // Points lying on segments.
    let roots: Vec<usize> = (0..pts.len()).map(|i| alias_root(&mut alias, i)).collect();
    let point_boxes: Vec<Aabb> = pts.iter().map(|p| flat_box(p, axis)).collect();
    let tree = (point_boxes.len() > SMALL).then(|| Bvh::new(&point_boxes));
    for (s, seg) in local.iter().enumerate() {
        if s.is_multiple_of(CHECK_EVERY) && cancel.load(Atomic::Relaxed) {
            return Ok(found);
        }
        let [a, b] = seg.ends.map(|e| roots[e]);
        let mut known: Vec<usize> = on[s].iter().map(|&i| roots[i]).collect();
        let mut hits = Vec::new();
        let around = seg_box(&pts, seg);
        match &tree {
            Some(tree) => tree.query(&around, |i| hits.push(i as usize)),
            None => hits
                .extend((0..point_boxes.len()).filter(|&i| bvh::overlap(&point_boxes[i], &around))),
        }
        hits.sort_unstable();
        for i in hits {
            if roots[i] != i || i == a || i == b || known.contains(&i) {
                continue;
            }
            if orient2d(input, axis, &pts[a], &pts[b], &pts[i]) == 0
                && strictly_between(input, axis, &pts[a], &pts[i], &pts[b])
            {
                known.push(i);
            }
        }
        for i in known {
            if i != a && i != b {
                found.on.push((seg.seg.pair(), keys[i]));
            }
        }
    }
    Ok(found)
}

/// The exact direction of a line.
fn line_direction(input: &Input, line: Line) -> E3 {
    match line {
        Line::Planes(s, t) => {
            let [a, b, c] = input.corners(s);
            let [d, e, f] = input.corners(t);
            cross_e(&normal_e(a, b, c), &normal_e(d, e, f))
        }
        Line::Edge(e) => {
            let [p, q] = input.edge_points(e);
            sub_e(q, p)
        }
    }
}

/// A segment's points in order from its first end to its second, ends included, after merging
/// (`rep` gives each table id's representative); coincident points are reported as merges.
fn segment_chain(
    input: &Input,
    table: &Table,
    rep: &[u32],
    seg: &Seg,
    inner: &[Key],
) -> Result<(Vec<Key>, Merges), Failure> {
    let r = |k: Key| table.keys[rep[table.id(k)] as usize];
    let ends = seg.ends.map(r);
    let mut keys: Vec<Key> = inner
        .iter()
        .map(|&k| r(k))
        .filter(|k| !ends.contains(k))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let d = line_direction(input, seg.line);
    let k = (0..3)
        .max_by(|&x, &y| {
            exp::estimate(&d[x])
                .abs()
                .total_cmp(&exp::estimate(&d[y]).abs())
        })
        .unwrap_or(0);
    let pe = ends.map(|e| table.key_pt(e));
    let up = match cmp_coord(input, &pe[0], &pe[1], k) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => return Err("the ends of a crossing segment coincide"),
    };
    let dir = |o: Ordering| if up { o } else { o.reverse() };
    let mut items: Vec<(Key, Pt)> = keys.iter().map(|&key| (key, table.key_pt(key))).collect();
    items.sort_by(|x, y| dir(cmp_coord(input, &x.1, &y.1, k)).then(x.0.cmp(&y.0)));
    let mut chain = vec![ends[0]];
    let mut merges = Vec::new();
    let (mut last, mut last_key) = (pe[0], ends[0]);
    for (key, pt) in items {
        match dir(cmp_coord(input, &last, &pt, k)) {
            Ordering::Less => {}
            Ordering::Equal => {
                merges.push((last_key, key));
                continue;
            }
            Ordering::Greater => return Err("a point found on a segment lies outside it"),
        }
        match dir(cmp_coord(input, &pt, &pe[1], k)) {
            Ordering::Less => {}
            Ordering::Equal => {
                merges.push((ends[1], key));
                continue;
            }
            Ordering::Greater => return Err("a point found on a segment lies outside it"),
        }
        chain.push(key);
        (last, last_key) = (pt, key);
    }
    chain.push(ends[1]);
    Ok((chain, merges))
}

// ---------------------------------------------------------------------------------------------
// Stage 4: triangulating a unit

/// Where a point lies in a triangulation.
enum Loc {
    Face(u32),
    Edge(u32, usize),
    Vertex(u32),
}

/// A triangulation of one unit's plane, seen along `axis`, over the unit's points plus the three
/// corners of an enclosing triangle (the last three points). Built incrementally with exact
/// orientation tests only: a point splits the triangle or the edge it lands in, a constraint is
/// recovered by Sloan's edge flips. Not Delaunay; `improve` flips towards it afterwards for
/// better-shaped pieces. Triangles turn counter-clockwise; neighbour i lies across the edge from
/// corner i to corner i + 1.
struct Cdt<'a> {
    input: &'a Input,
    axis: usize,
    pts: &'a [Pt],
    /// Points below this are real; the others are the enclosing triangle's corners.
    real: u32,
    tri: Vec<[u32; 3]>,
    nbr: Vec<[u32; 3]>,
    /// Bit i: edge i is a constraint.
    fixed: Vec<u8>,
    /// A triangle at each vertex.
    at: Vec<u32>,
    rng: u32,
    /// Set by the caller to stop long constraint recoveries early (the result is then dropped).
    cancel: Option<&'a AtomicBool>,
}

impl<'a> Cdt<'a> {
    fn new(input: &'a Input, axis: usize, pts: &'a [Pt]) -> Cdt<'a> {
        let n = pts.len() as u32 - 3;
        let mut at = vec![NONE; pts.len()];
        for v in n..n + 3 {
            at[v as usize] = 0;
        }
        Cdt {
            input,
            axis,
            pts,
            real: n,
            tri: vec![[n, n + 1, n + 2]],
            nbr: vec![[NONE; 3]],
            fixed: vec![0],
            at,
            rng: 0x9E37_79B9,
            cancel: None,
        }
    }

    /// The triangle `corners` (counter-clockwise) alone, its sides split at the points of `sides`
    /// (side k in order from corners[k] to corners[k + 1]): no enclosing triangle, and no predicate,
    /// as the points lie on the sides by construction. The sides are constraints.
    fn triangle(
        input: &'a Input,
        axis: usize,
        pts: &'a [Pt],
        corners: [u32; 3],
        sides: &[Vec<u32>; 3],
    ) -> Cdt<'a> {
        let mut at = vec![NONE; pts.len()];
        for &c in &corners {
            at[c as usize] = 0;
        }
        let mut cdt = Cdt {
            input,
            axis,
            pts,
            real: pts.len() as u32,
            tri: vec![corners],
            nbr: vec![[NONE; 3]],
            fixed: vec![0b111],
            at,
            rng: 0x9E37_79B9,
            cancel: None,
        };
        for k in 0..3 {
            let (mut from, to) = (corners[k], corners[(k + 1) % 3]);
            for &p in &sides[k] {
                if let Some((t, i)) = cdt.find_edge(from, to) {
                    cdt.split_edge(t, i, p);
                    from = p;
                }
            }
        }
        cdt
    }

    fn orient(&self, a: u32, b: u32, c: u32) -> i32 {
        orient2d(
            self.input,
            self.axis,
            &self.pts[a as usize],
            &self.pts[b as usize],
            &self.pts[c as usize],
        )
    }

    fn random(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    fn bit(&self, t: u32, i: usize) -> u8 {
        (self.fixed[t as usize] >> i) & 1
    }

    /// In triangle n (if any), the link to `old` becomes `new`.
    fn relink(&mut self, n: u32, old: u32, new: u32) {
        if n == NONE {
            return;
        }
        for link in self.nbr[n as usize].iter_mut() {
            if *link == old {
                *link = new;
                return;
            }
        }
    }

    /// The index of the edge from a to b in triangle t.
    fn edge_index(&self, t: u32, a: u32, b: u32) -> Option<usize> {
        let v = self.tri[t as usize];
        (0..3).find(|&i| v[i] == a && v[(i + 1) % 3] == b)
    }

    /// The triangle with the directed edge a → b and its index, walking round a.
    fn find_edge(&self, a: u32, b: u32) -> Option<(u32, usize)> {
        let start = self.at[a as usize];
        let mut t = start;
        for _ in 0..self.tri.len() + 1 {
            let v = self.tri[t as usize];
            let i = (0..3).find(|&i| v[i] == a)?;
            if v[(i + 1) % 3] == b {
                return Some((t, i));
            }
            // The next triangle counter-clockwise round a lies across the edge into a.
            t = self.nbr[t as usize][(i + 2) % 3];
            if t == NONE || t == start {
                break;
            }
        }
        // Round the other way, for vertices on the hull.
        let mut t = start;
        for _ in 0..self.tri.len() + 1 {
            let v = self.tri[t as usize];
            let i = (0..3).find(|&i| v[i] == a)?;
            if v[(i + 1) % 3] == b {
                return Some((t, i));
            }
            t = self.nbr[t as usize][i];
            if t == NONE || t == start {
                return None;
            }
        }
        None
    }

    fn locate(&mut self, p: u32, start: u32) -> Loc {
        let mut t = start;
        let limit = 4 * self.tri.len() + 64;
        for _ in 0..limit {
            let v = self.tri[t as usize];
            let r = (self.random() % 3) as usize;
            let mut zero = [false; 3];
            let mut across = None;
            for d in 0..3 {
                let i = (r + d) % 3;
                let o = self.orient(v[i], v[(i + 1) % 3], p);
                if o < 0 {
                    across = Some(i);
                    break;
                }
                zero[i] = o == 0;
            }
            match across {
                Some(i) => {
                    let n = self.nbr[t as usize][i];
                    if n == NONE {
                        return self.scan(p);
                    }
                    t = n;
                }
                None => return Self::classify(t, v, zero),
            }
        }
        self.scan(p)
    }

    fn classify(t: u32, v: [u32; 3], zero: [bool; 3]) -> Loc {
        match zero.iter().filter(|&&z| z).count() {
            0 => Loc::Face(t),
            1 => Loc::Edge(t, zero.iter().position(|&z| z).unwrap_or(0)),
            _ => {
                let k = (0..3).find(|&k| zero[k] && zero[(k + 2) % 3]).unwrap_or(0);
                Loc::Vertex(v[k])
            }
        }
    }

    /// `locate` by testing every triangle (when a walk does not settle).
    fn scan(&self, p: u32) -> Loc {
        for t in 0..self.tri.len() as u32 {
            let v = self.tri[t as usize];
            let o = [0, 1, 2].map(|i| self.orient(v[i], v[(i + 1) % 3], p));
            if o.iter().all(|&x| x >= 0) {
                return Self::classify(t, v, o.map(|x| x == 0));
            }
        }
        Loc::Face(0)
    }

    /// Inserts point p; returns the vertex that stands for it (an existing one when p coincides
    /// with it). The edges round p are flipped towards Delaunay, so the triangles stay fat and the
    /// walks to the next points short.
    fn insert(&mut self, p: u32, hint: &mut u32) -> u32 {
        match self.locate(p, *hint) {
            Loc::Vertex(v) => v,
            Loc::Face(t) => {
                let [a, b, c] = self.tri[t as usize];
                self.split_face(t, p);
                self.legalize(vec![(a, b), (b, c), (c, a)]);
                *hint = self.at[p as usize];
                p
            }
            Loc::Edge(t, i) => {
                let v = self.tri[t as usize];
                let (a, b, c) = (v[i], v[(i + 1) % 3], v[(i + 2) % 3]);
                let u = self.nbr[t as usize][i];
                let mut stack = vec![(b, c), (c, a)];
                if u != NONE
                    && let Some(j) = self.edge_index(u, b, a)
                {
                    let d = self.tri[u as usize][(j + 2) % 3];
                    stack.extend([(a, d), (d, b)]);
                }
                self.split_edge(t, i, p);
                self.legalize(stack);
                *hint = self.at[p as usize];
                p
            }
        }
    }

    fn split_face(&mut self, t: u32, p: u32) {
        let [a, b, c] = self.tri[t as usize];
        let [_, nb, nc] = self.nbr[t as usize];
        let f = self.fixed[t as usize];
        let t1 = self.tri.len() as u32;
        let t2 = t1 + 1;
        self.tri[t as usize] = [a, b, p];
        self.nbr[t as usize][1] = t1;
        self.nbr[t as usize][2] = t2;
        self.fixed[t as usize] = f & 1;
        self.tri.push([b, c, p]);
        self.nbr.push([nb, t2, t]);
        self.fixed.push((f >> 1) & 1);
        self.tri.push([c, a, p]);
        self.nbr.push([nc, t, t1]);
        self.fixed.push((f >> 2) & 1);
        self.relink(nb, t, t1);
        self.relink(nc, t, t2);
        self.at[c as usize] = t1;
        self.at[p as usize] = t;
        self.at[a as usize] = t;
        self.at[b as usize] = t;
    }

    fn split_edge(&mut self, t: u32, i: usize, p: u32) {
        let v = self.tri[t as usize];
        let (a, b, c) = (v[i], v[(i + 1) % 3], v[(i + 2) % 3]);
        let u = self.nbr[t as usize][i];
        let nb = self.nbr[t as usize][(i + 1) % 3];
        let nc = self.nbr[t as usize][(i + 2) % 3];
        let (fab, fbc, fca) = (
            self.bit(t, i),
            self.bit(t, (i + 1) % 3),
            self.bit(t, (i + 2) % 3),
        );
        let t2 = self.tri.len() as u32;
        self.tri[t as usize] = [a, p, c];
        self.nbr[t as usize] = [NONE, t2, nc];
        self.fixed[t as usize] = fab | (fca << 2);
        self.tri.push([p, b, c]);
        self.nbr.push([NONE, nb, t]);
        self.fixed.push(fab | (fbc << 1));
        self.relink(nb, t, t2);
        self.at[a as usize] = t;
        self.at[c as usize] = t;
        self.at[p as usize] = t;
        self.at[b as usize] = t2;
        if u != NONE {
            let j = self.edge_index(u, b, a).unwrap_or(0);
            let d = self.tri[u as usize][(j + 2) % 3];
            let ua = self.nbr[u as usize][(j + 1) % 3];
            let ud = self.nbr[u as usize][(j + 2) % 3];
            let (fad, fdb) = (self.bit(u, (j + 1) % 3), self.bit(u, (j + 2) % 3));
            let u2 = self.tri.len() as u32;
            self.tri[u as usize] = [b, p, d];
            self.nbr[u as usize] = [t2, u2, ud];
            self.fixed[u as usize] = fab | (fdb << 2);
            self.tri.push([p, a, d]);
            self.nbr.push([t, ua, u]);
            self.fixed.push(fab | (fad << 1));
            self.relink(ua, u, u2);
            self.nbr[t as usize][0] = u2;
            self.nbr[t2 as usize][0] = u;
            self.at[d as usize] = u;
        }
    }

    /// Flips the edge i of triangle t (its neighbour across it must exist).
    fn flip(&mut self, t: u32, i: usize) {
        let v = self.tri[t as usize];
        let (u, w, w1) = (v[i], v[(i + 1) % 3], v[(i + 2) % 3]);
        let n = self.nbr[t as usize][i];
        let j = self.edge_index(n, w, u).unwrap_or(0);
        let w2 = self.tri[n as usize][(j + 2) % 3];
        let t_vw1 = self.nbr[t as usize][(i + 1) % 3];
        let t_w1u = self.nbr[t as usize][(i + 2) % 3];
        let n_uw2 = self.nbr[n as usize][(j + 1) % 3];
        let n_w2v = self.nbr[n as usize][(j + 2) % 3];
        let (f_vw1, f_w1u) = (self.bit(t, (i + 1) % 3), self.bit(t, (i + 2) % 3));
        let (f_uw2, f_w2v) = (self.bit(n, (j + 1) % 3), self.bit(n, (j + 2) % 3));
        self.tri[t as usize] = [u, w2, w1];
        self.nbr[t as usize] = [n_uw2, n, t_w1u];
        self.fixed[t as usize] = f_uw2 | (f_w1u << 2);
        self.tri[n as usize] = [w, w1, w2];
        self.nbr[n as usize] = [t_vw1, t, n_w2v];
        self.fixed[n as usize] = f_vw1 | (f_w2v << 2);
        self.relink(n_uw2, n, t);
        self.relink(t_vw1, t, n);
        self.at[u as usize] = t;
        self.at[w1 as usize] = t;
        self.at[w as usize] = n;
        self.at[w2 as usize] = n;
    }

    /// Marks the edge a – b (it exists) as a constraint on both sides.
    fn fix(&mut self, a: u32, b: u32) -> Result<(), Failure> {
        let (t, i) = self
            .find_edge(a, b)
            .ok_or("a constraint did not become an edge")?;
        self.fixed[t as usize] |= 1 << i;
        let n = self.nbr[t as usize][i];
        if n != NONE
            && let Some(j) = self.edge_index(n, b, a)
        {
            self.fixed[n as usize] |= 1 << j;
        }
        Ok(())
    }

    /// True when segments ab and cd cross at a point inside both.
    fn crosses(&self, a: u32, b: u32, c: u32, d: u32) -> bool {
        if a == c || a == d || b == c || b == d {
            return false;
        }
        self.orient(a, b, c) * self.orient(a, b, d) < 0
            && self.orient(c, d, a) * self.orient(c, d, b) < 0
    }

    /// The edges that segment ab crosses, walking from a; Err when a vertex lies on the segment or
    /// it crosses a constraint (neither happens when the chains are complete).
    fn crossed(&self, a: u32, b: u32) -> Result<Vec<(u32, u32)>, Failure> {
        // The triangle at a whose corner contains the direction to b: b lies left of a → x and
        // right of a → y, so x lies right of the segment and y left of it.
        // Round a counter-clockwise from its triangle and, for a vertex on the hull (a unit of one
        // triangle has no enclosing triangle), clockwise from there too.
        let start = self.at[a as usize];
        let mut exit = None;
        'round: for ccw in [true, false] {
            let mut t = start;
            for _ in 0..self.tri.len() + 1 {
                let v = self.tri[t as usize];
                let i = (0..3)
                    .find(|&i| v[i] == a)
                    .ok_or("a vertex lost its triangle")?;
                let (x, y) = (v[(i + 1) % 3], v[(i + 2) % 3]);
                let (ox, oy) = (self.orient(a, x, b), self.orient(a, y, b));
                if ox > 0 && oy < 0 {
                    exit = Some((t, (i + 1) % 3, x, y));
                    break 'round;
                }
                for (o, w) in [(ox, x), (oy, y)] {
                    if o == 0
                        && strictly_between(
                            self.input,
                            self.axis,
                            &self.pts[a as usize],
                            &self.pts[w as usize],
                            &self.pts[b as usize],
                        )
                    {
                        return Err("a point lies on a constraint");
                    }
                }
                t = self.nbr[t as usize][if ccw { (i + 2) % 3 } else { i }];
                if t == start {
                    break 'round;
                }
                if t == NONE {
                    break;
                }
            }
        }
        // The segment leaves each triangle through edge x → y, x to its right and y to its left.
        let (mut t, mut i, mut x, mut y) = exit.ok_or("a constraint leaves its unit")?;
        let mut out = Vec::new();
        loop {
            if (self.fixed[t as usize] >> i) & 1 == 1 {
                return Err("two constraints cross");
            }
            out.push((x, y));
            let n = self.nbr[t as usize][i];
            if n == NONE {
                return Err("a constraint leaves its unit");
            }
            // n runs y → x → z.
            let j = self.edge_index(n, y, x).ok_or("broken neighbours")?;
            let z = self.tri[n as usize][(j + 2) % 3];
            if z == b {
                return Ok(out);
            }
            match self.orient(a, b, z) {
                0 => return Err("a point lies on a constraint"),
                o if o > 0 => {
                    y = z;
                    i = (j + 1) % 3;
                }
                _ => {
                    x = z;
                    i = (j + 2) % 3;
                }
            }
            t = n;
            if out.len() > self.tri.len() {
                return Err("a constraint walk did not end");
            }
        }
    }

    /// Makes a – b an edge and a constraint, flipping the edges it crosses (Sloan).
    fn constrain(&mut self, a: u32, b: u32) -> Result<(), Failure> {
        if a == b {
            return Ok(());
        }
        if self.find_edge(a, b).is_some() || self.find_edge(b, a).is_some() {
            return self.fix(a, b).or_else(|_| self.fix(b, a));
        }
        let mut queue: VecDeque<(u32, u32)> = self.crossed(a, b)?.into();
        let mut budget = 64 + 8 * queue.len() * queue.len();
        while let Some((u, v)) = queue.pop_front() {
            if budget == 0 {
                return Err("a constraint could not be recovered");
            }
            budget -= 1;
            if budget.is_multiple_of(CHECK_EVERY)
                && self.cancel.is_some_and(|c| c.load(Atomic::Relaxed))
            {
                return Err("cancelled");
            }
            let Some((t, i)) = self.find_edge(u, v) else {
                continue;
            };
            let n = self.nbr[t as usize][i];
            if n == NONE {
                return Err("a constraint leaves its unit");
            }
            let w1 = self.tri[t as usize][(i + 2) % 3];
            let j = self.edge_index(n, v, u).ok_or("broken neighbours")?;
            let w2 = self.tri[n as usize][(j + 2) % 3];
            if self.orient(w1, w2, u) * self.orient(w1, w2, v) < 0 {
                self.flip(t, i);
                if self.crosses(a, b, w1, w2) {
                    queue.push_back((w1, w2));
                }
            } else {
                queue.push_back((u, v));
            }
        }
        self.fix(a, b)
    }

    /// Flips unconstrained edges between real triangles towards Delaunay (judged in floating
    /// point: only the shapes depend on it), each flip checked to keep the quad convex exactly.
    fn improve(&mut self) {
        let real = self.real;
        let mut stack: Vec<(u32, u32)> = Vec::new();
        for t in 0..self.tri.len() {
            for i in 0..3 {
                let (a, b) = (self.tri[t][i], self.tri[t][(i + 1) % 3]);
                if a < b && a < real && b < real && self.bit(t as u32, i) == 0 {
                    stack.push((a, b));
                }
            }
        }
        self.legalize(stack);
    }

    /// `improve` for the edges on `stack` and those its flips expose.
    fn legalize(&mut self, mut stack: Vec<(u32, u32)>) {
        let real = self.real;
        let mut budget = 4 * self.tri.len() + 64;
        while let Some((a, b)) = stack.pop() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let Some((t, i)) = self.find_edge(a, b) else {
                continue;
            };
            let n = self.nbr[t as usize][i];
            if n == NONE || self.bit(t, i) == 1 {
                continue;
            }
            let w1 = self.tri[t as usize][(i + 2) % 3];
            let Some(j) = self.edge_index(n, b, a) else {
                continue;
            };
            let w2 = self.tri[n as usize][(j + 2) % 3];
            if a.max(b).max(w1).max(w2) >= real {
                continue;
            }
            let f = |v: u32| flat(&self.pts[v as usize], self.axis);
            if !inside_circle(f(a), f(b), f(w1), f(w2)) {
                continue;
            }
            if self.orient(w1, w2, a) * self.orient(w1, w2, b) >= 0 {
                continue;
            }
            self.flip(t, i);
            stack.extend([(a, w2), (w2, b), (b, w1), (w1, a)]);
        }
    }

    /// Per triangle, the members covering it (sorted), flooding out from the enclosing triangle
    /// (covered by none): crossing a member's edge enters or leaves that member.
    fn coverage(&self, on: &HashMap<(u32, u32), Vec<u32>>) -> Result<Vec<Vec<u32>>, Failure> {
        let n = self.pts.len() as u32 - 3;
        let mut sets: Vec<Option<Vec<u32>>> = vec![None; self.tri.len()];
        let first = self.at[n as usize];
        sets[first as usize] = Some(Vec::new());
        let mut queue = vec![first];
        while let Some(t) = queue.pop() {
            let here = sets[t as usize].clone().unwrap_or_default();
            for i in 0..3 {
                let m = self.nbr[t as usize][i];
                if m == NONE {
                    continue;
                }
                let (a, b) = (self.tri[t as usize][i], self.tri[t as usize][(i + 1) % 3]);
                let mut there = here.clone();
                if let Some(members) = on.get(&(a.min(b), a.max(b))) {
                    for &x in members {
                        match there.binary_search(&x) {
                            Ok(k) => {
                                there.remove(k);
                            }
                            Err(k) => there.insert(k, x),
                        }
                    }
                }
                match &sets[m as usize] {
                    Some(s) if *s != there => return Err("the coverage of a unit does not add up"),
                    Some(_) => {}
                    None => {
                        sets[m as usize] = Some(there);
                        queue.push(m);
                    }
                }
            }
        }
        Ok(sets.into_iter().map(|s| s.unwrap_or_default()).collect())
    }
}

/// The points `ids` in the order of a Hilbert curve through their projections, so consecutive
/// points lie close together and each insertion's walk starts near its point.
fn insertion_order(pts: &[Pt], axis: usize, ids: Vec<u32>) -> Vec<u32> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for &i in &ids {
        let q = flat(&pts[i as usize], axis);
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if size.is_nan() || size <= 0.0 {
        return ids;
    }
    let scale = 65535.0 / size;
    let mut keyed: Vec<(u64, u32)> = ids
        .into_iter()
        .map(|i| {
            let q = flat(&pts[i as usize], axis);
            let x = ((q[0] - lo[0]) * scale).clamp(0.0, 65535.0) as u32;
            let y = ((q[1] - lo[1]) * scale).clamp(0.0, 65535.0) as u32;
            (hilbert(x, y), i)
        })
        .collect();
    keyed.sort_unstable();
    keyed.into_iter().map(|(_, i)| i).collect()
}

/// The distance along a Hilbert curve over the 2^16 × 2^16 grid of the cell (x, y).
fn hilbert(mut x: u32, mut y: u32) -> u64 {
    const N: u32 = 1 << 16;
    let mut d = 0u64;
    let mut s = N / 2;
    while s > 0 {
        let rx = u32::from(x & s > 0);
        let ry = u32::from(y & s > 0);
        d += u64::from(s) * u64::from(s) * u64::from((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = N - 1 - x;
                y = N - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    d
}

/// True when d lies clearly inside the circle through a, b, c (counter-clockwise).
fn inside_circle(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (adx, ady) = (a[0] - d[0], a[1] - d[1]);
    let (bdx, bdy) = (b[0] - d[0], b[1] - d[1]);
    let (cdx, cdy) = (c[0] - d[0], c[1] - d[1]);
    let (al, bl, cl) = (
        adx * adx + ady * ady,
        bdx * bdx + bdy * bdy,
        cdx * cdx + cdy * cdy,
    );
    let det =
        al * (bdx * cdy - bdy * cdx) + bl * (cdx * ady - cdy * adx) + cl * (adx * bdy - ady * bdx);
    det > 1e-10 * (al + bl + cl) * (al + bl + cl)
}

/// One face of a unit: its corners (keys), how many layers it stands for, and its source triangle.
type UnitFace = ([Key; 3], u32, u32);

/// Points found to coincide, to be merged.
type Merges = Vec<(Key, Key)>;

/// `triangulate_unit` for a unit of one triangle t: the triangle's outline (its corners and the
/// points of its side chains, `sides`) is the region, split at those points without predicates;
/// then the inside points and the segment chains go in. Every face is t's, facing its way.
#[allow(clippy::too_many_arguments)]
fn triangulate_triangle(
    input: &Input,
    axis: usize,
    t: u32,
    keys: &[Key],
    pts: &[Pt],
    sides: &[Vec<Key>],
    seg_chains: &HashMap<(u32, u32), Vec<Key>>,
    pairs: &[(u32, u32)],
    cancel: &AtomicBool,
) -> Result<(Vec<UnitFace>, Merges), Failure> {
    let id = |k: Key| keys.binary_search(&k).unwrap_or(0) as u32;
    let corner = [0, 1, 2].map(|k| id(sides[k][0]));
    let chain: [Vec<u32>; 3] = [0, 1, 2].map(|k| {
        sides[k][1..sides[k].len() - 1]
            .iter()
            .map(|&x| id(x))
            .collect()
    });
    let c = input.corners(t).map(|p| exact::project(p, axis));
    let facing = exact::orient2d(c[0], c[1], c[2]);
    // Counter-clockwise in the projection: corners 0, 2, 1 with the chains run backwards when the
    // triangle faces away from the axis.
    let (ccw, outline) = if facing > 0 {
        (corner, chain)
    } else {
        let back = |v: &Vec<u32>| v.iter().rev().copied().collect::<Vec<u32>>();
        (
            [corner[0], corner[2], corner[1]],
            [back(&chain[2]), back(&chain[1]), back(&chain[0])],
        )
    };
    let mut cdt = Cdt::triangle(input, axis, pts, ccw, &outline);
    cdt.cancel = Some(cancel);
    // The outline split makes a fan of slivers; flipped towards Delaunay first, the walks to the
    // inside points stay short.
    cdt.improve();
    let mut on_outline = vec![false; keys.len()];
    for &v in ccw.iter().chain(outline.iter().flatten()) {
        on_outline[v as usize] = true;
    }
    let inner: Vec<u32> = (0..keys.len() as u32)
        .filter(|&v| !on_outline[v as usize])
        .collect();
    let order = insertion_order(pts, axis, inner);
    let mut vertex: Vec<u32> = (0..keys.len() as u32).collect();
    let mut merges = Vec::new();
    let mut hint = 0;
    for (n, &i) in order.iter().enumerate() {
        if n.is_multiple_of(CHECK_EVERY) && cancel.load(Atomic::Relaxed) {
            return Ok(Default::default());
        }
        let v = cdt.insert(i, &mut hint);
        vertex[i as usize] = v;
        if v != i {
            merges.push((keys[v as usize], keys[i as usize]));
        }
    }
    for (n, p) in pairs.iter().enumerate() {
        if n.is_multiple_of(CHECK_PAIRS) && cancel.load(Atomic::Relaxed) {
            return Ok(Default::default());
        }
        if let Some(chain) = seg_chains.get(p) {
            for w in chain.windows(2) {
                let (a, b) = (vertex[id(w[0]) as usize], vertex[id(w[1]) as usize]);
                if a != b {
                    cdt.constrain(a, b)?;
                }
            }
        }
    }
    cdt.improve();
    let faces = cdt
        .tri
        .iter()
        .map(|v| {
            let v = if facing > 0 { *v } else { [v[0], v[2], v[1]] };
            (v.map(|x| keys[x as usize]), 1, t)
        })
        .collect();
    Ok((faces, merges))
}

/// A unit's constrained triangulation and the faces with nonzero coverage, oriented the way the
/// coverage faces.
#[allow(clippy::too_many_arguments)]
fn triangulate_unit(
    input: &Input,
    table: &Table,
    rep: &[u32],
    chains: &Chains,
    seg_chains: &HashMap<(u32, u32), Vec<Key>>,
    members: &[u32],
    pairs: &[(u32, u32)],
    inside: &[Key],
    cancel: &AtomicBool,
) -> Result<(Vec<UnitFace>, Merges), Failure> {
    let r = |k: Key| table.keys[rep[table.id(k)] as usize];
    let axis = plane_axis(input, members[0])?;
    let sides: Vec<Vec<Key>> = members
        .iter()
        .flat_map(|&t| (0..3).map(move |k| (t, k)))
        .map(|(t, k)| side_chain(input, chains, t, k).into_iter().map(r).collect())
        .collect();
    let mut keys: Vec<Key> = sides.iter().flatten().copied().collect();
    for p in pairs {
        if let Some(chain) = seg_chains.get(p) {
            keys.extend(chain.iter().copied());
        }
    }
    keys.extend(inside.iter().map(|&k| r(k)));
    keys.sort_unstable();
    keys.dedup();
    let mut pts: Vec<Pt> = keys.iter().map(|&k| table.key_pt(k)).collect();
    if members.len() == 1 {
        return triangulate_triangle(
            input, axis, members[0], &keys, &pts, &sides, seg_chains, pairs, cancel,
        );
    }
    // The enclosing triangle, far around every point.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &pts {
        let q = flat(p, axis);
        for k in 0..2 {
            lo[k] = lo[k].min(q[k] - p.e);
            hi[k] = hi[k].max(q[k] + p.e);
        }
    }
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-30);
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    for corner in [
        [c[0] - 30.0 * size, c[1] - 30.0 * size],
        [c[0] + 30.0 * size, c[1] - 30.0 * size],
        [c[0], c[1] + 30.0 * size],
    ] {
        pts.push(Pt {
            g: Geom::Flat(corner),
            p: [corner[0], corner[1], 0.0],
            e: 0.0,
        });
    }
    let mut cdt = Cdt::new(input, axis, &pts);
    cdt.cancel = Some(cancel);
    let order = insertion_order(&pts, axis, (0..keys.len() as u32).collect());
    let mut vertex = vec![NONE; keys.len()];
    let mut merges = Vec::new();
    let mut hint = 0;
    for (n, i) in order.into_iter().enumerate() {
        if n.is_multiple_of(CHECK_EVERY) && cancel.load(Atomic::Relaxed) {
            return Ok(Default::default());
        }
        let v = cdt.insert(i, &mut hint);
        vertex[i as usize] = v;
        if v != i {
            merges.push((keys[v as usize], keys[i as usize]));
        }
    }
    let id = |k: Key| vertex[keys.binary_search(&k).unwrap_or(0)];
    let mut on: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (m, chain) in sides.chunks(3).enumerate() {
        if cancel.load(Atomic::Relaxed) {
            return Ok(Default::default());
        }
        for side in chain {
            for w in side.windows(2) {
                let (a, b) = (id(w[0]), id(w[1]));
                if a == b {
                    continue;
                }
                cdt.constrain(a, b)?;
                on.entry((a.min(b), a.max(b))).or_default().push(m as u32);
            }
        }
    }
    for (n, p) in pairs.iter().enumerate() {
        if n.is_multiple_of(CHECK_PAIRS) && cancel.load(Atomic::Relaxed) {
            return Ok(Default::default());
        }
        if let Some(chain) = seg_chains.get(p) {
            for w in chain.windows(2) {
                let (a, b) = (id(w[0]), id(w[1]));
                if a != b {
                    cdt.constrain(a, b)?;
                }
            }
        }
    }
    cdt.improve();
    let sets = cdt.coverage(&on)?;
    let facing: Vec<i32> = members
        .iter()
        .map(|&t| {
            let c = input.corners(t).map(|p| exact::project(p, axis));
            exact::orient2d(c[0], c[1], c[2])
        })
        .collect();
    let real = keys.len() as u32;
    let mut faces = Vec::new();
    for (t, set) in sets.iter().enumerate() {
        let v = cdt.tri[t];
        if v.iter().any(|&x| x >= real) || set.is_empty() {
            continue;
        }
        let c: i32 = set.iter().map(|&m| facing[m as usize]).sum();
        if c == 0 {
            continue;
        }
        let corners = if c > 0 { v } else { [v[0], v[2], v[1]] };
        let source = set
            .iter()
            .find(|&&m| facing[m as usize] == c.signum())
            .map_or(members[0], |&m| members[m as usize]);
        faces.push((corners.map(|x| keys[x as usize]), c.unsigned_abs(), source));
    }
    Ok((faces, merges))
}

/// A triangulation of the polygon `points[..boundary]` (in order, counter-clockwise, simple) with
/// the remaining points inside it, all plain 2D points, decided exactly on their coordinates: the
/// faces as indices into `points`, counter-clockwise. A boundary point a hair off the straight line
/// between its neighbours is fine (the outline is a constraint, the faces outside it are dropped).
/// Points that coincide with an earlier one are left out. None when the outline cannot be recovered.
pub fn triangulate_flat(points: &[P2], boundary: usize) -> Option<Vec<[usize; 3]>> {
    let input = Input::new(&Mesh::default());
    let input = &input;
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in points {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-30);
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let mut pts: Vec<Pt> = points
        .iter()
        .map(|&p| Pt {
            g: Geom::Flat(p),
            p: [p[0], p[1], 0.0],
            e: 0.0,
        })
        .collect();
    for corner in [
        [c[0] - 30.0 * size, c[1] - 30.0 * size],
        [c[0] + 30.0 * size, c[1] - 30.0 * size],
        [c[0], c[1] + 30.0 * size],
    ] {
        pts.push(Pt {
            g: Geom::Flat(corner),
            p: [corner[0], corner[1], 0.0],
            e: 0.0,
        });
    }
    let mut cdt = Cdt::new(input, 2, &pts);
    let mut vertex = vec![NONE; points.len()];
    let mut hint = 0;
    for (i, v) in vertex.iter_mut().enumerate() {
        *v = cdt.insert(i as u32, &mut hint);
    }
    let mut on: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for k in 0..boundary {
        let (a, b) = (vertex[k], vertex[(k + 1) % boundary]);
        if a == b {
            continue;
        }
        cdt.constrain(a, b).ok()?;
        on.entry((a.min(b), a.max(b))).or_default().push(0);
    }
    cdt.improve();
    let sets = cdt.coverage(&on).ok()?;
    let real = points.len() as u32;
    Some(
        cdt.tri
            .iter()
            .zip(&sets)
            .filter(|(v, set)| v.iter().all(|&x| x < real) && !set.is_empty())
            .map(|(v, _)| v.map(|x| x as usize))
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------------
// Stage 5: the whole arrangement

/// Counts for logs and tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    /// Pairs of triangles that cross or touch.
    pub pairs: usize,
    /// Crossing segments (per triangle).
    pub segments: usize,
    /// Units retriangulated, and how many of them are clusters of several triangles.
    pub units: usize,
    pub clusters: usize,
    /// Points made by intersections, and points merged because they coincided.
    pub points: usize,
    pub merges: usize,
    /// How long the stages took: the contacts, the points along edges and segments, the
    /// triangulations, the assembly.
    pub time_contacts: Duration,
    pub time_points: Duration,
    pub time_triangulation: Duration,
    pub time_assembly: Duration,
}

/// The arrangement of a set of triangles.
pub struct Arrangement {
    pub input: Input,
    /// Point ids below this are the input vertices (same index).
    pub vertex_count: usize,
    geoms: Vec<Geom>,
    approx: Vec<(P3, f64)>,
    /// The faces; each faces the way its source triangle does.
    pub faces: Vec<[u32; 3]>,
    /// Per face: how many coincident layers facing its way it stands for.
    pub coverage: Vec<u32>,
    /// Per face: the input triangle it came from.
    pub source: Vec<u32>,
    /// Input triangles that were retriangulated.
    pub split: usize,
    pub counts: Counts,
}

impl Arrangement {
    pub fn point_count(&self) -> usize {
        self.geoms.len()
    }

    /// Point `id` ready for predicates.
    pub fn pt(&self, id: u32) -> Pt {
        let (p, e) = self.approx[id as usize];
        Pt {
            g: self.geoms[id as usize],
            p,
            e,
        }
    }

    /// The point's position rounded to f32 (input vertices keep theirs exactly).
    pub fn position(&self, id: u32) -> [f32; 3] {
        self.approx[id as usize].0.map(|x| x as f32)
    }

    /// A point strictly inside face f, exactly on its source triangle's plane, for casting rays
    /// from: where a short line through the face's centroid along the plane's normal crosses the
    /// plane. None when the face is too thin for that point to land inside it.
    pub fn probe(&self, f: usize) -> Option<Pt> {
        let t = self.source[f];
        let corners = self.input.corners(t);
        let axis = exact::dominant_axis(corners[0], corners[1], corners[2])?;
        let tri = self.faces[f].map(|v| self.pt(v));
        let centroid = [0, 1, 2].map(|k| (tri[0].p[k] + tri[1].p[k] + tri[2].p[k]) / 3.0);
        let n = exact::normal(corners[0], corners[1], corners[2]);
        let n = exact::scale(n, 1.0 / exact::length(n));
        let size = (0..3)
            .map(|k| exact::length(exact::sub(tri[k].p, tri[(k + 1) % 3].p)))
            .fold(0.0f64, f64::max);
        let turn = orient2d(&self.input, axis, &tri[0], &tri[1], &tri[2]);
        if turn == 0 || size.is_nan() || size <= 0.0 {
            return None;
        }
        for grow in [1.0, 64.0, 4096.0] {
            let h = size * grow;
            let (s, e) = (
                exact::add(centroid, exact::scale(n, h)),
                exact::sub(centroid, exact::scale(n, h)),
            );
            let (ss, se) = (
                exact::orient3d(corners[0], corners[1], corners[2], s),
                exact::orient3d(corners[0], corners[1], corners[2], e),
            );
            if ss * se >= 0 {
                continue;
            }
            let probe = self.input.pt(Geom::Probe([s, e], t));
            let inside = (0..3)
                .all(|k| orient2d(&self.input, axis, &tri[k], &tri[(k + 1) % 3], &probe) == turn);
            return inside.then_some(probe);
        }
        None
    }
}

/// Iterations of a long loop inside one unit between two looks at the cancel flag.
const CHECK_EVERY: usize = 1024;
/// Crossing segments a unit's triangulation takes in between two looks at the cancel flag (each
/// may take several constraints, and a constraint through many slivers takes long).
const CHECK_PAIRS: usize = 16;

/// Parts a long parallel loop is split into, so it can stop and report progress between them.
const CHUNKS: usize = 64;

/// `f` for every index below `n`, on every core, in order: in `CHUNKS` parts, with a cancel check
/// and a progress report after each part. Long single items should check the cancel flag
/// themselves.
pub(super) fn par_chunks<T: Send>(
    n: usize,
    control: &mut Control,
    f: impl Fn(usize) -> T + Sync,
) -> RepairResult<Vec<T>> {
    let size = n.div_ceil(CHUNKS).max(1);
    let mut out = Vec::with_capacity(n);
    for start in (0..n).step_by(size) {
        control.check()?;
        let end = (start + size).min(n);
        out.par_extend((start..end).into_par_iter().map(&f));
        control.progress(end as f64 / n as f64);
    }
    control.check()?;
    Ok(out)
}

/// The arrangement of `mesh`'s triangles, which must be welded (one vertex per position), each
/// with area, and no two over the same corners facing the same way. Ok(Err(why)) when it could
/// not be built.
pub fn build(mesh: &Mesh, control: &mut Control) -> RepairResult<Result<Arrangement, Failure>> {
    let clock = Instant::now();
    let input = Input::new(mesh);
    let count = input.tris.len();
    let cancel = control.cancel_flag();
    control.check()?;
    // 1. Contacts.
    let pairs = Bvh::new(&bvh::triangle_boxes(mesh)).self_pairs_parallel(|a, b| {
        if cancel.load(Atomic::Relaxed) {
            return false;
        }
        let mut found = Vec::new();
        contacts(&input, a, b, &mut found);
        !found.is_empty()
    });
    control.check()?;
    control.progress(0.1);
    let found: Vec<Contact> = par_chunks(pairs.len(), &mut control.sub(0.1, 0.15), |i| {
        let (a, b) = pairs[i];
        let mut found = Vec::new();
        contacts(&input, a, b, &mut found);
        found
    })?
    .into_iter()
    .flatten()
    .collect();
    let mut counts = Counts {
        pairs: pairs.len(),
        time_contacts: clock.elapsed(),
        ..Counts::default()
    };
    let clock = Instant::now();
    drop(pairs);
    let mut cluster: Vec<u32> = (0..count as u32).collect();
    fn root(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            let up = parent[parent[x as usize] as usize];
            parent[x as usize] = up;
            x = up;
        }
        x
    }
    let mut on_edge: Vec<(u32, Key)> = Vec::new();
    let mut inside: Vec<(u32, Key)> = Vec::new();
    let mut segs: Vec<Seg> = Vec::new();
    for c in found {
        match c {
            Contact::Union(a, b) => {
                let (ra, rb) = (root(&mut cluster, a), root(&mut cluster, b));
                cluster[ra.max(rb) as usize] = ra.min(rb);
            }
            Contact::OnEdge(e, k) => on_edge.push((e, k)),
            Contact::Inside(t, k) => inside.push((t, k)),
            Contact::Segment(s) => segs.push(s),
        }
    }
    counts.segments = segs.len();
    // Which triangles change, grouped into units (a cluster's members, or a triangle alone).
    let mut touched = vec![false; count];
    let mut size = vec![0u32; count];
    for t in 0..count as u32 {
        size[root(&mut cluster, t) as usize] += 1;
    }
    for t in 0..count as u32 {
        touched[t as usize] = size[root(&mut cluster, t) as usize] > 1;
    }
    for &(t, _) in &inside {
        touched[t as usize] = true;
    }
    for s in &segs {
        touched[s.tri as usize] = true;
    }
    for &(e, _) in &on_edge {
        for &side in input.edges.sides(e) {
            touched[side as usize / 3] = true;
        }
    }
    let mut members: Vec<(u32, u32)> = (0..count as u32)
        .filter(|&t| touched[t as usize])
        .map(|t| (root(&mut cluster, t), t))
        .collect();
    members.sort_unstable();
    let units: Vec<Vec<u32>> = members
        .chunk_by(|x, y| x.0 == y.0)
        .map(|g| g.iter().map(|x| x.1).collect())
        .collect();
    counts.units = units.len();
    counts.clusters = units.iter().filter(|u| u.len() > 1).count();
    let split = members.len();
    drop(members);
    // The table of points.
    let mut keys: Vec<Key> = Vec::new();
    for unit in &units {
        for &t in unit {
            keys.extend(input.tris[t as usize].map(Key::V));
        }
    }
    keys.extend(on_edge.iter().map(|x| x.1));
    keys.extend(inside.iter().map(|x| x.1));
    for s in &segs {
        keys.extend(s.ends);
    }
    let mut table = Table::new(&input, keys);
    control.check()?;
    control.progress(0.25);
    let mut merges: Vec<(Key, Key)> = Vec::new();
    // 2. Points along edges.
    on_edge.par_sort_unstable();
    on_edge.dedup();
    let groups: Vec<&[(u32, Key)]> = on_edge.chunk_by(|x, y| x.0 == y.0).collect();
    let sorted: Vec<(u32, Vec<Key>, Merges)> =
        par_chunks(groups.len(), &mut control.sub(0.25, 0.28), |i| {
            let g = groups[i];
            let keys: Vec<Key> = g.iter().map(|x| x.1).collect();
            let (chain, merges) = edge_chain(&input, &table, g[0].0, &keys);
            (g[0].0, chain, merges)
        })?;
    let mut start = vec![0u32; input.edges.len() + 1];
    for (e, chain, _) in &sorted {
        start[*e as usize + 1] = chain.len() as u32;
    }
    for e in 0..input.edges.len() {
        start[e + 1] += start[e];
    }
    let mut chain_keys = Vec::with_capacity(start[input.edges.len()] as usize);
    for (_, chain, more) in sorted {
        chain_keys.extend(chain);
        merges.extend(more);
    }
    let chains = Chains {
        start,
        keys: chain_keys,
    };
    // Segments and inside points per triangle.
    segs.sort_by_key(|s| (s.tri, s.pair()));
    inside.sort_unstable();
    inside.dedup();
    let seg_range = |t: u32| {
        let a = segs.partition_point(|s| s.tri < t);
        let b = segs.partition_point(|s| s.tri <= t);
        a..b
    };
    let inside_range = |t: u32| {
        let a = inside.partition_point(|x| x.0 < t);
        let b = inside.partition_point(|x| x.0 <= t);
        a..b
    };
    let unit_segs: Vec<Vec<Seg>> = units
        .iter()
        .map(|u| {
            u.iter()
                .flat_map(|&t| segs[seg_range(t)].iter().copied())
                .collect()
        })
        .collect();
    let unit_inside: Vec<Vec<Key>> = units
        .iter()
        .map(|u| {
            u.iter()
                .flat_map(|&t| inside[inside_range(t)].iter().map(|x| x.1))
                .collect()
        })
        .collect();
    control.check()?;
    control.progress(0.3);
    // 3. Points on segments.
    let found: Vec<Result<Found, Failure>> =
        par_chunks(units.len(), &mut control.sub(0.3, 0.45), |u| {
            unit_points(
                &input,
                &table,
                &chains,
                &units[u],
                &unit_segs[u],
                &unit_inside[u],
                cancel,
            )
        })?;
    let mut on_seg: Vec<((u32, u32), Key)> = Vec::new();
    let mut fresh = Vec::new();
    for f in found {
        match f {
            Ok(f) => {
                on_seg.extend(f.on);
                merges.extend(f.merges);
                fresh.extend(f.fresh);
            }
            Err(why) => return Ok(Err(why)),
        }
    }
    counts.points = table
        .keys
        .iter()
        .filter(|k| !matches!(k, Key::V(_)))
        .count();
    table.add(fresh);
    counts.points = table
        .keys
        .iter()
        .filter(|k| !matches!(k, Key::V(_)))
        .count();
    let mut groups_of = Groups::new(table.keys.len());
    let join = |groups_of: &mut Groups, merges: &[(Key, Key)], counts: &mut Counts| {
        for &(a, b) in merges {
            if groups_of.join(table.id(a) as u32, table.id(b) as u32) {
                counts.merges += 1;
            }
        }
    };
    join(&mut groups_of, &merges, &mut counts);
    merges.clear();
    control.progress(0.45);
    // Per segment, the points of both its triangles, sorted along it.
    let rep = groups_of.roots();
    let mut seg_of: Vec<Seg> = segs.clone();
    seg_of.sort_by_key(|s| s.pair());
    seg_of.dedup_by_key(|s| s.pair());
    on_seg.par_sort_unstable();
    on_seg.dedup();
    let by_pair: Vec<((u32, u32), Vec<Key>)> = seg_of
        .iter()
        .map(|s| {
            let p = s.pair();
            let a = on_seg.partition_point(|x| x.0 < p);
            let b = on_seg.partition_point(|x| x.0 <= p);
            (p, on_seg[a..b].iter().map(|x| x.1).collect())
        })
        .collect();
    let chained: Vec<Result<(Vec<Key>, Merges), Failure>> =
        par_chunks(seg_of.len(), &mut control.sub(0.45, 0.55), |i| {
            segment_chain(&input, &table, &rep, &seg_of[i], &by_pair[i].1)
        })?;
    let mut seg_chains: HashMap<(u32, u32), Vec<Key>> = HashMap::with_capacity(seg_of.len());
    for (s, c) in seg_of.iter().zip(chained) {
        match c {
            Ok((chain, more)) => {
                seg_chains.insert(s.pair(), chain);
                merges.extend(more);
            }
            Err(why) => return Ok(Err(why)),
        }
    }
    join(&mut groups_of, &merges, &mut counts);
    merges.clear();
    let rep = groups_of.roots();
    // Segment chains in representatives.
    for chain in seg_chains.values_mut() {
        for k in chain.iter_mut() {
            *k = table.keys[rep[table.id(*k)] as usize];
        }
        chain.dedup();
    }
    control.progress(0.55);
    counts.time_points = clock.elapsed();
    let clock = Instant::now();
    // 4. Triangulate the units.
    let faced: Vec<Result<(Vec<UnitFace>, Merges), Failure>> =
        par_chunks(units.len(), &mut control.sub(0.55, 0.9), |u| {
            let mut pairs: Vec<(u32, u32)> = unit_segs[u].iter().map(Seg::pair).collect();
            pairs.sort_unstable();
            pairs.dedup();
            triangulate_unit(
                &input,
                &table,
                &rep,
                &chains,
                &seg_chains,
                &units[u],
                &pairs,
                &unit_inside[u],
                cancel,
            )
        })?;
    let mut unit_faces: Vec<UnitFace> = Vec::new();
    for f in faced {
        match f {
            Ok((faces, more)) => {
                unit_faces.extend(faces);
                merges.extend(more);
            }
            Err(why) => return Ok(Err(why)),
        }
    }
    join(&mut groups_of, &merges, &mut counts);
    let rep = groups_of.roots();
    control.progress(0.9);
    counts.time_triangulation = clock.elapsed();
    let clock = Instant::now();
    // 5. Assemble: input vertices keep their index, other representatives follow in key order.
    let vertex_count = input.points.len();
    let mut id_of = vec![NONE; table.keys.len()];
    let mut geoms: Vec<Geom> = (0..vertex_count as u32).map(Geom::V).collect();
    let mut approx: Vec<(P3, f64)> = input.points.iter().map(|&p| (p, 0.0)).collect();
    for i in 0..table.keys.len() {
        if rep[i] as usize != i {
            continue;
        }
        id_of[i] = match table.keys[i] {
            Key::V(v) => v,
            _ => {
                geoms.push(table.geoms[i]);
                approx.push(table.approx[i]);
                (geoms.len() - 1) as u32
            }
        };
    }
    let point = |k: Key| id_of[rep[table.id(k)] as usize];
    let mut all: Vec<(u32, [u32; 3], u32)> = Vec::with_capacity(count + unit_faces.len());
    for (t, tri) in input.tris.iter().enumerate() {
        if !touched[t] {
            all.push((t as u32, *tri, 1));
        }
    }
    for (corners, coverage, source) in unit_faces {
        let tri = corners.map(point);
        if tri[0] != tri[1] && tri[1] != tri[2] && tri[2] != tri[0] {
            all.push((source, tri, coverage));
        }
    }
    all.par_sort_by_key(|f| f.0);
    control.progress(1.0);
    counts.time_assembly = clock.elapsed();
    Ok(Ok(Arrangement {
        input,
        vertex_count,
        geoms,
        approx,
        faces: all.iter().map(|f| f.1).collect(),
        coverage: all.iter().map(|f| f.2).collect(),
        source: all.iter().map(|f| f.0).collect(),
        split,
        counts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::repair::exact::testing::{Rng, to_integers};
    use crate::repair::silent;
    use crate::repair::topology::side_ends;
    use crate::repair::tritri;
    use num_bigint::BigInt;
    use num_rational::BigRational;
    use num_traits::{Signed, Zero};

    fn mesh(points: &[[f32; 3]], tris: &[[u32; 3]]) -> Mesh {
        Mesh {
            positions: points.as_flattened().to_vec(),
            indices: tris.as_flattened().to_vec(),
        }
    }

    // --- Exact arithmetic -------------------------------------------------------------------

    /// The exact value of an expansion as a rational.
    fn value(e: &[f64]) -> BigRational {
        e.iter()
            .fold(BigRational::zero(), |acc, &x| acc + rational(x))
    }

    fn rational(x: f64) -> BigRational {
        BigRational::from_float(x).expect("finite")
    }

    #[test]
    fn expansions_add_and_multiply_exactly() {
        let mut rng = Rng(1);
        for round in 0..2000 {
            // Values of very different sizes, so components overlap little and much.
            let mut pick = || {
                let x = (rng.unit() - 0.5) * 2f64.powi(rng.int(-60, 60) as i32);
                if round % 7 == 0 { x.round() } else { x }
            };
            let (a, b, c, d) = (pick(), pick(), pick(), pick());
            let e = exp::add(&exp::diff(a, b), &exp::scale(&exp::diff(c, d), a));
            assert_eq!(
                value(&e),
                rational(a) - rational(b) + (rational(c) - rational(d)) * rational(a)
            );
            let f = exp::mul(&e, &exp::diff(d, b));
            assert_eq!(value(&f), value(&e) * (rational(d) - rational(b)));
            let g = exp::compress(&f);
            assert_eq!(value(&g), value(&f));
            let s = exp::sign(&f);
            let exact = value(&f);
            assert_eq!(
                s,
                if exact.is_zero() {
                    0
                } else if exact.is_positive() {
                    1
                } else {
                    -1
                }
            );
            // Exactly zero.
            let z = exp::sub(&f, &f);
            assert_eq!(exp::sign(&z), 0);
        }
    }

    // --- Implicit points against rational references ----------------------------------------

    type Q3 = [BigRational; 3];

    fn q3(p: P3) -> Q3 {
        p.map(rational)
    }

    fn qsub(a: &Q3, b: &Q3) -> Q3 {
        [&a[0] - &b[0], &a[1] - &b[1], &a[2] - &b[2]]
    }

    fn qcross(a: &Q3, b: &Q3) -> Q3 {
        [
            &a[1] * &b[2] - &a[2] * &b[1],
            &a[2] * &b[0] - &a[0] * &b[2],
            &a[0] * &b[1] - &a[1] * &b[0],
        ]
    }

    fn qdot(a: &Q3, b: &Q3) -> BigRational {
        &a[0] * &b[0] + &a[1] * &b[1] + &a[2] * &b[2]
    }

    /// The exact position of a point.
    fn exact_point(input: &Input, g: &Geom) -> Q3 {
        match *g {
            Geom::V(v) => q3(input.points[v as usize]),
            Geom::Flat(p) => q3([p[0], p[1], 0.0]),
            Geom::Lpi(e, plane) => lpi_q(input.edge_points(e), input.plane_points(plane)),
            Geom::Probe(line, t) => lpi_q(line, input.corners(t)),
            Geom::Tpi(ts) => {
                let planes = ts.map(|t| input.corners(t));
                let n: Vec<Q3> = planes
                    .iter()
                    .map(|[a, b, c]| qcross(&qsub(&q3(*b), &q3(*a)), &qsub(&q3(*c), &q3(*a))))
                    .collect();
                let d: Vec<BigRational> = (0..3).map(|i| qdot(&n[i], &q3(planes[i][0]))).collect();
                let c23 = qcross(&n[1], &n[2]);
                let c31 = qcross(&n[2], &n[0]);
                let c12 = qcross(&n[0], &n[1]);
                let w = qdot(&n[0], &c23);
                [0, 1, 2].map(|k| (&d[0] * &c23[k] + &d[1] * &c31[k] + &d[2] * &c12[k]) / &w)
            }
        }
    }

    fn lpi_q([p, q]: [P3; 2], [a, b, c]: [P3; 3]) -> Q3 {
        let (p, q, a) = (q3(p), q3(q), q3(a));
        let n = qcross(&qsub(&q3(b), &a), &qsub(&q3(c), &a));
        let dp = qdot(&n, &qsub(&p, &a));
        let dq = qdot(&n, &qsub(&q, &a));
        let t = &dp / (&dp - &dq);
        [0, 1, 2].map(|k| &p[k] + &t * (&q[k] - &p[k]))
    }

    fn sign_q(x: &BigRational) -> i32 {
        if x.is_zero() {
            0
        } else if x.is_positive() {
            1
        } else {
            -1
        }
    }

    /// Small integer grids force coplanar, collinear and coincident cases constantly.
    fn grid_input(rng: &mut Rng, vertices: usize, triangles: usize, grid: i64) -> Input {
        let points: Vec<[f32; 3]> = (0..vertices)
            .map(|_| [0, 1, 2].map(|_| rng.int(-grid, grid) as f32))
            .collect();
        let mut tris = Vec::new();
        while tris.len() < triangles {
            let t = [0, 1, 2].map(|_| rng.below(vertices as u64) as u32);
            let p = t.map(|v| points[v as usize].map(f64::from));
            if !exact::collinear(p[0], p[1], p[2]) {
                tris.push(t);
            }
        }
        Input::new(&mesh(&points, &tris))
    }

    #[test]
    fn implicit_points_and_predicates_match_rationals() {
        let mut rng = Rng(5);
        let mut decided_exactly = 0;
        for _ in 0..300 {
            let input = grid_input(&mut rng, 12, 8, 3);
            // Implicit points from edges crossing planes and triples of planes.
            let mut pts: Vec<Pt> = Vec::new();
            for e in 0..input.edges.len() as u32 {
                for t in 0..input.tris.len() as u32 {
                    let [p, q] = input.edge_points(e);
                    let [a, b, c] = input.corners(t);
                    if exact::orient3d(a, b, c, p) * exact::orient3d(a, b, c, q) < 0 {
                        pts.push(input.pt(Geom::Lpi(e, Plane::Tri(t))));
                    }
                }
            }
            let n = input.tris.len() as u32;
            for a in 0..n {
                for b in a + 1..n {
                    for c in b + 1..n {
                        let nv: Vec<P3> = [a, b, c]
                            .iter()
                            .map(|&t| {
                                let [p, q, r] = input.corners(t);
                                exact::normal(p, q, r)
                            })
                            .collect();
                        if exact::dot(nv[0], exact::cross(nv[1], nv[2])) != 0.0 {
                            pts.push(input.pt(Geom::Tpi([a, b, c])));
                        }
                    }
                }
            }
            for v in 0..input.points.len() as u32 {
                pts.push(input.pt(Geom::V(v)));
            }
            if pts.len() < 3 {
                continue;
            }
            let exact: Vec<Q3> = pts.iter().map(|p| exact_point(&input, &p.g)).collect();
            // Approximate positions are within their bounds.
            for (p, q) in pts.iter().zip(&exact) {
                for (&x, qx) in p.p.iter().zip(q) {
                    let d = (rational(x) - qx).abs();
                    assert!(d <= rational(p.e * 1.000001), "{:?}: {} > {}", p.g, d, p.e);
                }
            }
            for _ in 0..200 {
                let [i, j, l] = [0, 1, 2].map(|_| rng.below(pts.len() as u64) as usize);
                let axis = rng.below(3) as usize;
                let (x, y) = ((axis + 1) % 3, (axis + 2) % 3);
                let (a, b, c) = (&exact[i], &exact[j], &exact[l]);
                let det = (&b[x] - &a[x]) * (&c[y] - &a[y]) - (&b[y] - &a[y]) * (&c[x] - &a[x]);
                let o = orient2d(&input, axis, &pts[i], &pts[j], &pts[l]);
                assert_eq!(o, sign_q(&det));
                decided_exactly += (o == 0) as usize;
                let k = rng.below(3) as usize;
                assert_eq!(cmp_coord(&input, &pts[i], &pts[j], k), a[k].cmp(&b[k]));
                let t = rng.below(input.tris.len() as u64) as u32;
                let [pa, pb, pc] = input.corners(t);
                let n = qcross(&qsub(&q3(pb), &q3(pa)), &qsub(&q3(pc), &q3(pa)));
                assert_eq!(
                    orient3d_plane(&input, [pa, pb, pc], &pts[i]),
                    sign_q(&qdot(&n, &qsub(a, &q3(pa))))
                );
                let d = line_direction(
                    &input,
                    Line::Edge(rng.below(input.edges.len() as u64) as u32),
                );
                let dq: Q3 = [0, 1, 2].map(|k| value(&d[k]));
                assert_eq!(
                    dir_sign(&input, &d, &pts[i], &pts[j]),
                    sign_q(&qdot(&dq, &qsub(b, a)))
                );
            }
        }
        // The grid makes many cases exactly degenerate, which only the exact stage can decide.
        assert!(decided_exactly > 500, "{decided_exactly}");
        let _ = to_integers;
        let _ = BigInt::zero();
    }

    /// Whenever the construction says three points are collinear, they are, exactly: their
    /// orientation is zero seen along every axis.
    #[test]
    fn collinear_by_construction_is_never_wrong() {
        let mut rng = Rng(11);
        let mut claimed = 0;
        for _ in 0..120 {
            let input = grid_input(&mut rng, 12, 8, 3);
            let mut pts: Vec<Pt> = Vec::new();
            for e in 0..input.edges.len() as u32 {
                for t in 0..input.tris.len() as u32 {
                    let [p, q] = input.edge_points(e);
                    let [a, b, c] = input.corners(t);
                    if exact::orient3d(a, b, c, p) * exact::orient3d(a, b, c, q) < 0 {
                        pts.push(input.pt(Geom::Lpi(e, Plane::Tri(t))));
                    }
                }
            }
            let n = input.tris.len() as u32;
            for a in 0..n {
                for b in a + 1..n {
                    for c in b + 1..n {
                        let nv: Vec<P3> = [a, b, c]
                            .iter()
                            .map(|&t| {
                                let [p, q, r] = input.corners(t);
                                exact::normal(p, q, r)
                            })
                            .collect();
                        if exact::dot(nv[0], exact::cross(nv[1], nv[2])) != 0.0 {
                            pts.push(input.pt(Geom::Tpi([a, b, c])));
                        }
                    }
                }
            }
            for v in 0..input.points.len() as u32 {
                pts.push(input.pt(Geom::V(v)));
            }
            if pts.len() < 3 || pts.len() > 60 {
                continue;
            }
            let exact: Vec<Q3> = pts.iter().map(|p| exact_point(&input, &p.g)).collect();
            for i in 0..pts.len() {
                for j in i + 1..pts.len() {
                    for l in j + 1..pts.len() {
                        if !input.collinear_by_construction([&pts[i], &pts[j], &pts[l]]) {
                            continue;
                        }
                        claimed += 1;
                        let (a, b, c) = (&exact[i], &exact[j], &exact[l]);
                        for axis in 0..3 {
                            let (x, y) = ((axis + 1) % 3, (axis + 2) % 3);
                            let det = (&b[x] - &a[x]) * (&c[y] - &a[y])
                                - (&b[y] - &a[y]) * (&c[x] - &a[x]);
                            assert_eq!(
                                sign_q(&det),
                                0,
                                "{:?} {:?} {:?}",
                                pts[i].g,
                                pts[j].g,
                                pts[l].g
                            );
                        }
                    }
                }
            }
        }
        assert!(claimed > 300, "{claimed}");
    }

    // --- Pair classification ---------------------------------------------------------------

    fn contacts_of(input: &Input, a: u32, b: u32) -> Vec<Contact> {
        let mut out = Vec::new();
        contacts(input, a, b, &mut out);
        out
    }

    /// The order formula in `piece`: with the triangle in its own plane, the second end comes after
    /// the first along (own normal) × (other normal) exactly when `forward` says so.
    #[test]
    fn piece_order_follows_the_line_direction() {
        let mut rng = Rng(9);
        let mut checked = 0;
        for _ in 0..20000 {
            let p: Vec<P3> = (0..6)
                .map(|_| [0, 1, 2].map(|_| rng.int(-4, 4) as f64 + rng.unit() * 0.5))
                .collect();
            let (a, b) = ([p[0], p[1], p[2]], [p[3], p[4], p[5]]);
            if exact::collinear(a[0], a[1], a[2]) || exact::collinear(b[0], b[1], b[2]) {
                continue;
            }
            let s = [0, 1, 2].map(|k| exact::orient3d(b[0], b[1], b[2], a[k]));
            if one_side(s) || s == [0; 3] {
                continue;
            }
            let k = piece(s);
            if k.count < 2 {
                continue;
            }
            // The ends in floating point.
            let nb = exact::normal(b[0], b[1], b[2]);
            let at = |e: End| {
                let (u, v) = (a[e.u], a[e.v]);
                let (du, dv) = (
                    exact::dot(nb, exact::sub(u, b[0])),
                    exact::dot(nb, exact::sub(v, b[0])),
                );
                exact::add(u, exact::scale(exact::sub(v, u), du / (du - dv)))
            };
            let d = exact::cross(exact::normal(a[0], a[1], a[2]), nb);
            let along = exact::dot(d, exact::sub(at(k.ends[1]), at(k.ends[0])));
            if along.abs() < 1e-6 {
                continue;
            }
            assert_eq!(along > 0.0, k.forward, "{s:?} {a:?}");
            checked += 1;
        }
        assert!(checked > 5000, "{checked}");
    }

    /// tritri::intersect says two triangles intersect exactly when the classification finds a
    /// segment, an overlap or a point inside one of them; touching contacts only add points on
    /// edges. Checked on a small integer grid, where coplanar and collinear cases are everywhere.
    #[test]
    fn contacts_agree_with_the_exact_intersection_test() {
        let mut rng = Rng(11);
        let mut kinds = [0usize; 4];
        for _ in 0..4000 {
            let input = grid_input(&mut rng, 6, 2, 2);
            let (a, b) = (0u32, 1u32);
            if input.tris[0]
                .iter()
                .filter(|v| input.tris[1].contains(v))
                .count()
                == 3
            {
                continue;
            }
            let found = contacts_of(&input, a, b);
            let intersecting = found.iter().any(|c| {
                matches!(
                    c,
                    Contact::Segment(_) | Contact::Union(..) | Contact::Inside(..)
                )
            });
            let expected = tritri::intersect(&input.corners(a), &input.corners(b));
            assert_eq!(
                intersecting,
                expected,
                "{:?} {:?}: {found:?}",
                input.corners(a),
                input.corners(b)
            );
            for c in &found {
                kinds[match c {
                    Contact::Union(..) => 0,
                    Contact::OnEdge(..) => 1,
                    Contact::Inside(..) => 2,
                    Contact::Segment(_) => 3,
                }] += 1;
            }
            // Every point named lies where the contact says, exactly.
            for c in &found {
                match *c {
                    Contact::OnEdge(e, k) => {
                        let p = input.pt(input.geom(k));
                        let [s, t] = input.edges.ends(e).map(|v| input.pt(Geom::V(v)));
                        let q = exact_point(&input, &p.g);
                        let (qs, qt) = (exact_point(&input, &s.g), exact_point(&input, &t.g));
                        // On the line, strictly between the ends.
                        let cross = qcross(&qsub(&q, &qs), &qsub(&qt, &qs));
                        assert!(cross.iter().all(|x| x.is_zero()), "{k:?} off edge {e}");
                        let along = qdot(&qsub(&q, &qs), &qsub(&qt, &qs));
                        assert!(
                            along.is_positive() && along < qdot(&qsub(&qt, &qs), &qsub(&qt, &qs))
                        );
                    }
                    Contact::Inside(t, k) => {
                        let q = exact_point(&input, &input.geom(k));
                        let [pa, pb, pc] = input.corners(t).map(q3);
                        let n = qcross(&qsub(&pb, &pa), &qsub(&pc, &pa));
                        assert!(qdot(&n, &qsub(&q, &pa)).is_zero(), "{k:?} off triangle {t}");
                        for (u, v) in [(&pa, &pb), (&pb, &pc), (&pc, &pa)] {
                            let s = qdot(&n, &qcross(&qsub(v, u), &qsub(&q, u)));
                            assert!(s.is_positive(), "{k:?} not inside triangle {t}");
                        }
                    }
                    _ => {}
                }
            }
        }
        assert!(kinds.iter().all(|&k| k > 50), "{kinds:?}");
    }

    #[test]
    fn crossing_triangles_meet_in_a_segment_between_edge_crossings() {
        // Triangle 0 lies in z = 0; triangle 1 stands in y = 1 and crosses it.
        let m = mesh(
            &[
                [0.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
                [0.0, 4.0, 0.0],
                [1.0, 1.0, -1.0],
                [2.0, 1.0, 1.0],
                [-3.0, 1.0, 0.5],
            ],
            &[[0, 1, 2], [3, 4, 5]],
        );
        let input = Input::new(&m);
        let found = contacts_of(&input, 0, 1);
        let segs: Vec<&Seg> = found
            .iter()
            .filter_map(|c| match c {
                Contact::Segment(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(segs.len(), 2);
        let ends: Vec<P3> = segs[0]
            .ends
            .iter()
            .map(|&k| input.pt(input.geom(k)).p)
            .collect();
        // (0, 1, 0) where edge 2–0 crosses triangle 1, (1.5, 1, 0) where edge 3–4 crosses
        // triangle 0, in either order.
        let mut ends = ends;
        ends.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert_eq!(ends, vec![[0.0, 1.0, 0.0], [1.5, 1.0, 0.0]]);
    }

    // --- Arrangements ----------------------------------------------------------------------

    fn arrange(m: &Mesh) -> Arrangement {
        silent(|c| build(m, c))
            .expect("not cancelled")
            .expect("arrangement built")
    }

    /// A box with corners on the integer grid.
    fn grid_box(lo: [i32; 3], hi: [i32; 3]) -> Mesh {
        make_box(lo.map(|x| x as f32), hi.map(|x| x as f32))
    }

    /// Every edge of the arrangement carries as many face sides one way as the other, counting each
    /// face as often as its coverage: the faces of closed input surfaces stay closed.
    fn assert_balanced(arr: &Arrangement) {
        let mut sides: HashMap<(u32, u32), i64> = HashMap::new();
        for (f, t) in arr.faces.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let w = arr.coverage[f] as i64;
                *sides.entry((a.min(b), a.max(b))).or_default() += if a < b { w } else { -w };
            }
        }
        let open = sides.values().filter(|&&w| w != 0).count();
        assert_eq!(open, 0, "{open} unbalanced edges");
    }

    /// The faces cover the input's area, counting each face as often as its coverage (or less,
    /// where coplanar layers facing opposite ways cancelled); each face lies within its source
    /// triangle, and no face is flat.
    fn assert_areas(arr: &Arrangement, input_mesh: &Mesh, cancelled: bool) {
        let area_of = |p: [P3; 3]| exact::length(exact::normal(p[0], p[1], p[2])) * 0.5;
        let mut per_source = vec![0.0f64; input_mesh.triangle_count()];
        let mut total = 0.0;
        for (f, t) in arr.faces.iter().enumerate() {
            let a = area_of(t.map(|v| arr.pt(v).p));
            assert!(a > 0.0, "flat face {t:?}");
            per_source[arr.source[f] as usize] += a;
            total += a * arr.coverage[f] as f64;
        }
        let mut want_total = 0.0;
        for (t, &got) in per_source.iter().enumerate() {
            let want = area_of(exact::corners(input_mesh, t as u32));
            want_total += want;
            assert!(
                got <= want * (1.0 + 1e-9) + 1e-9,
                "triangle {t}: {got} > {want}"
            );
        }
        if cancelled {
            assert!(total <= want_total * (1.0 + 1e-9), "{total} > {want_total}");
        } else {
            assert!(
                (total - want_total).abs() <= want_total * 1e-9,
                "{total} vs {want_total}"
            );
        }
    }

    /// No two faces cross or overlap: tritri on the faces at their f64 positions (exact for most
    /// points of these inputs, within an ulp for the rest, which shared corners do not notice).
    /// And every edge has two or more sides: faces meet conformingly.
    fn assert_no_crossings(arr: &Arrangement) {
        let corners: Vec<[P3; 3]> = arr.faces.iter().map(|t| t.map(|v| arr.pt(v).p)).collect();
        let boxes: Vec<Aabb> = corners
            .iter()
            .map(|c| {
                let mut b = bvh::EMPTY;
                for &p in c {
                    bvh::include(&mut b, p);
                }
                b
            })
            .collect();
        let pairs = Bvh::new(&boxes).self_pairs_parallel(|i, j| {
            tritri::intersect(&corners[i as usize], &corners[j as usize])
        });
        assert!(
            pairs.is_empty(),
            "{} crossing face pairs: {:?}",
            pairs.len(),
            pairs
                .iter()
                .take(3)
                .map(|&(i, j)| (corners[i as usize], corners[j as usize]))
                .collect::<Vec<_>>()
        );
        let m = Mesh {
            positions: vec![0.0; arr.point_count() * 3],
            indices: arr.faces.as_flattened().to_vec(),
        };
        let edges = Edges::new(&m);
        for e in 0..edges.len() as u32 {
            assert!(edges.degree(e) >= 2, "edge {e} has one side");
        }
        let _ = side_ends;
    }

    #[test]
    fn two_interpenetrating_cubes_arrange_into_closed_conforming_faces() {
        let a = grid_box([0, 0, 0], [20, 20, 20]);
        let b = grid_box([10, 0, 0], [30, 20, 20]);
        let m = crate::ops::concat(&[&a, &b]).unwrap();
        let welded = crate::mesh::weld_indexed(&m.positions, &m.indices).unwrap();
        let arr = arrange(&welded);
        assert!(arr.split > 0);
        assert_balanced(&arr);
        assert_areas(&arr, &welded, true);
        assert_no_crossings(&arr);
        // The faces at y = 0 of both boxes overlap where x is in 10..20: one cluster there.
        assert!(arr.counts.clusters > 0);
        // Same input, same arrangement.
        let again = arrange(&welded);
        assert_eq!(again.faces, arr.faces);
        assert_eq!(again.source, arr.source);
    }

    #[test]
    fn grid_boxes_touching_and_overlapping_arrange_cleanly() {
        let mut rng = Rng(3);
        for round in 0..60 {
            let mut parts = Vec::new();
            for _ in 0..2 + round % 4 {
                let lo = [0, 1, 2].map(|_| rng.int(0, 5) as i32);
                let hi = lo.map(|x| x + rng.int(1, 4) as i32);
                parts.push(grid_box(lo, hi));
            }
            let refs: Vec<&Mesh> = parts.iter().collect();
            let m = crate::ops::concat(&refs).unwrap();
            let welded = crate::mesh::weld_indexed(&m.positions, &m.indices).unwrap();
            // Same-facing duplicates are what remove_doubles takes out before.
            let (single, _, _) = crate::repair::doubles::remove_doubles(&welded);
            let arr = arrange(&single);
            assert_balanced(&arr);
            assert_areas(&arr, &single, true);
            assert_no_crossings(&arr);
        }
    }

    #[test]
    fn a_rotated_box_through_another_splits_both_along_the_crossing() {
        let a = make_box([0.0; 3], [10.0; 3]);
        let mut b = make_box([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
        // Turned a little about two axes and moved into a's corner region.
        let (s, c) = (0.3f64.sin(), 0.3f64.cos());
        for p in b.positions.as_chunks_mut::<3>().0 {
            let [x, y, z] = p.map(f64::from);
            let (x, y) = (c * x - s * y, s * x + c * y);
            let (y, z) = (c * y - s * z, s * y + c * z);
            *p = [
                (x * 3.0 + 9.5) as f32,
                (y * 3.0 + 9.0) as f32,
                (z * 3.0 + 9.2) as f32,
            ];
        }
        let m = crate::ops::concat(&[&a, &b]).unwrap();
        let arr = arrange(&m);
        assert!(arr.split > 0);
        assert!(arr.counts.segments > 0);
        assert_balanced(&arr);
        assert_areas(&arr, &m, false);
    }
}
