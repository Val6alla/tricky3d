//! Inside or outside: exact ray casting against a set of triangles. Winding numbers for Wrap part
//! surface's classification, and nesting depths so cavities keep facing inward.
//!
//! A ray is the segment from a point to a far point beyond every triangle. It crosses a triangle
//! when its ends lie strictly on opposite sides of the triangle's plane and it passes strictly
//! inside the triangle (orient3d of the segment with each edge: three equal nonzero signs), both
//! decided exactly. A ray that meets an edge or a corner, or runs in a triangle's plane, decides
//! nothing, so it is cast again in the next of a fixed list of directions: the answers are exact
//! and the same on every run.

use super::bvh::{self, Aabb, Bvh, boxes_of};
use super::exact::{
    P3, add, collinear, dominant_axis, length, orient2d, orient3d, project, scale, sub, vertex,
};
use super::{Control, RepairResult};
use crate::mesh::Mesh;

/// Ray directions, tried in order: spread over the sphere, every component nonzero and no simple
/// ratio between components, so no axis-aligned or diagonal edge or face lines up with them.
const DIRECTIONS: [P3; 20] = [
    [0.5381, 0.7317, 0.4187],
    [-0.6143, 0.2251, 0.7562],
    [0.1427, -0.8692, 0.4733],
    [-0.3863, -0.4651, -0.7971],
    [0.8311, -0.2237, -0.5093],
    [-0.7672, 0.6157, -0.1797],
    [0.2731, 0.3359, -0.9019],
    [-0.0953, -0.9621, -0.2557],
    [0.9137, 0.3871, 0.1239],
    [-0.4517, 0.1663, -0.8767],
    [0.6529, -0.6893, 0.3139],
    [-0.8807, -0.3359, 0.3343],
    [0.0631, 0.6761, -0.7339],
    [0.4391, -0.4177, -0.7951],
    [-0.2473, 0.9397, 0.2363],
    [0.7219, 0.0773, 0.6877],
    [-0.5953, -0.6491, 0.4739],
    [0.3203, -0.1429, 0.9363],
    [-0.9733, 0.1229, -0.1931],
    [0.1859, 0.5543, 0.8111],
];

/// Most shell vertices a nesting test starts rays from: six extremes and six spread out.
const STARTS: usize = 12;

/// Triangles over f64 points, ready for ray casting.
pub struct Solid {
    pub points: Vec<P3>,
    pub triangles: Vec<[u32; 3]>,
    pub bvh: Bvh,
}

/// Where a ray starts.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Start {
    /// Anywhere: a start on a triangle means the point lies on the surface.
    Free,
    /// On the surface, leaving it: a triangle through the start point does not count, since the
    /// ray touches its plane only there.
    Surface,
}

/// Why a ray decided nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Miss {
    /// The start lies on a triangle (free starts only).
    OnSurface,
    /// The ray meets an edge or a corner, or lies in a triangle's plane.
    Degenerate,
}

impl Solid {
    pub fn new(points: Vec<P3>, triangles: Vec<[u32; 3]>) -> Solid {
        let bvh = Bvh::new(&boxes_of(&points, &triangles));
        Solid {
            points,
            triangles,
            bvh,
        }
    }

    /// The triangles `subset` of `mesh` (all of them when None). The points are all the mesh's
    /// vertices, so vertex ids stay the same; triangle k of the solid is `subset[k]`.
    pub fn from_mesh(mesh: &Mesh, subset: Option<&[u32]>) -> Solid {
        let points = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f64::from))
            .collect();
        let all = mesh.indices.as_chunks::<3>().0;
        let triangles = match subset {
            Some(subset) => subset.iter().map(|&t| all[t as usize]).collect(),
            None => all.to_vec(),
        };
        Solid::new(points, triangles)
    }

    /// Winding number at `p`: the signed count of triangles a ray from `p` to infinity crosses,
    /// +1 for each it leaves through the front (the side (b − a) × (c − a) points to), −1 for each
    /// it enters. Inside a closed outward-facing shell it is 1, outside 0. Rays that hit an edge,
    /// a corner or lie in a triangle's plane are retried in other directions. None when `p` lies
    /// on a triangle or every direction tried was degenerate.
    pub fn winding(&self, p: P3) -> Option<i32> {
        let Some(reach) = self.bvh.bounds() else {
            return Some(0);
        };
        for d in DIRECTIONS {
            let mut w = 0;
            match self.cast(p, far(p, d, &reach), Start::Free, &|_| false, |_, s| w += s) {
                Ok(()) => return Some(w),
                Err(Miss::OnSurface) => return None,
                Err(Miss::Degenerate) => {}
            }
        }
        None
    }

    /// The winding number just in front of a surface point: `p` lies on the triangles `on` (which
    /// the ray ignores) and the ray leaves along `toward`, which must point to the side wanted.
    /// None when every direction tried near `toward` was degenerate.
    ///
    /// The rays leave within about 15° of `toward`. Other triangles through `p` do not count (the
    /// ray only touches their plane at its start), unless the ray runs in their plane.
    pub fn winding_beside(&self, p: P3, toward: P3, on: &[u32]) -> Option<i32> {
        let size = length(toward);
        if !(size > 0.0 && size.is_finite()) {
            return None;
        }
        let Some(reach) = self.bvh.bounds() else {
            return Some(0);
        };
        let t = scale(toward, 1.0 / size);
        let skip = |k: u32| on.contains(&k);
        for d in DIRECTIONS {
            let dir = add(t, scale(d, 0.25 / length(d)));
            let mut w = 0;
            if self
                .cast(p, far(p, dir, &reach), Start::Surface, &skip, |_, s| w += s)
                .is_ok()
            {
                return Some(w);
            }
        }
        None
    }

    /// True when a ray from `p` crosses the triangles an odd number of times (orientation does not
    /// matter). None as for `winding`.
    pub fn odd_crossings(&self, p: P3) -> Option<bool> {
        // Each crossing adds ±1, so the winding number has the parity of the crossing count.
        self.winding(p).map(|w| w % 2 != 0)
    }

    /// Casts the ray from `p` to `q` over the triangles `skip` lets through, calling
    /// `cross(triangle, ±1)` for each one it crosses. A start on the surface outranks a degenerate
    /// hit, so `winding` can stop at once.
    fn cast(
        &self,
        p: P3,
        q: P3,
        start: Start,
        skip: &dyn Fn(u32) -> bool,
        mut cross: impl FnMut(u32, i32),
    ) -> Result<(), Miss> {
        let mut miss = None;
        self.bvh.segment(p, q, |t| {
            if miss == Some(Miss::OnSurface) || skip(t) {
                return;
            }
            match self.crossing(t, p, q, start) {
                Ok(0) => {}
                Ok(s) => cross(t, s),
                Err(m) => {
                    if miss != Some(Miss::OnSurface) {
                        miss = Some(m);
                    }
                }
            }
        });
        miss.map_or(Ok(()), Err)
    }

    /// How the ray from `p` to `q` meets triangle `t`: Ok(1) crossing from its back to its front,
    /// Ok(−1) from front to back, Ok(0) not at all. `q` lies beyond every triangle's box.
    fn crossing(&self, t: u32, p: P3, q: P3, start: Start) -> Result<i32, Miss> {
        let [a, b, c] = self.triangles[t as usize].map(|v| self.points[v as usize]);
        let sp = orient3d(a, b, c, p);
        let sq = orient3d(a, b, c, q);
        if sp == 0 && sq == 0 {
            // The ray lies in the triangle's plane, unless the triangle has no area: then it is no
            // surface at all and nothing crosses it.
            if collinear(a, b, c) {
                return Ok(0);
            }
            if start == Start::Free && in_triangle(p, a, b, c) {
                return Err(Miss::OnSurface);
            }
            return Err(Miss::Degenerate);
        }
        if sp == 0 {
            // The ray starts in the plane and leaves it at once.
            if start == Start::Surface {
                return Ok(0);
            }
            // The line pierces the plane at `p`, so the edge signs place `p` against the triangle.
            let e = edge_signs(p, q, a, b, c);
            return if e.contains(&1) && e.contains(&-1) {
                Ok(0)
            } else {
                Err(Miss::OnSurface)
            };
        }
        if sq == 0 {
            // The far end lies in the plane. It lies outside the triangle, but rather than lean on
            // that, take another direction (this practically never happens).
            return Err(Miss::Degenerate);
        }
        if sp == sq {
            return Ok(0);
        }
        // The ray crosses the plane strictly between its ends; where, relative to the edges?
        let e = edge_signs(p, q, a, b, c);
        if e.contains(&1) && e.contains(&-1) {
            return Ok(0);
        }
        if e.contains(&0) {
            return Err(Miss::Degenerate);
        }
        Ok(if sp < 0 { 1 } else { -1 })
    }

    /// How many of `containers` (shell ids, marked in `container`) hold `shell`: rays from several
    /// of its vertices all cross the container an odd number of times. A start that lies on a
    /// container decides nothing and is skipped, so a cavity touching its container still counts;
    /// a shell that crosses into another, with vertices on both sides, does not. `odd` and `seen`
    /// are scratch space, one entry per shell.
    #[allow(clippy::too_many_arguments)]
    fn containing(
        &self,
        mesh: &Mesh,
        shell: &[u32],
        containers: &[usize],
        container: &[bool],
        shell_of: &[u32],
        reach: &Aabb,
        odd: &mut [bool],
        seen: &mut [u8],
    ) -> u32 {
        const INSIDE: u8 = 1;
        const OUTSIDE: u8 = 2;
        let skip = |t: u32| !container[shell_of[t as usize] as usize];
        for &c in containers {
            seen[c] = 0;
        }
        for p in start_points(mesh, shell) {
            for d in DIRECTIONS {
                for &c in containers {
                    odd[c] = false;
                }
                let flip = |t: u32, _| {
                    let s = shell_of[t as usize] as usize;
                    odd[s] = !odd[s];
                };
                match self.cast(p, far(p, d, reach), Start::Free, &skip, flip) {
                    Ok(()) => {
                        for &c in containers {
                            seen[c] |= if odd[c] { INSIDE } else { OUTSIDE };
                        }
                        break;
                    }
                    Err(Miss::OnSurface) => break,
                    Err(Miss::Degenerate) => {}
                }
            }
        }
        containers.iter().filter(|&&c| seen[c] == INSIDE).count() as u32
    }
}

/// orient3d of the line p→q with each edge of the triangle: all equal and nonzero when the line
/// passes strictly inside, a zero on an edge's line, mixed signs outside.
fn edge_signs(p: P3, q: P3, a: P3, b: P3, c: P3) -> [i32; 3] {
    [
        orient3d(p, q, a, b),
        orient3d(p, q, b, c),
        orient3d(p, q, c, a),
    ]
}

/// True when `p`, which lies in the plane of the triangle (with area), is inside it or on its
/// border. Exact.
fn in_triangle(p: P3, a: P3, b: P3, c: P3) -> bool {
    let Some(axis) = dominant_axis(a, b, c) else {
        return false;
    };
    let [a, b, c, p] = [a, b, c, p].map(|x| project(x, axis));
    let s = [orient2d(a, b, p), orient2d(b, c, p), orient2d(c, a, p)];
    !(s.contains(&1) && s.contains(&-1))
}

/// The far end of a ray from `p` along `d`: beyond the sphere around `reach`, so the ray leaves
/// every triangle in that box behind.
fn far(p: P3, d: P3, reach: &Aabb) -> P3 {
    let center = [0, 1, 2].map(|k| (reach[0][k] + reach[1][k]) * 0.5);
    let radius = length(sub(reach[1], reach[0])) * 0.5;
    let distance = 2.0 * (length(sub(p, center)) + radius) + 1.0;
    add(p, scale(d, distance / length(d)))
}

/// Where rays from a shell start: its extreme vertices along each axis (the lowest first), then
/// corners of triangles spread evenly over its list, at most `STARTS` distinct points. Spread out,
/// so a shell that crosses into another has starts on both sides.
fn start_points(mesh: &Mesh, shell: &[u32]) -> Vec<P3> {
    let corners = mesh.indices.as_chunks::<3>().0;
    let mut extremes: [Option<P3>; 6] = [None; 6];
    for &t in shell {
        for &v in &corners[t as usize] {
            let p = vertex(mesh, v);
            for axis in 0..3 {
                // Ties go to the lexicographically smaller (or larger) point, so the choice does
                // not depend on the triangle order.
                let key = |q: &P3| [q[axis], q[(axis + 1) % 3], q[(axis + 2) % 3]];
                let less = |a: &P3, b: &P3| key(a) < key(b);
                if extremes[axis * 2].is_none_or(|e| less(&p, &e)) {
                    extremes[axis * 2] = Some(p);
                }
                if extremes[axis * 2 + 1].is_none_or(|e| less(&e, &p)) {
                    extremes[axis * 2 + 1] = Some(p);
                }
            }
        }
    }
    let mut out: Vec<P3> = Vec::with_capacity(STARTS);
    if shell.is_empty() {
        return out;
    }
    let spread = (0..STARTS / 2).map(|k| {
        let t = shell[k * shell.len() / (STARTS / 2)];
        vertex(mesh, corners[t as usize][0])
    });
    for p in extremes.into_iter().flatten().chain(spread) {
        if out.len() < STARTS && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// True when `inner` lies strictly inside `outer` on every axis.
fn strictly_inside(inner: &Aabb, outer: &Aabb) -> bool {
    (0..3).all(|k| outer[0][k] < inner[0][k] && inner[1][k] < outer[1][k])
}

/// For each closed shell (its triangles), how many of the other shells contain it: their bounding
/// box holds its box and rays from its vertices cross them an odd number of times.
///
/// Candidates are the shells whose box strictly holds this shell's box (a box query). Rays from up
/// to twelve of the shell's vertices (its extremes along each axis and some spread over it) count
/// crossings for all candidates at once, ignoring every other shell's triangles. A candidate holds
/// the shell when every ray that decides finds it inside: a start lying on a candidate decides
/// nothing (a cavity may touch its container), and a shell with starts on both sides of a candidate
/// only crosses into it, which on broken parts with overlapping shells is common and no cavity.
pub fn nesting_depths(
    mesh: &Mesh,
    shells: &[Vec<u32>],
    control: &mut Control,
) -> RepairResult<Vec<u32>> {
    let mut depths = vec![0u32; shells.len()];
    if shells.len() < 2 {
        return Ok(depths);
    }
    control.check()?;
    let corners = mesh.indices.as_chunks::<3>().0;
    let boxes: Vec<Aabb> = shells
        .iter()
        .map(|shell| {
            let mut b = bvh::EMPTY;
            for &t in shell {
                for &v in &corners[t as usize] {
                    bvh::include(&mut b, vertex(mesh, v));
                }
            }
            b
        })
        .collect();
    let by_box = Bvh::new(&boxes);
    // One solid over every shell's triangles, with the shell of each.
    let total: usize = shells.iter().map(Vec::len).sum();
    let mut triangles = Vec::with_capacity(total);
    let mut shell_of = Vec::with_capacity(total);
    for (s, shell) in shells.iter().enumerate() {
        for &t in shell {
            triangles.push(corners[t as usize]);
            shell_of.push(s as u32);
        }
    }
    let solid = Solid::new(
        mesh.positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f64::from))
            .collect(),
        triangles,
    );
    control.progress(0.3);
    control.check()?;

    let mut container = vec![false; shells.len()];
    let mut odd = vec![false; shells.len()];
    let mut seen = vec![0u8; shells.len()];
    let mut found = Vec::new();
    for (s, shell) in shells.iter().enumerate() {
        if s % 64 == 63 {
            control.check()?;
            control.progress(0.3 + 0.7 * s as f64 / shells.len() as f64);
        }
        found.clear();
        by_box.query(&boxes[s], |c| {
            if c as usize != s && strictly_inside(&boxes[s], &boxes[c as usize]) {
                found.push(c as usize);
            }
        });
        if found.is_empty() {
            continue;
        }
        // The rays only have to get past the candidates.
        let mut reach = bvh::EMPTY;
        for &c in &found {
            container[c] = true;
            bvh::grow(&mut reach, &boxes[c]);
        }
        depths[s] = solid.containing(
            mesh, shell, &found, &container, &shell_of, &reach, &mut odd, &mut seen,
        );
        for &c in &found {
            container[c] = false;
        }
    }
    control.progress(1.0);
    Ok(depths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::exact::{dot, normal};
    use crate::repair::silent;
    use std::time::Instant;

    fn inverted(mut mesh: Mesh) -> Mesh {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        mesh
    }

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    #[test]
    fn cube_inside_outside_and_on_the_surface() {
        let solid = Solid::from_mesh(&cube(), None);
        for p in [
            [10.0, 10.0, 10.0],
            [0.001, 19.999, 5.0],
            [1e-9, 1e-9, 1e-9],
            [10.0, 10.0, 20.0 - 2f64.powi(-40)],
        ] {
            assert_eq!(solid.winding(p), Some(1), "{p:?}");
            assert_eq!(solid.odd_crossings(p), Some(true), "{p:?}");
        }
        for p in [
            [-1.0, 10.0, 10.0],
            [30.0, 30.0, 30.0],
            [10.0, 10.0, 20.0 + 2f64.powi(-40)],
            [-1e-30, 5.0, 5.0],
        ] {
            assert_eq!(solid.winding(p), Some(0), "{p:?}");
            assert_eq!(solid.odd_crossings(p), Some(false), "{p:?}");
        }
        // On a face, on a face diagonal (two triangles), on a cube edge, at a corner.
        for p in [
            [10.0, 5.0, 20.0],
            [10.0, 10.0, 0.0],
            [0.0, 0.0, 10.0],
            [20.0, 20.0, 20.0],
            [0.0, 7.0, 3.0],
        ] {
            assert_eq!(solid.winding(p), None, "{p:?}");
            assert_eq!(solid.odd_crossings(p), None, "{p:?}");
        }
        // In the plane of a face but outside the cube.
        assert_eq!(solid.winding([-5.0, 10.0, 20.0]), Some(0));
        assert_eq!(solid.winding([30.0, 30.0, 0.0]), Some(0));
    }

    #[test]
    fn orientation_and_nesting_set_the_winding_number() {
        let solid = Solid::from_mesh(&inverted(cube()), None);
        assert_eq!(solid.winding([10.0, 10.0, 10.0]), Some(-1));
        assert_eq!(solid.odd_crossings([10.0, 10.0, 10.0]), Some(true));
        assert_eq!(solid.winding([30.0, 10.0, 10.0]), Some(0));

        // A cube with an inward-facing cube inside: a hollow part.
        let hollow = concat(&[&cube(), &inverted(make_box([5.0; 3], [10.0; 3]))]).unwrap();
        let solid = Solid::from_mesh(&hollow, None);
        assert_eq!(solid.winding([7.0, 7.0, 7.0]), Some(0));
        assert_eq!(solid.winding([2.0, 2.0, 2.0]), Some(1));
        assert_eq!(solid.winding([15.0, 7.0, 7.0]), Some(1));
        assert_eq!(solid.odd_crossings([7.0, 7.0, 7.0]), Some(false));
        assert_eq!(solid.odd_crossings([2.0, 2.0, 2.0]), Some(true));

        // Two overlapping outward cubes count twice where they overlap.
        let both = concat(&[&cube(), &make_box([10.0; 3], [30.0; 3])]).unwrap();
        let solid = Solid::from_mesh(&both, None);
        assert_eq!(solid.winding([15.0, 15.0, 15.0]), Some(2));
        assert_eq!(solid.winding([5.0, 5.0, 5.0]), Some(1));
        assert_eq!(solid.winding([25.0, 25.0, 25.0]), Some(1));
        assert_eq!(solid.winding([25.0, 5.0, 5.0]), Some(0));
        assert_eq!(solid.odd_crossings([15.0, 15.0, 15.0]), Some(false));

        // A subset: only the second cube.
        let second: Vec<u32> = (12..24).collect();
        let solid = Solid::from_mesh(&both, Some(&second));
        assert_eq!(solid.triangles.len(), 12);
        assert_eq!(solid.winding([5.0, 5.0, 5.0]), Some(0));
        assert_eq!(solid.winding([15.0, 15.0, 15.0]), Some(1));

        let empty = Solid::new(Vec::new(), Vec::new());
        assert_eq!(empty.winding([1.0, 2.0, 3.0]), Some(0));
        assert_eq!(
            empty.winding_beside([1.0, 2.0, 3.0], [0.0, 0.0, 1.0], &[]),
            Some(0)
        );
    }

    #[test]
    fn triangles_without_area_are_no_surface() {
        // Needles across the whole cube (three collinear corners, and a repeated corner) lie in
        // the path of most rays; they must neither count nor make every ray degenerate.
        let mut mesh = cube();
        mesh.positions.extend_from_slice(&[
            1.0, 1.0, 1.0, 19.0, 19.0, 19.0, 10.0, 10.0, 10.0, 1.0, 19.0, 1.0, 19.0, 1.0, 19.0,
        ]);
        mesh.indices
            .extend_from_slice(&[8, 9, 10, 11, 12, 12, 8, 8, 8]);
        let solid = Solid::from_mesh(&mesh, None);
        let mut rng = Rng(24);
        for _ in 0..200 {
            let p = [0, 1, 2].map(|_| rng.unit() * 18.0 + 1.0);
            assert_eq!(solid.winding(p), Some(1), "{p:?}");
        }
        assert_eq!(solid.winding([10.0, 10.0, 10.0]), Some(1));
        assert_eq!(
            solid.winding_beside([10.0, 5.0, 20.0], [0.0, 0.0, -1.0], &[2]),
            Some(1)
        );
    }

    #[test]
    fn rays_through_edges_corners_and_planes_are_degenerate() {
        let solid = Solid::from_mesh(&cube(), None);
        let free = |p: P3, q: P3| solid.cast(p, q, Start::Free, &|_| false, |_, _| {});
        // In the plane of the top face, through it.
        assert_eq!(
            free([-5.0, 10.0, 20.0], [100.0, 10.0, 20.0]),
            Err(Miss::Degenerate)
        );
        // Through the vertical edge at x = y = 0, and through two corners.
        assert_eq!(
            free([-5.0, -5.0, 10.0], [25.0, 25.0, 10.0]),
            Err(Miss::Degenerate)
        );
        assert_eq!(
            free([-5.0, -5.0, -5.0], [25.0, 25.0, 25.0]),
            Err(Miss::Degenerate)
        );
        // Through a face diagonal (an edge between two triangles of the face).
        assert_eq!(
            free([5.0, 5.0, -5.0], [15.0, 15.0, 25.0]),
            Err(Miss::Degenerate)
        );
        // Starting on a face: on the surface.
        assert_eq!(
            free([10.0, 5.0, 20.0], [10.0, 5.0, 100.0]),
            Err(Miss::OnSurface)
        );
        // A clean ray through two faces.
        let mut w = Vec::new();
        let clean = solid.cast(
            [10.0, 5.0, -5.0],
            [11.0, 6.0, 100.0],
            Start::Free,
            &|_| false,
            |t, s| w.push((t, s)),
        );
        assert_eq!(clean, Ok(()));
        // Enters the bottom face (−1), leaves through the top (+1).
        assert_eq!(w.iter().map(|&(_, s)| s).sum::<i32>(), 0);
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn a_degenerate_first_ray_is_retried() {
        // A big cube whose box fixes the rays' reach, and inside it a small tetrahedron with a
        // corner exactly on the first ray from the origin, a quarter of the way out (dividing by
        // four is exact, so the corner lies exactly on the ray).
        let big = make_box([-50.0; 3], [50.0; 3]);
        let reach = [[-50.0; 3], [50.0; 3]];
        let q = far([0.0; 3], DIRECTIONS[0], &reach);
        let x = scale(q, 0.25);
        assert!(x.iter().all(|v| v.abs() < 45.0), "{x:?}");
        let corners = [
            x,
            add(x, [3.0, 0.0, 0.0]),
            add(x, [0.0, 3.0, 0.0]),
            add(x, [0.0, 0.0, 3.0]),
        ];
        let mut points: Vec<P3> = big
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f64::from))
            .collect();
        let base = points.len() as u32;
        points.extend(corners);
        let mut triangles: Vec<[u32; 3]> = big.indices.as_chunks::<3>().0.to_vec();
        for t in [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]] {
            triangles.push(t.map(|k| base + k));
        }
        let solid = Solid::new(points, triangles);
        assert_eq!(solid.bvh.bounds(), Some(reach));
        // The tetrahedron faces outward.
        let v = |k: usize| solid.points[base as usize + k];
        assert!(dot(normal(v(0), v(2), v(1)), sub(v(3), v(0))) < 0.0);
        assert_eq!(
            solid.cast([0.0; 3], q, Start::Free, &|_| false, |_, _| {}),
            Err(Miss::Degenerate)
        );
        assert_eq!(solid.winding([0.0; 3]), Some(1));
        let inside_tetra = add(x, [0.5, 0.5, 0.5]);
        assert_eq!(solid.winding(inside_tetra), Some(2));
    }

    #[test]
    fn winding_beside_a_surface_point() {
        let mesh = cube();
        let solid = Solid::from_mesh(&mesh, None);
        // Triangle 2 is (0,0,20) (20,0,20) (20,20,20) on the top face, facing +z.
        let p = [10.0, 5.0, 20.0];
        assert_eq!(solid.winding_beside(p, [0.0, 0.0, 1.0], &[2]), Some(0));
        assert_eq!(solid.winding_beside(p, [0.0, 0.0, -1.0], &[2]), Some(1));
        assert_eq!(solid.winding_beside(p, [0.0, 0.0, 0.0], &[2]), None);
        // On the face diagonal, lying on triangles 2 and 3.
        let d = [15.0, 15.0, 20.0];
        assert_eq!(solid.winding_beside(d, [0.0, 0.0, 1.0], &[2, 3]), Some(0));
        assert_eq!(solid.winding_beside(d, [0.1, -0.2, -1.0], &[2, 3]), Some(1));
        // Naming only one of them: the ray leaves the other's plane at once, so it does not count.
        assert_eq!(solid.winding_beside(d, [0.0, 0.0, 1.0], &[2]), Some(0));
        assert_eq!(solid.winding_beside(d, [0.0, 0.0, -1.0], &[3]), Some(1));
        // A cube corner, on six triangles of which only one is named.
        let corner = [20.0, 20.0, 20.0];
        assert_eq!(solid.winding_beside(corner, [1.0, 1.0, 1.0], &[2]), Some(0));
        assert_eq!(
            solid.winding_beside(corner, [-1.0, -1.0, -1.0], &[2]),
            Some(1)
        );

        // On a's top face where it runs inside b.
        let both = concat(&[&mesh, &make_box([10.0; 3], [30.0; 3])]).unwrap();
        let solid = Solid::from_mesh(&both, None);
        assert_eq!(solid.winding_beside(d, [0.0, 0.0, 1.0], &[2, 3]), Some(1));
        assert_eq!(solid.winding_beside(d, [0.0, 0.0, -1.0], &[2, 3]), Some(2));

        // A centroid on a triangle of a fine torus: outside in front, inside behind.
        let ring = torus(30.0, 10.0, 60, 30);
        let solid = Solid::from_mesh(&ring, None);
        let mut rng = Rng(21);
        for _ in 0..200 {
            let t = rng.below(ring.triangle_count() as u64) as u32;
            let [a, b, c] = crate::repair::exact::corners(&ring, t);
            let centroid = scale(add(add(a, b), c), 1.0 / 3.0);
            let n = normal(a, b, c);
            assert_eq!(solid.winding_beside(centroid, n, &[t]), Some(0), "{t}");
            assert_eq!(
                solid.winding_beside(centroid, scale(n, -1.0), &[t]),
                Some(1),
                "{t}"
            );
        }
    }

    #[test]
    fn winding_on_a_torus_matches_the_shape() {
        let ring = torus(30.0, 10.0, 90, 45);
        let solid = Solid::from_mesh(&ring, None);
        let mut rng = Rng(22);
        let mut seen = [0usize; 2];
        for _ in 0..2000 {
            let p = [0, 1, 2].map(|_| rng.unit() * 90.0 - 45.0);
            // Distance from the tube's centre circle; the faceted surface lies within 0.1 of the
            // smooth one, so points nearer than that to it are skipped.
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            let tube = ((r - 30.0).powi(2) + p[2] * p[2]).sqrt();
            if (tube - 10.0).abs() < 0.1 {
                continue;
            }
            let inside = tube < 10.0;
            assert_eq!(solid.winding(p), Some(inside as i32), "{p:?}");
            seen[inside as usize] += 1;
        }
        assert!(seen[0] > 100 && seen[1] > 100, "{seen:?}");
    }

    fn depths(mesh: &Mesh, shells: &[Vec<u32>]) -> Vec<u32> {
        silent(|c| nesting_depths(mesh, shells, c)).unwrap()
    }

    fn shells_of(meshes: &[&Mesh]) -> (Mesh, Vec<Vec<u32>>) {
        let mut shells = Vec::new();
        let mut first = 0u32;
        for m in meshes {
            let n = m.triangle_count() as u32;
            shells.push((first..first + n).collect());
            first += n;
        }
        (concat(meshes).unwrap(), shells)
    }

    #[test]
    fn nesting_depths_of_boxes() {
        let outer = cube();
        let inner = make_box([5.0; 3], [10.0; 3]);
        let core = make_box([6.0; 3], [7.0; 3]);
        let beside = make_box([30.0; 3], [40.0; 3]);
        let (mesh, shells) = shells_of(&[&inner, &outer]);
        assert_eq!(depths(&mesh, &shells), vec![1, 0]);
        // Orientation does not matter: crossings are counted, not signed.
        let (mesh, shells) = shells_of(&[&outer, &inverted(inner.clone())]);
        assert_eq!(depths(&mesh, &shells), vec![0, 1]);
        let (mesh, shells) = shells_of(&[&core, &outer, &beside, &inner]);
        assert_eq!(depths(&mesh, &shells), vec![2, 0, 0, 1]);
        let (mesh, shells) = shells_of(&[&outer, &beside]);
        assert_eq!(depths(&mesh, &shells), vec![0, 0]);
        // Two copies of one cube: equal boxes, neither strictly holds the other.
        let (mesh, shells) = shells_of(&[&outer, &outer]);
        assert_eq!(depths(&mesh, &shells), vec![0, 0]);
        // A tetrahedron on four corners of the cube, dented to a vertex at the cube's centre: the
        // same box as the cube and a vertex strictly inside it, still not nested (it touches).
        let dented = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 20.0, 20.0, 0.0, 20.0, 0.0, 20.0, 0.0, 20.0, 20.0, 10.0, 10.0, 10.0,
            ],
            indices: vec![4, 1, 2, 0, 1, 2, 0, 2, 3, 0, 3, 1, 2, 3, 4, 3, 1, 4],
        };
        let (mesh, shells) = shells_of(&[&dented, &outer]);
        assert_eq!(depths(&mesh, &shells), vec![0, 0]);
        assert_eq!(depths(&outer, &[(0..12).collect()]), vec![0]);
        assert_eq!(depths(&Mesh::default(), &[]), Vec::<u32>::new());
        // An empty shell is in nothing and holds nothing.
        let (mesh, mut shells) = shells_of(&[&outer, &inner]);
        shells.push(Vec::new());
        assert_eq!(depths(&mesh, &shells), vec![0, 1, 0]);
    }

    #[test]
    fn nesting_needs_more_than_a_box() {
        // A torus's box holds its hole: a cube there is not inside it, one in the tube is.
        let ring = torus(30.0, 10.0, 48, 24);
        let in_hole = make_box([-2.0; 3], [2.0; 3]);
        let in_tube = make_box([29.0, -1.0, -1.0], [31.0, 1.0, 1.0]);
        // Across the tube's inner wall: its lowest corner lies inside the tube, its other corners
        // in the hole. It crosses into the torus, it is not inside it.
        let across = make_box([-25.0, -2.0, -2.0], [-15.0, 2.0, 2.0]);
        let (mesh, shells) = shells_of(&[&in_hole, &ring, &in_tube, &across]);
        let solid = Solid::from_mesh(&mesh, Some(&shells[1]));
        assert_eq!(solid.winding([-25.0, -2.0, -2.0]), Some(1));
        assert_eq!(solid.winding([-15.0, 2.0, 2.0]), Some(0));
        assert_eq!(depths(&mesh, &shells), vec![0, 0, 1, 0]);
    }

    #[test]
    fn nesting_tries_another_vertex_when_one_touches_the_container() {
        // An octahedron and, inside it, a tetrahedron whose lowest corner lies on the octahedron's
        // face −x + y + z = 10.
        let octahedron = Mesh {
            positions: vec![
                -10.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, -10.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, -10.0,
                0.0, 0.0, 10.0,
            ],
            indices: vec![
                1, 3, 5, 3, 0, 5, 0, 2, 5, 2, 1, 5, 3, 1, 4, 0, 3, 4, 2, 0, 4, 1, 2, 4,
            ],
        };
        let tetra = Mesh {
            positions: vec![
                -5.0, 2.5, 2.5, -4.0, 2.0, 2.0, -4.0, 3.0, 2.0, -4.0, 2.5, 3.0,
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
        };
        let (mesh, shells) = shells_of(&[&tetra, &octahedron]);
        let solid = Solid::from_mesh(&mesh, Some(&shells[1]));
        assert_eq!(solid.winding([-5.0, 2.5, 2.5]), None);
        assert_eq!(depths(&mesh, &shells), vec![1, 0]);
    }

    #[test]
    fn nesting_depths_stop_when_cancelled() {
        use std::sync::atomic::AtomicBool;
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        let (mesh, shells) = shells_of(&[&cube(), &make_box([5.0; 3], [6.0; 3])]);
        assert_eq!(
            nesting_depths(&mesh, &shells, &mut control),
            Err(crate::repair::RepairError::Cancelled)
        );
    }

    /// Real models: the nesting depth of every shell (components over shared vertices). Set
    /// TRICKY3D_MODELS to STL paths separated by `;` and run in release with
    /// `cargo test --release --lib repair::winding -- --ignored --nocapture`.
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
            let roots = crate::mesh::vertex_roots(mesh);
            let mut shell_of_root = vec![u32::MAX; roots.len()];
            let mut shells: Vec<Vec<u32>> = Vec::new();
            for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
                let slot = &mut shell_of_root[roots[tri[0] as usize] as usize];
                if *slot == u32::MAX {
                    *slot = shells.len() as u32;
                    shells.push(Vec::new());
                }
                shells[*slot as usize].push(t as u32);
            }
            let started = Instant::now();
            let d = depths(mesh, &shells);
            let elapsed = started.elapsed();
            let mut histogram = std::collections::BTreeMap::new();
            for &x in &d {
                *histogram.entry(x).or_insert(0) += 1;
            }
            println!(
                "{path}: {} triangles in {} shells, nesting depths (depth: shells) {histogram:?}, \
                 in {elapsed:?}",
                mesh.triangle_count(),
                shells.len()
            );
        }
    }

    /// Ray casting on a million-triangle torus, in release:
    /// `cargo test --release --lib repair::winding -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn million_triangle_timing() {
        let ring = torus(30.0, 10.0, 1000, 500);
        let started = Instant::now();
        let solid = Solid::from_mesh(&ring, None);
        let built = started.elapsed();
        let mut rng = Rng(23);
        let queries = 10_000;
        let mut inside = 0;
        for _ in 0..queries {
            let p = [0, 1, 2].map(|_| rng.unit() * 90.0 - 45.0);
            inside += (solid.winding(p) == Some(1)) as usize;
        }
        let elapsed = started.elapsed() - built;
        println!(
            "1M-triangle torus: solid built in {built:?}, {queries} winding queries in {elapsed:?} \
             ({inside} inside)"
        );
        // Shell nesting with a thousand small cubes, half of them inside the tube.
        let mut parts: Vec<Mesh> = vec![ring];
        for k in 0..1000 {
            let angle = k as f64 * 0.00628;
            let (s, c) = angle.sin_cos();
            let r = if k % 2 == 0 { 30.0 } else { 50.0 };
            let at = [r * c, r * s, 0.0].map(|x| x as f32);
            parts.push(make_box(at.map(|x| x - 0.5), at.map(|x| x + 0.5)));
        }
        let refs: Vec<&Mesh> = parts.iter().collect();
        let (mesh, shells) = shells_of(&refs);
        let started = Instant::now();
        let d = depths(&mesh, &shells);
        println!(
            "nesting depths of {} shells ({} triangles) in {:?}",
            shells.len(),
            mesh.triangle_count(),
            started.elapsed()
        );
        assert_eq!(d.iter().filter(|&&x| x == 1).count(), 500);
    }
}
