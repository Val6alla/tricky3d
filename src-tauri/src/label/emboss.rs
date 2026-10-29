//! Putting a label's shape into a part's surface, raised or sunk, without a general Boolean.
//!
//! The label lies in a frame: w along the surface normal at the point clicked, u along the text
//! and v up it, w = 0 the tangent plane there. The surface under the label must be one piece that
//! faces up the w axis (a height field over the label), which holds on flat and gently curved
//! surfaces; the frame's column over the label must hold nothing else of the part between the
//! surface and the label's top (raised) or floor (sunk).
//!
//! 1. The patch: the triangles joined edge to edge to the one under the frame's origin, as far as
//!    they reach over the label's box. All must face up w.
//! 2. The outline's points are found on the patch (heights from the triangle under each), and
//!    every outline side is cut where it crosses a patch edge; the cut points lie on those edges.
//! 3. Each patch triangle an outline side passes through is split along it (a constrained
//!    triangulation of its corners, the cut points on its edges and the outline points in it);
//!    the pieces outside the label stay, those inside go, as do whole triangles inside.
//! 4. Walls stand on the cut outline, straight along w: up to a flat top at the label's height
//!    (raised), or down to a flat floor at its depth (sunk); the label's shape closes the top or
//!    the floor. The walls share their bottom points with the cut surface, so the result is
//!    closed wherever the part was.
//!
//! Exactly degenerate cases (an outline point on a patch edge, a side through a patch corner) and
//! anything rounding to f32 makes wrong are found and the label is shifted by a fraction of a
//! micrometre and tried again.

use std::collections::{HashMap, VecDeque};

use spade::handles::{FixedFaceHandle, FixedVertexHandle, InnerTag};
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use super::region::{P2, Region};
use crate::mesh::{Mesh, cross, dot, sub};
use crate::repair::defaults::{DEGENERATE, effective_degenerate_tolerance};
use crate::repair::exact::{P3, orient2d};
use crate::repair::{Control, RepairResult, degenerate, status, tritri};

/// Closest the surface may come to the label's top or floor, in mm.
const MIN_GAP: f64 = 0.01;
/// Thinnest wall a sunk label may leave in front of the part's other side, in mm.
const MIN_WALL: f64 = 0.1;
/// Least share of a patch triangle's normal along w: steeper than about 84° is an edge.
const MIN_SLOPE: f64 = 0.1;
/// Tries, each with the label shifted a little more, before giving up on a degenerate case.
const ATTEMPTS: usize = 6;

pub(crate) const NOT_ON_SURFACE: &str =
    "The label's centre is not on the part's surface. Place it again with Place label.";
pub(crate) const OVER_EDGE: &str = "The label runs over an edge or a steep slope of the part. Make it smaller or move it onto a flatter spot.";
pub(crate) const PAST_EDGE: &str =
    "The label reaches past the edge of the part's surface. Make it smaller or move it.";
pub(crate) const TOO_LOW: &str =
    "The surface under the label rises above its top. Make Depth larger or move the label.";
pub(crate) const CURVES_AWAY: &str = "The surface under the label curves away more than the label is deep. Make Depth larger or the label smaller.";
pub(crate) const BLOCKED: &str =
    "Another part of the surface is in the way above the label. Move it to an open spot.";
pub(crate) const THIN: &str =
    "The part is too thin under the label for that depth. Make Depth smaller or move the label.";
pub(crate) const FAILED: &str =
    "The label could not be joined to the part cleanly here. Move it a little or change its size.";

/// An orthonormal frame: `w = u × v`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub origin: P3,
    pub u: P3,
    pub v: P3,
    pub w: P3,
}

impl Frame {
    /// The frame of a label at `point` on a surface facing `normal`: w along the normal, v "up"
    /// on the surface (world Z laid onto it, or world Y on a level surface) and u = v × w along
    /// the text, both turned `angle` radians anticlockwise about w. None for a zero normal.
    /// src/label/frame.ts repeats this to draw the label where Rust will put it.
    pub(crate) fn label(point: P3, normal: P3, angle: f64) -> Option<Frame> {
        let w = unit(normal)?;
        let up = if w[2].abs() < 0.99 {
            [0.0, 0.0, 1.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let v0 = unit(sub(up, scale(w, dot(up, w))))?;
        let u0 = cross(v0, w);
        let (s, c) = angle.sin_cos();
        let u = add(scale(u0, c), scale(v0, s));
        Some(Frame {
            origin: point,
            u,
            v: cross(w, u),
            w,
        })
    }

    pub(crate) fn local(&self, p: P3) -> P3 {
        let d = sub(p, self.origin);
        [dot(d, self.u), dot(d, self.v), dot(d, self.w)]
    }

    pub(crate) fn world(&self, q: P3) -> P3 {
        add(
            self.origin,
            add(
                scale(self.u, q[0]),
                add(scale(self.v, q[1]), scale(self.w, q[2])),
            ),
        )
    }
}

pub(crate) fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn scale(a: P3, s: f64) -> P3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn unit(a: P3) -> Option<P3> {
    let l = dot(a, a).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

/// How the label meets the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Relief {
    /// Raised to a flat top this far above the frame's plane (mm).
    Raise(f64),
    /// Sunk to a flat floor this far below it (mm, positive).
    Sink(f64),
    /// Raised to this height with the top left open, for a tag plate to close.
    Socket(f64),
}

pub(crate) struct Embossed {
    pub mesh: Mesh,
    /// For a socket, the vertices round the open top, one list per region loop, in loop order.
    pub top: Vec<Vec<u32>>,
}

enum Fail {
    /// Exactly or nearly degenerate, or spoilt by rounding (why, for tests): shift the label and
    /// try again.
    Degenerate(&'static str),
    /// A sentence for the user.
    Refused(&'static str),
}

/// `region` (mm, in the frame's u, v) put into `mesh` (world space) as `relief`. The result is a
/// new mesh; the sentence says why the label cannot go there.
pub(crate) fn emboss(
    mesh: &Mesh,
    frame: &Frame,
    region: &Region,
    relief: Relief,
    control: &mut Control,
) -> RepairResult<Result<Embossed, String>> {
    for attempt in 0..ATTEMPTS {
        let shaken;
        let region = if attempt == 0 {
            region
        } else {
            // A turn of a few ten-thousandths of a radian and a shift of a hundredth of a
            // millimetre or so: too little to see or print, enough to leave a coincidence by more
            // than Repair's shortest edge.
            let k = attempt as f64;
            let (s, c) = (3.1e-4 * k).sin_cos();
            let d = [0.011 * k * (1.7 * k).cos(), 0.013 * k * (1.7 * k).sin()];
            shaken = region.mapped(
                |p| [c * p[0] - s * p[1] + d[0], s * p[0] + c * p[1] + d[1]],
                false,
            );
            &shaken
        };
        match attempt_emboss(mesh, frame, region, relief, control)? {
            Ok(done) => return Ok(Ok(done)),
            Err(Fail::Refused(why)) => return Ok(Err(why.to_string())),
            Err(Fail::Degenerate(_why)) => {
                #[cfg(test)]
                eprintln!("label attempt {attempt}: {_why}");
                continue;
            }
        }
    }
    Ok(Err(FAILED.to_string()))
}

/// Patch triangles bucketed on a grid over the label's box.
struct Grid {
    lo: P2,
    cell: [f64; 2],
    n: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(lo: P2, hi: P2, n: usize) -> Grid {
        let n = n.clamp(1, 256);
        Grid {
            lo,
            cell: [
                ((hi[0] - lo[0]) / n as f64).max(1e-12),
                ((hi[1] - lo[1]) / n as f64).max(1e-12),
            ],
            n,
            cells: vec![Vec::new(); n * n],
        }
    }

    fn range(&self, a: f64, b: f64, k: usize) -> (usize, usize) {
        let f =
            |x: f64| (((x - self.lo[k]) / self.cell[k]).floor().max(0.0) as usize).min(self.n - 1);
        (f(a.min(b)), f(a.max(b)))
    }

    fn insert(&mut self, t: u32, lo: P2, hi: P2) {
        let (i0, i1) = self.range(lo[0], hi[0], 0);
        let (j0, j1) = self.range(lo[1], hi[1], 1);
        for j in j0..=j1 {
            for i in i0..=i1 {
                self.cells[j * self.n + i].push(t);
            }
        }
    }

    /// Every triangle in the cells the box `a`–`b` touches, each once (`stamp` marks them).
    fn near(&self, a: P2, b: P2, stamp: &mut HashMap<u32, usize>, round: usize) -> Vec<u32> {
        let (i0, i1) = self.range(a[0], b[0], 0);
        let (j0, j1) = self.range(a[1], b[1], 1);
        let mut out = Vec::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                for &t in &self.cells[j * self.n + i] {
                    if stamp.insert(t, round) != Some(round) {
                        out.push(t);
                    }
                }
            }
        }
        out
    }
}

fn cross2(a: P2, b: P2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn lerp3(a: P3, b: P3, t: f64) -> P3 {
    [0, 1, 2].map(|k| a[k] + t * (b[k] - a[k]))
}

/// A cut point on an outline side: how far along the side (0..1), the point, and the patch edge.
type SideCut = (f64, u32, (u32, u32));

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// A point the cut adds: where it is in the world and in the frame.
struct NewPoint {
    world: P3,
    local: P3,
}

fn attempt_emboss(
    mesh: &Mesh,
    frame: &Frame,
    region: &Region,
    relief: Relief,
    control: &mut Control,
) -> RepairResult<Result<Embossed, Fail>> {
    let tris: &[[u32; 3]] = mesh.indices.as_chunks::<3>().0;
    let world: Vec<P3> = (0..mesh.vertex_count() as u32)
        .map(|i| mesh.point(i))
        .collect();
    let loc: Vec<P3> = world.iter().map(|&p| frame.local(p)).collect();
    let flat = |i: u32| -> P2 { [loc[i as usize][0], loc[i as usize][1]] };
    let Some((lo, hi)) = region.bounds() else {
        return Ok(Err(Fail::Refused(FAILED)));
    };
    let pad = 1e-6 * (hi[0] - lo[0]).max(hi[1] - lo[1]) + 1e-9;
    let (lo, hi) = ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad]);

    // 1. The patch.
    let box_of = |t: &[u32; 3]| -> (P2, P2) {
        let p = t.map(flat);
        (
            [
                p.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min),
                p.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min),
            ],
            [
                p.iter().map(|q| q[0]).fold(f64::NEG_INFINITY, f64::max),
                p.iter().map(|q| q[1]).fold(f64::NEG_INFINITY, f64::max),
            ],
        )
    };
    let over = |t: &[u32; 3]| {
        let (a, b) = box_of(t);
        a[0] < hi[0] && b[0] > lo[0] && a[1] < hi[1] && b[1] > lo[1]
    };
    let candidates: Vec<u32> = (0..tris.len() as u32)
        .filter(|&t| over(&tris[t as usize]))
        .collect();
    control.check()?;
    let facing = |t: u32| {
        let [a, b, c] = tris[t as usize].map(|i| loc[i as usize]);
        let n = cross(sub(b, a), sub(c, a));
        n[2] > MIN_SLOPE * dot(n, n).sqrt()
    };
    // The triangle under the origin nearest to the tangent plane.
    let mut seed: Option<(u32, f64)> = None;
    for &t in &candidates {
        let [a, b, c] = tris[t as usize].map(flat);
        let o = [0.0, 0.0];
        if !facing(t) || orient2d(a, b, o) < 0 || orient2d(b, c, o) < 0 || orient2d(c, a, o) < 0 {
            continue;
        }
        let w = height_at(tris[t as usize].map(|i| loc[i as usize]), o).abs();
        if seed.is_none_or(|s| w < s.1) {
            seed = Some((t, w));
        }
    }
    let Some((seed, _)) = seed else {
        return Ok(Err(Fail::Refused(NOT_ON_SURFACE)));
    };
    let mut edge_tris: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &t in &candidates {
        let [a, b, c] = tris[t as usize];
        for (p, q) in [(a, b), (b, c), (c, a)] {
            edge_tris.entry(edge_key(p, q)).or_default().push(t);
        }
    }
    let mut in_patch: HashMap<u32, ()> = HashMap::new();
    let mut queue = VecDeque::from([seed]);
    in_patch.insert(seed, ());
    while let Some(t) = queue.pop_front() {
        let [a, b, c] = tris[t as usize];
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let around = &edge_tris[&edge_key(p, q)];
            if around.len() > 2 {
                return Ok(Err(Fail::Refused(FAILED)));
            }
            for &n in around {
                if in_patch.insert(n, ()).is_none() {
                    queue.push_back(n);
                }
            }
        }
    }
    let patch: Vec<u32> = candidates
        .iter()
        .copied()
        .filter(|t| in_patch.contains_key(t))
        .collect();
    if patch.iter().any(|&t| !facing(t)) {
        return Ok(Err(Fail::Refused(OVER_EDGE)));
    }
    // The other patch triangle across an edge, if any.
    let across = |t: u32, key: (u32, u32)| -> Option<u32> {
        edge_tris
            .get(&key)?
            .iter()
            .copied()
            .find(|&n| n != t && in_patch.contains_key(&n))
    };
    let mut grid = Grid::new(lo, hi, (patch.len() as f64).sqrt() as usize);
    for &t in &patch {
        let (a, b) = box_of(&tris[t as usize]);
        grid.insert(t, a, b);
    }
    let mut stamp: HashMap<u32, usize> = HashMap::new();
    let mut round = 0;
    control.check()?;

    // 2. Outline points on the patch.
    let rverts: Vec<P2> = region.loops().flatten().copied().collect();
    let mut loops: Vec<(usize, usize)> = Vec::new();
    {
        let mut start = 0;
        for l in region.loops() {
            loops.push((start, l.len()));
            start += l.len();
        }
    }
    let base = mesh.vertex_count() as u32;
    let mut points: Vec<NewPoint> = Vec::new();
    let add_point = |points: &mut Vec<NewPoint>, world: P3| -> u32 {
        points.push(NewPoint {
            world,
            local: frame.local(world),
        });
        base + points.len() as u32 - 1
    };
    let mut on_surface: Vec<(u32, u32)> = Vec::with_capacity(rverts.len());
    for &r in &rverts {
        round += 1;
        let mut found = None;
        for t in grid.near(r, r, &mut stamp, round) {
            let [a, b, c] = tris[t as usize].map(flat);
            let s = [orient2d(a, b, r), orient2d(b, c, r), orient2d(c, a, r)];
            if s.iter().all(|&x| x > 0) {
                found = Some(t);
                break;
            }
            if s.iter().all(|&x| x >= 0) {
                return Ok(Err(Fail::Degenerate("an outline point on a patch edge")));
            }
        }
        let Some(t) = found else {
            return Ok(Err(Fail::Refused(PAST_EDGE)));
        };
        let [ia, ib, ic] = tris[t as usize];
        let (a, b, c) = (flat(ia), flat(ib), flat(ic));
        let area = cross2(sub2(b, a), sub2(c, a));
        let beta = cross2(sub2(r, a), sub2(c, a)) / area;
        let gamma = cross2(sub2(b, a), sub2(r, a)) / area;
        let (wa, wb, wc) = (world[ia as usize], world[ib as usize], world[ic as usize]);
        let p = add(wa, add(scale(sub(wb, wa), beta), scale(sub(wc, wa), gamma)));
        let id = add_point(&mut points, p);
        on_surface.push((id, t));
    }
    control.check()?;

    // Cut points: by patch edge, and by outline side (with how far along it they are).
    let mut cuts_on_edge: HashMap<(u32, u32), Vec<(f64, u32)>> = HashMap::new();
    let mut seen: HashMap<((u32, u32), usize), ()> = HashMap::new();
    let side_count = rverts.len();
    // Per outline side: how far along it, the cut point, and the patch edge it is on.
    let mut cuts_on_side: Vec<Vec<SideCut>> = vec![Vec::new(); side_count];
    let next_of = |i: usize| -> usize {
        let &(start, len) = loops
            .iter()
            .find(|&&(s, l)| i >= s && i < s + l)
            .expect("vertex in a loop");
        start + (i - start + 1) % len
    };
    for i in 0..side_count {
        let j = next_of(i);
        let (ri, rj) = (rverts[i], rverts[j]);
        round += 1;
        let near = grid.near(
            [ri[0].min(rj[0]), ri[1].min(rj[1])],
            [ri[0].max(rj[0]), ri[1].max(rj[1])],
            &mut stamp,
            round,
        );
        for t in near {
            let [a, b, c] = tris[t as usize];
            for (p, q) in [(a, b), (b, c), (c, a)] {
                let key = edge_key(p, q);
                if seen.insert((key, i), ()).is_some() {
                    continue;
                }
                let (pa, pb) = (flat(key.0), flat(key.1));
                let o1 = orient2d(ri, rj, pa);
                let o2 = orient2d(ri, rj, pb);
                let o3 = orient2d(pa, pb, ri);
                let o4 = orient2d(pa, pb, rj);
                if ((o1 == 0 || o2 == 0) && o3 * o4 <= 0) || ((o3 == 0 || o4 == 0) && o1 * o2 <= 0)
                {
                    return Ok(Err(Fail::Degenerate(
                        "an outline side through a patch corner",
                    )));
                }
                if o1 * o2 >= 0 || o3 * o4 >= 0 {
                    continue;
                }
                let d = sub2(rj, ri);
                let (d1, d2) = (cross2(d, sub2(pa, ri)), cross2(d, sub2(pb, ri)));
                let e = sub2(pb, pa);
                let (d3, d4) = (cross2(e, sub2(ri, pa)), cross2(e, sub2(rj, pa)));
                let tm = d1 / (d1 - d2);
                let te = d3 / (d3 - d4);
                let id = add_point(
                    &mut points,
                    lerp3(world[key.0 as usize], world[key.1 as usize], tm),
                );
                cuts_on_edge.entry(key).or_default().push((tm, id));
                cuts_on_side[i].push((te, id, key));
            }
        }
    }
    control.check()?;

    // Each side walked through the patch: which triangle each piece of it lies in.
    let mut pieces: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    let mut inner_points: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(id, t) in &on_surface {
        inner_points.entry(t).or_default().push(id);
    }
    for (i, cuts) in cuts_on_side.iter_mut().enumerate() {
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let j = next_of(i);
        let (start, mut tri) = on_surface[i];
        let mut prev = start;
        for &(_, id, key) in cuts.iter() {
            let corners = tris[tri as usize];
            if !corners.contains(&key.0) || !corners.contains(&key.1) {
                return Ok(Err(Fail::Degenerate("cut points out of order")));
            }
            pieces.entry(tri).or_default().push((prev, id));
            let Some(next) = across(tri, key) else {
                return Ok(Err(Fail::Refused(PAST_EDGE)));
            };
            tri = next;
            prev = id;
        }
        let (end, end_tri) = on_surface[j];
        if tri != end_tri {
            return Ok(Err(Fail::Degenerate("a side ends in another triangle")));
        }
        pieces.entry(tri).or_default().push((prev, end));
    }

    // Heights of the surface under the label: at the outline, the cut points, and the patch
    // corners inside the outline.
    let mut h_lo = f64::INFINITY;
    let mut h_hi = f64::NEG_INFINITY;
    for p in &points {
        h_lo = h_lo.min(p.local[2]);
        h_hi = h_hi.max(p.local[2]);
    }
    for &t in &patch {
        for i in tris[t as usize] {
            if region.contains(flat(i)) {
                h_lo = h_lo.min(loc[i as usize][2]);
                h_hi = h_hi.max(loc[i as usize][2]);
            }
        }
    }
    match relief {
        Relief::Raise(top) | Relief::Socket(top) if top <= h_hi + MIN_GAP => {
            return Ok(Err(Fail::Refused(TOO_LOW)));
        }
        Relief::Sink(depth) if -depth >= h_lo - MIN_GAP => {
            return Ok(Err(Fail::Refused(CURVES_AWAY)));
        }
        _ => {}
    }
    // Nothing else of the part in the column between the surface and the top or floor.
    for &t in &candidates {
        if in_patch.contains_key(&t) {
            continue;
        }
        let ws = tris[t as usize].map(|i| loc[i as usize][2]);
        let (w_lo, w_hi) = (
            ws.iter().copied().fold(f64::INFINITY, f64::min),
            ws.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        );
        match relief {
            Relief::Raise(top) | Relief::Socket(top) if w_hi > h_lo - MIN_GAP && w_lo < top => {
                return Ok(Err(Fail::Refused(BLOCKED)));
            }
            Relief::Sink(depth) if w_hi > -depth - MIN_WALL && w_lo < h_hi => {
                return Ok(Err(Fail::Refused(THIN)));
            }
            _ => {}
        }
    }
    control.check()?;

    // 3. The patch, cut.
    let point_2d = |id: u32| -> P2 {
        if id < base {
            flat(id)
        } else {
            let q = points[(id - base) as usize].local;
            [q[0], q[1]]
        }
    };
    let mut out: Vec<[u32; 3]> = Vec::with_capacity(tris.len() + 4 * side_count);
    for (t, tri) in tris.iter().enumerate() {
        let t = t as u32;
        if !in_patch.contains_key(&t) {
            out.push(*tri);
            continue;
        }
        let Some(cut) = pieces.get(&t) else {
            let [a, b, c] = tri.map(flat);
            let centre = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            if !region.contains(centre) {
                out.push(*tri);
            }
            continue;
        };
        // The triangle's outline, corner to corner through the cut points on each edge.
        let mut ring: Vec<u32> = Vec::with_capacity(3);
        for (p, q) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            ring.push(p);
            if let Some(cuts) = cuts_on_edge.get(&edge_key(p, q)) {
                let mut along: Vec<(f64, u32)> = cuts.clone();
                // `tm` runs from the edge's lower vertex.
                along.sort_by(|a, b| a.0.total_cmp(&b.0));
                if p > q {
                    along.reverse();
                }
                ring.extend(along.iter().map(|c| c.1));
            }
        }
        let inner: Vec<u32> = inner_points
            .get(&t)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        match split(&ring, &inner, cut, &point_2d) {
            Ok(kept) => out.extend(kept),
            Err(why) => return Ok(Err(Fail::Degenerate(why))),
        }
    }
    control.check()?;

    // 4. Walls, and the top or floor.
    let mut lid: Vec<u32> = Vec::with_capacity(rverts.len());
    let level = match relief {
        Relief::Raise(top) | Relief::Socket(top) => top,
        Relief::Sink(depth) => -depth,
    };
    for &r in &rverts {
        let id = add_point(&mut points, frame.world([r[0], r[1], level]));
        lid.push(id);
    }
    let local_of = |points: &[NewPoint], id: u32| -> P3 {
        if id < base {
            loc[id as usize]
        } else {
            points[(id - base) as usize].local
        }
    };
    for i in 0..side_count {
        let j = next_of(i);
        let (ri, rj) = (rverts[i], rverts[j]);
        let length = sub2(rj, ri)[0].hypot(sub2(rj, ri)[1]);
        let mut chain: Vec<(f64, u32)> = vec![(0.0, on_surface[i].0)];
        chain.extend(cuts_on_side[i].iter().map(|&(te, id, _)| (te * length, id)));
        chain.push((length, on_surface[j].0));
        let mut ring: Vec<(P2, u32)> = Vec::with_capacity(chain.len() + 2);
        let sunk = matches!(relief, Relief::Sink(_));
        if sunk {
            ring.push(([0.0, level], lid[i]));
            ring.push(([length, level], lid[j]));
            for &(s, id) in chain.iter().rev() {
                ring.push(([s, local_of(&points, id)[2]], id));
            }
        } else {
            for &(s, id) in &chain {
                ring.push(([s, local_of(&points, id)[2]], id));
            }
            ring.push(([length, level], lid[j]));
            ring.push(([0.0, level], lid[i]));
        }
        let flat_ring: Vec<P2> = ring.iter().map(|r| r.0).collect();
        let Some(faces) = fill(&flat_ring) else {
            return Ok(Err(Fail::Degenerate("a wall could not be filled")));
        };
        for [a, b, c] in faces {
            let (a, b, c) = (ring[a].1, ring[b].1, ring[c].1);
            // Anticlockwise in (along the side, w) faces away from the label; a sunk label's
            // walls face into its pocket.
            out.push(if sunk { [a, c, b] } else { [a, b, c] });
        }
    }
    let mut top = Vec::new();
    match relief {
        Relief::Socket(_) => {
            for &(start, len) in &loops {
                top.push(lid[start..start + len].to_vec());
            }
        }
        _ => {
            let faces = region.triangles().map_err(|_| ()).ok();
            let Some(faces) = faces else {
                return Ok(Err(Fail::Degenerate("the top could not be filled")));
            };
            for [a, b, c] in faces {
                out.push([lid[a], lid[b], lid[c]]);
            }
        }
    }
    control.check()?;

    // The result. A cut point that fell within Repair's shortest edge of a corner (a fine mesh
    // has many corners) leaves an edge too short; Remove degenerate faces collapses those, as the
    // part library does. Then the result must have no more open edges than the part had (a
    // socket adds its open top) and nothing may cross near the label.
    let mut positions = mesh.positions.clone();
    for p in &points {
        positions.extend(p.world.map(|x| x as f32));
    }
    let (mut lo3, mut hi3) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &points {
        for k in 0..3 {
            lo3[k] = lo3[k].min(p.world[k]);
            hi3[k] = hi3[k].max(p.world[k]);
        }
    }
    let indices: Vec<u32> = out.into_iter().flatten().collect();
    let used = {
        let mut used = vec![false; positions.len() / 3];
        for &i in &indices {
            used[i as usize] = true;
        }
        used
    };
    let mut result = crate::ops::compact(&positions, indices);
    let tolerance = effective_degenerate_tolerance(&DEGENERATE);
    let top = if top.is_empty() {
        if !status::degenerate_triangles(&result, tolerance).is_empty() {
            result = degenerate::run(&result, &[], &DEGENERATE, control)?.mesh;
        }
        top
    } else {
        // `compact` keeps the vertices in order; the socket's top follows. Its points must stay
        // as they are for the plate, so a socket is shifted and tried again instead.
        let mut remap = vec![u32::MAX; positions.len() / 3];
        let mut next = 0u32;
        for (v, slot) in remap.iter_mut().enumerate() {
            if used[v] {
                *slot = next;
                next += 1;
            }
        }
        top.into_iter()
            .map(|lp| lp.into_iter().map(|i| remap[i as usize]).collect())
            .collect()
    };
    if !status::degenerate_triangles(&result, tolerance).is_empty() {
        return Ok(Err(Fail::Degenerate("a degenerate triangle")));
    }
    let socket_edges: usize = top.iter().map(|lp: &Vec<u32>| lp.len()).sum();
    if open_sides(&result) != open_sides(mesh) + socket_edges {
        return Ok(Err(Fail::Degenerate("an edge not closed")));
    }
    control.check()?;
    let tris_out: &[[u32; 3]] = result.indices.as_chunks::<3>().0;
    let near: Vec<u32> = tris_out
        .iter()
        .filter(|t| {
            let [a, b, c] = t.map(|i| result.point(i));
            (0..3).all(|k| {
                a[k].min(b[k]).min(c[k]) <= hi3[k] + 1e-3
                    && a[k].max(b[k]).max(c[k]) >= lo3[k] - 1e-3
            })
        })
        .flatten()
        .copied()
        .collect();
    let local = Mesh {
        positions: result.positions.clone(),
        indices: near,
    };
    if !tritri::intersecting_pairs(&local, control)?.is_empty() {
        return Ok(Err(Fail::Degenerate("triangles crossing")));
    }
    Ok(Ok(Embossed { mesh: result, top }))
}

/// How many triangle sides have no side running the other way to close them.
fn open_sides(mesh: &Mesh) -> usize {
    let mut directed: HashMap<(u32, u32), i32> = HashMap::new();
    for t in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *directed.entry((a, b)).or_default() += 1;
        }
    }
    directed
        .iter()
        .map(|(&(a, b), &n)| (n - directed.get(&(b, a)).copied().unwrap_or(0)).max(0) as usize)
        .sum()
}

fn sub2(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

/// The height of the plane through `t` above the point `p` of the frame's plane.
fn height_at(t: [P3; 3], p: P2) -> f64 {
    let [a, b, c] = t.map(|q| [q[0], q[1]]);
    let area = cross2(sub2(b, a), sub2(c, a));
    let beta = cross2(sub2(p, a), sub2(c, a)) / area;
    let gamma = cross2(sub2(b, a), sub2(p, a)) / area;
    t[0][2] + beta * (t[1][2] - t[0][2]) + gamma * (t[2][2] - t[0][2])
}

/// The pieces of one patch triangle outside the label: a constrained triangulation of its outline
/// `ring` (anticlockwise: its corners and the cut points on its edges), the outline points `inner`
/// inside it, and `cut`, the label outline's pieces in it, each with the label on its left. The
/// ring is constrained too: a cut point rounded a hair inside or outside the triangle still
/// closes it. Fails, saying why, when the points are degenerate.
fn split(
    ring: &[u32],
    inner: &[u32],
    cut: &[(u32, u32)],
    at: &impl Fn(u32) -> P2,
) -> Result<Vec<[u32; 3]>, &'static str> {
    #[derive(Clone, Copy, PartialEq)]
    enum Side {
        Inside,
        Outside,
        /// Outside the triangle: a sliver between its rounded outline and the hull.
        Off,
    }
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let mut handle: HashMap<u32, FixedVertexHandle> = HashMap::new();
    let mut id_of: Vec<u32> = Vec::new();
    for &id in ring.iter().chain(inner) {
        if handle.contains_key(&id) {
            continue;
        }
        let p = at(id);
        let h = cdt.insert(Point2::new(p[0], p[1])).map_err(|_| "insert")?;
        if h.index() != id_of.len() {
            return Err("two points at one place");
        }
        id_of.push(id);
        handle.insert(id, h);
    }
    let n = ring.len();
    let edges: Vec<(u32, u32, Option<Side>)> = (0..n)
        .map(|k| (ring[k], ring[(k + 1) % n], None))
        .chain(cut.iter().map(|&(a, b)| (a, b, Some(Side::Inside))))
        .collect();
    for &(a, b, _) in &edges {
        let (ha, hb) = (handle[&a], handle[&b]);
        if !cdt.can_add_constraint(ha, hb) {
            return Err("pieces cross");
        }
        cdt.add_constraint(ha, hb);
    }
    // Left of a piece is inside the label and right of it outside; right of the ring is off the
    // triangle. The rest follow their neighbours across edges that are not constrained.
    let mut side: HashMap<FixedFaceHandle<InnerTag>, Side> = HashMap::new();
    let mut queue = VecDeque::new();
    for &(a, b, piece) in &edges {
        let e = cdt
            .get_edge_from_neighbors(handle[&a], handle[&b])
            .ok_or("piece through a point")?;
        let pairs = match piece {
            Some(_) => [
                (e.face().as_inner(), Side::Inside),
                (e.rev().face().as_inner(), Side::Outside),
            ],
            None => [(None, Side::Off), (e.rev().face().as_inner(), Side::Off)],
        };
        for (face, value) in pairs {
            if let Some(f) = face {
                match side.insert(f.fix(), value) {
                    Some(old) if old != value => return Err("a face on both sides"),
                    None => queue.push_back(f.fix()),
                    _ => {}
                }
            }
        }
    }
    while let Some(f) = queue.pop_front() {
        let value = side[&f];
        for e in cdt.face(f).adjacent_edges() {
            if cdt.is_constraint_edge(e.as_undirected().fix()) {
                continue;
            }
            if let Some(nf) = e.rev().face().as_inner() {
                match side.insert(nf.fix(), value) {
                    Some(old) if old != value => return Err("a face reached from both sides"),
                    None => queue.push_back(nf.fix()),
                    _ => {}
                }
            }
        }
    }
    let mut kept = Vec::new();
    for face in cdt.inner_faces() {
        match side.get(&face.fix()) {
            Some(Side::Outside) => kept.push(face.vertices().map(|v| id_of[v.fix().index()])),
            Some(_) => {}
            None => return Err("a face not reached"),
        }
    }
    Ok(kept)
}

/// Triangles filling the simple polygon `ring` (anticlockwise), indices into it, anticlockwise.
/// None when the ring is degenerate.
pub(crate) fn fill(ring: &[P2]) -> Option<Vec<[usize; 3]>> {
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let mut handles = Vec::with_capacity(ring.len());
    for (k, p) in ring.iter().enumerate() {
        let h = cdt.insert(Point2::new(p[0], p[1])).ok()?;
        if h.index() != k {
            return None;
        }
        handles.push(h);
    }
    let n = ring.len();
    for k in 0..n {
        let (a, b) = (handles[k], handles[(k + 1) % n]);
        if !cdt.can_add_constraint(a, b) {
            return None;
        }
        cdt.add_constraint(a, b);
    }
    let mut inside: HashMap<FixedFaceHandle<InnerTag>, bool> = HashMap::new();
    let mut queue = VecDeque::new();
    for k in 0..n {
        let e = cdt.get_edge_from_neighbors(handles[k], handles[(k + 1) % n])?;
        if let Some(f) = e.face().as_inner()
            && inside.insert(f.fix(), true).is_none()
        {
            queue.push_back(f.fix());
        }
    }
    while let Some(f) = queue.pop_front() {
        for e in cdt.face(f).adjacent_edges() {
            if cdt.is_constraint_edge(e.as_undirected().fix()) {
                continue;
            }
            if let Some(nf) = e.rev().face().as_inner()
                && inside.insert(nf.fix(), true).is_none()
            {
                queue.push_back(nf.fix());
            }
        }
    }
    let faces: Vec<[usize; 3]> = cdt
        .inner_faces()
        .filter(|f| inside.contains_key(&f.fix()))
        .map(|f| f.vertices().map(|v| v.fix().index()))
        .collect();
    // A simple polygon of n points has n − 2 triangles.
    (faces.len() == n - 2).then_some(faces)
}
