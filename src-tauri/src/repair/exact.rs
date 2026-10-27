//! Exact geometric predicates (Shewchuk's adaptive arithmetic, via the `robust` crate) with the sign
//! conventions the repair code uses, plus small f64 vector helpers. Mesh coordinates are f32, so
//! they convert to f64 exactly and every predicate on them is exact.

use robust::{Coord, Coord3D};

use crate::mesh::Mesh;

pub type P3 = [f64; 3];
pub type P2 = [f64; 2];

/// +1 when `d` lies on the side of the plane through `a`, `b`, `c` that the normal
/// (b − a) × (c − a) points to, −1 on the other side, 0 when the four points are coplanar. Exact
/// for all finite inputs whose products of three coordinate differences neither overflow nor
/// underflow, which covers every f32 coordinate and f64 coordinates of any ordinary size.
pub fn orient3d(a: P3, b: P3, c: P3, d: P3) -> i32 {
    // robust's sign is the other way round: positive when `d` lies below the plane, "below" being
    // the side from which `a`, `b`, `c` turn clockwise.
    -sign(robust::orient3d(coord3(a), coord3(b), coord3(c), coord3(d)))
}

/// +1 when `a`, `b`, `c` turn counter-clockwise, −1 clockwise, 0 when collinear. Exact.
pub fn orient2d(a: P2, b: P2, c: P2) -> i32 {
    sign(robust::orient2d(coord2(a), coord2(b), coord2(c)))
}

/// True when the three points lie on one line (coincident points count). Exact.
pub fn collinear(a: P3, b: P3, c: P3) -> bool {
    // The normal (b − a) × (c − a) is zero exactly when none of the three projections has area.
    (0..3).all(|axis| orient2d(project(a, axis), project(b, axis), project(c, axis)) == 0)
}

/// `p` seen along `axis`: the other two coordinates in cyclic order (y, z for x; z, x for y; x, y
/// for z), so a projected triangle turns counter-clockwise exactly when the `axis` component of its
/// normal is positive.
#[inline]
pub fn project(p: P3, axis: usize) -> P2 {
    [p[(axis + 1) % 3], p[(axis + 2) % 3]]
}

/// The axis to project a triangle along without losing its area: the one its f64 normal points
/// along most, checked exactly (another one if rounding misled), so the projection keeps every
/// incidence and orientation within the triangle's plane. None for a triangle with no area.
pub fn dominant_axis(a: P3, b: P3, c: P3) -> Option<usize> {
    let n = normal(a, b, c).map(f64::abs);
    let mut axes = [0, 1, 2];
    axes.sort_by(|&i, &j| n[j].total_cmp(&n[i]));
    axes.into_iter()
        .find(|&axis| orient2d(project(a, axis), project(b, axis), project(c, axis)) != 0)
}

#[inline]
fn sign(x: f64) -> i32 {
    (x > 0.0) as i32 - (x < 0.0) as i32
}

#[inline]
fn coord2(p: P2) -> Coord<f64> {
    Coord { x: p[0], y: p[1] }
}

#[inline]
fn coord3(p: P3) -> Coord3D<f64> {
    Coord3D {
        x: p[0],
        y: p[1],
        z: p[2],
    }
}

/// A mesh vertex as f64.
#[inline]
pub fn vertex(mesh: &Mesh, v: u32) -> P3 {
    let i = v as usize * 3;
    [
        mesh.positions[i] as f64,
        mesh.positions[i + 1] as f64,
        mesh.positions[i + 2] as f64,
    ]
}

/// The three corners of triangle `t` as f64.
#[inline]
pub fn corners(mesh: &Mesh, t: u32) -> [P3; 3] {
    let i = t as usize * 3;
    [
        vertex(mesh, mesh.indices[i]),
        vertex(mesh, mesh.indices[i + 1]),
        vertex(mesh, mesh.indices[i + 2]),
    ]
}

#[inline]
pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn scale(a: P3, s: f64) -> P3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
pub fn length(a: P3) -> f64 {
    dot(a, a).sqrt()
}

/// The length of `a` as JavaScript's `Math.hypot` gives it in V8: each component divided by the
/// largest, the squares summed with Kahan compensation. The Home edits that used to run in the
/// window (refine, open_edges) measure with it, so they place points and break ties between equal
/// edges exactly as they did there.
pub fn hypot(a: P3) -> f64 {
    let abs = a.map(f64::abs);
    let mut max = 0.0f64;
    for v in abs {
        if v > max {
            max = v;
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if abs.iter().any(|v| v.is_nan()) {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let (mut sum, mut compensation) = (0.0f64, 0.0f64);
    for v in abs {
        let n = v / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// (b − a) × (c − a): twice the triangle's area as a vector along its normal (not exact).
#[inline]
pub fn normal(a: P3, b: P3, c: P3) -> P3 {
    cross(sub(b, a), sub(c, a))
}

/// Helpers the repair tests share: a small deterministic random generator and exact integer
/// versions of f64 coordinates.
#[cfg(test)]
pub(crate) mod testing {
    use num_bigint::{BigInt, Sign};
    use num_traits::{Float, Zero};

    use crate::mesh::Mesh;

    /// splitmix64: deterministic, so a failing case can be replayed from its seed.
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform in 0..n.
        pub fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// Uniform integer in lo..=hi.
        pub fn int(&mut self, lo: i64, hi: i64) -> i64 {
            lo + self.below((hi - lo + 1) as u64) as i64
        }

        /// Uniform in [0, 1).
        pub fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }

        /// True with probability `p`.
        pub fn chance(&mut self, p: f64) -> bool {
            self.unit() < p
        }

        /// `x` moved by at most one unit in the last place, either way or not at all.
        pub fn nudge(&mut self, x: f64) -> f64 {
            match self.below(3) {
                0 => x.next_down(),
                1 => x,
                _ => x.next_up(),
            }
        }
    }

    /// The values as integers after scaling all of them by one power of two, which changes no
    /// orientation, so exact integer arithmetic can decide what the f64 predicates decide.
    pub fn to_integers(values: &[f64]) -> Vec<BigInt> {
        let decoded: Vec<(u64, i16, i8)> = values.iter().map(|v| v.integer_decode()).collect();
        let low = decoded
            .iter()
            .filter(|d| d.0 != 0)
            .map(|d| d.1)
            .min()
            .unwrap_or(0);
        decoded
            .iter()
            .map(|&(mantissa, exponent, sign)| {
                if mantissa == 0 {
                    return BigInt::zero();
                }
                let n = BigInt::from(mantissa) << (exponent - low) as usize;
                if sign < 0 { -n } else { n }
            })
            .collect()
    }

    /// Sign of the 3 × 3 determinant with rows `r`.
    pub fn det3_sign(r: [[BigInt; 3]; 3]) -> i32 {
        let d = &r[0][0] * (&r[1][1] * &r[2][2] - &r[1][2] * &r[2][1])
            - &r[0][1] * (&r[1][0] * &r[2][2] - &r[1][2] * &r[2][0])
            + &r[0][2] * (&r[1][0] * &r[2][1] - &r[1][1] * &r[2][0]);
        sign_of(&d)
    }

    pub fn sign_of(n: &BigInt) -> i32 {
        match n.sign() {
            Sign::Minus => -1,
            Sign::NoSign => 0,
            Sign::Plus => 1,
        }
    }

    /// A closed torus around the z axis facing outward: `nu` steps around the axis, `nv` around
    /// the tube, two triangles per step.
    pub fn torus(major: f64, minor: f64, nu: usize, nv: usize) -> Mesh {
        use std::f64::consts::TAU;
        let mut positions = Vec::with_capacity(nu * nv * 3);
        for i in 0..nu {
            let u = i as f64 / nu as f64 * TAU;
            for j in 0..nv {
                let v = j as f64 / nv as f64 * TAU;
                let r = major + minor * v.cos();
                positions.extend_from_slice(&[
                    (r * u.cos()) as f32,
                    (r * u.sin()) as f32,
                    (minor * v.sin()) as f32,
                ]);
            }
        }
        let id = |i: usize, j: usize| ((i % nu) * nv + j % nv) as u32;
        let mut indices = Vec::with_capacity(nu * nv * 6);
        for i in 0..nu {
            for j in 0..nv {
                let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        Mesh { positions, indices }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Rng, det3_sign, sign_of, to_integers};
    use super::*;
    use num_bigint::BigInt;

    /// orient3d decided with exact integers: the sign of det[b − a, c − a, d − a].
    fn exact_orient3d(a: P3, b: P3, c: P3, d: P3) -> i32 {
        let v = to_integers(&[a, b, c, d].concat());
        let p: Vec<[BigInt; 3]> = v
            .chunks(3)
            .map(|c| [c[0].clone(), c[1].clone(), c[2].clone()])
            .collect();
        let row = |q: &[BigInt; 3]| [&q[0] - &p[0][0], &q[1] - &p[0][1], &q[2] - &p[0][2]];
        det3_sign([row(&p[1]), row(&p[2]), row(&p[3])])
    }

    fn exact_orient2d(a: P2, b: P2, c: P2) -> i32 {
        let v = to_integers(&[a, b, c].concat());
        let d = (&v[2] - &v[0]) * (&v[5] - &v[1]) - (&v[3] - &v[1]) * (&v[4] - &v[0]);
        sign_of(&d)
    }

    #[test]
    fn signs_follow_the_documented_conventions() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        // The normal (b − a) × (c − a) is +z.
        assert_eq!(orient3d(a, b, c, [0.3, 0.3, 1.0]), 1);
        assert_eq!(orient3d(a, b, c, [5.0, -7.0, -1e-30]), -1);
        assert_eq!(orient3d(a, b, c, [9.0, 9.0, 0.0]), 0);
        assert_eq!(orient3d(a, c, b, [0.3, 0.3, 1.0]), -1);
        assert_eq!(orient2d([0.0, 0.0], [1.0, 0.0], [0.0, 1.0]), 1);
        assert_eq!(orient2d([0.0, 0.0], [0.0, 1.0], [1.0, 0.0]), -1);
        assert_eq!(orient2d([0.0, 0.0], [1.0, 1.0], [3.0, 3.0]), 0);
    }

    #[test]
    fn projections_keep_the_normal_component_sign() {
        let mut rng = Rng(7);
        for _ in 0..1000 {
            let p: Vec<P3> = (0..3)
                .map(|_| [0, 1, 2].map(|_| rng.int(-5, 5) as f64))
                .collect();
            let n = normal(p[0], p[1], p[2]);
            for (axis, &component) in n.iter().enumerate() {
                let o = orient2d(
                    project(p[0], axis),
                    project(p[1], axis),
                    project(p[2], axis),
                );
                assert_eq!(o, sign(component), "{p:?} axis {axis}");
            }
        }
    }

    #[test]
    fn collinear_needs_all_three_projections() {
        let o = [0.0; 3];
        assert!(collinear(o, o, o));
        assert!(collinear(o, o, [1.0, 2.0, 3.0]));
        assert!(collinear(
            [1.0, 2.0, 3.0],
            [3.0, 6.0, 9.0],
            [-1.0, -2.0, -3.0]
        ));
        // Collinear seen along z (all y = 0) but not in space.
        assert!(!collinear(o, [1.0, 0.0, 0.0], [2.0, 0.0, 1.0]));
        // One part in 2^-40 off the line.
        assert!(!collinear(
            o,
            [1.0, 1.0, 1.0],
            [2.0, 2.0, 2.0 + 2f64.powi(-40)]
        ));
        assert_eq!(dominant_axis(o, [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]), None);
        assert_eq!(dominant_axis(o, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), Some(2));
        assert_eq!(dominant_axis(o, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]), Some(0));
    }

    #[test]
    fn dominant_axis_survives_a_misleading_f64_normal() {
        // A needle whose f64 normal is mostly rounding noise: the exact check still finds a
        // projection with area, or none when there truly is none.
        let a = [1e8, 1e8, 1e8];
        let b = [1e8 + 1.0, 1e8 + 1.0, 1e8 + 1.0];
        let c = [1e8 + 2.0, 1e8 + 2.0, 1e8 + 2.0 + 2f64.powi(-26)];
        let axis = dominant_axis(a, b, c).unwrap();
        assert_ne!(
            orient2d(project(a, axis), project(b, axis), project(c, axis)),
            0
        );
        assert_eq!(dominant_axis(a, b, [1e8 + 2.0; 3]), None);
    }

    #[test]
    fn orient3d_matches_exact_integers_on_random_and_nearly_coplanar_points() {
        let mut rng = Rng(11);
        let mut zeros = 0;
        let mut naive_wrong = 0;
        for round in 0..20_000 {
            let mut p: Vec<P3> = (0..4)
                .map(|_| [0, 1, 2].map(|_| rng.unit() * 2.0 - 1.0))
                .collect();
            match round % 4 {
                // d on the line through a and b (up to rounding).
                0 => p[3] = add(p[0], scale(sub(p[1], p[0]), 3.0)),
                // d in the plane up to rounding, then nudged by at most one unit in the last place.
                1 => {
                    let t = rng.unit();
                    let s = rng.unit();
                    let q = add(
                        p[0],
                        add(scale(sub(p[1], p[0]), t), scale(sub(p[2], p[0]), s)),
                    );
                    let k = rng.below(3) as usize;
                    p[3] = q;
                    p[3][k] = rng.nudge(q[k]);
                }
                // Integer grid: exactly coplanar quadruples are common.
                2 => {
                    for q in &mut p {
                        *q = [0, 1, 2].map(|_| rng.int(-2, 2) as f64);
                    }
                }
                _ => {}
            }
            let exact = exact_orient3d(p[0], p[1], p[2], p[3]);
            assert_eq!(orient3d(p[0], p[1], p[2], p[3]), exact, "{p:?}");
            zeros += (exact == 0) as usize;
            let naive = dot(normal(p[0], p[1], p[2]), sub(p[3], p[0]));
            naive_wrong += (sign(naive) != exact) as usize;
        }
        // The cases are hard enough that plain f64 gets some wrong, and exactly coplanar ones occur.
        assert!(zeros > 1000, "{zeros}");
        assert!(naive_wrong > 100, "{naive_wrong}");
    }

    #[test]
    fn orient2d_matches_exact_integers_on_nearly_collinear_points() {
        let mut rng = Rng(12);
        for _ in 0..20_000 {
            let a = [rng.unit(), rng.unit()];
            let b = [rng.unit() * 100.0, rng.unit() * 100.0];
            let t = rng.unit() * 3.0 - 1.0;
            let mut c = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let k = rng.below(2) as usize;
            c[k] = rng.nudge(c[k]);
            assert_eq!(
                orient2d(a, b, c),
                exact_orient2d(a, b, c),
                "{a:?} {b:?} {c:?}"
            );
        }
    }
}
