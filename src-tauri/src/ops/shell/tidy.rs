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
//! The collapses run cheapest first by quadric error (Garland and Heckbert). Their quadrics come
//! from the mesh's own triangles, which keeps its shape; with `features` they come from the
//! tangent planes of the true offset surface at each vertex instead, so a collapse across an edge
//! that marching tetrahedra cut off puts the vertex back on the sharp edge or corner. Those
//! collapses (the two ends see different tangent planes) all run before any other, so the flat
//! parts round an edge are not reduced first into triangles too long to fold back onto it.

use std::collections::BinaryHeap;

use super::field::{Offset, Surface, level};
use crate::mesh::Mesh;
use crate::repair::bvh::{self, Aabb};
use crate::repair::exact::{self, P3, add, cross, dot, scale, sub};
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
/// A triangle whose normal turns further than this (cosine) is turned over; so is one that leans
/// further than `MIN_LEAN` from the mean normal of the triangles round the collapse.
const MIN_TURN: f64 = 0.2;
const MIN_LEAN: f64 = 0.3;
/// Below this quality an old triangle's normal says too little to compare a new one with.
const TRUSTED: f64 = 0.1;
/// Triangle quality (1 for equilateral, 0 for a sliver) a collapse may not go below.
const MIN_QUALITY: f64 = 0.02;
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
        let mut work = Work::new(&mesh, s);
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

/// For each vertex of `mesh`, whether a triangle not its own lies within `reach` along its normal,
/// either way.
pub(super) fn near_other_sheets(
    mesh: &Mesh,
    reach: f64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Vec<bool> {
    use rayon::prelude::*;
    let points: Vec<P3> = (0..mesh.vertex_count() as u32)
        .map(|v| exact::vertex(mesh, v))
        .collect();
    let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    let bvh = bvh::Bvh::new(&bvh::boxes_of(&points, &triangles));
    let mut normals = vec![[0.0; 3]; points.len()];
    for t in &triangles {
        let n = normal(&t.map(|v| points[v as usize]));
        for &v in t {
            normals[v as usize] = add(normals[v as usize], n);
        }
    }
    (0..points.len())
        .into_par_iter()
        .map(|v| {
            let n = normals[v];
            let length = exact::length(n);
            if length == 0.0 || cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            let p = points[v];
            let d = scale(n, reach / length);
            let (a, b) = (sub(p, d), add(p, d));
            let mut near = false;
            bvh.segment(a, b, |t| {
                let tri = triangles[t as usize];
                if near || tri.contains(&(v as u32)) {
                    return;
                }
                let [x, y, z] = tri.map(|w| points[w as usize]);
                near = crate::repair::levelset::segment_hit(a, sub(b, a), x, y, z).is_some();
            });
            near
        })
        .collect()
}

/// Locks every vertex of `mesh` within `reach` of a triangle of `tidied` named in `pairs`.
fn lock_round(mesh: &Mesh, tidied: &Mesh, pairs: &[(u32, u32)], reach: f64, locked: &mut [bool]) {
    use rayon::prelude::*;
    let boxes: Vec<Aabb> = pairs
        .iter()
        .flat_map(|&(a, b)| [a, b])
        .map(|t| {
            let b = box_of(&exact::corners(tidied, t));
            [b[0].map(|x| x - reach), b[1].map(|x| x + reach)]
        })
        .collect();
    let mut all = bvh::EMPTY;
    boxes.iter().for_each(|b| bvh::grow(&mut all, b));
    let inside = |p: P3, b: &Aabb| (0..3).all(|k| b[0][k] <= p[k] && p[k] <= b[1][k]);
    locked.par_iter_mut().enumerate().for_each(|(v, l)| {
        let p = exact::vertex(mesh, v as u32);
        if inside(p, &all) && boxes.iter().any(|b| inside(p, b)) {
            *l = true;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// The working mesh

struct Work {
    pos: Vec<P3>,
    tris: Vec<[u32; 3]>,
    alive: Vec<bool>,
    /// Each vertex's triangles (alive ones only).
    around: Vec<Vec<u32>>,
    longest: f64,
}

impl Work {
    fn new(mesh: &Mesh, s: &Settings) -> Work {
        let pos: Vec<P3> = (0..mesh.vertex_count() as u32)
            .map(|v| exact::vertex(mesh, v))
            .collect();
        let tris: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let mut around = vec![Vec::new(); pos.len()];
        for (t, tri) in tris.iter().enumerate() {
            for &v in tri {
                around[v as usize].push(t as u32);
            }
        }
        Work {
            alive: vec![true; tris.len()],
            longest: s.longest.min(MAX_EDGE * s.cell),
            pos,
            tris,
            around,
        }
    }

    fn corners(&self, t: u32) -> [P3; 3] {
        self.tris[t as usize].map(|v| self.pos[v as usize])
    }

    /// The vertices joined to `v` by an edge.
    fn neighbours(&self, v: u32, out: &mut Vec<u32>) {
        out.clear();
        for &t in &self.around[v as usize] {
            for w in self.tris[t as usize] {
                if w != v && !out.contains(&w) {
                    out.push(w);
                }
            }
        }
    }

    fn finish(&self) -> Mesh {
        let mut indices = Vec::with_capacity(3 * self.tris.len());
        for (t, tri) in self.tris.iter().enumerate() {
            if self.alive[t] {
                indices.extend_from_slice(tri);
            }
        }
        let positions: Vec<f32> = self.pos.iter().flatten().map(|&x| x as f32).collect();
        super::super::compact(&positions, indices)
    }
}

/// The point as the mesh will store it, so every check sees the final coordinates.
fn rounded(p: P3) -> P3 {
    p.map(|x| x as f32 as f64)
}

fn box_of(c: &[P3; 3]) -> Aabb {
    let mut b = bvh::EMPTY;
    c.iter().for_each(|&p| bvh::include(&mut b, p));
    b
}

fn normal(c: &[P3; 3]) -> P3 {
    cross(sub(c[1], c[0]), sub(c[2], c[0]))
}

/// 4√3 · area / (sum of squared sides): 1 for an equilateral triangle, 0 without area.
fn quality(c: &[P3; 3]) -> f64 {
    let sides: f64 = [(0, 1), (1, 2), (2, 0)]
        .iter()
        .map(|&(i, j)| {
            let d = sub(c[j], c[i]);
            dot(d, d)
        })
        .sum();
    if sides <= 0.0 {
        return 0.0;
    }
    2.0 * 3f64.sqrt() * exact::length(normal(c)) / sides
}

fn centre(c: &[P3; 3]) -> P3 {
    [0, 1, 2].map(|k| (c[0][k] + c[1][k] + c[2][k]) / 3.0)
}

fn longest_side(c: &[P3; 3]) -> f64 {
    [(0, 1), (1, 2), (2, 0)]
        .iter()
        .map(|&(i, j)| exact::length(sub(c[j], c[i])))
        .fold(0.0, f64::max)
}

/// True when `new` faces about the same way as `old` (when that is no sliver) and as `mean`, and
/// is no sliver it was not already.
fn shape_kept(old: &[P3; 3], new: &[P3; 3], mean: P3) -> bool {
    let (a, b) = (normal(old), normal(new));
    let (la, lb, lm) = (exact::length(a), exact::length(b), exact::length(mean));
    if lb == 0.0 {
        return false;
    }
    let old_quality = quality(old);
    if old_quality >= TRUSTED && dot(a, b) < MIN_TURN * la * lb {
        return false;
    }
    if lm > 0.0 && dot(mean, b) < MIN_LEAN * lm * lb {
        return false;
    }
    quality(new) >= MIN_QUALITY.min(old_quality)
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
                try_move(work, v as u32, x);
            }
        }
    }
    control.progress(1.0);
    Ok(())
}

/// Moves vertex `v` to `x` if its triangles keep their shape.
fn try_move(work: &mut Work, v: u32, x: P3) -> bool {
    let mut mean = [0.0; 3];
    for &t in &work.around[v as usize] {
        mean = add(mean, normal(&work.corners(t)));
    }
    for &t in &work.around[v as usize] {
        let c = work.corners(t);
        let moved = work.tris[t as usize].map(|w| if w == v { x } else { work.pos[w as usize] });
        if !shape_kept(&c, &moved, mean) {
            return false;
        }
    }
    work.pos[v as usize] = x;
    true
}

// ---------------------------------------------------------------------------------------------
// Reducing

/// A quadric error: the sum of w (n·x − d)² over planes n·x = d, as A, b, c with
/// error(x) = xᵀAx − 2bᵀx + c, and the total weight.
#[derive(Clone, Copy, Debug, Default)]
struct Quadric {
    /// xx, xy, xz, yy, yz, zz.
    a: [f64; 6],
    b: P3,
    c: f64,
    w: f64,
}

impl Quadric {
    /// The plane through `p` with unit normal `n`, weighted `w`.
    fn plane(n: P3, p: P3, w: f64) -> Quadric {
        let d = dot(n, p);
        Quadric {
            a: [
                w * n[0] * n[0],
                w * n[0] * n[1],
                w * n[0] * n[2],
                w * n[1] * n[1],
                w * n[1] * n[2],
                w * n[2] * n[2],
            ],
            b: scale(n, w * d),
            c: w * d * d,
            w,
        }
    }

    fn plus(&self, o: &Quadric) -> Quadric {
        Quadric {
            a: [0, 1, 2, 3, 4, 5].map(|i| self.a[i] + o.a[i]),
            b: add(self.b, o.b),
            c: self.c + o.c,
            w: self.w + o.w,
        }
    }

    fn times(&self, x: P3) -> P3 {
        let a = &self.a;
        [
            a[0] * x[0] + a[1] * x[1] + a[2] * x[2],
            a[1] * x[0] + a[3] * x[1] + a[4] * x[2],
            a[2] * x[0] + a[4] * x[1] + a[5] * x[2],
        ]
    }

    /// Mean squared distance to the planes at `x`.
    fn cost(&self, x: P3) -> f64 {
        if self.w <= 0.0 {
            return 0.0;
        }
        ((dot(x, self.times(x)) - 2.0 * dot(self.b, x) + self.c) / self.w).max(0.0)
    }

    /// The point nearest `from` among those of least error: `from` moved by the pseudo-inverse
    /// of A, ignoring directions whose eigenvalue is under 5 % of the largest (as dual contouring
    /// does), so near-parallel planes do not throw it far.
    /// Also how many directions it fixes: 1 for planes alike, 2 along an edge, 3 at a corner.
    fn optimum(&self, from: P3) -> (P3, usize) {
        let (values, vectors) = eigen(&self.a);
        let top = values.iter().fold(0.0f64, |m, &l| m.max(l));
        if top <= 0.0 {
            return (from, 0);
        }
        let r = sub(self.b, self.times(from));
        let (mut x, mut rank) = (from, 0);
        for k in 0..3 {
            if values[k] > 0.05 * top {
                let v = vectors[k];
                x = add(x, scale(v, dot(v, r) / values[k]));
                rank += 1;
            }
        }
        (x, rank)
    }
}

/// Eigenvalues and unit eigenvectors of a symmetric 3 × 3 matrix (xx, xy, xz, yy, yz, zz), by
/// cyclic Jacobi rotations.
fn eigen(a: &[f64; 6]) -> ([f64; 3], [P3; 3]) {
    let mut m = [[a[0], a[1], a[2]], [a[1], a[3], a[4]], [a[2], a[4], a[5]]];
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..24 {
        let off = m[0][1].abs() + m[0][2].abs() + m[1][2].abs();
        let size = m[0][0].abs() + m[1][1].abs() + m[2][2].abs();
        if off <= 1e-15 * size || off == 0.0 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if m[p][q] == 0.0 {
                continue;
            }
            let theta = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
            let t = if theta == 0.0 {
                1.0
            } else {
                theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt())
            };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for row in &mut m {
                let (mp, mq) = (row[p], row[q]);
                row[p] = c * mp - s * mq;
                row[q] = s * mp + c * mq;
            }
            let (rp, rq) = (m[p], m[q]);
            m[p] = [0, 1, 2].map(|k| c * rp[k] - s * rq[k]);
            m[q] = [0, 1, 2].map(|k| s * rp[k] + c * rq[k]);
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let values = [m[0][0], m[1][1], m[2][2]];
    let vectors = [0, 1, 2].map(|k| [v[0][k], v[1][k], v[2][k]]);
    (values, vectors)
}

/// Each vertex's quadric: its triangles' planes (area weighted), or with `features` the offset
/// surface's tangent plane where the vertex lies (weighted by its share of the area).
fn quadrics(work: &Work, surface: &Surface, s: &Settings) -> Vec<Quadric> {
    use rayon::prelude::*;
    let t = s.offset.thickness();
    let reach = t + 2.0 * s.cell;
    (0..work.pos.len())
        .into_par_iter()
        .map(|v| {
            let p = work.pos[v];
            let mut q = Quadric::default();
            let mut share = 0.0;
            for &tri in &work.around[v] {
                let c = work.corners(tri);
                let n = normal(&c);
                let area = exact::length(n) / 2.0;
                share += area / 3.0;
                if !s.features && area > 0.0 {
                    q = q.plus(&Quadric::plane(scale(n, 0.5 / area), c[0], area));
                }
            }
            if s.features
                && let Some((d, near)) = surface.nearest(p, reach * reach)
                && d > 0.0
            {
                // From the part's surface toward the vertex, and the offset's outward normal.
                let away = scale(sub(p, near), 1.0 / d.sqrt());
                let n = match s.offset {
                    Offset::Outer(_) => away,
                    Offset::Inner(_) => scale(away, -1.0),
                };
                q = Quadric::plane(n, add(near, scale(away, t)), share.max(1e-12));
            }
            q
        })
        .collect()
}

/// What pricing an edge needs: each vertex's quadric, the vertices left alone, the dearest
/// collapse allowed, and whether only edges between different planes count (the first round with
/// `features`).
struct Pricing<'a> {
    q: Vec<Quadric>,
    locked: &'a [bool],
    max_cost: f64,
    edges_only: bool,
}

impl Pricing<'_> {
    /// The cost of collapsing edge u–v, or None when it may not collapse. Priced at the middle;
    /// the optimum (an eigen solve) only where the middle is too dear or edges alone count.
    fn price(&self, work: &Work, u: u32, v: u32) -> Option<f64> {
        if self.locked[u as usize] || self.locked[v as usize] {
            return None;
        }
        let both = self.q[u as usize].plus(&self.q[v as usize]);
        let mid = scale(add(work.pos[u as usize], work.pos[v as usize]), 0.5);
        let mut cost = both.cost(mid);
        if self.edges_only || cost > self.max_cost {
            let (best, rank) = both.optimum(mid);
            if self.edges_only && rank < 2 {
                return None;
            }
            cost = cost.min(both.cost(best));
        }
        (cost <= self.max_cost).then_some(cost)
    }
}

/// An edge in the queue, cheapest first; `stamps` are its ends' versions when it was queued.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Candidate {
    cost: f64,
    ends: [u32; 2],
    stamps: [u32; 2],
}

impl Eq for Candidate {}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.cost.total_cmp(&self.cost)
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn reduce(
    work: &mut Work,
    surface: &Surface,
    s: &Settings,
    locked: &[bool],
    control: &mut Control,
) -> RepairResult<()> {
    let q = quadrics(work, surface, s);
    control.check()?;
    let mut pricing = Pricing {
        q,
        locked,
        max_cost: MAX_COST * s.cell * s.cell,
        edges_only: s.features,
    };
    let rounds = if s.features { 2 } else { 1 };
    for round in 0..rounds {
        pricing.edges_only = round + 1 < rounds;
        let span = 1.0 / rounds as f64;
        let mut part = control.sub(round as f64 * span, (round + 1) as f64 * span);
        collapse_all(work, surface, s, &mut pricing, &mut part)?;
    }
    Ok(())
}

/// Queues every edge, then collapses them cheapest first while the checks allow.
fn collapse_all(
    work: &mut Work,
    surface: &Surface,
    s: &Settings,
    pricing: &mut Pricing,
    control: &mut Control,
) -> RepairResult<()> {
    let mut version = vec![0u32; work.pos.len()];
    let mut heap = BinaryHeap::new();
    let queue = |heap: &mut BinaryHeap<Candidate>,
                 work: &Work,
                 p: &Pricing,
                 version: &[u32],
                 u: u32,
                 v: u32| {
        if let Some(cost) = p.price(work, u, v) {
            heap.push(Candidate {
                cost,
                ends: [u, v],
                stamps: [version[u as usize], version[v as usize]],
            });
        }
    };
    for t in 0..work.tris.len() {
        if !work.alive[t] {
            continue;
        }
        let tri = work.tris[t];
        for k in 0..3 {
            let (u, v) = (tri[k], tri[(k + 1) % 3]);
            if u < v {
                queue(&mut heap, work, pricing, &version, u, v);
            }
        }
    }
    let total = heap.len().max(1) as f64;
    let mut popped = 0usize;
    let mut k = Scratch::default();
    let mut ring = Vec::new();
    while let Some(c) = heap.pop() {
        popped += 1;
        if popped.is_multiple_of(4096) {
            control.check()?;
            // The queue refills as collapses change their neighbourhoods: about four pops a
            // first entry.
            control.progress((popped as f64 / (4.0 * total)).min(1.0));
        }
        let [u, v] = c.ends;
        if c.stamps != [version[u as usize], version[v as usize]]
            || work.around[u as usize].is_empty()
            || work.around[v as usize].is_empty()
        {
            continue;
        }
        let Some(p) = plan(
            work,
            surface,
            s,
            &pricing.q,
            pricing.max_cost,
            c.ends,
            &mut k,
        ) else {
            continue;
        };
        collapse(work, p.u, p.v, p.x, &p.shared);
        pricing.q[p.v as usize] = p.both;
        version[p.v as usize] += 1;
        version[p.u as usize] += 1;
        work.neighbours(p.v, &mut ring);
        for &w in &ring {
            queue(&mut heap, work, pricing, &version, p.v, w);
        }
    }
    control.progress(1.0);
    Ok(())
}

/// A collapse that passed every check: u–v into v at `x`, through the edge's two triangles.
struct Plan {
    u: u32,
    v: u32,
    x: P3,
    shared: [u32; 2],
    both: Quadric,
}

/// The collapse of edge u–v at its best position that passes the checks, if any: the optimum
/// when it stays near the edge, else the middle.
fn plan(
    work: &Work,
    surface: &Surface,
    s: &Settings,
    q: &[Quadric],
    max_cost: f64,
    [u, v]: [u32; 2],
    k: &mut Scratch,
) -> Option<Plan> {
    if !linked(work, u, v, k) {
        return None;
    }
    let both = q[u as usize].plus(&q[v as usize]);
    let (pu, pv) = (work.pos[u as usize], work.pos[v as usize]);
    let mid = scale(add(pu, pv), 0.5);
    let (best, _) = both.optimum(mid);
    let reach = exact::length(sub(pu, pv)).max(0.5 * s.cell);
    let near = exact::length(sub(best, mid)) <= reach;
    [best, mid]
        .into_iter()
        .skip(if near { 0 } else { 1 })
        .map(rounded)
        .find(|&x| both.cost(x) <= max_cost && fits(work, surface, s, u, v, x, k))
        .map(|x| Plan {
            u,
            v,
            x,
            shared: k.shared,
            both,
        })
}

/// What `linked` found out about an edge, and reused buffers.
#[derive(Default)]
struct Scratch {
    /// The edge's two triangles.
    shared: [u32; 2],
    /// The other triangles of its ends.
    star: Vec<u32>,
    ring: Vec<u32>,
    of_u: Vec<u32>,
}

/// True when collapsing edge u–v keeps the mesh a closed manifold: the edge has two triangles, u
/// and v have no neighbours in common but the two across the edge (the link condition), and no
/// vertex is left with fewer than three neighbours. Fills `k.shared` and `k.star`.
fn linked(work: &Work, u: u32, v: u32, k: &mut Scratch) -> bool {
    let (su, sv) = (&work.around[u as usize], &work.around[v as usize]);
    let mut count = 0;
    for &t in su {
        if sv.contains(&t) {
            if count == 2 {
                return false;
            }
            k.shared[count] = t;
            count += 1;
        }
    }
    if count != 2 {
        return false;
    }
    let opposite = k.shared.map(|t| {
        let tri = work.tris[t as usize];
        tri.into_iter().find(|&w| w != u && w != v).unwrap_or(u)
    });
    if opposite[0] == opposite[1] {
        return false;
    }
    work.neighbours(u, &mut k.of_u);
    work.neighbours(v, &mut k.ring);
    let common = k.of_u.iter().filter(|w| k.ring.contains(w)).count();
    if common != 2 || k.of_u.len() <= 3 || k.ring.len() <= 3 {
        return false;
    }
    for w in opposite {
        work.neighbours(w, &mut k.ring);
        if k.ring.len() <= 3 {
            return false;
        }
    }
    k.star.clear();
    for &t in su.iter().chain(sv) {
        if t != k.shared[0] && t != k.shared[1] && !k.star.contains(&t) {
            k.star.push(t);
        }
    }
    true
}

/// True when the triangles round a linked edge u–v, with u and v at `x`, keep their shape, no edge
/// grows past `longest`, and the surface stays on the offset (cheapest checks first).
fn fits(work: &Work, surface: &Surface, s: &Settings, u: u32, v: u32, x: P3, k: &Scratch) -> bool {
    let mut mean = [0.0; 3];
    for &t in k.star.iter().chain(&k.shared) {
        mean = add(mean, normal(&work.corners(t)));
    }
    let moved = |t: u32| {
        work.tris[t as usize].map(|w| {
            if w == u || w == v {
                x
            } else {
                work.pos[w as usize]
            }
        })
    };
    for &t in &k.star {
        let new = moved(t);
        if longest_side(&new) > work.longest || !shape_kept(&work.corners(t), &new, mean) {
            return false;
        }
    }
    if off(surface, s, x).abs() > VERTEX_SLACK * s.cell {
        return false;
    }
    k.star.iter().all(|&t| {
        let new = moved(t);
        longest_side(&new) <= CHECKED * s.cell
            || off(surface, s, centre(&new)).abs() <= CENTRE_SLACK * s.cell
    })
}

/// Collapses linked edge u–v, with triangles `shared`, into v at `x`.
fn collapse(work: &mut Work, u: u32, v: u32, x: P3, shared: &[u32; 2]) {
    for &t in shared {
        work.alive[t as usize] = false;
        for w in work.tris[t as usize] {
            work.around[w as usize].retain(|&x| x != t);
        }
    }
    let moved = std::mem::take(&mut work.around[u as usize]);
    for &t in &moved {
        for w in &mut work.tris[t as usize] {
            if *w == u {
                *w = v;
            }
        }
    }
    work.around[v as usize].extend(moved);
    work.pos[v as usize] = x;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadric_optimum_and_eigen() {
        // Planes x = 1 and y = 2: the line through (1, 2, z), nearest (5, 5, 5).
        let q = Quadric::plane([1.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0).plus(&Quadric::plane(
            [0.0, 1.0, 0.0],
            [0.0, 2.0, 0.0],
            1.0,
        ));
        let (x, rank) = q.optimum([5.0, 5.0, 5.0]);
        assert_eq!(rank, 2);
        assert!(exact::length(sub(x, [1.0, 2.0, 5.0])) < 1e-12, "{x:?}");
        assert!(q.cost(x) < 1e-20);
        // A third plane makes it a corner.
        let q = q.plus(&Quadric::plane([0.0, 0.0, 1.0], [0.0, 0.0, 3.0], 2.0));
        let (x, rank) = q.optimum([5.0, 5.0, 5.0]);
        assert_eq!(rank, 3);
        assert!(exact::length(sub(x, [1.0, 2.0, 3.0])) < 1e-12, "{x:?}");
        // A tilted matrix: A v = λ v for each pair.
        let a = [4.0, 1.0, -2.0, 3.0, 0.5, 5.0];
        let (values, vectors) = eigen(&a);
        let q = Quadric {
            a,
            ..Quadric::default()
        };
        for k in 0..3 {
            let av = q.times(vectors[k]);
            assert!(exact::length(sub(av, scale(vectors[k], values[k]))) < 1e-9);
            assert!((exact::length(vectors[k]) - 1.0).abs() < 1e-12);
        }
    }
}
