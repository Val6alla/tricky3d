//! The convex hull of a set of points (Quickhull): the smallest convex solid holding them. Every
//! decision is an exact orientation test on the f32 input coordinates (src/repair/exact.rs), so no
//! face ends up facing the wrong way and no point is left outside; points on the hull's faces that
//! are not corners are left out.

use std::collections::HashMap;

use super::build::{Builder, P};
use crate::mesh::Mesh;
use crate::repair::exact::orient3d;
use crate::repair::{Control, RepairError, RepairResult};

struct Face {
    v: [u32; 3],
    /// The points outside this face that no earlier face claimed.
    outside: Vec<u32>,
    alive: bool,
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The hull of `points` as a closed, outward-facing mesh (world space, the input's coordinates).
/// Fails with a sentence when the points do not span a solid (all in one plane).
pub(crate) fn convex_hull(points: &[[f32; 3]], control: &mut Control) -> RepairResult<Mesh> {
    // One copy of each position.
    let mut seen = HashMap::with_capacity(points.len());
    let mut pts: Vec<P> = Vec::new();
    for p in points {
        let key = p.map(f32::to_bits);
        if seen.insert(key, ()).is_none() {
            pts.push(p.map(f64::from));
        }
    }
    let flat = || {
        RepairError::Failed(
            "The selection is flat (all in one plane), so its hull has no volume.".into(),
        )
    };
    if pts.len() < 4 {
        return Err(flat());
    }
    control.check()?;
    // A first tetrahedron from extreme points.
    let along = |k: usize| {
        let lo = (0..pts.len()).min_by(|&a, &b| pts[a][k].total_cmp(&pts[b][k]));
        let hi = (0..pts.len()).max_by(|&a, &b| pts[a][k].total_cmp(&pts[b][k]));
        (lo.unwrap(), hi.unwrap())
    };
    let mut best = (0, 0, -1.0);
    for k in 0..3 {
        let (a, b) = along(k);
        let d = dot(sub(pts[a], pts[b]), sub(pts[a], pts[b]));
        if d > best.2 {
            best = (a, b, d);
        }
    }
    let (a, b) = (best.0, best.1);
    if best.2 <= 0.0 {
        return Err(flat());
    }
    let ab = sub(pts[b], pts[a]);
    let c = (0..pts.len())
        .max_by(|&i, &j| {
            let ci = cross(ab, sub(pts[i], pts[a]));
            let cj = cross(ab, sub(pts[j], pts[a]));
            dot(ci, ci).total_cmp(&dot(cj, cj))
        })
        .unwrap();
    let normal = cross(ab, sub(pts[c], pts[a]));
    if dot(normal, normal) == 0.0 {
        return Err(flat());
    }
    let d = (0..pts.len())
        .max_by(|&i, &j| {
            dot(normal, sub(pts[i], pts[a]))
                .abs()
                .total_cmp(&dot(normal, sub(pts[j], pts[a])).abs())
        })
        .unwrap();
    let [a, b, c, d] = [a as u32, b as u32, c as u32, d as u32];
    let side = orient3d(
        pts[a as usize],
        pts[b as usize],
        pts[c as usize],
        pts[d as usize],
    );
    if side == 0 {
        return Err(flat());
    }
    // Faces turned so the fourth point is behind each.
    let (b, c) = if side > 0 { (c, b) } else { (b, c) };
    let mut faces: Vec<Face> = [[a, b, c], [a, d, b], [b, d, c], [c, d, a]]
        .into_iter()
        .map(|v| Face {
            v,
            outside: Vec::new(),
            alive: true,
        })
        .collect();
    // Directed edge → the face that has it.
    let mut edge_face: HashMap<(u32, u32), usize> = HashMap::new();
    for (f, face) in faces.iter().enumerate() {
        for k in 0..3 {
            edge_face.insert((face.v[k], face.v[(k + 1) % 3]), f);
        }
    }
    let sees = |pts: &[P], v: [u32; 3], p: u32| {
        orient3d(
            pts[v[0] as usize],
            pts[v[1] as usize],
            pts[v[2] as usize],
            pts[p as usize],
        ) > 0
    };
    for p in 0..pts.len() as u32 {
        if [a, b, c, d].contains(&p) {
            continue;
        }
        if let Some(face) = faces.iter_mut().find(|f| sees(&pts, f.v, p)) {
            face.outside.push(p);
        }
    }
    let height = |pts: &[P], v: [u32; 3], p: u32| {
        let n = cross(
            sub(pts[v[1] as usize], pts[v[0] as usize]),
            sub(pts[v[2] as usize], pts[v[0] as usize]),
        );
        dot(n, sub(pts[p as usize], pts[v[0] as usize]))
    };
    let mut queue: Vec<usize> = (0..faces.len()).collect();
    let mut steps = 0usize;
    while let Some(f) = queue.pop() {
        if !faces[f].alive || faces[f].outside.is_empty() {
            continue;
        }
        steps += 1;
        if steps.is_multiple_of(256) {
            control.check()?;
        }
        // The point furthest out from this face.
        let eye = *faces[f]
            .outside
            .iter()
            .max_by(|&&i, &&j| height(&pts, faces[f].v, i).total_cmp(&height(&pts, faces[f].v, j)))
            .unwrap();
        // Every face the eye sees, from this one across edges.
        let mut visible = vec![f];
        let mut is_visible: HashMap<usize, bool> = HashMap::from([(f, true)]);
        let mut horizon: Vec<(u32, u32)> = Vec::new();
        let mut i = 0;
        while i < visible.len() {
            let g = visible[i];
            i += 1;
            let v = faces[g].v;
            for k in 0..3 {
                let (u, w) = (v[k], v[(k + 1) % 3]);
                let n = edge_face[&(w, u)];
                let seen = *is_visible
                    .entry(n)
                    .or_insert_with(|| sees(&pts, faces[n].v, eye));
                if seen {
                    if !visible.contains(&n) {
                        visible.push(n);
                    }
                } else {
                    horizon.push((u, w));
                }
            }
        }
        // New faces from the horizon to the eye; the visible faces' points go to them.
        let mut orphans = Vec::new();
        for &g in &visible {
            faces[g].alive = false;
            orphans.append(&mut faces[g].outside);
            let v = faces[g].v;
            for k in 0..3 {
                edge_face.remove(&(v[k], v[(k + 1) % 3]));
            }
        }
        let first = faces.len();
        for &(u, w) in &horizon {
            let n = faces.len();
            faces.push(Face {
                v: [u, w, eye],
                outside: Vec::new(),
                alive: true,
            });
            for (x, y) in [(u, w), (w, eye), (eye, u)] {
                edge_face.insert((x, y), n);
            }
        }
        for p in orphans {
            if p == eye {
                continue;
            }
            if let Some(g) = (first..faces.len()).find(|&g| sees(&pts, faces[g].v, p)) {
                faces[g].outside.push(p);
            }
        }
        queue.extend(first..faces.len());
    }
    // The mesh of the living faces, with only the points they use.
    let mut b = Builder::default();
    let mut id: HashMap<u32, u32> = HashMap::new();
    for face in faces.iter().filter(|f| f.alive) {
        let v = face
            .v
            .map(|p| *id.entry(p).or_insert_with(|| b.point(pts[p as usize])));
        b.triangle(v[0], v[1], v[2]);
    }
    b.finish_in_place().map_err(RepairError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tests::assert_clean;
    use crate::repair::silent;

    #[test]
    fn the_hull_of_a_cube_and_inner_points_is_the_cube() {
        let mut pts = Vec::new();
        for x in [0.0, 10.0] {
            for y in [0.0, 10.0] {
                for z in [0.0, 10.0] {
                    pts.push([x, y, z]);
                }
            }
        }
        // Points inside, on faces and on edges are left out.
        for k in 0..200 {
            let t = k as f32 / 200.0;
            pts.push([10.0 * t, 5.0, 3.0]);
            pts.push([10.0 * t, 0.0, 0.0]);
            pts.push([5.0, 10.0, 10.0 * t]);
        }
        let hull = silent(|c| convex_hull(&pts, c)).unwrap();
        assert_clean(&hull, "cube hull");
        assert_eq!(crate::mesh::stats(&hull).volume, 1000.0);
    }

    #[test]
    fn random_points_end_up_inside_their_hull() {
        let mut seed = 12345u64;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) as f32 * 20.0 - 10.0
        };
        let pts: Vec<[f32; 3]> = (0..3000).map(|_| [rnd(), rnd(), rnd()]).collect();
        let hull = silent(|c| convex_hull(&pts, c)).unwrap();
        assert_clean(&hull, "random hull");
        // Every input point is on or behind every face.
        for t in hull.indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|v| hull.point(v));
            for p in &pts {
                assert!(orient3d(a, b, c, p.map(f64::from)) <= 0);
            }
        }
    }

    #[test]
    fn flat_input_is_refused() {
        let pts: Vec<[f32; 3]> = (0..50)
            .map(|k| [k as f32, (k * k % 7) as f32, 0.0])
            .collect();
        let err = silent(|c| convex_hull(&pts, c)).unwrap_err();
        assert!(matches!(err, RepairError::Failed(s) if s.contains("flat")));
    }
}
