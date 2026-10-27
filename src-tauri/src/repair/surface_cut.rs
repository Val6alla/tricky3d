//! Home › Repair › Cut surfaces: a line drawn on the surface through points the user clicks, and
//! the cut along it. The window picks the points (a triangle and a spot on it each) and draws the
//! line; everything that reads or changes the mesh is here.
//!
//! `route` finds the line. Between two consecutive points it follows the surface across the
//! triangles: it walks along the plane through the two points that stands on the surface (it holds
//! the mean normal of the surface at both ends), from triangle to triangle through the edges that
//! plane crosses, so the line runs straight as seen from above the surface. Then a clicked point,
//! or a crossing, within the snap distance of a vertex goes through that vertex instead. With
//! Smooth cut on, the clicked points are first joined by a smooth curve (centripetal Catmull-Rom)
//! sampled every step length, each sample laid on the nearest surface, and the walks join those
//! samples.
//!
//! `cut` cuts along that line: every point of it becomes a vertex (splitting the edge or triangle
//! it lies on), every step of it an edge (splitting the triangles it crosses), and then each side
//! of the line gets vertices of its own, so the line becomes open edges. A closed line leaves the
//! surface it encloses a shell of its own.
//!
//! Both run in f64 on the input's f32 positions; the new vertices are rounded to f32 at the end. A
//! point closer than `Surface::tiny` to a vertex is taken to be at it, so no edge comes out shorter
//! than f32 rounding could keep apart; and whatever the snap distance, a point or crossing closer
//! than `Surface::apart` to a vertex goes through it, so the cut leaves no edge shorter than
//! Remove degenerate faces' tolerance (the Status tab would count each as a degenerate face).

use std::collections::{HashMap, HashSet};

use super::bvh::{Bvh, triangle_boxes};
use super::defaults::DEGENERATE;
use super::exact::{
    P2, P3, add, collinear, cross, dominant_axis, dot, length, normal, orient2d, project, scale,
    sub, vertex,
};
use super::{Control, Origin, Outcome, RepairError, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// A clicked point: where on the surface, and the triangle the click hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutPoint {
    pub at: P3,
    pub triangle: u32,
}

/// The cut panel's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutOptions {
    /// Join the points with a smooth curve instead of straight lines on the surface.
    pub smooth: bool,
    /// mm between the smooth curve's samples.
    pub step: f64,
    /// mm; a point or crossing this close to a vertex goes through the vertex.
    pub snap: f64,
    /// Join the last point back to the first (from three points on).
    pub closed: bool,
}

/// The cut panel's defaults (src/repair/cut/store.ts shows the same).
pub const OPTIONS: CutOptions = CutOptions {
    smooth: false,
    step: 2.0,
    snap: 0.5,
    closed: false,
};

/// The shortest Smooth step length, so a smooth line never takes millions of samples.
pub const MIN_STEP: f64 = 0.01;

/// Most points one line may have.
pub const MAX_POINTS: usize = 10_000;

/// Most samples of a smooth line.
const MAX_SAMPLES: usize = 200_000;

/// Walk steps between cancel checks.
const CHECK_EVERY: usize = 4096;

/// The points and settings of one line.
#[derive(Clone, Debug, PartialEq)]
pub struct CutLine {
    pub points: Vec<CutPoint>,
    pub options: CutOptions,
}

impl CutLine {
    /// True when the line goes back to its first point.
    fn closes(&self) -> bool {
        self.options.closed && self.points.len() >= 3
    }

    /// The pieces between consecutive points, as point indices: one fewer than the points, and one
    /// more for a closed line.
    fn spans(&self) -> Vec<(usize, usize)> {
        let n = self.points.len();
        let mut spans: Vec<(usize, usize)> = (1..n).map(|i| (i - 1, i)).collect();
        if self.closes() {
            spans.push((n - 1, 0));
        }
        spans
    }
}

// ---------------------------------------------------------------------------------------------
// The surface and points on it

/// Where a point of the line lies on the mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Vertex(u32),
    /// On the edge from the lower vertex to the higher, `t` (0 < t < 1) of the way along.
    Edge(u32, u32, f64),
    /// Inside a triangle.
    Face(u32),
}

/// A point of the line: where it lies and its position.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stop {
    spot: Spot,
    at: P3,
}

/// The mesh with the triangles round each vertex, for walking across it.
struct Surface<'a> {
    mesh: &'a Mesh,
    /// The triangles round vertex v are `fan[start[v]..start[v + 1]]`.
    start: Vec<u32>,
    fan: Vec<u32>,
    /// mm: closer than this is the same point. Eight f32 steps at the mesh's largest coordinate.
    tiny: f64,
    /// mm: the largest coordinate (at least 1).
    size: f64,
    /// mm: the closest a new point comes to a vertex, the snap distance aside: Remove degenerate
    /// faces' tolerance, so the cut makes no edge it would count as degenerate.
    apart: f64,
}

impl<'a> Surface<'a> {
    fn new(mesh: &'a Mesh) -> Surface<'a> {
        let verts = mesh.vertex_count();
        let mut start = vec![0u32; verts + 1];
        for &v in &mesh.indices {
            start[v as usize + 1] += 1;
        }
        for v in 0..verts {
            start[v + 1] += start[v];
        }
        let mut fill = start[..verts].to_vec();
        let mut fan = vec![0u32; mesh.indices.len()];
        for (t, corners) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            for &v in corners {
                // A corner repeated in one triangle lists it once.
                let at = &mut fill[v as usize];
                if *at > start[v as usize] && fan[*at as usize - 1] == t as u32 {
                    continue;
                }
                fan[*at as usize] = t as u32;
                *at += 1;
            }
        }
        // Rows of vertices with repeated corners end early; close the gaps.
        let mut packed = Vec::with_capacity(fan.len());
        let mut begin = vec![0u32; verts + 1];
        for v in 0..verts {
            packed.extend_from_slice(&fan[start[v] as usize..fill[v] as usize]);
            begin[v + 1] = packed.len() as u32;
        }
        let size = mesh
            .positions
            .iter()
            .fold(1.0f64, |m, &x| m.max((x as f64).abs()));
        Surface {
            mesh,
            start: begin,
            fan: packed,
            tiny: 8.0 * f32::EPSILON as f64 * size,
            size,
            apart: DEGENERATE.tolerance.max(8.0 * f32::EPSILON as f64 * size),
        }
    }

    fn around(&self, v: u32) -> &[u32] {
        &self.fan[self.start[v as usize] as usize..self.start[v as usize + 1] as usize]
    }

    fn corners(&self, t: u32) -> [u32; 3] {
        let i = t as usize * 3;
        [
            self.mesh.indices[i],
            self.mesh.indices[i + 1],
            self.mesh.indices[i + 2],
        ]
    }

    fn point(&self, v: u32) -> P3 {
        vertex(self.mesh, v)
    }

    fn has_area(&self, t: u32) -> bool {
        let [a, b, c] = self.corners(t).map(|v| self.point(v));
        !collinear(a, b, c)
    }

    /// The triangles a point at `spot` lies in.
    fn triangles_at(&self, spot: Spot) -> Vec<u32> {
        match spot {
            Spot::Vertex(v) => self.around(v).to_vec(),
            Spot::Edge(a, b, _) => self
                .around(a)
                .iter()
                .copied()
                .filter(|&t| self.corners(t).contains(&b))
                .collect(),
            Spot::Face(t) => vec![t],
        }
    }

    fn contains(&self, t: u32, spot: Spot) -> bool {
        let c = self.corners(t);
        match spot {
            Spot::Vertex(v) => c.contains(&v),
            Spot::Edge(a, b, _) => c.contains(&a) && c.contains(&b),
            Spot::Face(f) => t == f,
        }
    }

    /// The surface's unit normal at `spot`: the area-weighted mean of the triangles it lies in, or
    /// zero where they have none.
    fn normal_at(&self, spot: Spot) -> P3 {
        let mut sum = [0.0; 3];
        for t in self.triangles_at(spot) {
            let [a, b, c] = self.corners(t).map(|v| self.point(v));
            sum = add(sum, normal(a, b, c));
        }
        unit(sum)
    }

    /// The stop on edge (a, b) at `t` from `a`: a vertex when it is within `tiny` of an end.
    fn edge_stop(&self, a: u32, b: u32, t: f64) -> Stop {
        let (lo, hi, t) = if a < b { (a, b, t) } else { (b, a, 1.0 - t) };
        let (p, q) = (self.point(lo), self.point(hi));
        let len = length(sub(q, p));
        if t * len <= self.tiny {
            return self.vertex_stop(lo);
        }
        if (1.0 - t) * len <= self.tiny {
            return self.vertex_stop(hi);
        }
        Stop {
            spot: Spot::Edge(lo, hi, t),
            at: lerp(p, q, t),
        }
    }

    fn vertex_stop(&self, v: u32) -> Stop {
        Stop {
            spot: Spot::Vertex(v),
            at: self.point(v),
        }
    }

    /// The stop for a point near triangle `t`: the nearest point of the triangle, or the nearest
    /// vertex round it within `snap` (or `apart`), or the nearest of its edges within `apart`. So a
    /// point inside a triangle lies farther than `apart` from its border, and the line's steps to
    /// and from it are no shorter than that.
    fn locate(&self, t: u32, p: P3, snap: f64) -> Stop {
        let corners = self.corners(t);
        let [a, b, c] = corners.map(|v| self.point(v));
        let q = closest_on_triangle(p, a, b, c);
        // The vertices of the triangles round its corners, nearest first.
        let reach = snap.max(self.apart);
        let mut best: Option<(f64, u32)> = None;
        for &corner in &corners {
            for &n in self.around(corner) {
                for v in self.corners(n) {
                    let d = length(sub(self.point(v), q));
                    if d <= reach && best.is_none_or(|(e, w)| d < e || (d == e && v < w)) {
                        best = Some((d, v));
                    }
                }
            }
        }
        if let Some((_, v)) = best {
            return self.vertex_stop(v);
        }
        let mut edge: Option<(f64, u32, u32, f64)> = None;
        for k in 0..3 {
            let (u, w) = (corners[k], corners[(k + 1) % 3]);
            let (pu, pw) = (self.point(u), self.point(w));
            let d = sub(pw, pu);
            let s = (dot(sub(q, pu), d) / dot(d, d)).clamp(0.0, 1.0);
            let gap = length(sub(lerp(pu, pw, s), q));
            if gap <= self.apart && edge.is_none_or(|(g, ..)| gap < g) {
                edge = Some((gap, u, w, s));
            }
        }
        let Some((_, u, w, s)) = edge else {
            return Stop {
                spot: Spot::Face(t),
                at: q,
            };
        };
        let len = length(sub(self.point(w), self.point(u)));
        if s * len <= self.apart {
            self.vertex_stop(u)
        } else if (1.0 - s) * len <= self.apart {
            self.vertex_stop(w)
        } else {
            self.edge_stop(u, w, s)
        }
    }

    /// Where the plane crosses triangle `t`'s border, other than at `from` itself: its corners on
    /// the plane and the points where it crosses an edge. A corner within `apart` of the plane is on
    /// it: a line along an edge the plane holds (up to f32 rounding) runs from corner to corner, and
    /// every edge crossing lies farther than `apart` from both ends of its edge.
    fn crossings(&self, t: u32, plane: &Plane, from: Spot) -> Vec<Stop> {
        let corners = self.corners(t);
        let side = corners.map(|v| {
            let s = plane.side(self.point(v));
            if s.abs() <= self.apart { 0.0 } else { s }
        });
        let mut out = Vec::with_capacity(2);
        for k in 0..3 {
            let v = corners[k];
            if side[k] == 0.0 {
                if from != Spot::Vertex(v) {
                    out.push(self.vertex_stop(v));
                }
                continue;
            }
            let (w, sw) = (corners[(k + 1) % 3], side[(k + 1) % 3]);
            if side[k] * sw >= 0.0 {
                continue;
            }
            let (lo, hi, slo, shi) = if v < w {
                (v, w, side[k], sw)
            } else {
                (w, v, sw, side[k])
            };
            if matches!(from, Spot::Edge(a, b, _) if (a, b) == (lo, hi)) {
                continue;
            }
            let stop = self.edge_stop(lo, hi, slo / (slo - shi));
            if stop.spot != from {
                out.push(stop);
            }
        }
        out
    }

    /// The corner of triangle `t` nearest `p`.
    fn nearest_corner(&self, t: u32, p: P3) -> u32 {
        let corners = self.corners(t);
        let gap = |v: u32| length(sub(self.point(v), p));
        corners
            .into_iter()
            .min_by(|&a, &b| gap(a).total_cmp(&gap(b)))
            .unwrap_or(corners[0])
    }

    /// A stop on an edge within `snap` of one of the edge's ends goes to that end.
    fn snapped(&self, stop: Stop, snap: f64) -> Stop {
        let Spot::Edge(lo, hi, _) = stop.spot else {
            return stop;
        };
        let (dl, dh) = (
            length(sub(self.point(lo), stop.at)),
            length(sub(self.point(hi), stop.at)),
        );
        if dl.min(dh) > snap {
            return stop;
        }
        self.vertex_stop(if dl <= dh { lo } else { hi })
    }

    /// The line on the surface from `a` to `b`: the stops after `a` up to `b`, and the triangle
    /// each step runs in. None when the walk meets an open edge, or loses its way before b is
    /// reached: nowhere left to go but where it has been, or far longer than the way straight there.
    ///
    /// The walk follows the plane exactly; then each crossing within `snap` of a vertex moves to
    /// it (`snap_path`), so every point stays within `snap` of the plane.
    fn walk(
        &self,
        a: Stop,
        b: Stop,
        snap: f64,
        control: &mut Control,
    ) -> RepairResult<Option<(Vec<Stop>, Vec<u32>)>> {
        let mut stops = Vec::new();
        let mut faces = Vec::new();
        if length(sub(b.at, a.at)) <= self.tiny {
            return Ok(Some((stops, faces)));
        }
        let up = unit(add(self.normal_at(a.spot), self.normal_at(b.spot)));
        let up = if up == [0.0; 3] {
            self.normal_at(a.spot)
        } else {
            up
        };
        let plane = Plane::through(a.at, b.at, up);
        // The vertices and edges the line has been through: it never goes back to one, so the
        // walk ends. A walk far longer than the way straight there has lost its way.
        let mut visited: HashSet<(u32, u32)> = HashSet::from([spot_key(a.spot)]);
        let limit = 20.0 * length(sub(b.at, a.at)) + 100.0 * snap.max(self.tiny);
        let mut walked = 0.0;
        let mut from: Option<u32> = None;
        let mut cur = a;
        // Which way along the plane is forward: towards b at first, then the way the last step went.
        let mut ahead = sub(b.at, a.at);
        for step in 0usize.. {
            if step % CHECK_EVERY == 0 {
                control.check()?;
            }
            let here = self.triangles_at(cur.spot);
            if let Some(&t) = here.iter().find(|&&t| self.contains(t, b.spot)) {
                stops.push(b);
                faces.push(t);
                return Ok(Some(self.snap_path(a, stops, faces, snap)));
            }
            let mut best: Option<(f64, f64, u32, Stop, bool)> = None;
            // From an edge the line goes on into the triangle across it.
            let entered = |t: &&u32| !matches!(cur.spot, Spot::Edge(..)) || Some(**t) != from;
            for &t in here.iter().filter(entered) {
                if !self.has_area(t) {
                    continue;
                }
                for crossing in self.crossings(t, &plane, cur.spot) {
                    // A crossing hardly past the point the line is at (a sliver's two edges near
                    // their shared corner): the line goes through the corner, rather than make an
                    // edge shorter than `apart`; the crossing itself when it has been there.
                    let corner = (length(sub(crossing.at, cur.at)) <= self.apart)
                        .then(|| self.vertex_stop(self.nearest_corner(t, crossing.at)));
                    let options = [corner.map(|c| (c, true)), Some((crossing, false))];
                    let Some((x, jump, d, len)) =
                        options.into_iter().flatten().find_map(|(x, jump)| {
                            let d = sub(x.at, cur.at);
                            let len = length(d);
                            (!visited.contains(&spot_key(x.spot)) && len > self.tiny)
                                .then_some((x, jump, d, len))
                        })
                    else {
                        continue;
                    };
                    let score = dot(d, ahead) / (len * length(ahead));
                    if best.is_none_or(|(s, ..)| score > s) {
                        best = Some((score, len, t, x, jump));
                    }
                }
            }
            let Some((_, len, t, x, jump)) = best else {
                return Ok(None);
            };
            walked += len;
            if walked > limit {
                return Ok(None);
            }
            visited.insert(spot_key(x.spot));
            stops.push(x);
            faces.push(t);
            // A jump to a corner runs across the plane, not along it: forward stays as it was.
            if !jump {
                ahead = sub(x.at, cur.at);
            }
            from = Some(t);
            cur = x;
        }
        unreachable!("the walk ends when every vertex and edge is visited")
    }

    /// The walk from `start` with every crossing within `snap` (or `apart`) of a vertex moved to
    /// the nearest end of its edge (the last stop, the far point, stays). Both ends of a crossed edge are
    /// corners of the triangle a step runs in, so each step still runs in its triangle. A step
    /// that snapping left without length goes, and so does a there-and-back along one edge.
    fn snap_path(
        &self,
        start: Stop,
        stops: Vec<Stop>,
        faces: Vec<u32>,
        snap: f64,
    ) -> (Vec<Stop>, Vec<u32>) {
        let n = stops.len();
        let mut out: Vec<Stop> = Vec::with_capacity(n);
        let mut through: Vec<u32> = Vec::with_capacity(n);
        for (i, (stop, face)) in stops.into_iter().zip(faces).enumerate() {
            let stop = if i + 1 < n {
                self.snapped(stop, snap.max(self.apart))
            } else {
                stop
            };
            let prev = out.last().copied().unwrap_or(start);
            if prev.spot == stop.spot {
                continue;
            }
            let before = if out.len() >= 2 {
                out[out.len() - 2]
            } else {
                start
            };
            if !out.is_empty() && before.spot == stop.spot {
                out.pop();
                through.pop();
                continue;
            }
            out.push(stop);
            through.push(face);
        }
        (out, through)
    }

    /// The point of the surface nearest `p` that faces along `up` (any point when none does), or
    /// None when the mesh has no triangle with area.
    fn nearest(&self, bvh: &Bvh, p: P3, up: P3, reach: f64) -> Option<(u32, P3)> {
        let mut r = reach.max(self.tiny);
        let far = 4.0 * self.size + reach;
        loop {
            let mut facing: Option<(f64, u32, P3)> = None;
            let mut any: Option<(f64, u32, P3)> = None;
            let query = [p.map(|x| x - r), p.map(|x| x + r)];
            bvh.query(&query, |t| {
                if !self.has_area(t) {
                    return;
                }
                let [a, b, c] = self.corners(t).map(|v| self.point(v));
                let q = closest_on_triangle(p, a, b, c);
                let d = length(sub(q, p));
                if d > r {
                    return;
                }
                let better = |slot: &Option<(f64, u32, P3)>| {
                    slot.is_none_or(|(e, u, _)| d < e || (d == e && t < u))
                };
                if better(&any) {
                    any = Some((d, t, q));
                }
                if dot(normal(a, b, c), up) > 0.0 && better(&facing) {
                    facing = Some((d, t, q));
                }
            });
            if let Some((_, t, q)) = facing.or(any) {
                return Some((t, q));
            }
            if r > far {
                return None;
            }
            r *= 2.0;
        }
    }
}

/// The vertex, edge or triangle a spot is on, as a set key.
fn spot_key(spot: Spot) -> (u32, u32) {
    match spot {
        Spot::Vertex(v) => (v, u32::MAX),
        Spot::Edge(lo, hi, _) => (lo, hi),
        Spot::Face(t) => (u32::MAX, t),
    }
}

/// A plane given by a point on it and its unit normal.
struct Plane {
    origin: P3,
    normal: P3,
}

impl Plane {
    /// The plane through `a` and `b` that holds the direction `up`; with `up` along a→b, the one
    /// that holds the axis least along a→b.
    fn through(a: P3, b: P3, up: P3) -> Plane {
        let d = sub(b, a);
        let mut n = cross(d, up);
        if length(n) <= 1e-9 * length(d) * length(up).max(1.0) {
            let k = (0..3)
                .min_by(|&i, &j| d[i].abs().total_cmp(&d[j].abs()))
                .unwrap_or(0);
            let mut axis = [0.0; 3];
            axis[k] = 1.0;
            n = cross(d, axis);
        }
        Plane {
            origin: a,
            normal: unit(n),
        }
    }

    /// Signed distance of `p` from the plane.
    fn side(&self, p: P3) -> f64 {
        dot(sub(p, self.origin), self.normal)
    }
}

fn unit(v: P3) -> P3 {
    let len = length(v);
    if len > 0.0 {
        scale(v, 1.0 / len)
    } else {
        [0.0; 3]
    }
}

fn lerp(a: P3, b: P3, t: f64) -> P3 {
    add(a, scale(sub(b, a), t))
}

/// The point of triangle abc nearest p (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_on_triangle(p: P3, a: P3, b: P3, c: P3) -> P3 {
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
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

// ---------------------------------------------------------------------------------------------
// Routing

/// The routed line: one piece per span between consecutive points, None where no way along the
/// surface was found, and where each clicked point went (snapped to a vertex, or onto the
/// surface).
struct Route {
    pieces: Vec<Option<Piece>>,
    anchors: Vec<Stop>,
    /// Point indices each piece joins.
    spans: Vec<(usize, usize)>,
}

/// The stops of one piece in order, its first point's included, and the input triangle each step
/// between two of them runs in.
struct Piece {
    stops: Vec<Stop>,
    faces: Vec<u32>,
}

/// What the window draws: each piece as positions (None for one that found no way), the clicked
/// points where they went, and the point indices each piece joins.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub pieces: Vec<Option<Vec<P3>>>,
    pub anchors: Vec<P3>,
    pub spans: Vec<(usize, usize)>,
}

/// Checks the points against `mesh`: each on a triangle it has, at a finite position.
fn check_line(mesh: &Mesh, line: &CutLine) -> RepairResult<()> {
    let fail = |text: &str| Err(RepairError::Failed(text.into()));
    if line.points.len() > MAX_POINTS {
        return fail("The cut line has too many points.");
    }
    let triangles = mesh.triangle_count();
    if line
        .points
        .iter()
        .any(|p| (p.triangle as usize) >= triangles || !p.at.iter().all(|x| x.is_finite()))
    {
        return fail(
            "A point of the cut line is not on the working copy any more. Delete the points and click them again.",
        );
    }
    let o = &line.options;
    if !(o.snap.is_finite() && o.snap >= 0.0) || !(o.step.is_finite() && o.step >= MIN_STEP) {
        return fail("The cut settings are not valid.");
    }
    Ok(())
}

/// The line through `line`'s points along the surface of `mesh`.
fn route_on(surface: &Surface, line: &CutLine, control: &mut Control) -> RepairResult<Route> {
    let snap = line.options.snap;
    let anchors: Vec<Stop> = line
        .points
        .iter()
        .map(|p| surface.locate(p.triangle, p.at, snap))
        .collect();
    let spans = line.spans();
    let mut pieces = Vec::with_capacity(spans.len());
    let samples = if line.options.smooth && !spans.is_empty() {
        Some(smooth_samples(surface, line, &anchors, control)?)
    } else {
        None
    };
    for (i, &(from, to)) in spans.iter().enumerate() {
        let mut stops = vec![anchors[from]];
        let mut faces = Vec::new();
        let ends: Vec<Stop> = match &samples {
            Some(all) => all[i].iter().copied().chain([anchors[to]]).collect(),
            None => vec![anchors[to]],
        };
        let last = ends.len() - 1;
        let mut ok = true;
        for (k, end) in ends.into_iter().enumerate() {
            let start = *stops.last().unwrap_or(&anchors[from]);
            match surface.walk(start, end, snap, control)? {
                Some((more, through)) => {
                    stops.extend(more);
                    faces.extend(through);
                }
                // A smooth curve's sample can lie nearest another shell (the curve between points
                // on a convex part runs inside it): the line goes on to the next sample instead.
                None if k < last => {}
                None => {
                    ok = false;
                    break;
                }
            }
        }
        pieces.push(ok.then_some(Piece { stops, faces }));
    }
    Ok(Route {
        pieces,
        anchors,
        spans,
    })
}

/// The inner samples of the smooth curve through the points, per span, each laid on the surface.
fn smooth_samples(
    surface: &Surface,
    line: &CutLine,
    anchors: &[Stop],
    control: &mut Control,
) -> RepairResult<Vec<Vec<Stop>>> {
    let o = &line.options;
    let n = anchors.len();
    let closed = line.closes();
    let at = |i: isize| -> P3 {
        if closed {
            anchors[i.rem_euclid(n as isize) as usize].at
        } else if i < 0 {
            // Mirrored ends, so the curve leaves the first point and reaches the last one straight.
            sub(scale(anchors[0].at, 2.0), anchors[1.min(n - 1)].at)
        } else if i as usize >= n {
            sub(
                scale(anchors[n - 1].at, 2.0),
                anchors[n.saturating_sub(2)].at,
            )
        } else {
            anchors[i as usize].at
        }
    };
    let spans = line.spans();
    let total: f64 = spans
        .iter()
        .map(|&(a, b)| length(sub(anchors[b].at, anchors[a].at)) / o.step)
        .sum();
    if total > MAX_SAMPLES as f64 {
        return Err(RepairError::Failed(
            "The smooth step length is too short for a line this long.".into(),
        ));
    }
    let boxes = triangle_boxes(surface.mesh);
    let bvh = Bvh::new(&boxes);
    control.check()?;
    let mut out = Vec::with_capacity(spans.len());
    for &(a, b) in &spans {
        let i = a as isize;
        let (p0, p1, p2, p3) = (at(i - 1), anchors[a].at, anchors[b].at, at(i + 2));
        let (n1, n2) = (
            surface.normal_at(anchors[a].spot),
            surface.normal_at(anchors[b].spot),
        );
        let chord = length(sub(p2, p1));
        let count = (chord / o.step).ceil() as usize;
        let mut samples = Vec::with_capacity(count.saturating_sub(1));
        for k in 1..count {
            let f = k as f64 / count as f64;
            let p = catmull_rom(p0, p1, p2, p3, f);
            let up = unit(add(scale(n1, 1.0 - f), scale(n2, f)));
            if let Some((t, q)) = surface.nearest(&bvh, p, up, o.step) {
                samples.push(surface.locate(t, q, o.snap));
            }
        }
        out.push(samples);
    }
    Ok(out)
}

/// The centripetal Catmull-Rom curve from p1 to p2 (p0 and p3 their neighbours) at `f` (0..1) of
/// the way in its own parameter (Barry-Goldman).
fn catmull_rom(p0: P3, p1: P3, p2: P3, p3: P3, f: f64) -> P3 {
    // Knot spacing: the square root of each chord, kept above zero for repeated points.
    let knot = |a: P3, b: P3| length(sub(b, a)).sqrt().max(1e-9);
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let t = t1 + (t2 - t1) * f;
    let mix = |a: P3, b: P3, ta: f64, tb: f64| {
        add(
            scale(a, (tb - t) / (tb - ta)),
            scale(b, (t - ta) / (tb - ta)),
        )
    };
    let a1 = mix(p0, p1, t0, t1);
    let a2 = mix(p1, p2, t1, t2);
    let a3 = mix(p2, p3, t2, t3);
    let b1 = mix(a1, a2, t0, t2);
    let b2 = mix(a2, a3, t1, t3);
    mix(b1, b2, t1, t2)
}

/// The line through the points along the surface, for the window to draw.
pub fn preview(mesh: &Mesh, line: &CutLine, control: &mut Control) -> RepairResult<Preview> {
    control.check()?;
    check_line(mesh, line)?;
    let surface = Surface::new(mesh);
    let route = route_on(&surface, line, control)?;
    Ok(Preview {
        pieces: route
            .pieces
            .iter()
            .map(|p| p.as_ref().map(|p| p.stops.iter().map(|s| s.at).collect()))
            .collect(),
        anchors: route.anchors.iter().map(|s| s.at).collect(),
        spans: route.spans,
    })
}

// ---------------------------------------------------------------------------------------------
// Cutting

/// The mesh being cut: the input's triangles, split as the line goes in.
struct Work<'a> {
    surface: &'a Surface<'a>,
    pos: Vec<P3>,
    tris: Vec<[u32; 3]>,
    /// The input triangle each triangle is part of.
    origin: Vec<u32>,
    /// Triangles round the vertices whose fan changed (new vertices always).
    fans: HashMap<u32, Vec<u32>>,
    /// The triangles each split input triangle became.
    subs: HashMap<u32, Vec<u32>>,
    /// The new vertices on each input edge (lower, higher vertex), by how far along they are.
    chains: HashMap<(u32, u32), Vec<(f64, u32)>>,
    /// The input edge a new vertex lies on, and how far along.
    on_edge: HashMap<u32, (u32, u32, f64)>,
    /// The edges along the line, as (lower, higher) vertex pairs.
    cut: HashSet<(u32, u32)>,
    /// Each input triangle's plane view, on first use: the axis it is seen along and whether that
    /// view turns it clockwise.
    views: HashMap<u32, (usize, bool)>,
}

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// 2D cross product of b − a and c − a.
fn cross2(a: P2, b: P2, c: P2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

impl<'a> Work<'a> {
    fn new(surface: &'a Surface<'a>) -> Work<'a> {
        let mesh = surface.mesh;
        Work {
            surface,
            pos: (0..mesh.vertex_count() as u32)
                .map(|v| vertex(mesh, v))
                .collect(),
            tris: mesh.indices.as_chunks::<3>().0.to_vec(),
            origin: (0..mesh.triangle_count() as u32).collect(),
            fans: HashMap::new(),
            subs: HashMap::new(),
            chains: HashMap::new(),
            on_edge: HashMap::new(),
            cut: HashSet::new(),
            views: HashMap::new(),
        }
    }

    fn fan(&self, v: u32) -> &[u32] {
        match self.fans.get(&v) {
            Some(list) => list,
            None => self.surface.around(v),
        }
    }

    fn fan_mut(&mut self, v: u32) -> &mut Vec<u32> {
        let surface = self.surface;
        self.fans
            .entry(v)
            .or_insert_with(|| surface.around(v).to_vec())
    }

    fn new_vertex(&mut self, p: P3) -> u32 {
        let v = self.pos.len() as u32;
        self.pos.push(p);
        self.fans.insert(v, Vec::new());
        v
    }

    /// Replaces corner `old` of triangle `t` with `new`.
    fn replace_corner(&mut self, t: u32, old: u32, new: u32) {
        let tri = &mut self.tris[t as usize];
        for c in tri.iter_mut().filter(|c| **c == old) {
            *c = new;
        }
        self.fan_mut(old).retain(|&x| x != t);
        self.fan_mut(new).push(t);
    }

    fn add_triangle(&mut self, corners: [u32; 3], origin: u32) {
        let t = self.tris.len() as u32;
        self.tris.push(corners);
        self.origin.push(origin);
        for c in corners {
            self.fan_mut(c).push(t);
        }
        self.subs
            .entry(origin)
            .or_insert_with(|| vec![origin])
            .push(t);
    }

    fn subs_of(&self, f: u32) -> Vec<u32> {
        self.subs.get(&f).cloned().unwrap_or_else(|| vec![f])
    }

    /// Where vertex `x` lies along input edge (lo, hi): 0, 1, or a new vertex's place on it.
    fn along(&self, x: u32, lo: u32, hi: u32) -> Option<f64> {
        if x == lo {
            Some(0.0)
        } else if x == hi {
            Some(1.0)
        } else {
            match self.on_edge.get(&x) {
                Some(&(a, b, t)) if (a, b) == (lo, hi) => Some(t),
                _ => None,
            }
        }
    }

    /// The input edge the edge (a, b) is part of, with where a and b lie along it.
    fn input_edge(&self, a: u32, b: u32) -> Option<(u32, u32, f64, f64)> {
        let original = self.surface.mesh.vertex_count() as u32;
        let (lo, hi) = match (self.on_edge.get(&a), self.on_edge.get(&b)) {
            (Some(&(lo, hi, _)), _) | (None, Some(&(lo, hi, _))) => (lo, hi),
            // Two input vertices are joined only by an input edge: the cut's own edges all end
            // at a new vertex.
            (None, None) if a < original && b < original => key(a, b),
            _ => return None,
        };
        Some((lo, hi, self.along(a, lo, hi)?, self.along(b, lo, hi)?))
    }

    /// Splits edge (a, b) at `s` of the way from `a`, and every triangle on it. The new vertex.
    fn split_edge(&mut self, a: u32, b: u32, s: f64) -> u32 {
        let input = self.input_edge(a, b);
        let p = match input {
            // On an input edge the point is placed as the route places it, from the edge's ends.
            Some((lo, hi, ta, tb)) => lerp(
                self.pos[lo as usize],
                self.pos[hi as usize],
                ta + s * (tb - ta),
            ),
            None => lerp(self.pos[a as usize], self.pos[b as usize], s),
        };
        let w = self.new_vertex(p);
        let on: Vec<u32> = self
            .fan(a)
            .iter()
            .copied()
            .filter(|&t| self.tris[t as usize].contains(&b))
            .collect();
        for t in on {
            let tri = self.tris[t as usize];
            let Some(k) = (0..3).find(|&k| key(tri[k], tri[(k + 1) % 3]) == key(a, b)) else {
                continue;
            };
            let (end, other) = (tri[(k + 1) % 3], tri[(k + 2) % 3]);
            // (start, end, other) becomes (start, w, other) and (w, end, other): same winding.
            self.replace_corner(t, end, w);
            let origin = self.origin[t as usize];
            self.subs.entry(origin).or_insert_with(|| vec![origin]);
            self.add_triangle([w, end, other], origin);
        }
        if self.cut.remove(&key(a, b)) {
            self.cut.insert(key(a, w));
            self.cut.insert(key(w, b));
        }
        if let Some((lo, hi, ta, tb)) = input {
            let t = ta + s * (tb - ta);
            self.on_edge.insert(w, (lo, hi, t));
            let chain = self.chains.entry((lo, hi)).or_default();
            let at = chain.partition_point(|&(u, _)| u < t);
            chain.insert(at, (t, w));
        }
        w
    }

    /// Splits triangle `t` at `p` into three. The new vertex.
    fn split_face(&mut self, t: u32, p: P3) -> u32 {
        let w = self.new_vertex(p);
        let [a, b, c] = self.tris[t as usize];
        let origin = self.origin[t as usize];
        self.subs.entry(origin).or_insert_with(|| vec![origin]);
        self.replace_corner(t, c, w);
        self.add_triangle([b, c, w], origin);
        self.add_triangle([c, a, w], origin);
        w
    }

    /// Input triangle `f`'s view: `p` seen along the axis that keeps its area, mirrored where
    /// needed so that `f` turns counter-clockwise. None for a triangle with no area.
    fn flat(&mut self, f: u32, p: P3) -> Option<P2> {
        let (axis, mirrored) = match self.views.get(&f) {
            Some(&view) => view,
            None => {
                let [a, b, c] = self.surface.corners(f).map(|v| self.surface.point(v));
                let axis = dominant_axis(a, b, c)?;
                let turn = orient2d(project(a, axis), project(b, axis), project(c, axis));
                let view = (axis, turn < 0);
                self.views.insert(f, view);
                view
            }
        };
        let q = project(p, axis);
        Some(if mirrored { [q[1], q[0]] } else { q })
    }

    /// The vertex for a stop of the line, splitting the edge or triangle it lies on.
    fn insert(&mut self, stop: Stop) -> RepairResult<u32> {
        let (tiny, apart) = (self.surface.tiny, self.surface.apart);
        match stop.spot {
            Spot::Vertex(v) => Ok(v),
            Spot::Edge(lo, hi, t) => {
                let len = length(sub(self.pos[hi as usize], self.pos[lo as usize]));
                let chain = self.chains.get(&(lo, hi)).cloned().unwrap_or_default();
                if let Some(&(_, v)) = chain.iter().find(|&&(u, _)| (u - t).abs() * len <= apart) {
                    return Ok(v);
                }
                let at = chain.partition_point(|&(u, _)| u < t);
                let (t0, v0) = if at == 0 { (0.0, lo) } else { chain[at - 1] };
                let (t1, v1) = chain.get(at).copied().unwrap_or((1.0, hi));
                Ok(self.split_edge(v0, v1, (t - t0) / (t1 - t0)))
            }
            Spot::Face(f) => {
                let p = self.flat(f, stop.at).ok_or_else(no_area)?;
                // The piece of f that holds the point best: its smallest corner weight largest.
                let mut best: Option<(f64, u32)> = None;
                for t in self.subs_of(f) {
                    let c = self.tris[t as usize];
                    let [a, b, d] = [0, 1, 2].map(|k| self.pos[c[k] as usize]);
                    let (Some(a), Some(b), Some(d)) =
                        (self.flat(f, a), self.flat(f, b), self.flat(f, d))
                    else {
                        continue;
                    };
                    let area = cross2(a, b, d);
                    if area <= 0.0 {
                        continue;
                    }
                    let weight = cross2(p, b, d).min(cross2(a, p, d)).min(cross2(a, b, p)) / area;
                    if best.is_none_or(|(w, _)| weight > w) {
                        best = Some((weight, t));
                    }
                }
                let (_, t) = best.ok_or_else(no_area)?;
                let corners = self.tris[t as usize];
                if let Some(&v) = corners
                    .iter()
                    .find(|&&v| length(sub(self.pos[v as usize], stop.at)) <= apart)
                {
                    return Ok(v);
                }
                for k in 0..3 {
                    let (u, w) = (corners[k], corners[(k + 1) % 3]);
                    let (pu, pw) = (self.pos[u as usize], self.pos[w as usize]);
                    let d = sub(pw, pu);
                    let s = (dot(sub(stop.at, pu), d) / dot(d, d)).clamp(0.0, 1.0);
                    if length(sub(lerp(pu, pw, s), stop.at)) <= tiny {
                        return Ok(self.split_edge(u, w, s));
                    }
                }
                Ok(self.split_face(t, stop.at))
            }
        }
    }

    /// Joins vertices `u` and `w` (both in input triangle `f`) with edges along the straight line
    /// between them, splitting the pieces of `f` it crosses, and marks those edges as cut.
    fn connect(&mut self, mut u: u32, w: u32, f: u32) -> RepairResult<()> {
        let apart = self.surface.apart;
        let target = self.flat(f, self.pos[w as usize]).ok_or_else(no_area)?;
        for _ in 0..1_000_000 {
            if u == w {
                return Ok(());
            }
            if self
                .fan(u)
                .iter()
                .any(|&t| self.tris[t as usize].contains(&w))
            {
                self.cut.insert(key(u, w));
                return Ok(());
            }
            let pu = self.flat(f, self.pos[u as usize]).ok_or_else(no_area)?;
            let pieces: Vec<u32> = self
                .fan(u)
                .iter()
                .copied()
                .filter(|&t| self.origin[t as usize] == f)
                .collect();
            // Some(Some(x)): on to vertex x; Some(None): an edge flipped, look again from u.
            let mut next = None;
            for t in pieces {
                let tri = self.tris[t as usize];
                let Some(k) = tri.iter().position(|&c| c == u) else {
                    continue;
                };
                let (p, q) = (tri[(k + 1) % 3], tri[(k + 2) % 3]);
                let pp = self.flat(f, self.pos[p as usize]).ok_or_else(no_area)?;
                let pq = self.flat(f, self.pos[q as usize]).ok_or_else(no_area)?;
                let left = orient2d(pu, pp, target);
                let right = orient2d(pu, target, pq);
                if left < 0 || right < 0 {
                    continue;
                }
                let ahead = |x: P2| {
                    (x[0] - pu[0]) * (target[0] - pu[0]) + (x[1] - pu[1]) * (target[1] - pu[1])
                        > 0.0
                };
                if left == 0 && ahead(pp) {
                    next = Some(Some(p));
                    break;
                }
                if right == 0 && ahead(pq) {
                    next = Some(Some(q));
                    break;
                }
                if left == 0 || right == 0 {
                    continue;
                }
                // Across the far edge p–q. An edge inside f that is no edge of the line turns
                // into u–r when the two triangles on it make a convex quadrilateral: the line gets
                // its edge with no new point, so none lands a hair from a point of another piece
                // of the line. Otherwise the edge is split where the line meets it.
                if let Some((t2, r)) = self.across(t, p, q, f)
                    && !self.cut.contains(&key(p, q))
                {
                    let pr = self.flat(f, self.pos[r as usize]).ok_or_else(no_area)?;
                    if orient2d(pu, pr, pp) < 0 && orient2d(pu, pr, pq) > 0 {
                        // (u, p, q) and (q, p, r) become (u, p, r) and (q, u, r), turning as f does.
                        self.replace_corner(t, q, r);
                        self.replace_corner(t2, p, u);
                        next = Some(None);
                        break;
                    }
                }
                let (dl, dr) = (cross2(pu, pp, target).abs(), cross2(pu, target, pq).abs());
                let s = if dl + dr > 0.0 { dl / (dl + dr) } else { 0.5 };
                let len = length(sub(self.pos[q as usize], self.pos[p as usize]));
                next = Some(Some(if s * len <= apart {
                    p
                } else if (1.0 - s) * len <= apart {
                    q
                } else {
                    self.split_edge(p, q, s)
                }));
                break;
            }
            match next {
                None => {
                    return Err(RepairError::Failed(
                        "Could not cut along the line: it runs through triangles too thin to split. Run Remove degenerate faces first."
                            .into(),
                    ));
                }
                // Flipped: the line's next edge from u is there now, or the next edge to cross.
                Some(None) => {}
                Some(Some(x)) => {
                    self.cut.insert(key(u, x));
                    u = x;
                }
            }
        }
        Err(RepairError::Failed("Could not cut along the line.".into()))
    }

    /// The other piece of input triangle `f` on the edge (p, q) of piece `t`, and its corner
    /// across that edge; None on `f`'s own border.
    fn across(&self, t: u32, p: u32, q: u32, f: u32) -> Option<(u32, u32)> {
        let other = self.fan(p).iter().copied().find(|&s| {
            s != t && self.origin[s as usize] == f && self.tris[s as usize].contains(&q)
        })?;
        let r = self.tris[other as usize]
            .into_iter()
            .find(|&c| c != p && c != q)?;
        Some((other, r))
    }

    /// Gives each side of the cut edges vertices of their own: round every vertex on the line,
    /// the triangles joined through edges that are not cut keep one vertex, and each other such
    /// group gets a copy. The number of copies made.
    fn open(&mut self) -> usize {
        let mut touched: Vec<u32> = self.cut.iter().flat_map(|&(a, b)| [a, b]).collect();
        touched.sort_unstable();
        touched.dedup();
        let mut renames: Vec<(u32, u32, usize)> = Vec::new();
        let mut copies = 0;
        for &v in &touched {
            let mut around = self.fan(v).to_vec();
            around.sort_unstable();
            if around.len() < 2 {
                continue;
            }
            let mut parent: Vec<usize> = (0..around.len()).collect();
            let mut by_other: HashMap<u32, Vec<usize>> = HashMap::new();
            for (i, &t) in around.iter().enumerate() {
                for c in self.tris[t as usize] {
                    if c != v {
                        by_other.entry(c).or_default().push(i);
                    }
                }
            }
            for (x, list) in &by_other {
                if self.cut.contains(&key(v, *x)) {
                    continue;
                }
                for &i in &list[1..] {
                    let (a, b) = (find(&mut parent, list[0]), find(&mut parent, i));
                    if a != b {
                        parent[a.max(b)] = a.min(b);
                    }
                }
            }
            // The group of the lowest triangle keeps v; the others get one copy each.
            let mut group_copy: HashMap<usize, usize> = HashMap::new();
            for (i, &t) in around.iter().enumerate() {
                let root = find(&mut parent, i);
                if root == 0 {
                    continue;
                }
                let n = group_copy.len();
                let copy = *group_copy.entry(root).or_insert(n);
                renames.push((t, v, copies + copy));
            }
            copies += group_copy.len();
        }
        let base = self.pos.len();
        let mut sources = vec![0u32; copies];
        for &(_, v, copy) in &renames {
            sources[copy] = v;
        }
        for &v in &sources {
            let p = self.pos[v as usize];
            self.pos.push(p);
        }
        for (t, v, copy) in renames {
            for c in self.tris[t as usize].iter_mut().filter(|c| **c == v) {
                *c = (base + copy) as u32;
            }
        }
        copies
    }
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn no_area() -> RepairError {
    RepairError::Failed(
        "Could not cut along the line: it runs through a triangle with no area. Run Remove degenerate faces first."
            .into(),
    )
}

/// Cuts the surface open along the line through `line`'s points. Fails when a piece of the line
/// found no way along the surface (the window shows which), changes nothing for fewer than two
/// points. Leaves nothing selected.
pub fn cut(mesh: &Mesh, line: &CutLine, control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    check_line(mesh, line)?;
    if line.points.len() < 2 {
        return Ok(Outcome::unchanged(
            mesh,
            "Click at least two points on the surface to cut along.",
        ));
    }
    control.stage("Following the surface");
    let surface = Surface::new(mesh);
    let route = route_on(&surface, line, control)?;
    let mut stops: Vec<Stop> = Vec::new();
    let mut faces: Vec<u32> = Vec::new();
    for (piece, &(from, to)) in route.pieces.iter().zip(&route.spans) {
        let Some(piece) = piece else {
            return Err(RepairError::Failed(format!(
                "The line could not follow the surface from point {} to point {}. Move or delete one of them.",
                from + 1,
                to + 1
            )));
        };
        let skip = usize::from(!stops.is_empty());
        stops.extend(&piece.stops[skip..]);
        faces.extend(&piece.faces);
    }
    if faces.is_empty() {
        return Ok(Outcome::unchanged(mesh, "The cut line has no length."));
    }
    control.progress(0.3);
    control.stage("Cutting");
    let mut work = Work::new(&surface);
    let mut verts = Vec::with_capacity(stops.len());
    for (i, &stop) in stops.iter().enumerate() {
        if i % CHECK_EVERY == 0 {
            control.check()?;
        }
        verts.push(work.insert(stop)?);
    }
    control.progress(0.6);
    for (i, &f) in faces.iter().enumerate() {
        if i % CHECK_EVERY == 0 {
            control.check()?;
        }
        work.connect(verts[i], verts[i + 1], f)?;
    }
    let edges = work.cut.len();
    control.check()?;
    let copies = work.open();
    control.progress(0.9);
    let added = work.tris.len() - mesh.triangle_count();
    let out = Mesh {
        positions: work.pos.iter().flat_map(|p| p.map(|x| x as f32)).collect(),
        indices: work.tris.iter().flatten().copied().collect(),
    };
    control.progress(1.0);
    let unit = if edges == 1 { "edge" } else { "edges" };
    let mut message = format!(
        "Cut the surface open along {} {unit}, adding {} {}.",
        fmt_count(edges as u64),
        fmt_count(added as u64),
        if added == 1 { "triangle" } else { "triangles" }
    );
    let mut warnings = Vec::new();
    if copies == 0 {
        warnings.push(
            "The line is too short to open the surface: it only split triangles.".to_string(),
        );
    } else if line.closes() {
        message.push_str(" What the line encloses is a shell of its own now.");
    }
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(work.origin),
        message,
        warnings,
        concerns: Vec::new(),
        highlight: None,
        select: Some(Vec::new()),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::status;
    use crate::repair::topology::{Edges, Link, components};
    use crate::repair::{refine, silent};

    /// A flat plate in z = 0 from (0, 0) to (size, size), n × n quads, each split along a diagonal.
    fn plate(n: usize, size: f32) -> Mesh {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                positions.extend_from_slice(&[
                    size * i as f32 / n as f32,
                    size * j as f32 / n as f32,
                    0.0,
                ]);
            }
        }
        let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        let mut indices = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        Mesh { positions, indices }
    }

    /// A 20 mm cube with every face split into 4^levels pieces.
    fn fine_cube(levels: usize) -> Mesh {
        let mut m = make_box([0.0; 3], [20.0; 3]);
        for _ in 0..levels {
            m = silent(|c| refine::subdivide(&m, &[], c)).unwrap().mesh;
        }
        m
    }

    /// The triangle of `mesh` a ray straight down (or up, from below) at (x, y) hits first, and the point.
    fn hit(mesh: &Mesh, x: f64, y: f64, top: bool) -> CutPoint {
        let mut best: Option<(f64, u32)> = None;
        for t in 0..mesh.triangle_count() as u32 {
            let [a, b, c] = [0, 1, 2].map(|k| vertex(mesh, mesh.indices[t as usize * 3 + k]));
            let p = [x, y, 0.0];
            let flat = |q: P3| [q[0], q[1]];
            let (fa, fb, fc, fp) = (flat(a), flat(b), flat(c), flat(p));
            let area = cross2(fa, fb, fc);
            if area == 0.0 {
                continue;
            }
            let (u, v, w) = (
                cross2(fp, fb, fc) / area,
                cross2(fa, fp, fc) / area,
                cross2(fa, fb, fp) / area,
            );
            if u < 0.0 || v < 0.0 || w < 0.0 {
                continue;
            }
            let z = u * a[2] + v * b[2] + w * c[2];
            let key = if top { -z } else { z };
            if best.is_none_or(|(k, _)| key < k) {
                best = Some((key, t));
            }
        }
        let (key, t) = best.expect("a triangle under the point");
        CutPoint {
            at: [x, y, if top { -key } else { key }],
            triangle: t,
        }
    }

    fn line(points: Vec<CutPoint>, smooth: bool, snap: f64, closed: bool) -> CutLine {
        CutLine {
            points,
            options: CutOptions {
                smooth,
                step: 2.0,
                snap,
                closed,
            },
        }
    }

    /// Every triangle has area, every edge has one or two sides, the mesh checks out, and the cut
    /// made no face that Remove degenerate faces would count.
    fn assert_sound(input: &Mesh, m: &Mesh) {
        m.check().unwrap();
        let degenerate = |m: &Mesh| status::degenerate_triangles(m, DEGENERATE.tolerance).len();
        assert!(
            degenerate(m) <= degenerate(input),
            "{} degenerate faces, {} before",
            degenerate(m),
            degenerate(input)
        );
        for t in 0..m.triangle_count() as u32 {
            let [a, b, c] = [0, 1, 2].map(|k| vertex(m, m.indices[t as usize * 3 + k]));
            assert!(!collinear(a, b, c), "triangle {t} has no area");
        }
        let s = stats(m);
        assert_eq!(s.non_manifold_edges, 0);
    }

    fn open_edges(m: &Mesh) -> usize {
        let edges = Edges::new(m);
        (0..edges.len() as u32)
            .filter(|&e| edges.degree(e) == 1)
            .count()
    }

    #[test]
    fn a_straight_line_crosses_a_plate_on_the_plane_between_its_points() {
        let m = plate(10, 10.0);
        let l = line(
            vec![hit(&m, 0.35, 2.15, true), hit(&m, 9.45, 7.35, true)],
            false,
            0.0,
            false,
        );
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        assert_eq!(p.spans, vec![(0, 1)]);
        let piece = p.pieces[0].as_ref().unwrap();
        assert!(piece.len() > 10, "{}", piece.len());
        // Every point lies on the straight line between the ends (the plate is flat).
        let (a, b) = (piece[0], piece[piece.len() - 1]);
        let d = unit(sub(b, a));
        for q in piece {
            let off = sub(*q, a);
            let along = scale(d, dot(off, d));
            assert!(length(sub(off, along)) < 1e-9, "{q:?}");
            assert_eq!(q[2], 0.0);
        }
        assert!(
            length(sub(p.anchors[0], [0.35, 2.15, 0.0])) < 1e-12,
            "{:?}",
            p.anchors[0]
        );
    }

    #[test]
    fn points_and_crossings_snap_to_vertices_within_the_snap_distance() {
        let m = plate(10, 10.0);
        let l = line(
            vec![hit(&m, 1.1, 1.05, true), hit(&m, 8.95, 1.1, true)],
            false,
            0.3,
            false,
        );
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        assert_eq!(p.anchors[0], [1.0, 1.0, 0.0]);
        assert_eq!(p.anchors[1], [9.0, 1.0, 0.0]);
        // Along the row of vertices at y = 1, through every one of them.
        let piece = p.pieces[0].as_ref().unwrap();
        assert!(
            piece.iter().all(|q| (q[1] - 1.0).abs() < 1e-12),
            "{piece:?}"
        );
        assert_eq!(piece.len(), 9);
    }

    #[test]
    fn a_snap_distance_longer_than_the_edges_walks_from_vertex_to_vertex() {
        // 0.1 mm triangles, 0.5 mm snap: every crossing goes to a vertex.
        let m = plate(100, 10.0);
        let l = line(
            vec![hit(&m, 0.53, 1.27, true), hit(&m, 9.41, 8.66, true)],
            false,
            0.5,
            false,
        );
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        let piece = p.pieces[0].as_ref().expect("a way along the plate");
        // Within the snap distance of the straight line all the way, and never back.
        let (a, b) = (piece[0], piece[piece.len() - 1]);
        let d = unit(sub(b, a));
        let mut last = -1.0;
        for q in piece {
            let off = sub(*q, a);
            let along = dot(off, d);
            assert!(length(sub(off, scale(d, along))) <= 0.5, "{q:?}");
            assert!(along > last, "{q:?}");
            last = along;
        }
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn a_line_past_a_corner_of_a_sliver_goes_through_the_corner() {
        // A fan round V = 0 with a sliver V-P-Q (an angle of 1e-3 at V). The line x = 1e-4 crosses V-P and
        // V-Q 1e-4 from V (farther than `tiny`), and those two crossings only 1e-7 apart (nearer than it):
        // it goes through V instead of stopping there.
        let m = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, // V
                1.0, 0.0, 0.0, // P
                1.0, 0.001, 0.0, // Q
                0.5, 1.0, 0.0, // R
                -1.0, 0.0, 0.0, // L
                0.5, -1.0, 0.0, // S
            ],
            indices: vec![0, 5, 1, 0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 5],
        };
        let at = |x: f64, y: f64, t: u32| CutPoint {
            at: [x, y, 0.0],
            triangle: t,
        };
        let l = line(vec![at(1e-4, -0.5, 4), at(1e-4, 0.5, 3)], false, 0.0, false);
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        let piece = p.pieces[0].as_ref().expect("a way past the sliver");
        assert!(piece.contains(&[0.0, 0.0, 0.0]), "{piece:?}");
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
    }

    #[test]
    fn a_line_a_hair_from_vertices_goes_through_them_at_snap_0() {
        // y = 3.001 crosses every edge 0.001 mm from a vertex of the row y = 3: an edge that short
        // would be a degenerate face, so the line runs along the row instead.
        let m = plate(10, 10.0);
        let l = line(
            vec![hit(&m, 0.5, 3.001, true), hit(&m, 9.5, 3.001, true)],
            false,
            0.0,
            false,
        );
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        let piece = p.pieces[0].as_ref().unwrap();
        assert!(
            piece[1..piece.len() - 1].iter().all(|q| q[1] == 3.0),
            "{piece:?}"
        );
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
    }

    #[test]
    fn an_open_line_cuts_a_slit_into_a_plate() {
        let m = plate(10, 10.0);
        let before = open_edges(&m);
        let l = line(
            vec![
                hit(&m, 2.3, 3.4, true),
                hit(&m, 5.15, 6.7, true),
                hit(&m, 7.6, 3.3, true),
            ],
            false,
            0.0,
            false,
        );
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.select, Some(Vec::new()));
        assert_sound(&m, &out.mesh);
        let edges: usize = out
            .message
            .split(' ')
            .nth(5)
            .unwrap()
            .replace(',', "")
            .parse()
            .unwrap();
        // Each edge of the line is now two open edges.
        assert_eq!(open_edges(&out.mesh), before + 2 * edges, "{}", out.message);
        // The area stays; the plate is still one piece (the slit ends inside it).
        assert!((stats(&out.mesh).area - 100.0).abs() < 1e-3);
        let e = Edges::new(&out.mesh);
        assert_eq!(components(&out.mesh, &e, Link::Edge).1, 1);
        let Origin::Map(map) = &out.origin else {
            panic!("an origin map")
        };
        assert_eq!(map.len(), out.mesh.triangle_count());
        assert!(map.iter().all(|&t| (t as usize) < m.triangle_count()));
    }

    #[test]
    fn a_closed_line_on_a_cube_makes_what_it_encloses_a_shell_of_its_own() {
        let m = fine_cube(3);
        let l = line(
            vec![
                hit(&m, 4.3, 3.7, true),
                hit(&m, 15.1, 4.2, true),
                hit(&m, 16.2, 14.9, true),
                hit(&m, 5.4, 15.8, true),
            ],
            false,
            0.0,
            true,
        );
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
        assert!(
            out.message
                .ends_with("What the line encloses is a shell of its own now."),
            "{}",
            out.message
        );
        let e = Edges::new(&out.mesh);
        let (shell, count) = components(&out.mesh, &e, Link::Shell);
        assert_eq!(count, 2);
        // The inner shell is the patch on top.
        let small = (0..out.mesh.triangle_count())
            .filter(|&t| shell[t] != shell[0])
            .count();
        assert!(small > 0 && small < out.mesh.triangle_count() / 2);
        // The total area is the cube's.
        assert!((stats(&out.mesh).area - 2400.0).abs() < 1e-2);
        assert_eq!(open_edges(&out.mesh) % 2, 0);
    }

    #[test]
    fn a_line_round_a_cube_follows_its_sides() {
        let m = fine_cube(2);
        // From the top over the edge onto the side: the walk carries on down the side.
        let top = hit(&m, 10.3, 10.1, true);
        let side = {
            // A point on the +x side, at z = 7.
            let p = [20.0, 9.7, 7.0];
            let t = (0..m.triangle_count() as u32)
                .find(|&t| {
                    let [a, b, c] = [0, 1, 2].map(|k| vertex(&m, m.indices[t as usize * 3 + k]));
                    let q = closest_on_triangle(p, a, b, c);
                    length(sub(q, p)) < 1e-9
                })
                .unwrap();
            CutPoint { at: p, triangle: t }
        };
        let l = line(vec![top, side], false, 0.0, false);
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        let piece = p.pieces[0].as_ref().unwrap();
        // Every point is on the cube's surface.
        for q in piece {
            let on = q.iter().any(|&x| x.abs() < 1e-9 || (x - 20.0).abs() < 1e-9);
            assert!(on, "{q:?}");
        }
        assert!(
            piece
                .iter()
                .any(|q| (q[0] - 20.0).abs() < 1e-9 && (q[2] - 20.0).abs() < 1e-9)
        );
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
    }

    #[test]
    fn a_line_that_crosses_itself_still_cuts_a_sound_mesh() {
        let m = plate(12, 12.0);
        let l = line(
            vec![
                hit(&m, 1.3, 1.2, true),
                hit(&m, 10.6, 10.9, true),
                hit(&m, 10.2, 1.7, true),
                hit(&m, 1.9, 10.4, true),
            ],
            false,
            0.0,
            true,
        );
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
        assert!((stats(&out.mesh).area - 144.0).abs() < 1e-3);
        // A figure eight: two loops inside it and the plate round them.
        let e = Edges::new(&out.mesh);
        assert!(components(&out.mesh, &e, Link::Edge).1 >= 3);
    }

    #[test]
    fn a_line_cannot_cross_a_hole() {
        let mut m = plate(10, 10.0);
        // Take out the quads round the middle.
        let keep: Vec<bool> = (0..m.triangle_count())
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|k| vertex(&m, m.indices[t * 3 + k]));
                let x = (a[0] + b[0] + c[0]) / 3.0;
                let y = (a[1] + b[1] + c[1]) / 3.0;
                !((3.0..7.0).contains(&x) && (0.0..10.0).contains(&y))
            })
            .collect();
        m = crate::repair::topology::keep_triangles(&m, &keep).0;
        let l = line(
            vec![hit(&m, 1.5, 5.2, true), hit(&m, 8.5, 5.1, true)],
            false,
            0.0,
            false,
        );
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        assert_eq!(p.pieces, vec![None]);
        let err = silent(|c| cut(&m, &l, c)).unwrap_err();
        assert!(
            matches!(err, RepairError::Failed(ref s) if s.contains("from point 1 to point 2")),
            "{err:?}"
        );
    }

    #[test]
    fn a_smooth_line_runs_on_the_surface_through_its_points() {
        let m = fine_cube(4);
        let pts = vec![
            hit(&m, 3.0, 5.0, true),
            hit(&m, 10.0, 15.0, true),
            hit(&m, 17.0, 5.0, true),
        ];
        let l = line(pts.clone(), true, 0.0, false);
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        for piece in &p.pieces {
            let piece = piece.as_ref().unwrap();
            assert!(piece.iter().all(|q| (q[2] - 20.0).abs() < 1e-9));
        }
        // Curved: the line passes above the straight chord between the outer points' midpoints.
        let first = p.pieces[0].as_ref().unwrap();
        let straight = line(pts, false, 0.0, false);
        let s = silent(|c| preview(&m, &straight, c)).unwrap();
        assert_ne!(first, s.pieces[0].as_ref().unwrap());
        assert_eq!(p.anchors, s.anchors);
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
    }

    #[test]
    fn a_smooth_line_skips_samples_that_lie_nearest_another_shell() {
        // Points on three faces of a cube: the curve between them runs inside it, where a small
        // cube (a shell of its own) round the middle is nearer than the faces.
        let outer = fine_cube(3);
        let inner = make_box([4.0; 3], [16.0; 3]);
        let base = outer.vertex_count() as u32;
        let m = Mesh {
            positions: [outer.positions.clone(), inner.positions.clone()].concat(),
            indices: outer
                .indices
                .iter()
                .copied()
                .chain(inner.indices.iter().map(|&v| v + base))
                .collect(),
        };
        let on = |p: P3| {
            let t = (0..outer.triangle_count() as u32)
                .find(|&t| {
                    let [a, b, c] = [0, 1, 2].map(|k| vertex(&m, m.indices[t as usize * 3 + k]));
                    length(sub(closest_on_triangle(p, a, b, c), p)) < 1e-9
                })
                .unwrap();
            CutPoint { at: p, triangle: t }
        };
        let mut l = line(
            vec![
                on([10.3, 10.2, 20.0]),
                on([10.1, 0.0, 9.7]),
                on([20.0, 9.8, 10.4]),
            ],
            true,
            0.5,
            true,
        );
        l.options.step = 1.0;
        let p = silent(|c| preview(&m, &l, c)).unwrap();
        for piece in &p.pieces {
            let piece = piece.as_ref().expect("a way round the outside");
            for q in piece {
                assert!(
                    q.iter().any(|&x| x == 0.0 || x == 20.0),
                    "{q:?} is not on the outer cube"
                );
            }
        }
        let out = silent(|c| cut(&m, &l, c)).unwrap();
        assert_sound(&m, &out.mesh);
    }

    #[test]
    fn random_lines_cut_sound_meshes() {
        use crate::repair::exact::testing::Rng;
        // Few big triangles, so several pieces of a line often share one, and a finer cube.
        let plate = plate(3, 30.0);
        let cube = fine_cube(1);
        let mut rng = Rng(7);
        for case in 0..150 {
            let on_cube = case % 3 == 2;
            let m = if on_cube { &cube } else { &plate };
            let n = 2 + rng.below(4) as usize;
            let points: Vec<CutPoint> = (0..n)
                .map(|_| {
                    let (x, y) = (0.5 + rng.unit() * 19.0, 0.5 + rng.unit() * 19.0);
                    if on_cube {
                        hit(m, x, y, true)
                    } else {
                        hit(m, x * 1.5, y * 1.5, true)
                    }
                })
                .collect();
            let l = CutLine {
                points,
                options: CutOptions {
                    smooth: rng.chance(0.5),
                    step: 0.5 + rng.unit() * 3.0,
                    snap: if rng.chance(0.5) { 0.0 } else { rng.unit() },
                    closed: rng.chance(0.5),
                },
            };
            let out =
                silent(|c| cut(m, &l, c)).unwrap_or_else(|e| panic!("case {case}: {e:?} {l:?}"));
            if !out.changed {
                // Points that snapped onto one vertex.
                assert_eq!(out.message, "The cut line has no length.", "case {case}");
                continue;
            }
            assert_sound(m, &out.mesh);
            let area = |m: &Mesh| stats(m).area;
            assert!(
                (area(&out.mesh) - area(m)).abs() < 1e-6 * area(m),
                "case {case}"
            );
        }
    }

    #[test]
    fn bad_points_fail_and_one_point_changes_nothing() {
        let m = plate(4, 4.0);
        let one = line(vec![hit(&m, 1.0, 1.5, true)], false, 0.5, false);
        let out = silent(|c| cut(&m, &one, c)).unwrap();
        assert!(!out.changed);
        let far = line(
            vec![
                CutPoint {
                    at: [0.0; 3],
                    triangle: 999,
                },
                hit(&m, 1.0, 1.5, true),
            ],
            false,
            0.5,
            false,
        );
        assert!(matches!(
            silent(|c| cut(&m, &far, c)),
            Err(RepairError::Failed(_))
        ));
        let mut bad = line(
            vec![hit(&m, 1.0, 1.5, true), hit(&m, 3.0, 2.5, true)],
            false,
            0.5,
            false,
        );
        bad.options.snap = f64::NAN;
        assert!(matches!(
            silent(|c| preview(&m, &bad, c)),
            Err(RepairError::Failed(_))
        ));
    }

    #[test]
    fn a_cancelled_cut_stops() {
        let m = plate(10, 10.0);
        let l = line(
            vec![hit(&m, 1.0, 1.5, true), hit(&m, 8.0, 7.5, true)],
            false,
            0.0,
            false,
        );
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(cut(&m, &l, &mut control), Err(RepairError::Cancelled));
        assert!(matches!(
            preview(&m, &l, &mut control),
            Err(RepairError::Cancelled)
        ));
    }
}
