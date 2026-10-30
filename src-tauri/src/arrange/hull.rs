//! Convex hulls: the 3D hull of a part's points (quickhull), which Lay flat, Rotate to smallest box
//! and Orient take their candidate directions from, and the 2D hull with its smallest enclosing
//! rectangle (rotating calipers).

use std::collections::HashMap;

use super::{V3, cross, dot, len, sub};

pub type V2 = [f64; 2];

/// A closed convex hull: its corner points and its triangles, wound counter-clockwise seen from
/// outside.
#[derive(Clone, Debug)]
pub struct Hull {
    pub points: Vec<V3>,
    pub faces: Vec<[usize; 3]>,
}

impl Hull {
    /// The outward unit normal and area of each face.
    pub fn face_normals(&self) -> Vec<(V3, f64)> {
        self.faces
            .iter()
            .map(|f| {
                let n = cross(
                    sub(self.points[f[1]], self.points[f[0]]),
                    sub(self.points[f[2]], self.points[f[0]]),
                );
                let l = len(n);
                if l > 0.0 {
                    ([n[0] / l, n[1] / l, n[2] / l], l / 2.0)
                } else {
                    ([0.0, 0.0, 0.0], 0.0)
                }
            })
            .collect()
    }
}

struct Face {
    v: [usize; 3],
    n: V3,
    d: f64,
    outside: Vec<usize>,
    alive: bool,
}

fn plane(points: &[V3], v: [usize; 3]) -> (V3, f64) {
    let n = cross(
        sub(points[v[1]], points[v[0]]),
        sub(points[v[2]], points[v[0]]),
    );
    let l = len(n);
    let n = if l > 0.0 {
        [n[0] / l, n[1] / l, n[2] / l]
    } else {
        n
    };
    (n, dot(n, points[v[0]]))
}

/// The convex hull of `points`; None when they are all (nearly) in one plane or fewer than four.
pub fn hull(points: &[V3]) -> Option<Hull> {
    if points.len() < 4 {
        return None;
    }
    // Extremes along the axes give the first two corners: the farthest-apart pair among them.
    let mut lo = [0usize; 3];
    let mut hi = [0usize; 3];
    for (i, p) in points.iter().enumerate() {
        for a in 0..3 {
            if p[a] < points[lo[a]][a] {
                lo[a] = i;
            }
            if p[a] > points[hi[a]][a] {
                hi[a] = i;
            }
        }
    }
    let extent = (0..3)
        .map(|a| points[hi[a]][a] - points[lo[a]][a])
        .fold(0.0, f64::max);
    if extent <= 0.0 || !extent.is_finite() {
        return None;
    }
    let scale = points
        .iter()
        .flat_map(|p| p.iter())
        .fold(extent, |m, v| m.max(v.abs()));
    let eps = scale * 1e-11 * 64.0;
    let (mut a, mut b) = (lo[0], hi[0]);
    for k in 0..3 {
        if len(sub(points[hi[k]], points[lo[k]])) > len(sub(points[b], points[a])) {
            (a, b) = (lo[k], hi[k]);
        }
    }
    let ab = sub(points[b], points[a]);
    let line_dist = |p: V3| len(cross(ab, sub(p, points[a]))) / len(ab);
    let c =
        (0..points.len()).max_by(|&i, &j| line_dist(points[i]).total_cmp(&line_dist(points[j])))?;
    if line_dist(points[c]) <= eps {
        return None;
    }
    let (n0, d0) = plane(points, [a, b, c]);
    let d = (0..points.len()).max_by(|&i, &j| {
        (dot(n0, points[i]) - d0)
            .abs()
            .total_cmp(&(dot(n0, points[j]) - d0).abs())
    })?;
    if (dot(n0, points[d]) - d0).abs() <= eps {
        return None;
    }

    let mut faces: Vec<Face> = Vec::new();
    let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
    let tetra = if dot(n0, points[d]) - d0 > 0.0 {
        // d is above abc, so abc seen from outside (below) runs the other way.
        [[a, c, b], [a, b, d], [b, c, d], [c, a, d]]
    } else {
        [[a, b, c], [a, d, b], [b, d, c], [c, d, a]]
    };
    for v in tetra {
        add_face(&mut faces, &mut edges, points, v);
    }
    // Every other point goes to the first face it is outside of.
    let used = [a, b, c, d];
    for i in 0..points.len() {
        if used.contains(&i) {
            continue;
        }
        assign(&mut faces, 0..4, points, i, eps);
    }

    let mut queue: Vec<usize> = (0..faces.len()).collect();
    while let Some(f) = queue.pop() {
        if !faces[f].alive || faces[f].outside.is_empty() {
            continue;
        }
        let eye = *faces[f].outside.iter().max_by(|&&i, &&j| {
            let face = &faces[f];
            (dot(face.n, points[i]) - face.d).total_cmp(&(dot(face.n, points[j]) - face.d))
        })?;
        let p = points[eye];
        // The faces the eye sees, found by walking across edges from `f`.
        let mut visible = vec![f];
        let mut seen: HashMap<usize, bool> = HashMap::from([(f, true)]);
        let mut i = 0;
        while i < visible.len() {
            let g = visible[i];
            i += 1;
            let v = faces[g].v;
            for k in 0..3 {
                let (x, y) = (v[k], v[(k + 1) % 3]);
                let h = *edges.get(&(y, x))?;
                if seen.contains_key(&h) {
                    continue;
                }
                let sees = dot(faces[h].n, p) - faces[h].d > eps;
                seen.insert(h, sees);
                if sees {
                    visible.push(h);
                }
            }
        }
        // The horizon: edges of visible faces whose neighbour is not visible.
        let mut horizon = Vec::new();
        for &g in &visible {
            let v = faces[g].v;
            for k in 0..3 {
                let (x, y) = (v[k], v[(k + 1) % 3]);
                let h = edges[&(y, x)];
                if !seen[&h] {
                    horizon.push((x, y));
                }
            }
        }
        let mut orphans = Vec::new();
        for &g in &visible {
            let v = faces[g].v;
            for k in 0..3 {
                edges.remove(&(v[k], v[(k + 1) % 3]));
            }
            faces[g].alive = false;
            orphans.append(&mut faces[g].outside);
        }
        let first = faces.len();
        for (x, y) in horizon {
            if edges.contains_key(&(x, y)) || edges.contains_key(&(y, eye)) {
                // A horizon that is not a simple loop: the points are too close to flat for the
                // tolerance. Give up rather than build a broken hull.
                return None;
            }
            add_face(&mut faces, &mut edges, points, [x, y, eye]);
        }
        let last = faces.len();
        for i in orphans {
            if i != eye {
                assign(&mut faces, first..last, points, i, eps);
            }
        }
        queue.extend(first..last);
    }

    // Keep the corners the faces use, numbered afresh.
    let mut remap = HashMap::new();
    let mut out = Hull {
        points: Vec::new(),
        faces: Vec::new(),
    };
    for f in faces.iter().filter(|f| f.alive) {
        let mut v = [0; 3];
        for k in 0..3 {
            v[k] = *remap.entry(f.v[k]).or_insert_with(|| {
                out.points.push(points[f.v[k]]);
                out.points.len() - 1
            });
        }
        out.faces.push(v);
    }
    Some(out)
}

fn add_face(
    faces: &mut Vec<Face>,
    edges: &mut HashMap<(usize, usize), usize>,
    points: &[V3],
    v: [usize; 3],
) {
    let (n, d) = plane(points, v);
    let id = faces.len();
    for k in 0..3 {
        edges.insert((v[k], v[(k + 1) % 3]), id);
    }
    faces.push(Face {
        v,
        n,
        d,
        outside: Vec::new(),
        alive: true,
    });
}

fn assign(faces: &mut [Face], range: std::ops::Range<usize>, points: &[V3], i: usize, eps: f64) {
    for f in range {
        if faces[f].alive && dot(faces[f].n, points[i]) - faces[f].d > eps {
            faces[f].outside.push(i);
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2D

fn cross2(o: V2, a: V2, b: V2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The 2D convex hull, counter-clockwise, without collinear points (Andrew's monotone chain).
/// Points closer together or nearer to a line than rounding can tell apart count as one, so the
/// polygon is strictly convex: the corners of a box seen along an edge come out as four.
pub fn hull2(points: &[V2]) -> Vec<V2> {
    let mut p: Vec<V2> = points.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let scale = p
        .iter()
        .flat_map(|q| q.iter())
        .fold(0.0f64, |m, v| m.max(v.abs()))
        .max(f64::MIN_POSITIVE);
    let near = scale * 1e-12;
    p.dedup_by(|b, a| (a[0] - b[0]).abs() <= near && (a[1] - b[1]).abs() <= near);
    if p.len() < 3 {
        return p;
    }
    // Twice the area of a triangle with sides of about `scale` and height `near`.
    let flat = scale * near * 4.0;
    let mut h: Vec<V2> = Vec::with_capacity(2 * p.len());
    for pass in 0..2 {
        let start = h.len();
        let iter: Box<dyn Iterator<Item = &V2>> = if pass == 0 {
            Box::new(p.iter())
        } else {
            Box::new(p.iter().rev())
        };
        for &q in iter {
            while h.len() >= start + 2 && cross2(h[h.len() - 2], h[h.len() - 1], q) <= flat {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// Area of a simple polygon (positive counter-clockwise).
pub fn area2(poly: &[V2]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// The smallest rectangle round a convex polygon (counter-clockwise, as `hull2` gives it): the unit
/// direction of its first side and its two side lengths, along that direction and across it. One
/// side of the smallest rectangle always lies along a side of the polygon.
pub fn min_rectangle(poly: &[V2]) -> (V2, f64, f64) {
    let n = poly.len();
    if n == 0 {
        return ([1.0, 0.0], 0.0, 0.0);
    }
    let extents = |dir: V2| {
        let (mut lo, mut hi, mut lo2, mut hi2) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in poly {
            let s = p[0] * dir[0] + p[1] * dir[1];
            let t = -p[0] * dir[1] + p[1] * dir[0];
            lo = lo.min(s);
            hi = hi.max(s);
            lo2 = lo2.min(t);
            hi2 = hi2.max(t);
        }
        (hi - lo, hi2 - lo2)
    };
    if n < 3 {
        let dir = if n == 2 {
            let d = [poly[1][0] - poly[0][0], poly[1][1] - poly[0][1]];
            let l = d[0].hypot(d[1]);
            if l > 0.0 {
                [d[0] / l, d[1] / l]
            } else {
                [1.0, 0.0]
            }
        } else {
            [1.0, 0.0]
        };
        let (w, h) = extents(dir);
        return (dir, w, h);
    }
    // Rotating calipers: for each side, the farthest point along it, across it and against it
    // only ever move forward round the polygon.
    let dir_of = |i: usize| {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]);
        [d[0] / l, d[1] / l]
    };
    let along = |p: V2, d: V2| p[0] * d[0] + p[1] * d[1];
    let across = |p: V2, d: V2| -p[0] * d[1] + p[1] * d[0];
    let next = |k: usize| (k + 1) % n;
    let d0 = dir_of(0);
    let (mut j, mut k, mut l) = (0, 0, 0);
    for m in 0..n {
        if along(poly[m], d0) > along(poly[j], d0) {
            j = m;
        }
        if across(poly[m], d0) > across(poly[k], d0) {
            k = m;
        }
        if along(poly[m], d0) < along(poly[l], d0) {
            l = m;
        }
    }
    let mut best = (f64::INFINITY, d0, 0.0, 0.0);
    for i in 0..n {
        let d = dir_of(i);
        if !d[0].is_finite() {
            continue;
        }
        while along(poly[next(j)], d) > along(poly[j], d) {
            j = next(j);
        }
        while across(poly[next(k)], d) > across(poly[k], d) {
            k = next(k);
        }
        while along(poly[next(l)], d) < along(poly[l], d) {
            l = next(l);
        }
        let w = along(poly[j], d) - along(poly[l], d);
        let h = across(poly[k], d) - across(poly[i], d);
        if w * h < best.0 {
            best = (w * h, d, w, h);
        }
    }
    // Rounding may leave the pointers a step short on a polygon with near-parallel sides; measure
    // the winner directly.
    let (w, h) = extents(best.1);
    (best.1, w, h)
}

/// Point `p` projected into the plane through the origin with unit normal `n`, in the frame `(u,
/// v)` of that plane.
pub fn project(p: V3, u: V3, v: V3) -> V2 {
    [dot(p, u), dot(p, v)]
}

/// Two unit vectors that make a right-handed frame `(u, v, n)` with the unit vector `n`.
pub fn frame(n: V3) -> (V3, V3) {
    let helper = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = cross(helper, n);
    let l = len(u);
    let u = [u[0] / l, u[1] / l, u[2] / l];
    (u, cross(n, u))
}

/// The vector in the plane `(u, v)` at 2D direction `d`.
pub fn lift(d: V2, u: V3, v: V3) -> V3 {
    [
        u[0] * d[0] + v[0] * d[1],
        u[1] * d[0] + v[1] * d[1],
        u[2] * d[0] + v[2] * d[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_points(size: f64) -> Vec<V3> {
        let mut p = Vec::new();
        for i in 0..=4 {
            for j in 0..=4 {
                for k in 0..=4 {
                    p.push([
                        size * i as f64 / 4.0,
                        size * j as f64 / 4.0,
                        size * k as f64 / 4.0,
                    ]);
                }
            }
        }
        p
    }

    #[test]
    fn the_hull_of_a_cube_grid_is_its_eight_corners_and_twelve_triangles() {
        let h = hull(&cube_points(10.0)).unwrap();
        // Points on the faces are not corners: they are inside the tolerance.
        assert_eq!(h.points.len(), 8, "{:?}", h.points);
        assert_eq!(h.faces.len(), 12);
        let area: f64 = h.face_normals().iter().map(|(_, a)| a).sum();
        assert!((area - 600.0).abs() < 1e-9, "{area}");
        // Every face faces out: the centre is behind all of them.
        for (n, _) in h.face_normals() {
            let f = h.points.iter().map(|p| dot(n, *p)).fold(f64::MIN, f64::max);
            assert!(dot(n, [5.0; 3]) < f);
        }
    }

    #[test]
    fn the_hull_of_a_sphere_has_every_point_on_it() {
        // A golden-angle spiral: no four points in one plane, so every point is a corner.
        let mut p: Vec<V3> = (0..800)
            .map(|i| {
                let z = 1.0 - (i as f64 + 0.5) / 400.0;
                let r = (1.0 - z * z).sqrt();
                let f = i as f64 * std::f64::consts::PI * (3.0 - 5f64.sqrt());
                [r * f.cos(), r * f.sin(), z]
            })
            .collect();
        p.push([0.0; 3]);
        let h = hull(&p).unwrap();
        assert_eq!(h.points.len(), 800);
        // Euler: a closed triangulated sphere has 2V - 4 faces.
        assert_eq!(h.faces.len(), 2 * 800 - 4);
    }

    #[test]
    fn flat_points_have_no_3d_hull() {
        let p: Vec<V3> = (0..10).map(|i| [i as f64, (i * i) as f64, 0.0]).collect();
        assert!(hull(&p).is_none());
    }

    #[test]
    fn the_smallest_rectangle_round_a_turned_rectangle_is_that_rectangle() {
        let (c, s) = (0.3f64.cos(), 0.3f64.sin());
        let corners: Vec<V2> = [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [0.0, 10.0],
            [7.0, 3.0],
        ]
        .iter()
        .map(|p: &V2| [p[0] * c - p[1] * s, p[0] * s + p[1] * c])
        .collect();
        let poly = hull2(&corners);
        assert_eq!(poly.len(), 4);
        assert!((area2(&poly) - 200.0).abs() < 1e-9);
        let (d, w, h) = min_rectangle(&poly);
        let mut sides = [w, h];
        sides.sort_by(f64::total_cmp);
        assert!(
            (sides[0] - 10.0).abs() < 1e-9 && (sides[1] - 20.0).abs() < 1e-9,
            "{sides:?}"
        );
        // Along a side of the rectangle: 0.3 rad, give or take quarter turns.
        let angle = d[1].atan2(d[0]).rem_euclid(std::f64::consts::FRAC_PI_2);
        assert!((angle - 0.3).abs() < 1e-9, "{angle}");
    }
}
