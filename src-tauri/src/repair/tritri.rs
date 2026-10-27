//! Exact triangle–triangle intersection: which triangles of a mesh cut through each other. Used by
//! Detect, Split-off and Remove self-intersections, Remove problem areas, and to tell whether Wrap
//! part surface has anything to do.
//!
//! Two triangles intersect when some point of one lies in the relative interior of the other:
//! they cross, one pierces the other, an edge of one lies inside the other's face, or they overlap
//! with positive area in a shared plane. Touching only along boundaries does not count: a shared
//! corner or edge, a corner on the other's edge, collinear overlapping edges (a T-junction), or
//! edges that cross at a point. Corners are shared when their coordinates are equal, whatever
//! their vertex index. Degenerate triangles (zero area) never intersect anything.
//!
//! Every decision is an exact orientation test on the input coordinates; no intersection point is
//! ever computed. Triangles in different planes each meet the other's plane in a piece of the line
//! where the planes cross; with each piece's ends written as "the edge from corner u to corner v
//! meets the other plane", one orient3d of two such ends tells which comes first along that line.
//! Triangles in one plane are compared in a projection that keeps the plane's orientations.

use super::bvh::{Bvh, triangle_boxes};
use super::exact::{P2, P3, collinear, corners, dominant_axis, orient2d, orient3d, project};
use super::{Control, RepairResult};
use crate::mesh::Mesh;

/// True when the two triangles intersect by the rule above. Exact.
pub fn intersect(a: &[P3; 3], b: &[P3; 3]) -> bool {
    // Corners the triangles share. A corner equal to one of a triangle's corners lies in its plane,
    // so its side needs no predicate.
    let mut a_shared = [false; 3];
    let mut b_shared = [false; 3];
    for (i, p) in a.iter().enumerate() {
        for (j, q) in b.iter().enumerate() {
            if p == q {
                a_shared[i] = true;
                b_shared[j] = true;
            }
        }
    }
    let sb = sides(a, b, b_shared);
    if one_side(sb) {
        return false;
    }
    if sb == [0; 3] {
        // All of b lies in a's plane: a has no area, or the triangles share a plane.
        return !collinear(a[0], a[1], a[2])
            && !collinear(b[0], b[1], b[2])
            && coplanar(a, b, a_shared, b_shared);
    }
    let sa = sides(b, a, a_shared);
    // All of a in b's plane while b is not all in a's: b has no area.
    if one_side(sa) || sa == [0; 3] {
        return false;
    }
    across(a, sa, b, sb)
}

/// The side of `t`'s plane each corner of `u` lies on (orient3d); a shared corner lies in it.
fn sides(t: &[P3; 3], u: &[P3; 3], shared: [bool; 3]) -> [i32; 3] {
    std::array::from_fn(|k| {
        if shared[k] {
            0
        } else {
            orient3d(t[0], t[1], t[2], u[k])
        }
    })
}

/// All three strictly on the same side.
fn one_side(s: [i32; 3]) -> bool {
    s[0] != 0 && s[0] == s[1] && s[1] == s[2]
}

/// Triangles with area in different planes. Each meets the other's plane in a piece of the line L
/// where the planes cross: a point or a segment. Points inside a piece that cuts through its
/// triangle lie in that triangle's relative interior and its ends on the triangle's border; a
/// piece that is only a corner or an edge lies entirely on the border. So the triangles intersect
/// exactly when some piece cuts through its triangle and the two pieces overlap in more than a
/// shared end: some end of one lies strictly before some end of the other along L, and some end
/// strictly after.
fn across(a: &[P3; 3], sa: [i32; 3], b: &[P3; 3], sb: [i32; 3]) -> bool {
    let (ends_a, count_a, cuts_a) = piece(sa);
    let (ends_b, count_b, cuts_b) = piece(sb);
    if !cuts_a && !cuts_b {
        return false;
    }
    let mut before = false;
    let mut after = false;
    for &(u, v) in &ends_a[..count_a] {
        for &(x, y) in &ends_b[..count_b] {
            // With n_a, n_b the planes' normals, p the end of a's piece on edge u→v and q the end
            // of b's piece on edge x→y: det[v − u, x − u, y − u] = λ (v − u)·n_b (y − x)·n_a where
            // q − p = λ (n_a × n_b). `piece` orients both edges towards the positive side of the
            // other plane, so both dot products are positive and the sign is the sign of λ.
            match orient3d(a[u], a[v], b[x], b[y]) {
                1 => after = true,
                -1 => before = true,
                _ => {}
            }
            if before && after {
                return true;
            }
        }
    }
    false
}

/// Where a triangle meets the other triangle's plane, from its corners' sides (not all zero, not
/// all strictly on one side): the ends of that piece, how many there are (one for a single corner),
/// and whether the piece cuts through the triangle's interior rather than being a corner or an
/// edge. An end (u, v) is the point where the edge from corner u to corner v meets the plane, with
/// u on the negative side or in the plane and v on the positive side or in it, not both in it.
fn piece(s: [i32; 3]) -> ([(usize, usize); 2], usize, bool) {
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
            if s[l] < 0 {
                ([(l, m), (l, n)], 2, true)
            } else {
                ([(m, l), (n, l)], 2, true)
            }
        }
        1 => {
            let z = s.iter().position(|&x| x == 0).unwrap_or(0);
            let (m, n) = ((z + 1) % 3, (z + 2) % 3);
            if s[m] != s[n] {
                // From corner z through the opposite edge.
                let (neg, pos) = if s[m] < 0 { (m, n) } else { (n, m) };
                ([(z, pos), (neg, pos)], 2, true)
            } else if s[m] > 0 {
                ([(z, m), (z, m)], 1, false)
            } else {
                ([(m, z), (m, z)], 1, false)
            }
        }
        _ => {
            // The edge between the two corners in the plane.
            let e = s.iter().position(|&x| x != 0).unwrap_or(0);
            let (y, z) = ((e + 1) % 3, (e + 2) % 3);
            if s[e] > 0 {
                ([(y, e), (z, e)], 2, false)
            } else {
                ([(e, y), (e, z)], 2, false)
            }
        }
    }
}

/// Triangles with area in one plane. They intersect exactly when they overlap with positive area,
/// that is when no line through one of their six edges has one triangle on each side (touching the
/// line allowed). Decided in a projection that keeps every orientation within the plane.
fn coplanar(a: &[P3; 3], b: &[P3; 3], a_shared: [bool; 3], b_shared: [bool; 3]) -> bool {
    let shared = a_shared.iter().filter(|&&s| s).count();
    if shared == 3 {
        // The same three points, in either orientation.
        return true;
    }
    let Some(axis) = dominant_axis(a[0], a[1], a[2]) else {
        return false;
    };
    let pa = a.map(|p| project(p, axis));
    let pb = b.map(|p| project(p, axis));
    if shared == 2 {
        // A shared edge: they overlap exactly when the third corners lie on the same side of it.
        let i = a_shared.iter().position(|&s| !s).unwrap_or(0);
        let j = b_shared.iter().position(|&s| !s).unwrap_or(0);
        let (s, t) = (pa[(i + 1) % 3], pa[(i + 2) % 3]);
        return orient2d(s, t, pa[i]) == orient2d(s, t, pb[j]);
    }
    !separated(&pa, &pb) && !separated(&pb, &pa)
}

/// True when the line through some edge of `t` has all of `u` on the side away from `t` or on it.
fn separated(t: &[P2; 3], u: &[P2; 3]) -> bool {
    let turn = orient2d(t[0], t[1], t[2]);
    (0..3).any(|i| {
        let (p, q) = (t[i], t[(i + 1) % 3]);
        u.iter().all(|&r| orient2d(p, q, r) * turn <= 0)
    })
}

/// Every intersecting pair (i < j) of the mesh's triangles, sorted.
pub fn intersecting_pairs(mesh: &Mesh, control: &mut Control) -> RepairResult<Vec<(u32, u32)>> {
    control.check()?;
    let bvh = Bvh::new(&triangle_boxes(mesh));
    control.progress(0.2);
    control.check()?;
    // The pair tests run on several threads, which cannot reach `control`; on a million triangles
    // they take well under a second, so a cancel waits at most that long.
    let pairs = bvh.self_pairs_parallel(|i, j| intersect(&corners(mesh, i), &corners(mesh, j)));
    control.progress(1.0);
    control.check()?;
    Ok(pairs)
}

/// The triangles that belong to any intersecting pair, sorted.
pub fn intersecting_triangles(mesh: &Mesh, control: &mut Control) -> RepairResult<Vec<u32>> {
    let pairs = intersecting_pairs(mesh, control)?;
    let mut hit = vec![false; mesh.triangle_count()];
    for &(i, j) in &pairs {
        hit[i as usize] = true;
        hit[j as usize] = true;
    }
    Ok((0..mesh.triangle_count() as u32)
        .filter(|&t| hit[t as usize])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;
    use crate::repair::exact::testing::{Rng, to_integers, torus};
    use crate::repair::silent;
    use num_bigint::BigInt;
    use std::time::Instant;

    /// The rule decided from first principles in exact integer arithmetic, sharing nothing with the
    /// code under test.
    ///
    /// A ∩ B is the convex set of points x with n_a·x = c_a, n_b·x = c_b (the two planes) and
    /// w·x ≥ c for the six edge half-spaces (w = n × edge points into the triangle). Its corners are
    /// the feasible points where three constraints with independent normals are tight, found by
    /// trying all 56 triples with Cramer's rule. The corners' average m lies in the relative
    /// interior of A ∩ B, and A ∩ B meets relint(A) exactly when m does (convexity). Since every
    /// corner satisfies every edge constraint, m satisfies one strictly exactly when some corner
    /// does. So: the triangles intersect when A ∩ B has a corner and, for A or for B, each of its
    /// three edge constraints is strict at some corner.
    mod reference {
        use num_traits::Signed;

        pub type V<T> = [T; 3];

        fn sub<T: Signed + Clone>(a: &V<T>, b: &V<T>) -> V<T> {
            [
                a[0].clone() - b[0].clone(),
                a[1].clone() - b[1].clone(),
                a[2].clone() - b[2].clone(),
            ]
        }

        fn dot<T: Signed + Clone>(a: &V<T>, b: &V<T>) -> T {
            a[0].clone() * b[0].clone() + a[1].clone() * b[1].clone() + a[2].clone() * b[2].clone()
        }

        fn cross<T: Signed + Clone>(a: &V<T>, b: &V<T>) -> V<T> {
            [
                a[1].clone() * b[2].clone() - a[2].clone() * b[1].clone(),
                a[2].clone() * b[0].clone() - a[0].clone() * b[2].clone(),
                a[0].clone() * b[1].clone() - a[1].clone() * b[0].clone(),
            ]
        }

        fn det<T: Signed + Clone>(rows: &[V<T>; 3]) -> T {
            dot(&rows[0], &cross(&rows[1], &rows[2]))
        }

        /// n·x = c, or the half-space n·x ≥ c.
        struct Plane<T> {
            n: V<T>,
            c: T,
        }

        pub fn intersect<T: Signed + Clone>(a: &[V<T>; 3], b: &[V<T>; 3]) -> bool {
            let na = cross(&sub(&a[1], &a[0]), &sub(&a[2], &a[0]));
            let nb = cross(&sub(&b[1], &b[0]), &sub(&b[2], &b[0]));
            if na.iter().all(|x| x.is_zero()) || nb.iter().all(|x| x.is_zero()) {
                return false;
            }
            let mut planes = vec![
                Plane {
                    c: dot(&na, &a[0]),
                    n: na.clone(),
                },
                Plane {
                    c: dot(&nb, &b[0]),
                    n: nb.clone(),
                },
            ];
            for (t, n) in [(a, &na), (b, &nb)] {
                for i in 0..3 {
                    let w = cross(n, &sub(&t[(i + 1) % 3], &t[i]));
                    planes.push(Plane {
                        c: dot(&w, &t[i]),
                        n: w,
                    });
                }
            }
            let mut found = false;
            let mut strict = [false; 6];
            for i in 0..8 {
                for j in i + 1..8 {
                    for k in j + 1..8 {
                        let m = [
                            planes[i].n.clone(),
                            planes[j].n.clone(),
                            planes[k].n.clone(),
                        ];
                        let d = det(&m);
                        if d.is_zero() {
                            continue;
                        }
                        let c = [
                            planes[i].c.clone(),
                            planes[j].c.clone(),
                            planes[k].c.clone(),
                        ];
                        let x: V<T> = std::array::from_fn(|axis| {
                            let mut r = m.clone();
                            for row in 0..3 {
                                r[row][axis] = c[row].clone();
                            }
                            det(&r)
                        });
                        // Each plane's n·p − c at the point p = x / d, times |d|.
                        let value = |p: &Plane<T>| {
                            let v = dot(&p.n, &x) - p.c.clone() * d.clone();
                            if d.is_negative() { -v } else { v }
                        };
                        if !value(&planes[0]).is_zero() || !value(&planes[1]).is_zero() {
                            continue;
                        }
                        let values: Vec<T> = planes[2..].iter().map(value).collect();
                        if values.iter().any(|v| v.is_negative()) {
                            continue;
                        }
                        found = true;
                        for (e, v) in values.iter().enumerate() {
                            strict[e] |= v.is_positive();
                        }
                    }
                }
            }
            found && (strict[..3].iter().all(|&s| s) || strict[3..].iter().all(|&s| s))
        }
    }

    type T3 = [P3; 3];

    fn reference_f64(a: &T3, b: &T3) -> bool {
        let v = to_integers(&[a.concat(), b.concat()].concat());
        let p: Vec<[BigInt; 3]> = v
            .chunks(3)
            .map(|c| [c[0].clone(), c[1].clone(), c[2].clone()])
            .collect();
        reference::intersect(
            &[p[0].clone(), p[1].clone(), p[2].clone()],
            &[p[3].clone(), p[4].clone(), p[5].clone()],
        )
    }

    fn reference_int<T: num_traits::Signed + Clone + From<i64>>(
        a: &[[i64; 3]; 3],
        b: &[[i64; 3]; 3],
    ) -> bool {
        let conv = |t: &[[i64; 3]; 3]| t.map(|p| p.map(T::from));
        reference::intersect(&conv(a), &conv(b))
    }

    fn to_f64(t: &[[i64; 3]; 3]) -> T3 {
        t.map(|p| p.map(|x| x as f64))
    }

    const PERMUTATIONS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [1, 2, 0],
        [2, 0, 1],
        [0, 2, 1],
        [2, 1, 0],
        [1, 0, 2],
    ];

    fn permuted(t: &T3, p: usize) -> T3 {
        PERMUTATIONS[p].map(|k| t[k])
    }

    /// `intersect` on one variant of the pair: corners permuted (odd permutations reverse the
    /// orientation) and possibly the triangles swapped.
    fn variant(a: &T3, b: &T3, pa: usize, pb: usize, swap: bool) -> bool {
        let (a, b) = (permuted(a, pa), permuted(b, pb));
        if swap {
            intersect(&b, &a)
        } else {
            intersect(&a, &b)
        }
    }

    /// Checks `intersect` against `expected` on the pair, on one random variant, and on all 72
    /// variants when `all` is set. Returns a description of the first disagreement.
    fn check(rng: &mut Rng, a: &T3, b: &T3, expected: bool, all: bool) -> Result<(), String> {
        if intersect(a, b) != expected {
            return Err(format!("{a:?} {b:?}: expected {expected}"));
        }
        let (pa, pb, swap) = (
            rng.below(6) as usize,
            rng.below(6) as usize,
            rng.chance(0.5),
        );
        if variant(a, b, pa, pb, swap) != expected {
            return Err(format!(
                "{a:?} {b:?}: expected {expected} for permutations {pa} {pb} swap {swap}"
            ));
        }
        if all {
            for pa in 0..6 {
                for pb in 0..6 {
                    for swap in [false, true] {
                        if variant(a, b, pa, pb, swap) != expected {
                            return Err(format!(
                                "{a:?} {b:?}: expected {expected} for permutations {pa} {pb} \
                                 swap {swap}"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Some features of a pair, to show which cases a random family exercised.
    #[derive(Default, Debug)]
    struct Tally {
        pairs: usize,
        intersecting: usize,
        coplanar: usize,
        coplanar_intersecting: usize,
        shared_corner: usize,
        shared_edge: usize,
        degenerate: usize,
        touching: usize,
    }

    impl Tally {
        fn add(&mut self, a: &T3, b: &T3, expected: bool) {
            self.pairs += 1;
            self.intersecting += expected as usize;
            let shared = a.iter().filter(|p| b.contains(p)).count();
            self.shared_corner += (shared == 1) as usize;
            self.shared_edge += (shared == 2) as usize;
            let degenerate = collinear(a[0], a[1], a[2]) || collinear(b[0], b[1], b[2]);
            self.degenerate += degenerate as usize;
            if !degenerate && b.iter().all(|&q| orient3d(a[0], a[1], a[2], q) == 0) {
                self.coplanar += 1;
                self.coplanar_intersecting += expected as usize;
            }
            // Boundary contact without intersection: the closed triangles still meet.
            if !expected && !degenerate && closed_meet(a, b) {
                self.touching += 1;
            }
        }
    }

    /// Whether the closed triangles have any point in common: some edge of one meets the other
    /// closed triangle (exact, for the tally only).
    fn closed_meet(a: &T3, b: &T3) -> bool {
        let edge_meets = |p: P3, q: P3, t: &T3| {
            let sp = orient3d(t[0], t[1], t[2], p);
            let sq = orient3d(t[0], t[1], t[2], q);
            if sp * sq > 0 {
                return false;
            }
            if sp == 0 && sq == 0 {
                // In the plane: a corner on the triangle or the segment crossing an edge.
                return in_closed(p, t)
                    || in_closed(q, t)
                    || (0..3).any(|i| segments_meet(p, q, t[i], t[(i + 1) % 3], t));
            }
            let s = [0, 1, 2].map(|i| orient3d(p, q, t[i], t[(i + 1) % 3]));
            if sp == 0 {
                return in_closed(p, t);
            }
            if sq == 0 {
                return in_closed(q, t);
            }
            !(s.contains(&1) && s.contains(&-1))
        };
        (0..3).any(|i| edge_meets(a[i], a[(i + 1) % 3], b) || edge_meets(b[i], b[(i + 1) % 3], a))
    }

    fn in_closed(p: P3, t: &T3) -> bool {
        let Some(axis) = dominant_axis(t[0], t[1], t[2]) else {
            return false;
        };
        let q = project(p, axis);
        let s = [0, 1, 2].map(|i| orient2d(project(t[i], axis), project(t[(i + 1) % 3], axis), q));
        !(s.contains(&1) && s.contains(&-1))
    }

    fn segments_meet(p: P3, q: P3, r: P3, s: P3, t: &T3) -> bool {
        let Some(axis) = dominant_axis(t[0], t[1], t[2]) else {
            return false;
        };
        let [p, q, r, s] = [p, q, r, s].map(|x| project(x, axis));
        let (d1, d2) = (orient2d(p, q, r), orient2d(p, q, s));
        let (d3, d4) = (orient2d(r, s, p), orient2d(r, s, q));
        if d1 * d2 < 0 && d3 * d4 < 0 {
            return true;
        }
        let on = |a: P2, b: P2, c: P2| {
            orient2d(a, b, c) == 0
                && c[0] >= a[0].min(b[0])
                && c[0] <= a[0].max(b[0])
                && c[1] >= a[1].min(b[1])
                && c[1] <= a[1].max(b[1])
        };
        on(p, q, r) || on(p, q, s) || on(r, s, p) || on(r, s, q)
    }

    /// Pair counts: the full numbers in release builds, a tenth in debug builds so the default
    /// `cargo test` stays quick.
    fn count(release: usize) -> usize {
        if cfg!(debug_assertions) {
            release / 10
        } else {
            release
        }
    }

    type IntPair = ([[i64; 3]; 3], [[i64; 3]; 3]);

    /// Runs `pairs` random integer pairs from `make` against the reference (in i64, or i128 when
    /// `wide`), with every variant checked on one pair in eight. Returns the tally.
    fn run_family(
        name: &str,
        seed: u64,
        pairs: usize,
        wide: bool,
        mut make: impl FnMut(&mut Rng) -> IntPair,
    ) -> Tally {
        let mut rng = Rng(seed);
        let mut tally = Tally::default();
        let mut failures = Vec::new();
        for n in 0..pairs {
            let (ia, ib) = make(&mut rng);
            let expected = if wide {
                reference_int::<i128>(&ia, &ib)
            } else {
                reference_int::<i64>(&ia, &ib)
            };
            let (a, b) = (to_f64(&ia), to_f64(&ib));
            tally.add(&a, &b, expected);
            if let Err(e) = check(&mut rng, &a, &b, expected, n % 8 == 0) {
                failures.push(e);
            }
        }
        println!("{name}: {tally:?}");
        assert!(
            failures.is_empty(),
            "{name}: {} disagreements, first: {}",
            failures.len(),
            failures[0]
        );
        tally
    }

    fn grid_point(rng: &mut Rng, r: i64) -> [i64; 3] {
        [0, 1, 2].map(|_| rng.int(-r, r))
    }

    #[test]
    fn matches_reference_on_grid_3() {
        let t = run_family("grid ±3", 101, count(600_000), false, |rng| {
            (
                [0, 1, 2].map(|_| grid_point(rng, 3)),
                [0, 1, 2].map(|_| grid_point(rng, 3)),
            )
        });
        assert!(
            t.intersecting > t.pairs / 20 && t.touching > t.pairs / 100,
            "{t:?}"
        );
        assert!(
            t.coplanar > 0 && t.shared_corner > 0 && t.degenerate > 0,
            "{t:?}"
        );
    }

    #[test]
    fn matches_reference_on_grid_2() {
        let t = run_family("grid ±2", 102, count(500_000), false, |rng| {
            (
                [0, 1, 2].map(|_| grid_point(rng, 2)),
                [0, 1, 2].map(|_| grid_point(rng, 2)),
            )
        });
        assert!(
            t.intersecting > t.pairs / 10 && t.touching > t.pairs / 50,
            "{t:?}"
        );
        assert!(t.coplanar_intersecting > 0 && t.shared_edge > 0, "{t:?}");
    }

    #[test]
    fn matches_reference_with_shared_corners() {
        // b borrows corners of a: shared corners, shared edges and duplicates all the time.
        let t = run_family("shared corners", 103, count(400_000), false, |rng| {
            let a = [0, 1, 2].map(|_| grid_point(rng, 2));
            let b = [0, 1, 2].map(|_| {
                if rng.chance(0.45) {
                    a[rng.below(3) as usize]
                } else {
                    grid_point(rng, 2)
                }
            });
            (a, b)
        });
        assert!(
            t.shared_edge > t.pairs / 10 && t.shared_corner > t.pairs / 5,
            "{t:?}"
        );
        assert!(t.coplanar_intersecting > t.pairs / 100, "{t:?}");
    }

    #[test]
    fn matches_reference_in_shared_planes() {
        // All six points on one plane: overlaps, touching edges, T-junctions in 2D.
        let t = run_family("coplanar", 104, count(400_000), false, |rng| {
            let kind = rng.below(4);
            let k = rng.int(-1, 1);
            let mut point = || {
                let (x, y) = (rng.int(-2, 2), rng.int(-2, 2));
                match kind {
                    0 => [x, y, k],
                    1 => [x, k - x, y],
                    2 => [x, y, k - x - y],
                    _ => [x, y, x + 2 * y - k],
                }
            };
            ([point(), point(), point()], [point(), point(), point()])
        });
        assert!(t.coplanar > t.pairs / 2, "{t:?}");
        assert!(
            t.coplanar_intersecting > t.pairs / 10 && t.touching > t.pairs / 50,
            "{t:?}"
        );
    }

    #[test]
    fn matches_reference_near_each_other() {
        // b's corners next to a's: crossings and piercings at short range.
        let t = run_family("near", 105, count(400_000), false, |rng| {
            let a = [0, 1, 2].map(|_| grid_point(rng, 3));
            let b = [0, 1, 2].map(|_| {
                let p = a[rng.below(3) as usize];
                [0, 1, 2].map(|k| p[k] + rng.int(-1, 1))
            });
            (a, b)
        });
        assert!(t.intersecting > t.pairs / 10, "{t:?}");
    }

    #[test]
    fn matches_reference_on_a_fine_grid() {
        // Coordinates up to ±128: mostly general position, still exact in i128.
        run_family("fine grid", 106, count(200_000), true, |rng| {
            let a = [0, 1, 2].map(|_| grid_point(rng, 128));
            let b = [0, 1, 2].map(|_| {
                if rng.chance(0.2) {
                    a[rng.below(3) as usize]
                } else {
                    grid_point(rng, 128)
                }
            });
            (a, b)
        });
    }

    #[test]
    fn matches_reference_on_real_coordinates() {
        let mut rng = Rng(107);
        let mut tally = Tally::default();
        let mut failures = Vec::new();
        // The reference runs in big integers here, which is slow in debug builds.
        let pairs = if cfg!(debug_assertions) { 1500 } else { 60_000 };
        for n in 0..pairs {
            let (a, b): (T3, T3) = match n % 3 {
                // Random doubles with full mantissas (kept in one binade so the exact integers stay
                // small enough to be quick).
                0 => {
                    let mut p = || [0, 1, 2].map(|_| rng.unit() + 1.0);
                    ([p(), p(), p()], [p(), p(), p()])
                }
                // Random floats in a small box, as mesh coordinates are.
                1 => {
                    let mut p = || [0, 1, 2].map(|_| (rng.unit() * 4.0 + 10.0) as f32 as f64);
                    ([p(), p(), p()], [p(), p(), p()])
                }
                // A degenerate grid configuration, moved to [2, 6] so a unit in the last place is a
                // relative nudge, with a few coordinates nudged: nearly coplanar, nearly touching,
                // nearly sharing corners (and some left exactly degenerate).
                _ => {
                    let ia = [0, 1, 2].map(|_| grid_point(&mut rng, 2));
                    let ib = [0, 1, 2].map(|_| {
                        if rng.chance(0.4) {
                            ia[rng.below(3) as usize]
                        } else {
                            grid_point(&mut rng, 2)
                        }
                    });
                    let shift = |t: &[[i64; 3]; 3]| t.map(|p| p.map(|x| x as f64 + 4.0));
                    let (mut a, mut b) = (shift(&ia), shift(&ib));
                    for _ in 0..rng.int(1, 3) {
                        let t = if rng.chance(0.5) { &mut a } else { &mut b };
                        let (c, k) = (rng.below(3) as usize, rng.below(3) as usize);
                        t[c][k] = rng.nudge(t[c][k]);
                    }
                    (a, b)
                }
            };
            let expected = reference_f64(&a, &b);
            tally.add(&a, &b, expected);
            if let Err(e) = check(&mut rng, &a, &b, expected, n % 8 == 0) {
                failures.push(e);
            }
        }
        println!("real coordinates: {tally:?}");
        assert!(
            failures.is_empty(),
            "{} disagreements, first: {}",
            failures.len(),
            failures[0]
        );
        assert!(tally.intersecting > tally.pairs / 20, "{tally:?}");
    }

    #[test]
    fn far_from_the_origin_and_scaled_the_answers_stay() {
        // Translations by large integers and scalings by powers of two are exact in f64, so every
        // answer must stay; far from the origin plain floating point cancels badly.
        let mut rng = Rng(110);
        let shifts = [
            [1e6, -2e6, 3e6],
            [16_777_216.0, 0.0, -16_777_216.0],
            [-1e9, 1e9, 7.0],
        ];
        for n in 0..count(200_000) {
            let a = [0, 1, 2].map(|_| grid_point(&mut rng, 2));
            let b = [0, 1, 2].map(|_| {
                if rng.chance(0.4) {
                    a[rng.below(3) as usize]
                } else {
                    grid_point(&mut rng, 2)
                }
            });
            let expected = reference_int::<i64>(&a, &b);
            let shift = shifts[n % 3];
            // Multiples of 2^-10 below 2^32 need at most 42 bits: still exact.
            let s = 2f64.powi(rng.int(-10, 30) as i32);
            let moved =
                |t: &[[i64; 3]; 3]| t.map(|p| [0, 1, 2].map(|k| p[k] as f64 * s + shift[k]));
            let (ma, mb) = (moved(&a), moved(&b));
            if let Err(e) = check(&mut rng, &ma, &mb, expected, n % 16 == 0) {
                panic!("shift {shift:?} scale {s}: {e}");
            }
        }
    }

    #[test]
    fn reference_is_permutation_invariant() {
        let mut rng = Rng(108);
        for _ in 0..2000 {
            let a = [0, 1, 2].map(|_| grid_point(&mut rng, 2));
            let b = [0, 1, 2].map(|_| {
                if rng.chance(0.4) {
                    a[rng.below(3) as usize]
                } else {
                    grid_point(&mut rng, 2)
                }
            });
            let expected = reference_int::<i64>(&a, &b);
            let (pa, pb) = (rng.below(6) as usize, rng.below(6) as usize);
            let a2 = PERMUTATIONS[pa].map(|k| a[k]);
            let b2 = PERMUTATIONS[pb].map(|k| b[k]);
            assert_eq!(reference_int::<i64>(&a2, &b2), expected);
            assert_eq!(reference_int::<i64>(&b2, &a2), expected);
        }
    }

    /// A named case: the answer, checked on all 72 variants and against the reference.
    fn case(name: &str, a: T3, b: T3, expected: bool) {
        assert_eq!(
            reference_f64(&a, &b),
            expected,
            "reference disagrees on {name}"
        );
        let mut rng = Rng(0);
        if let Err(e) = check(&mut rng, &a, &b, expected, true) {
            panic!("{name}: {e}");
        }
    }

    const O: P3 = [0.0, 0.0, 0.0];
    const X2: P3 = [2.0, 0.0, 0.0];
    const Y2: P3 = [0.0, 2.0, 0.0];
    /// The base triangle: in z = 0, legs of length 2 along x and y.
    const BASE: T3 = [O, X2, Y2];

    #[test]
    fn named_crossing_and_piercing() {
        case(
            "crossing",
            BASE,
            [[0.5, 0.5, -1.0], [0.5, 0.5, 1.0], [-1.0, -1.0, 0.5]],
            true,
        );
        case(
            "piercing edge",
            BASE,
            [[0.5, 0.5, -1.0], [0.5, 0.5, 1.0], [5.0, 5.0, 3.0]],
            true,
        );
        case(
            "miss beside",
            BASE,
            [[3.0, 3.0, -1.0], [3.0, 3.0, 1.0], [5.0, 5.0, 0.0]],
            false,
        );
        case(
            "parallel planes",
            BASE,
            [[0.0, 0.0, 1.0], [2.0, 0.0, 1.0], [0.0, 2.0, 1.0]],
            false,
        );
        case(
            "corner poking the face from one side",
            BASE,
            [[0.5, 0.5, 0.0], [0.5, 0.5, 1.0], [1.0, 0.5, 1.0]],
            true,
        );
        case(
            "corner touching the face's edge from one side",
            BASE,
            [[1.0, 0.0, 0.0], [1.0, 0.5, 1.0], [1.5, 0.5, 1.0]],
            false,
        );
        case(
            "corner touching the face's corner from one side",
            BASE,
            [X2, [2.0, 0.5, 1.0], [2.5, 0.5, 1.0]],
            false,
        );
    }

    #[test]
    fn named_edges_in_faces() {
        case(
            "edge inside the face, triangle standing on it",
            BASE,
            [[0.5, 0.25, 0.0], [1.0, 0.25, 0.0], [0.75, 0.25, 1.0]],
            true,
        );
        case(
            "edge inside the face, partly",
            BASE,
            [[1.0, 0.5, 0.0], [3.0, 0.5, 0.0], [2.0, 0.5, 1.0]],
            true,
        );
        case(
            "edge lying on the face's edge (T-junction, other plane)",
            BASE,
            [[0.5, 0.0, 0.0], [3.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            false,
        );
        case(
            "edge lying on the face's edge, triangle hanging below the other plane",
            BASE,
            [[0.5, 0.0, 0.0], [3.0, 0.0, 0.0], [1.0, -1.0, -1.0]],
            false,
        );
        case(
            "edges crossing at one point, planes apart",
            [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
            [[0.0, -1.0, 1.0], [0.0, 1.0, -1.0], [0.0, 1.0, 1.0]],
            false,
        );
        case(
            "edges crossing at one point, interiors overlapping beyond it",
            [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
            [[0.0, -1.0, 1.0], [0.0, 1.0, -1.0], [0.0, -1.0, -1.0]],
            true,
        );
    }

    #[test]
    fn named_shared_corners_and_edges() {
        case(
            "shared edge, dihedral angle",
            BASE,
            [O, X2, [0.0, 0.0, 2.0]],
            false,
        );
        case(
            "shared edge, flat (coplanar, opposite sides)",
            BASE,
            [O, X2, [1.0, -1.0, 0.0]],
            false,
        );
        case(
            "shared edge, folded flat onto the other (coplanar, same side)",
            BASE,
            [O, X2, [1.0, 0.5, 0.0]],
            true,
        );
        case(
            "shared edge, fold sticking out of the other",
            BASE,
            [X2, O, [3.0, 3.0, 0.0]],
            true,
        );
        case(
            "shared vertex, crossing through the other",
            BASE,
            [O, [1.0, 1.0, 1.0], [1.0, 1.0, -1.0]],
            true,
        );
        case(
            "shared vertex, touching only there",
            BASE,
            [O, [-1.0, -1.0, 1.0], [-1.0, -1.0, -1.0]],
            false,
        );
        case(
            "shared vertex, coplanar, touching only there",
            BASE,
            [O, [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
            false,
        );
        case(
            "shared vertex, coplanar, overlapping",
            BASE,
            [O, [1.0, 2.0, 0.0], [3.0, 3.0, 0.0]],
            true,
        );
        case(
            "shared vertex, the other's edge lying along an edge",
            BASE,
            [O, [1.0, 0.0, 0.0], [0.0, -1.0, 1.0]],
            false,
        );
        case(
            "shared corner by coordinates, not by index",
            BASE,
            [[2.0, 0.0, 0.0], [3.0, 1.0, 1.0], [3.0, -1.0, 1.0]],
            false,
        );
    }

    #[test]
    fn named_coplanar_cases() {
        case(
            "coplanar overlap",
            BASE,
            [[1.0, 1.0, 0.0], [-1.0, 0.5, 0.0], [0.5, -1.0, 0.0]],
            true,
        );
        case(
            "coplanar, one inside the other",
            BASE,
            [[0.25, 0.25, 0.0], [1.0, 0.25, 0.0], [0.25, 1.0, 0.0]],
            true,
        );
        case(
            "coplanar T-junction: a corner on the other's edge, outside",
            BASE,
            [[1.0, 0.0, 0.0], [2.0, -1.0, 0.0], [0.0, -1.0, 0.0]],
            false,
        );
        case(
            "coplanar, collinear overlapping edges, opposite sides",
            BASE,
            [[1.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, -1.0, 0.0]],
            false,
        );
        case(
            "coplanar, collinear overlapping edges, same side",
            BASE,
            [[1.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]],
            true,
        );
        case(
            "coplanar, apart",
            BASE,
            [[3.0, 3.0, 0.0], [4.0, 3.0, 0.0], [3.0, 4.0, 0.0]],
            false,
        );
        case(
            "coplanar, corner to corner",
            BASE,
            [Y2, [-1.0, 3.0, 0.0], [-2.0, 2.0, 0.0]],
            false,
        );
        case("duplicated triangle", BASE, BASE, true);
        case("duplicated triangle, reversed", BASE, [O, Y2, X2], true);
    }

    #[test]
    fn named_degenerate_triangles_never_intersect() {
        let needle = [[-1.0, 0.5, 0.5], [3.0, 0.5, 0.5], [1.0, 0.5, 0.5]];
        case("needle through the face", BASE, needle, false);
        case(
            "needle with a repeated corner",
            BASE,
            [[0.5, 0.5, -1.0], [0.5, 0.5, 1.0], [0.5, 0.5, 1.0]],
            false,
        );
        case("point inside the face", BASE, [[0.5, 0.5, 0.0]; 3], false);
        case(
            "needle in the plane",
            BASE,
            [[-1.0, 0.5, 0.0], [3.0, 0.5, 0.0], [1.0, 0.5, 0.0]],
            false,
        );
        case(
            "two needles",
            needle,
            [[1.0, 0.0, 0.5], [1.0, 1.0, 0.5], [1.0, 2.0, 0.5]],
            false,
        );
    }

    fn brute_pairs(mesh: &Mesh) -> Vec<(u32, u32)> {
        let n = mesh.triangle_count() as u32;
        let mut out = Vec::new();
        for i in 0..n {
            for j in i + 1..n {
                if intersect(&corners(mesh, i), &corners(mesh, j)) {
                    out.push((i, j));
                }
            }
        }
        out
    }

    #[test]
    fn interpenetrating_cubes() {
        let a = make_box([0.0; 3], [20.0; 3]);
        assert!(silent(|c| intersecting_pairs(&a, c)).unwrap().is_empty());
        for offset in [[10.0, 0.0, 0.0], [10.0, 10.0, 10.0], [5.0, 3.0, -7.0]] {
            let b = make_box(offset, offset.map(|x| x + 20.0));
            let both = concat(&[&a, &b]).unwrap();
            let pairs = silent(|c| intersecting_pairs(&both, c)).unwrap();
            assert_eq!(pairs, brute_pairs(&both), "offset {offset:?}");
            // Only pairs across the two cubes, and the reference agrees on every pair.
            assert!(!pairs.is_empty());
            assert!(pairs.iter().all(|&(i, j)| i < 12 && j >= 12));
            for i in 0..12 {
                for j in 12..24 {
                    let (p, q) = (corners(&both, i), corners(&both, j));
                    assert_eq!(
                        pairs.contains(&(i, j)),
                        reference_f64(&p, &q),
                        "offset {offset:?} pair {i} {j}"
                    );
                }
            }
        }
        // Offset 10 in x: the two y = 0 faces overlap in the plane, so their triangles intersect;
        // so do a's x = 20 face and b's x = 10 face with the faces they pass through.
        let b = make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0]);
        let both = concat(&[&a, &b]).unwrap();
        let triangles = silent(|c| intersecting_triangles(&both, c)).unwrap();
        // -y faces (4, 5 and 16, 17) and +x of a (8, 9), -x of b (22, 23).
        for t in [4, 5, 16, 17, 8, 9, 22, 23] {
            assert!(triangles.contains(&t), "{t} in {triangles:?}");
        }
        // Two cubes side by side sharing a face area intersect there; apart they do not.
        let c = make_box([20.0, 0.0, 0.0], [40.0, 20.0, 20.0]);
        let side = concat(&[&a, &c]).unwrap();
        assert_eq!(
            silent(|ctl| intersecting_pairs(&side, ctl)).unwrap(),
            brute_pairs(&side)
        );
        assert!(!brute_pairs(&side).is_empty());
        let d = make_box([20.5, 0.0, 0.0], [40.0, 20.0, 20.0]);
        assert!(brute_pairs(&concat(&[&a, &d]).unwrap()).is_empty());
    }

    #[test]
    fn mesh_pairs_match_brute_force_on_random_triangle_soups() {
        let mut rng = Rng(109);
        for n in [0usize, 1, 2, 10, 60, 300] {
            let mut mesh = Mesh::default();
            for _ in 0..n * 3 {
                mesh.positions
                    .extend([0, 1, 2].map(|_| rng.int(-3, 3) as f32));
            }
            // Reuse vertices now and then so triangles share corners by index too.
            mesh.indices = (0..n as u32 * 3)
                .map(|i| {
                    if rng.chance(0.3) {
                        rng.below(n as u64 * 3) as u32
                    } else {
                        i
                    }
                })
                .collect();
            let pairs = silent(|c| intersecting_pairs(&mesh, c)).unwrap();
            assert_eq!(pairs, brute_pairs(&mesh), "n {n}");
        }
    }

    #[test]
    fn duplicates_folds_and_clean_surfaces() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        // A duplicated triangle intersects its original (indices shared or not).
        let mut doubled = cube.clone();
        doubled.indices.extend_from_slice(&[0, 2, 1]);
        assert_eq!(
            silent(|c| intersecting_pairs(&doubled, c)).unwrap(),
            vec![(0, 12)]
        );
        let mut copied = cube.clone();
        copied
            .positions
            .extend_from_slice(&[0.0, 0.0, 0.0, 20.0, 20.0, 0.0, 20.0, 0.0, 0.0]);
        copied.indices.extend_from_slice(&[8, 9, 10]);
        assert_eq!(
            silent(|c| intersecting_pairs(&copied, c)).unwrap(),
            vec![(0, 12)]
        );
        // Folding one bottom triangle onto its neighbour (corner 3 moved into the other's half).
        let mut folded = cube.clone();
        folded.positions[3 * 3] = 15.0;
        folded.positions[3 * 3 + 1] = 5.0;
        let triangles = silent(|c| intersecting_triangles(&folded, c)).unwrap();
        assert!(
            triangles.contains(&0) && triangles.contains(&1),
            "{triangles:?}"
        );
        // A smooth closed surface has none.
        let ring = torus(30.0, 10.0, 120, 60);
        assert!(silent(|c| intersecting_pairs(&ring, c)).unwrap().is_empty());
        // Degenerate triangles in a mesh are ignored.
        let mut needles = cube.clone();
        needles.indices.extend_from_slice(&[0, 0, 6, 0, 6, 6]);
        assert!(
            silent(|c| intersecting_pairs(&needles, c))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cancel_stops_the_search() {
        use std::sync::atomic::AtomicBool;
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        let cube = make_box([0.0; 3], [1.0; 3]);
        assert_eq!(
            intersecting_pairs(&cube, &mut control),
            Err(crate::repair::RepairError::Cancelled)
        );
    }

    /// Real models: set TRICKY3D_MODELS to STL paths separated by `;` and run in release with
    /// `cargo test --release --lib repair::tritri -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_models() {
        let Ok(paths) = std::env::var("TRICKY3D_MODELS") else {
            println!("TRICKY3D_MODELS is not set");
            return;
        };
        for path in paths.split(';').filter(|p| !p.is_empty()) {
            let loaded = crate::formats::stl::read_path(
                std::path::Path::new(path),
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            let mesh = &loaded.objects[0].mesh;
            let started = Instant::now();
            let pairs = silent(|c| intersecting_pairs(mesh, c)).unwrap();
            let elapsed = started.elapsed();
            let mut hit = vec![false; mesh.triangle_count()];
            for &(i, j) in &pairs {
                hit[i as usize] = true;
                hit[j as usize] = true;
            }
            println!(
                "{path}: {} triangles, {} intersecting pairs, {} intersecting triangles, in {elapsed:?}",
                mesh.triangle_count(),
                pairs.len(),
                hit.iter().filter(|&&h| h).count()
            );
        }
    }

    /// Two overlapping tori with a million triangles in total, in release:
    /// `cargo test --release --lib repair::tritri -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn million_triangle_timing() {
        let clean = torus(30.0, 10.0, 1000, 500);
        let started = Instant::now();
        let pairs = silent(|c| intersecting_pairs(&clean, c)).unwrap();
        println!(
            "clean torus, {} triangles: {} intersecting pairs in {:?}",
            clean.triangle_count(),
            pairs.len(),
            started.elapsed()
        );
        assert!(pairs.is_empty());

        let mut other = torus(30.0, 10.0, 708, 353);
        crate::ops::translate(&mut other, [12.0, 7.0, 3.0]);
        let half = torus(30.0, 10.0, 708, 354);
        let both = concat(&[&half, &other]).unwrap();
        let started = Instant::now();
        let pairs = silent(|c| intersecting_pairs(&both, c)).unwrap();
        let elapsed = started.elapsed();
        let triangles = silent(|c| intersecting_triangles(&both, c)).unwrap();
        println!(
            "two overlapping tori, {} triangles: {} intersecting pairs ({} triangles) in {:?}",
            both.triangle_count(),
            pairs.len(),
            triangles.len(),
            elapsed
        );
        assert!(!pairs.is_empty());
    }
}
