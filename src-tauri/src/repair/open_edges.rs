//! Home › Repair edits on open edges: Add triangle and Grow a band from a hole edge, plus the open
//! side list Bridge a gap starts from. They moved here from the window (src/repair/edits.ts in PR
//! #1) and make the same triangles and points.
//!
//! The loops here are PR #1's: open sides joined head to tail, greedily, in mesh order. They are
//! not topology::boundary_loops, which the hole filling uses and which also untangles holes that
//! touch at a corner; a walk here that does not come back to its start makes no loop.

use super::edit::triangles;
use super::exact::{P3, cross, dot, hypot, normal, sub, vertex};
use super::holes;
use super::topology::{Edges, NONE};
use super::{Control, NEW, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// One triangle's side on an edge that no other triangle has, from `a` to `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenSide {
    pub a: u32,
    pub b: u32,
    pub triangle: u32,
}

/// The open sides in mesh order: every one, or with a selection those of selected triangles.
pub fn open_sides(mesh: &Mesh, edges: &Edges, selection: &[u32]) -> Vec<OpenSide> {
    let (selected, count) = super::edit::selected_mask(mesh, selection);
    let mut out = Vec::new();
    for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        if count > 0 && !selected[t] {
            continue;
        }
        for k in 0..3 {
            let e = edges.edge_of((3 * t + k) as u32);
            if e != NONE && edges.degree(e) == 1 {
                out.push(OpenSide {
                    a: c[k],
                    b: c[(k + 1) % 3],
                    triangle: t as u32,
                });
            }
        }
    }
    out
}

/// The closed loops of `sides`, as indices into it: from each side not yet used, the walk goes on
/// with the first unused side that starts where the last one ended, until none does. A walk that
/// ends where it began, after three sides or more, is a loop; the sides of any other walk are used
/// up all the same.
fn loops(sides: &[OpenSide], vertex_count: usize) -> Vec<Vec<usize>> {
    // The sides leaving each vertex, in order, and how many of them are used: sides are only ever
    // taken, so the first unused one never moves back.
    let mut start = vec![0usize; vertex_count + 1];
    for s in sides {
        start[s.a as usize + 1] += 1;
    }
    for v in 0..vertex_count {
        start[v + 1] += start[v];
    }
    let mut fill = start[..vertex_count].to_vec();
    let mut leaving = vec![0usize; sides.len()];
    for (i, s) in sides.iter().enumerate() {
        leaving[fill[s.a as usize]] = i;
        fill[s.a as usize] += 1;
    }
    let mut taken = start[..vertex_count].to_vec();
    let mut used = vec![false; sides.len()];
    let mut out = Vec::new();
    for first in 0..sides.len() {
        if used[first] {
            continue;
        }
        let mut walk = Vec::new();
        let mut cursor = Some(first);
        while let Some(i) = cursor.filter(|&i| !used[i]) {
            used[i] = true;
            walk.push(i);
            let v = sides[i].b as usize;
            while taken[v] < start[v + 1] && used[leaving[taken[v]]] {
                taken[v] += 1;
            }
            cursor = (taken[v] < start[v + 1]).then(|| leaving[taken[v]]);
        }
        if walk.len() >= 3 && sides[walk[walk.len() - 1]].b == sides[walk[0]].a {
            out.push(walk);
        }
    }
    out
}

/// The triangle (a, b, c), or (a, c, b) when that one faces against `hint` (with no hint, as
/// given).
pub fn facing(points: &dyn Fn(u32) -> P3, [a, b, c]: [u32; 3], hint: P3) -> [u32; 3] {
    if dot(hint, hint) == 0.0 {
        return [a, b, c];
    }
    let n = normal(points(a), points(b), points(c));
    if dot(n, hint) < 0.0 {
        [a, c, b]
    } else {
        [a, b, c]
    }
}

/// The normal (not unit) of mesh triangle `t`.
pub fn triangle_normal(mesh: &Mesh, t: u32) -> P3 {
    let i = t as usize * 3;
    let [a, b, c] = [0, 1, 2].map(|k| vertex(mesh, mesh.indices[i + k]));
    normal(a, b, c)
}

/// Fills every hole of three open edges with one triangle: every such hole, or with a selection
/// those whose open sides all belong to selected triangles. With none, Close trivial holes runs
/// instead (on the selection, if one exists).
pub fn add_triangle(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    let edges = Edges::new(mesh);
    let sides = open_sides(mesh, &edges, selection);
    drop(edges);
    control.progress(0.1);
    control.check()?;
    let threes: Vec<Vec<usize>> = loops(&sides, mesh.vertex_count())
        .into_iter()
        .filter(|l| l.len() == 3)
        .collect();
    if threes.is_empty() {
        return holes::close_trivial(mesh, selection, &mut control.sub(0.2, 1.0));
    }
    control.progress(0.2);
    control.check()?;
    let mut out = mesh.clone();
    for l in &threes {
        // Against the walk, so the new face meets each open side the other way.
        out.indices
            .extend_from_slice(&[sides[l[0]].a, sides[l[2]].a, sides[l[1]].a]);
    }
    control.progress(1.0);
    Ok(appended(
        mesh,
        out,
        format!("Added {}.", triangles(threes.len())),
    ))
}

/// Grows a band of triangles out from each open loop: every loop, or with a selection those whose
/// sides all belong to selected triangles. Each corner gets a new point a quarter of the mean
/// length of its two loop edges away, out across the loop in the plane of the triangle beside it,
/// and each loop edge two triangles to the new points, facing as that triangle faces.
pub fn grow_band(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    grow_band_by(mesh, selection, None, control)
}

/// Grow a band with each corner's new point `width` out (Modify's Offset hole edges), measured at
/// right angles to the loop's edges: at a corner the point goes out along the bisector, further by
/// 1 / cos(half the turn), at most twice the width. None: a quarter of the mean length of the
/// corner's two loop edges, as `grow_band`.
pub fn grow_band_by(
    mesh: &Mesh,
    selection: &[u32],
    width: Option<f64>,
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    let edges = Edges::new(mesh);
    let sides = open_sides(mesh, &edges, selection);
    drop(edges);
    control.progress(0.1);
    control.check()?;
    let found = loops(&sides, mesh.vertex_count());
    if found.is_empty() {
        return Ok(Outcome::unchanged(
            mesh,
            "No open hole edge to grow a band from.",
        ));
    }
    control.progress(0.3);
    // The new points as the window computed them: in f64 from the f32 corners, then rounded.
    let at = |v: u32| vertex(mesh, v);
    let mut points: Vec<f32> = Vec::new();
    let mut fresh: Vec<Vec<u32>> = Vec::with_capacity(found.len());
    let base = mesh.vertex_count() as u32;
    for l in &found {
        let n = l.len();
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let prev = sides[l[(i + n - 1) % n]];
            let side = sides[l[i]];
            let hint = triangle_normal(mesh, side.triangle);
            let into = |from: u32, to: u32| {
                let dir = cross(sub(at(to), at(from)), hint);
                let len = hypot(dir);
                if len == 0.0 {
                    [0.0; 3]
                } else {
                    [dir[0] / len, dir[1] / len, dir[2] / len]
                }
            };
            let (a, b) = (into(prev.a, prev.b), into(side.a, side.b));
            let mut d = [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
            let len = hypot(d);
            if len == 0.0 {
                d = b;
            } else {
                d = [d[0] / len, d[1] / len, d[2] / len];
            }
            let span =
                (hypot(sub(at(prev.b), at(prev.a))) + hypot(sub(at(side.b), at(side.a)))) / 2.0;
            let dist = match width {
                // |a + b| / 2 is the cosine of half the turn between the two edges' directions.
                Some(w) => w / (len / 2.0).max(0.5),
                None => span * 0.25,
            };
            let p = at(side.a);
            ids.push(base + (points.len() / 3) as u32);
            for k in 0..3 {
                points.push((p[k] + d[k] * dist) as f32);
            }
        }
        fresh.push(ids);
    }
    control.check()?;
    control.progress(0.7);
    let mut out = mesh.clone();
    out.positions.extend_from_slice(&points);
    let rounded = |v: u32| {
        let i = v as usize * 3;
        [0, 1, 2].map(|k| out.positions[i + k] as f64)
    };
    let mut added = Vec::new();
    for (l, ids) in found.iter().zip(&fresh) {
        let n = l.len();
        for i in 0..n {
            let side = sides[l[i]];
            let hint = triangle_normal(mesh, side.triangle);
            let (na, nb) = (ids[i], ids[(i + 1) % n]);
            added.extend_from_slice(&facing(&rounded, [side.b, side.a, na], hint));
            added.extend_from_slice(&facing(&rounded, [side.b, na, nb], hint));
        }
    }
    out.indices.extend_from_slice(&added);
    control.progress(1.0);
    let message = format!(
        "Added a band of {} along {} {}.",
        triangles(added.len() / 3),
        fmt_count(found.len() as u64),
        if found.len() == 1 {
            "open loop"
        } else {
            "open loops"
        }
    );
    Ok(appended(mesh, out, message))
}

/// The outcome of an edit that only appended triangles (and maybe points) to `mesh`, with nothing
/// selected afterwards.
pub fn appended(mesh: &Mesh, out: Mesh, message: String) -> Outcome {
    let kept = mesh.triangle_count() as u32;
    let origin = (0..kept)
        .chain(std::iter::repeat_n(
            NEW,
            out.triangle_count() - kept as usize,
        ))
        .collect();
    Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(origin),
        message,
        warnings: Vec::new(),
        concerns: Vec::new(),
        highlight: None,
        select: Some(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::{RepairError, silent};

    /// Three faces of a corner, facing out of the octant; the open loop is 1-2-3.
    fn open_corner() -> Mesh {
        Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            indices: vec![0, 2, 1, 0, 3, 2, 0, 1, 3],
        }
    }

    fn face() -> Mesh {
        Mesh {
            positions: vec![0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 4.0, 0.0],
            indices: vec![0, 1, 2],
        }
    }

    fn open_count(m: &Mesh) -> usize {
        stats(m).open_edges as usize
    }

    #[test]
    fn open_sides_follow_the_mesh_and_the_selection() {
        let m = open_corner();
        let edges = Edges::new(&m);
        let all = open_sides(&m, &edges, &[]);
        let ends: Vec<(u32, u32, u32)> = all.iter().map(|s| (s.a, s.b, s.triangle)).collect();
        assert_eq!(ends, [(2, 1, 0), (3, 2, 1), (1, 3, 2)]);
        let one = open_sides(&m, &edges, &[1]);
        assert_eq!(one, vec![all[1]]);
        assert_eq!(loops(&all, m.vertex_count()), vec![vec![0, 2, 1]]);
        // A walk that does not come back makes no loop.
        assert!(loops(&one, m.vertex_count()).is_empty());
    }

    #[test]
    fn a_three_edge_hole_gets_one_triangle_against_its_border() {
        let m = open_corner();
        let out = silent(|c| add_triangle(&m, &[], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.message, "Added 1 triangle.");
        assert_eq!(out.mesh.indices[9..], [2, 3, 1]);
        assert_eq!(out.origin, Origin::Map(vec![0, 1, 2, NEW]));
        assert_eq!(out.select, Some(Vec::new()));
        let s = stats(&out.mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert!(s.volume > 0.0, "faces out like the rest: {}", s.volume);
    }

    #[test]
    fn with_no_three_edge_hole_trivial_holes_close_instead() {
        // A missing cube face is a flat hole of four edges.
        let mut m = make_box([0.0; 3], [20.0; 3]);
        m.indices.drain(6..12);
        let out = silent(|c| add_triangle(&m, &[], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.message, "Closed 1 hole, added 2 triangles.");
        assert_eq!(out.select, None, "the selection follows its triangles");
        assert_eq!(stats(&out.mesh).volume, 8000.0);
        let closed = silent(|c| add_triangle(&out.mesh, &[], c)).unwrap();
        assert!(!closed.changed);
        assert_eq!(closed.message, "No holes to close.");
    }

    #[test]
    fn a_band_grows_out_from_an_open_loop() {
        let m = face();
        let out = silent(|c| grow_band(&m, &[], c)).unwrap();
        assert!(out.changed);
        assert_eq!(
            out.message,
            "Added a band of 6 triangles along 1 open loop."
        );
        assert_eq!(out.mesh.triangle_count(), 7);
        assert_eq!(out.mesh.vertex_count(), 6);
        assert_eq!(out.select, Some(Vec::new()));
        // The old border is covered; the band's outer rim is the new border.
        assert_eq!(open_count(&out.mesh), 3);
        assert_eq!(stats(&out.mesh).non_manifold_edges, 0);
        // The new points lie in the face's plane, outside the face, and every band triangle faces
        // the way the face does.
        for v in 3..6 {
            let p = out.mesh.vertex(v);
            assert_eq!(p[2], 0.0);
            assert!(p[0] < 0.0 || p[1] < 0.0 || p[0] + p[1] > 4.0, "{p:?}");
        }
        for t in 1..7 {
            assert!(triangle_normal(&out.mesh, t)[2] > 0.0, "triangle {t}");
        }
        // Each corner moved a quarter of the mean length of its two edges.
        let p = out.mesh.vertex(4);
        let d = (p[0] as f64 - 4.0).hypot(p[1] as f64);
        assert!((d - 0.25 * (4.0 + 32f64.sqrt()) / 2.0).abs() < 1e-5, "{d}");
    }

    #[test]
    fn a_band_needs_an_open_loop() {
        let closed = silent(|c| grow_band(&make_box([0.0; 3], [2.0; 3]), &[], c)).unwrap();
        assert!(!closed.changed);
        assert_eq!(closed.message, "No open hole edge to grow a band from.");
        // A selection that covers only part of the loop's sides makes no loop either.
        let mut two = face();
        two.positions.extend_from_slice(&[4.0, 4.0, 0.0]);
        two.indices.extend_from_slice(&[1, 3, 2]);
        let out = silent(|c| grow_band(&two, &[0], c)).unwrap();
        assert!(!out.changed);
    }

    #[test]
    fn a_cancelled_edit_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            add_triangle(&open_corner(), &[], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(
            grow_band(&face(), &[], &mut control),
            Err(RepairError::Cancelled)
        );
    }
}
