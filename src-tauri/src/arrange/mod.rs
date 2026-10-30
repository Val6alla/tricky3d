//! Arrange: Align, Duplicate, Lay flat, Rotate to smallest box, Orient and Pack. Each works out new
//! matrices for parts, and for copies of them, from the parts' world-space geometry; the window
//! applies them as one undo step. Plain data and functions, so all of it runs without Tauri.

pub mod align;
pub mod collide;
pub mod duplicate;
pub mod hull;
pub mod orient;
pub mod pack;

#[cfg(test)]
mod tests;

use crate::mesh::{Matrix, Mesh, linear_determinant, multiply, transform_point};

pub type V3 = [f64; 3];
/// An axis-aligned box: min and max corners.
pub type Aabb = [V3; 2];
/// A 3x3 rotation, by rows.
pub type Rot = [V3; 3];

pub const IDENTITY_ROT: Rot = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` scaled to length 1; None for a zero (or not finite) vector.
pub fn unit(a: V3) -> Option<V3> {
    let l = len(a);
    (l > 0.0 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

pub fn centre(b: &Aabb) -> V3 {
    scale(add(b[0], b[1]), 0.5)
}

pub fn size(b: &Aabb) -> V3 {
    sub(b[1], b[0])
}

/// The box round `points`; None when there are none.
pub fn bounds(points: &[V3]) -> Option<Aabb> {
    let mut it = points.iter();
    let first = *it.next()?;
    let mut b = [first, first];
    for p in it {
        for a in 0..3 {
            b[0][a] = b[0][a].min(p[a]);
            b[1][a] = b[1][a].max(p[a]);
        }
    }
    Some(b)
}

/// The smallest box holding both.
pub fn union(a: &Aabb, b: &Aabb) -> Aabb {
    let mut out = *a;
    for k in 0..3 {
        out[0][k] = out[0][k].min(b[0][k]);
        out[1][k] = out[1][k].max(b[1][k]);
    }
    out
}

/// The box round all of `boxes`; None when there are none.
pub fn union_all<'a>(boxes: impl IntoIterator<Item = &'a Aabb>) -> Option<Aabb> {
    boxes.into_iter().copied().reduce(|a, b| union(&a, &b))
}

pub fn apply(r: &Rot, p: V3) -> V3 {
    [dot(r[0], p), dot(r[1], p), dot(r[2], p)]
}

/// `a` then `b`: the rotation that applies `a` first.
pub fn compose(b: &Rot, a: &Rot) -> Rot {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| b[i][k] * a[k][j]).sum();
        }
    }
    out
}

/// The rotation about the Z axis by `angle` radians (counter-clockwise seen from above).
pub fn rot_z(angle: f64) -> Rot {
    let (s, c) = angle.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// The smallest rotation that turns unit vector `a` onto unit vector `b`. When they point opposite
/// ways it turns half round an axis square to `a`.
pub fn rotation_between(a: V3, b: V3) -> Rot {
    let v = cross(a, b);
    let c = dot(a, b);
    if c < -1.0 + 1e-12 {
        let (u, _) = hull::frame(a);
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x = 2.0 * u[i] * u[j] - if i == j { 1.0 } else { 0.0 };
            }
        }
        return r;
    }
    // Rodrigues: I + [v]x + [v]x^2 / (1 + c).
    let k = 1.0 / (1.0 + c);
    [
        [
            1.0 - k * (v[1] * v[1] + v[2] * v[2]),
            -v[2] + k * v[0] * v[1],
            v[1] + k * v[0] * v[2],
        ],
        [
            v[2] + k * v[0] * v[1],
            1.0 - k * (v[0] * v[0] + v[2] * v[2]),
            -v[0] + k * v[1] * v[2],
        ],
        [
            -v[1] + k * v[0] * v[2],
            v[0] + k * v[1] * v[2],
            1.0 - k * (v[0] * v[0] + v[1] * v[1]),
        ],
    ]
}

/// `m` moved by `d` in world space.
pub fn translated(m: &Matrix, d: V3) -> Matrix {
    let mut out = *m;
    for k in 0..3 {
        out[12 + k] += d[k];
    }
    out
}

/// `m` followed by the world-space rotation `r` about `pivot`.
pub fn rotated_about(m: &Matrix, r: &Rot, pivot: V3) -> Matrix {
    let t = sub(pivot, apply(r, pivot));
    let mut world = [0.0; 16];
    for row in 0..3 {
        for col in 0..3 {
            world[col * 4 + row] = r[row][col];
        }
        world[12 + row] = t[row];
    }
    world[15] = 1.0;
    multiply(&world, m)
}

/// A part's points and triangles in world space, in f64: its own vertices through its matrix,
/// with the winding reversed under a mirroring matrix so triangles keep facing out.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub points: Vec<V3>,
    pub triangles: Vec<[u32; 3]>,
}

impl World {
    pub fn new(mesh: &Mesh, m: &Matrix) -> World {
        let points = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| transform_point(m, [p[0] as f64, p[1] as f64, p[2] as f64]))
            .collect();
        let flip = linear_determinant(m) < 0.0;
        let triangles = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| if flip { [t[0], t[2], t[1]] } else { *t })
            .collect();
        World { points, triangles }
    }

    pub fn bounds(&self) -> Option<Aabb> {
        bounds(&self.points)
    }

    pub fn corners(&self, t: [u32; 3]) -> [V3; 3] {
        t.map(|i| self.points[i as usize])
    }

    /// The triangle's unit normal and area; a zero normal for a triangle with no area.
    pub fn normal_area(&self, t: [u32; 3]) -> (V3, f64) {
        let [a, b, c] = self.corners(t);
        let n = cross(sub(b, a), sub(c, a));
        let l = len(n);
        if l > 0.0 {
            (scale(n, 1.0 / l), l / 2.0)
        } else {
            ([0.0; 3], 0.0)
        }
    }

    /// Volume in mm³ (positive for a closed part facing out).
    pub fn volume(&self) -> f64 {
        self.triangles
            .iter()
            .map(|&t| {
                let [a, b, c] = self.corners(t);
                dot(a, cross(b, c))
            })
            .sum::<f64>()
            / 6.0
    }

    /// The same part turned by `r` about `pivot`.
    pub fn rotated(&self, r: &Rot, pivot: V3) -> World {
        World {
            points: self
                .points
                .iter()
                .map(|&p| add(apply(r, sub(p, pivot)), pivot))
                .collect(),
            triangles: self.triangles.clone(),
        }
    }
}
