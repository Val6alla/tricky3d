//! Edge collapses on a closed manifold triangle mesh, shared by Generate shell's tidying
//! (src/ops/shell/tidy.rs) and Triangle reduction (src/modify/reduce.rs). Each user says what a
//! collapse may cost and where its vertex may go (`Rules`); the shape checks here keep the mesh a
//! closed manifold and stop triangles turning over or becoming slivers.
//!
//! The collapses run in rounds over all cores (`collapse_rounds`): every candidate edge is
//! checked at once against the mesh as it is, the cheapest are picked so that no two touch the
//! same vertex, those are applied, and only the edges round them are checked again. Two picked
//! collapses share no vertex of their stars, so each one's checks still hold when both are
//! applied: a collapse only changes its own star's triangles, its own vertices' neighbours and
//! one position. With `ordered` the round takes the cheaper half of the candidates first, so the
//! mesh is reduced evenly; without, it takes every candidate it can in mesh order (Fast mode).

use rayon::prelude::*;

use crate::mesh::Mesh;
use crate::repair::bvh::{self, Aabb};
use crate::repair::exact::{self, P3, add, cross, dot, scale, sub};
use crate::repair::{Control, RepairResult};

/// A triangle whose normal turns further than this (cosine) is turned over; so is one that leans
/// further than `MIN_LEAN` from the mean normal of the triangles round the collapse.
const MIN_TURN: f64 = 0.2;
const MIN_LEAN: f64 = 0.3;
/// Below this quality an old triangle's normal says too little to compare a new one with.
const TRUSTED: f64 = 0.1;
/// Triangle quality (1 for equilateral, 0 for a sliver) a collapse may not go below.
const MIN_QUALITY: f64 = 0.02;

// ---------------------------------------------------------------------------------------------
// The working mesh

pub(crate) struct Work {
    pub pos: Vec<P3>,
    pub tris: Vec<[u32; 3]>,
    pub alive: Vec<bool>,
    /// Each vertex's triangles (alive ones only).
    pub around: Vec<Vec<u32>>,
    /// Longest edge a collapse may make.
    pub longest: f64,
    /// Let a vertex with three neighbours collapse (`linked`); Generate shell keeps the stricter
    /// rule it was measured with.
    pub relaxed: bool,
    /// Triangles alive.
    pub live: usize,
}

impl Work {
    pub fn new(mesh: &Mesh, longest: f64) -> Work {
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
            live: tris.len(),
            relaxed: false,
            longest,
            pos,
            tris,
            around,
        }
    }

    pub fn corners(&self, t: u32) -> [P3; 3] {
        self.tris[t as usize].map(|v| self.pos[v as usize])
    }

    /// The vertices joined to `v` by an edge.
    pub fn neighbours(&self, v: u32, out: &mut Vec<u32>) {
        out.clear();
        for &t in &self.around[v as usize] {
            for w in self.tris[t as usize] {
                if w != v && !out.contains(&w) {
                    out.push(w);
                }
            }
        }
    }

    /// The mesh as it is now: the triangles alive in their order, the vertices they use.
    pub fn finish(&self) -> Mesh {
        let mut indices = Vec::with_capacity(3 * self.live);
        for (t, tri) in self.tris.iter().enumerate() {
            if self.alive[t] {
                indices.extend_from_slice(tri);
            }
        }
        let positions: Vec<f32> = self.pos.iter().flatten().map(|&x| x as f32).collect();
        super::compact(&positions, indices)
    }

    /// The input triangle of each triangle `finish` keeps, in order.
    pub fn survivors(&self) -> Vec<u32> {
        (0..self.tris.len() as u32)
            .filter(|&t| self.alive[t as usize])
            .collect()
    }

    /// Every edge of the alive triangles once, smaller end first.
    pub fn edges(&self) -> Vec<[u32; 2]> {
        let mut out = Vec::with_capacity(3 * self.live / 2 + 3);
        for (t, tri) in self.tris.iter().enumerate() {
            if !self.alive[t] {
                continue;
            }
            for k in 0..3 {
                let (u, v) = (tri[k], tri[(k + 1) % 3]);
                if u < v {
                    out.push([u, v]);
                }
            }
        }
        out
    }
}

/// The point as the mesh will store it, so every check sees the final coordinates.
pub(crate) fn rounded(p: P3) -> P3 {
    p.map(|x| x as f32 as f64)
}

pub(crate) fn box_of(c: &[P3; 3]) -> Aabb {
    let mut b = bvh::EMPTY;
    c.iter().for_each(|&p| bvh::include(&mut b, p));
    b
}

pub(crate) fn normal(c: &[P3; 3]) -> P3 {
    cross(sub(c[1], c[0]), sub(c[2], c[0]))
}

/// 4√3 · area / (sum of squared sides): 1 for an equilateral triangle, 0 without area.
pub(crate) fn quality(c: &[P3; 3]) -> f64 {
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

pub(crate) fn centre(c: &[P3; 3]) -> P3 {
    [0, 1, 2].map(|k| (c[0][k] + c[1][k] + c[2][k]) / 3.0)
}

pub(crate) fn longest_side(c: &[P3; 3]) -> f64 {
    [(0, 1), (1, 2), (2, 0)]
        .iter()
        .map(|&(i, j)| exact::length(sub(c[j], c[i])))
        .fold(0.0, f64::max)
}

/// True when `new` faces about the same way as `old` (when that is no sliver) and as `mean`, and
/// is no sliver it was not already.
pub(crate) fn shape_kept(old: &[P3; 3], new: &[P3; 3], mean: P3) -> bool {
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

/// The corners of `v`'s triangle `t` with `v` at `x`.
pub(crate) fn moved_one(work: &Work, t: u32, v: u32, x: P3) -> [P3; 3] {
    work.tris[t as usize].map(|w| if w == v { x } else { work.pos[w as usize] })
}

/// True when vertex `v` may move to `x`: its triangles keep their shape.
pub(crate) fn may_move(work: &Work, v: u32, x: P3) -> bool {
    let mut mean = [0.0; 3];
    for &t in &work.around[v as usize] {
        mean = add(mean, normal(&work.corners(t)));
    }
    work.around[v as usize]
        .iter()
        .all(|&t| shape_kept(&work.corners(t), &moved_one(work, t, v, x), mean))
}

/// Moves vertex `v` to `x` if its triangles keep their shape.
pub(crate) fn try_move(work: &mut Work, v: u32, x: P3) -> bool {
    if !may_move(work, v, x) {
        return false;
    }
    work.pos[v as usize] = x;
    true
}

// ---------------------------------------------------------------------------------------------
// Keeping clear of crossings

/// For each vertex of `mesh`, whether a triangle not its own lies within `reach` along its normal,
/// either way.
pub(crate) fn near_other_sheets(
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
pub(crate) fn lock_round(
    mesh: &Mesh,
    tidied: &Mesh,
    pairs: &[(u32, u32)],
    reach: f64,
    locked: &mut [bool],
) {
    use rayon::prelude::*;
    let boxes: Vec<Aabb> = pairs
        .iter()
        .flat_map(|&(a, b)| [a, b])
        .map(|t| {
            let b = box_of(&exact::corners(tidied, t));
            [b[0].map(|x| x - reach), b[1].map(|x| x + reach)]
        })
        .collect();
    if boxes.is_empty() {
        return;
    }
    let tree = bvh::Bvh::new(&boxes);
    locked.par_iter_mut().enumerate().for_each(|(v, l)| {
        let p = exact::vertex(mesh, v as u32);
        let mut hit = false;
        tree.query(&[p, p], |_| hit = true);
        if hit {
            *l = true;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Quadrics

/// A quadric error: the sum of w (n·x − d)² over planes n·x = d, as A, b, c with
/// error(x) = xᵀAx − 2bᵀx + c, and the total weight.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Quadric {
    /// xx, xy, xz, yy, yz, zz.
    pub a: [f64; 6],
    pub b: P3,
    pub c: f64,
    pub w: f64,
}

impl Quadric {
    /// The plane through `p` with unit normal `n`, weighted `w`.
    pub fn plane(n: P3, p: P3, w: f64) -> Quadric {
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

    pub fn plus(&self, o: &Quadric) -> Quadric {
        Quadric {
            a: [0, 1, 2, 3, 4, 5].map(|i| self.a[i] + o.a[i]),
            b: add(self.b, o.b),
            c: self.c + o.c,
            w: self.w + o.w,
        }
    }

    pub fn times(&self, x: P3) -> P3 {
        let a = &self.a;
        [
            a[0] * x[0] + a[1] * x[1] + a[2] * x[2],
            a[1] * x[0] + a[3] * x[1] + a[4] * x[2],
            a[2] * x[0] + a[4] * x[1] + a[5] * x[2],
        ]
    }

    /// Mean squared distance to the planes at `x`.
    pub fn cost(&self, x: P3) -> f64 {
        if self.w <= 0.0 {
            return 0.0;
        }
        ((dot(x, self.times(x)) - 2.0 * dot(self.b, x) + self.c) / self.w).max(0.0)
    }

    /// The point nearest `from` among those of least error: `from` moved by the pseudo-inverse
    /// of A, ignoring directions whose eigenvalue is under 5 % of the largest (as dual contouring
    /// does), so near-parallel planes do not throw it far.
    /// Also how many directions it fixes: 1 for planes alike, 2 along an edge, 3 at a corner.
    pub fn optimum(&self, from: P3) -> (P3, usize) {
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
pub(crate) fn eigen(a: &[f64; 6]) -> ([f64; 3], [P3; 3]) {
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

/// Each vertex's quadric from its triangles' planes, area weighted: the mesh's own shape.
pub(crate) fn plane_quadrics(work: &Work) -> Vec<Quadric> {
    (0..work.pos.len())
        .into_par_iter()
        .map(|v| {
            let mut q = Quadric::default();
            for &tri in &work.around[v] {
                let c = work.corners(tri);
                let n = normal(&c);
                let area = exact::length(n) / 2.0;
                if area > 0.0 {
                    q = q.plus(&Quadric::plane(scale(n, 0.5 / area), c[0], area));
                }
            }
            q
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// One collapse

/// What `linked` found out about an edge, and reused buffers.
#[derive(Default)]
pub(crate) struct Scratch {
    /// The edge's two triangles.
    pub shared: [u32; 2],
    /// The other triangles of its ends.
    pub star: Vec<u32>,
    pub ring: Vec<u32>,
    pub of_u: Vec<u32>,
}

/// True when collapsing edge u–v keeps the mesh a closed manifold: the edge has two triangles, u
/// and v have no neighbours in common but the two across the edge (the link condition), and no
/// vertex is left with fewer than three neighbours: the merged vertex has u's and v's but for each
/// other and the two across, and those two lose one each. (A tetrahedron, the only closed shell
/// where three neighbours each would meet, is left alone that way.) Without `work.relaxed`, u and
/// v must each have more than three neighbours too. Fills `k.shared` and `k.star`.
pub(crate) fn linked(work: &Work, u: u32, v: u32, k: &mut Scratch) -> bool {
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
    let (nu, nv) = (k.of_u.len(), k.ring.len());
    let enough = if work.relaxed {
        nu + nv >= 3 + 4
    } else {
        nu > 3 && nv > 3
    };
    if common != 2 || !enough {
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

/// The corners of star triangle `t` once u and v are at `x`.
pub(crate) fn moved(work: &Work, t: u32, u: u32, v: u32, x: P3) -> [P3; 3] {
    work.tris[t as usize].map(|w| {
        if w == u || w == v {
            x
        } else {
            work.pos[w as usize]
        }
    })
}

/// True when the star triangles of a linked edge u–v, with u and v at `x`, keep their shape and
/// no edge grows past `work.longest`.
pub(crate) fn shape_fits(work: &Work, u: u32, v: u32, x: P3, k: &Scratch) -> bool {
    let mut mean = [0.0; 3];
    for &t in k.star.iter().chain(&k.shared) {
        mean = add(mean, normal(&work.corners(t)));
    }
    k.star.iter().all(|&t| {
        let new = moved(work, t, u, v, x);
        longest_side(&new) <= work.longest && shape_kept(&work.corners(t), &new, mean)
    })
}

/// Collapses linked edge u–v, with triangles `shared`, into v at `x`.
pub(crate) fn collapse(work: &mut Work, u: u32, v: u32, x: P3, shared: &[u32; 2]) {
    for &t in shared {
        work.alive[t as usize] = false;
        for w in work.tris[t as usize] {
            work.around[w as usize].retain(|&x| x != t);
        }
    }
    work.live -= 2;
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

// ---------------------------------------------------------------------------------------------
// Rounds

/// A collapse that passed every check: u–v into v at `x`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Plan {
    pub u: u32,
    pub v: u32,
    pub x: P3,
}

/// An edge whose collapse is cheap enough to try, by its price.
#[derive(Clone, Copy, Debug)]
struct Priced {
    u: u32,
    v: u32,
    cost: f64,
}

/// What a user of `collapse_rounds` decides. Both checks may only read the triangles round u and
/// v, their corners and those vertices' quadrics: see the module comment.
pub(crate) trait Rules: Sync {
    /// A quick price for collapsing edge u–v (both alive), or None when it may not collapse at
    /// all: run on every candidate edge, so it should cost little.
    fn price(&self, work: &Work, q: &[Quadric], u: u32, v: u32) -> Option<f64>;

    /// The collapse of a priced edge if it passes every check: run on the edges a round picks.
    fn plan(&self, work: &Work, q: &[Quadric], u: u32, v: u32, k: &mut Scratch) -> Option<Plan>;

    /// Called once per round with the plans that passed, before they are applied (the work is
    /// as the plans saw it), for bookkeeping of the user's own.
    fn before_apply(&mut self, _work: &Work, _plans: &[Plan]) {}
}

/// How `collapse_rounds` goes on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rounds {
    /// Cheapest first (even reduction); else mesh order (Fast mode).
    pub ordered: bool,
    /// Stop once this few triangles are left (0: as long as any collapse is allowed).
    pub target: usize,
}

/// Collapses edges in rounds until none is allowed or `target` triangles are left (module
/// comment). Each round prices the edges that are new or changed (in parallel), picks the cheaper
/// half of all priced edges so that no two share a vertex of their stars, runs the full checks on
/// the picked ones (in parallel), and applies those that pass. An edge that failed its full check
/// is not tried again until something round it changes. `q` is updated as vertices merge.
pub(crate) fn collapse_rounds<R: Rules>(
    work: &mut Work,
    q: &mut [Quadric],
    rules: &mut R,
    how: Rounds,
    control: &mut Control,
) -> RepairResult<()> {
    let start = work.live;
    let mut todo = work.edges();
    let mut cached: Vec<Priced> = Vec::new();
    // A vertex is marked with the round that picked an edge whose star holds it, or that left it
    // changed or next to a change.
    let mut picked_in = vec![u32::MAX; work.pos.len()];
    let mut dirty_in = vec![u32::MAX; work.pos.len()];
    let mut round = 0u32;
    let mut ring = Vec::new();
    while work.live > how.target {
        control.check()?;
        let cancel = control.cancel_flag();
        let stop = || cancel.load(std::sync::atomic::Ordering::Relaxed);
        let (w, qs, r): (&Work, &[Quadric], &R) = (work, q, rules);
        let fresh: Vec<Priced> = todo
            .par_iter()
            .with_min_len(1024)
            .filter_map(|&[u, v]| {
                if stop() || w.around[u as usize].is_empty() || w.around[v as usize].is_empty() {
                    return None;
                }
                r.price(w, qs, u, v).map(|cost| Priced { u, v, cost })
            })
            .collect();
        control.check()?;
        let mut priced = cached;
        priced.extend(fresh);
        if priced.is_empty() {
            break;
        }
        if how.ordered {
            priced.par_sort_unstable_by(|a, b| {
                a.cost.total_cmp(&b.cost).then((a.u, a.v).cmp(&(b.u, b.v)))
            });
        } else {
            priced.par_sort_unstable_by_key(|p| (p.u, p.v));
        }
        // The cheaper half (all of them in Fast mode), as long as no two touch the same vertex
        // and the target is not passed.
        let limit = if how.ordered {
            priced.len().div_ceil(2)
        } else {
            priced.len()
        };
        let mut picked: Vec<Priced> = Vec::new();
        let mut kept: Vec<Priced> = Vec::new();
        for (i, p) in priced.into_iter().enumerate() {
            if i >= limit || work.live - 2 * picked.len().min(work.live / 2) <= how.target {
                kept.push(p);
                continue;
            }
            let star = star_vertices(work, p.u, p.v, &mut ring);
            if star.iter().any(|&s| picked_in[s as usize] == round) {
                kept.push(p);
                continue;
            }
            for &s in star.iter() {
                picked_in[s as usize] = round;
            }
            picked.push(p);
        }
        let (w, qs, r): (&Work, &[Quadric], &R) = (work, q, rules);
        let chosen: Vec<Plan> = picked
            .par_iter()
            .map_init(Scratch::default, |k, p| {
                if stop() {
                    return None;
                }
                r.plan(w, qs, p.u, p.v, k)
            })
            .flatten()
            .collect();
        control.check()?;
        if !chosen.is_empty() {
            rules.before_apply(work, &chosen);
        }
        let mut k = Scratch::default();
        for p in &chosen {
            // The plan was checked against the mesh as it is (module comment), so it links.
            if linked(work, p.u, p.v, &mut k) {
                let shared = k.shared;
                collapse(work, p.u, p.v, p.x, &shared);
                q[p.v as usize] = q[p.u as usize].plus(&q[p.v as usize]);
            }
        }
        // Everything round a changed star is priced again next round: the star's vertices (v and
        // its neighbours) and their neighbours.
        let mut dirty = Vec::new();
        let mut mark = |z: u32, dirty: &mut Vec<u32>| {
            if dirty_in[z as usize] != round {
                dirty_in[z as usize] = round;
                dirty.push(z);
            }
        };
        for p in &chosen {
            work.neighbours(p.v, &mut ring);
            mark(p.v, &mut dirty);
            for &w in &ring {
                for &t in &work.around[w as usize] {
                    for z in work.tris[t as usize] {
                        mark(z, &mut dirty);
                    }
                }
            }
        }
        let mut next = Vec::new();
        for &v in &dirty {
            for &t in &work.around[v as usize] {
                let tri = work.tris[t as usize];
                for kk in 0..3 {
                    let (a, b) = (tri[kk], tri[(kk + 1) % 3]);
                    if a < b && (a == v || b == v) {
                        next.push([a, b]);
                    }
                }
            }
        }
        next.par_sort_unstable();
        next.dedup();
        // Picked edges that failed their checks drop out until something round them changes;
        // the rest keep their price unless their star changed.
        cached = kept
            .into_iter()
            .filter(|p| {
                dirty_in[p.u as usize] != round
                    && dirty_in[p.v as usize] != round
                    && !work.around[p.u as usize].is_empty()
                    && !work.around[p.v as usize].is_empty()
            })
            .collect();
        todo = next;
        round += 1;
        let done = start - work.live;
        let wanted = start.saturating_sub(how.target).max(1);
        control.progress((done as f64 / wanted as f64).min(1.0));
    }
    control.progress(1.0);
    Ok(())
}

/// An edge in the serial queue, cheapest first; `stamps` are its ends' versions when it was queued.
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

/// Collapses edges one at a time, cheapest first (a heap of priced edges; an edge whose ends
/// changed since it was queued is skipped and priced again with them). Generate shell's tidying
/// runs this, as it always has: on its marching-tetrahedra surfaces most picked collapses fail
/// the offset checks, which `collapse_rounds` pays for in whole rounds.
pub(crate) fn collapse_serial<R: Rules>(
    work: &mut Work,
    q: &mut [Quadric],
    rules: &mut R,
    control: &mut Control,
) -> RepairResult<()> {
    use std::collections::BinaryHeap;
    let mut version = vec![0u32; work.pos.len()];
    let mut heap = BinaryHeap::new();
    for [u, v] in work.edges() {
        if let Some(cost) = rules.price(work, q, u, v) {
            heap.push(Candidate {
                cost,
                ends: [u, v],
                stamps: [0, 0],
            });
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
        let Some(p) = rules.plan(work, q, u, v, &mut k) else {
            continue;
        };
        if !linked(work, p.u, p.v, &mut k) {
            continue;
        }
        rules.before_apply(work, &[p]);
        let shared = k.shared;
        collapse(work, p.u, p.v, p.x, &shared);
        q[p.v as usize] = q[p.u as usize].plus(&q[p.v as usize]);
        version[p.v as usize] += 1;
        version[p.u as usize] += 1;
        work.neighbours(p.v, &mut ring);
        for &w in &ring {
            if let Some(cost) = rules.price(work, q, p.v, w) {
                heap.push(Candidate {
                    cost,
                    ends: [p.v, w],
                    stamps: [version[p.v as usize], version[w as usize]],
                });
            }
        }
    }
    control.progress(1.0);
    Ok(())
}

/// Every vertex of the triangles round u and v.
fn star_vertices<'a>(work: &Work, u: u32, v: u32, out: &'a mut Vec<u32>) -> &'a [u32] {
    out.clear();
    for &t in work.around[u as usize]
        .iter()
        .chain(&work.around[v as usize])
    {
        for w in work.tris[t as usize] {
            if !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
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
