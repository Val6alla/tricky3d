//! Do placed parts collide? Two parts collide when their surfaces cross (the exact triangle test the
//! repair tools use: touching along an edge or at a corner does not count, lying face to face does)
//! or when one lies wholly inside the other. Packing runs this on the meshes, not their boxes,
//! after every pack, and reports any collision as a failure.

use rayon::prelude::*;

use super::{Aabb, V3, World, dot, sub};
use crate::repair::bvh::{self, Bvh, overlap};
use crate::repair::tritri::intersect;
use crate::repair::{Control, RepairResult};

/// A placed part ready for collision tests: its world triangles in a bounding volume hierarchy.
pub struct Body {
    world: World,
    boxes: Vec<Aabb>,
    bvh: Bvh,
    bounds: Option<Aabb>,
}

impl Body {
    pub fn new(world: World) -> Body {
        let boxes = bvh::boxes_of(&world.points, &world.triangles);
        let bvh = Bvh::new(&boxes);
        let bounds = bvh.bounds();
        Body {
            world,
            boxes,
            bvh,
            bounds,
        }
    }

    /// Generalised winding number of the surface round `p`: about 1 inside a closed part facing
    /// out, about 0 outside.
    fn winding(&self, p: [f64; 3]) -> f64 {
        let mut total = 0.0;
        for &t in &self.world.triangles {
            let [a, b, c] = self.world.corners(t).map(|q| sub(q, p));
            let (la, lb, lc) = (super::len(a), super::len(b), super::len(c));
            let num = super::dot(a, super::cross(b, c));
            let den = la * lb * lc
                + super::dot(a, b) * lc
                + super::dot(a, c) * lb
                + super::dot(b, c) * la;
            total += 2.0 * num.atan2(den);
        }
        total / (4.0 * std::f64::consts::PI)
    }

    /// A point of the part's surface, to test whether the part is inside another.
    fn any_point(&self) -> Option<[f64; 3]> {
        let t = *self.world.triangles.first()?;
        let [a, b, c] = self.world.corners(t);
        Some([
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ])
    }

    fn inside(&self, p: [f64; 3]) -> bool {
        self.bounds.is_some_and(|b| overlap(&b, &[p, p])) && self.winding(p).abs() > 0.5
    }
}

/// True when the two parts collide.
pub fn collide(a: &Body, b: &Body) -> bool {
    let (Some(ba), Some(bb)) = (a.bounds, b.bounds) else {
        return false;
    };
    if !overlap(&ba, &bb) {
        return false;
    }
    // The smaller one's triangles against the larger one's tree.
    let (small, large) = if a.boxes.len() <= b.boxes.len() {
        (a, b)
    } else {
        (b, a)
    };
    let large_bounds = large.bounds.unwrap_or(bb);
    let crossing = small.world.triangles.par_iter().enumerate().any(|(i, &t)| {
        let tb = &small.boxes[i];
        if !overlap(tb, &large_bounds) {
            return false;
        }
        let corners = small.world.corners(t);
        let mut hit = false;
        large.bvh.query(tb, |j| {
            if !hit {
                let u = large.world.triangles[j as usize];
                hit = intersect(&corners, &large.world.corners(u));
            }
        });
        hit
    });
    if crossing {
        return true;
    }
    a.any_point().is_some_and(|p| b.inside(p)) || b.any_point().is_some_and(|p| a.inside(p))
}

/// The triangles of `a` and of `b` that cross the other part's surface (sorted), to draw where
/// two colliding parts meet. Empty for parts that collide only by one lying wholly inside the other.
pub fn crossing_triangles(a: &Body, b: &Body) -> (Vec<u32>, Vec<u32>) {
    let (Some(ba), Some(bb)) = (a.bounds, b.bounds) else {
        return (Vec::new(), Vec::new());
    };
    if !overlap(&ba, &bb) {
        return (Vec::new(), Vec::new());
    }
    let hits: Vec<(u32, Vec<u32>)> = a
        .world
        .triangles
        .par_iter()
        .enumerate()
        .filter_map(|(i, &t)| {
            let tb = &a.boxes[i];
            if !overlap(tb, &bb) {
                return None;
            }
            let corners = a.world.corners(t);
            let mut found = Vec::new();
            b.bvh.query(tb, |j| {
                if intersect(&corners, &b.world.corners(b.world.triangles[j as usize])) {
                    found.push(j);
                }
            });
            (!found.is_empty()).then_some((i as u32, found))
        })
        .collect();
    let mine: Vec<u32> = hits.iter().map(|h| h.0).collect();
    let mut theirs: Vec<u32> = hits.into_iter().flat_map(|h| h.1).collect();
    theirs.sort_unstable();
    theirs.dedup();
    (mine, theirs)
}

/// The closest points of segments pq and rs (Ericson, Real-Time Collision Detection 5.1.9).
fn closest_between_segments(p: V3, q: V3, r: V3, s: V3) -> (V3, V3) {
    let (d1, d2, w) = (sub(q, p), sub(s, r), sub(p, r));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, w));
    let point = |o: V3, d: V3, t: f64| [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
    if a <= f64::EPSILON && e <= f64::EPSILON {
        return (p, r);
    }
    let (sc, tc);
    if a <= f64::EPSILON {
        sc = 0.0;
        tc = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, w);
        if e <= f64::EPSILON {
            tc = 0.0;
            sc = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s1 = if denom > 0.0 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t1 = (b * s1 + f) / e;
            if t1 < 0.0 {
                t1 = 0.0;
                s1 = (-c / a).clamp(0.0, 1.0);
            } else if t1 > 1.0 {
                t1 = 1.0;
                s1 = ((b - c) / a).clamp(0.0, 1.0);
            }
            sc = s1;
            tc = t1;
        }
    }
    (point(p, d1, sc), point(r, d2, tc))
}

/// The closest points of two triangles that do not cross: the nearest of the corner-to-triangle
/// and edge-to-edge pairs.
pub fn closest_between_triangles(a: &[V3; 3], b: &[V3; 3]) -> (V3, V3) {
    use crate::analysis::compare::closest_on_triangle;
    let mut best = (a[0], b[0]);
    let mut best_d = f64::INFINITY;
    let mut consider = |p: V3, q: V3| {
        let d = sub(p, q);
        let d = dot(d, d);
        if d < best_d {
            best_d = d;
            best = (p, q);
        }
    };
    for &p in a {
        consider(p, closest_on_triangle(p, b[0], b[1], b[2]));
    }
    for &q in b {
        consider(closest_on_triangle(q, a[0], a[1], a[2]), q);
    }
    for i in 0..3 {
        for j in 0..3 {
            let (p, q) = closest_between_segments(a[i], a[(i + 1) % 3], b[j], b[(j + 1) % 3]);
            consider(p, q);
        }
    }
    best
}

/// The closest points of two parts that do not collide, when they are less than `limit` apart
/// (mm); None when they are not.
pub fn closer_than(a: &Body, b: &Body, limit: f64) -> Option<(f64, V3, V3)> {
    let (Some(ba), Some(bb)) = (a.bounds, b.bounds) else {
        return None;
    };
    let grown = |x: &Aabb| [sub(x[0], [limit; 3]), super::add(x[1], [limit; 3])];
    if !overlap(&grown(&ba), &bb) {
        return None;
    }
    let (small, large) = if a.boxes.len() <= b.boxes.len() {
        (a, b)
    } else {
        (b, a)
    };
    let found = small
        .world
        .triangles
        .par_iter()
        .enumerate()
        .filter_map(|(i, &t)| {
            let reach = grown(&small.boxes[i]);
            let corners = small.world.corners(t);
            let mut best: Option<(f64, V3, V3)> = None;
            large.bvh.query(&reach, |j| {
                let (p, q) = closest_between_triangles(
                    &corners,
                    &large.world.corners(large.world.triangles[j as usize]),
                );
                let d = super::len(sub(p, q));
                if d < limit && best.is_none_or(|b| d < b.0) {
                    best = Some((d, p, q));
                }
            });
            best
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))?;
    // The closest points in the order the parts were given.
    Some(if std::ptr::eq(small, a) {
        found
    } else {
        (found.0, found.2, found.1)
    })
}

impl Body {
    /// The part's world geometry.
    pub fn world(&self) -> &World {
        &self.world
    }
}

/// Every pair of `bodies` that collides, where at least one of the two is `moved`. Cancel stops it
/// between pairs.
pub fn collisions(
    bodies: &[Body],
    moved: &[bool],
    control: &mut Control,
) -> RepairResult<Vec<(usize, usize)>> {
    let mut pairs = Vec::new();
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            if !(moved[i] || moved[j]) {
                continue;
            }
            if let (Some(a), Some(b)) = (bodies[i].bounds, bodies[j].bounds)
                && overlap(&a, &b)
            {
                pairs.push((i, j));
            }
        }
    }
    let mut out = Vec::new();
    for (k, &(i, j)) in pairs.iter().enumerate() {
        control.check()?;
        if collide(&bodies[i], &bodies[j]) {
            out.push((i, j));
        }
        control.progress((k + 1) as f64 / pairs.len() as f64);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{IDENTITY, make_box};

    fn body(min: [f32; 3], max: [f32; 3]) -> Body {
        Body::new(World::new(&make_box(min, max), &IDENTITY))
    }

    #[test]
    fn crossing_inside_touching_and_apart() {
        let a = body([0.0; 3], [10.0; 3]);
        // Crossing.
        assert!(collide(&a, &body([5.0; 3], [15.0; 3])));
        // Wholly inside, no surfaces crossing.
        assert!(collide(&a, &body([2.0; 3], [3.0; 3])));
        assert!(collide(&body([2.0; 3], [3.0; 3]), &a));
        // Apart, also when the boxes nearly touch.
        assert!(!collide(&a, &body([10.001, 0.0, 0.0], [20.0, 10.0, 10.0])));
        // Corner to corner only: touching, not colliding.
        assert!(!collide(&a, &body([10.0; 3], [20.0; 3])));
        // Face to face counts: the faces overlap in their plane.
        assert!(collide(&a, &body([10.0, 0.0, 0.0], [20.0, 10.0, 10.0])));
    }
}
