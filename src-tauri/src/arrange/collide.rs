//! Do placed parts collide? Two parts collide when their surfaces cross (the exact triangle test the
//! repair tools use: touching along an edge or at a corner does not count, lying face to face does)
//! or when one lies wholly inside the other. Packing runs this on the meshes, not their boxes,
//! after every pack, and reports any collision as a failure.

use rayon::prelude::*;

use super::{Aabb, World, sub};
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
