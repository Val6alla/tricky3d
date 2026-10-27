//! Home › Repair edits that split triangles: Subdivide triangles and Add point.
//! They moved here from the window (src/repair/edits.ts in PR #1) and do exactly what they did
//! there, down to the vertex order and the f32 rounding of the new points. Each one leaves nothing
//! selected afterwards, as the window's edits did.
//!
//! Splitting an edge splits every triangle on it, so the mesh never gets a T-junction: a triangle
//! with one split edge becomes two, with two splits three, with three splits four.

use super::edit::{NOTHING_SELECTED, selected_mask, triangles};
use super::topology::{Edges, NONE, side_ends};
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// Triangles between cancel checks.
const CHECK_EVERY: usize = 1 << 16;

/// Splits each triangle into four at its edge midpoints: every triangle, or with a selection the
/// selected ones. Their unselected neighbours split along the shared edges only.
pub fn subdivide(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let (selected, count) = selected_mask(mesh, selection);
    let edges = Edges::new(mesh);
    control.progress(0.2);
    let mut split = vec![false; edges.len()];
    for (t, &chosen) in selected.iter().enumerate() {
        if count > 0 && !chosen {
            continue;
        }
        for k in 0..3 {
            let e = edges.edge_of((3 * t + k) as u32);
            if e != NONE {
                split[e as usize] = true;
            }
        }
    }
    let Some(out) = split_edges(mesh, &edges, &split, control)? else {
        return Ok(Outcome::unchanged(mesh, "No triangles to subdivide."));
    };
    let added = out.mesh.triangle_count() - mesh.triangle_count();
    Ok(cleared(out, format!("Added {}.", triangles(added))))
}

/// Puts a point at the centre of each selected triangle and joins it to the three corners. The
/// other triangles stay as they are.
pub fn add_point(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let (selected, count) = selected_mask(mesh, selection);
    if count == 0 {
        return Ok(Outcome::unchanged(mesh, NOTHING_SELECTED));
    }
    let triangles_in = mesh.triangle_count();
    let mut positions = Vec::with_capacity(mesh.positions.len() + count * 3);
    positions.extend_from_slice(&mesh.positions);
    let mut indices = Vec::with_capacity(mesh.indices.len() + count * 6);
    let mut origin = Vec::with_capacity(triangles_in + count * 2);
    let mut v = mesh.vertex_count() as u32;
    for (t, &[a, b, c]) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        if t % CHECK_EVERY == 0 {
            control.check()?;
            control.progress(t as f64 / triangles_in as f64);
        }
        if !selected[t] {
            indices.extend_from_slice(&[a, b, c]);
            origin.push(t as u32);
            continue;
        }
        let (pa, pb, pc) = (mesh.vertex(a), mesh.vertex(b), mesh.vertex(c));
        for k in 0..3 {
            positions.push(((pa[k] as f64 + pb[k] as f64 + pc[k] as f64) / 3.0) as f32);
        }
        indices.extend_from_slice(&[a, b, v, b, c, v, c, a, v]);
        origin.extend_from_slice(&[t as u32; 3]);
        v += 1;
    }
    let out = Split {
        mesh: Mesh { positions, indices },
        origin,
    };
    let unit = if count == 1 { "point" } else { "points" };
    Ok(cleared(
        out,
        format!("Added {} {unit}.", fmt_count(count as u64)),
    ))
}

/// A mesh with some triangles split, and the input triangle of each output triangle.
struct Split {
    mesh: Mesh,
    origin: Vec<u32>,
}

/// The edit's outcome, with nothing selected afterwards.
fn cleared(out: Split, message: String) -> Outcome {
    Outcome {
        mesh: out.mesh,
        changed: true,
        origin: Origin::Map(out.origin),
        message,
        warnings: Vec::new(),
        concerns: Vec::new(),
        highlight: None,
        select: Some(Vec::new()),
    }
}

/// Splits the edges marked in `split` at their midpoints and every triangle on them. The new
/// points come after the old ones in edge order; each triangle's pieces take its place in order.
/// None when no edge is marked.
fn split_edges(
    mesh: &Mesh,
    edges: &Edges,
    split: &[bool],
    control: &mut Control,
) -> RepairResult<Option<Split>> {
    let extra = split.iter().filter(|&&s| s).count();
    if extra == 0 {
        return Ok(None);
    }
    control.check()?;
    let base = mesh.vertex_count() as u32;
    let mut mid = vec![NONE; split.len()];
    let mut positions = Vec::with_capacity(mesh.positions.len() + extra * 3);
    positions.extend_from_slice(&mesh.positions);
    for e in (0..split.len()).filter(|&e| split[e]) {
        mid[e] = (positions.len() / 3) as u32;
        let (a, b) = side_ends(mesh, edges.sides(e as u32)[0]);
        let (pa, pb) = (mesh.vertex(a), mesh.vertex(b));
        for k in 0..3 {
            positions.push(((pa[k] as f64 + pb[k] as f64) / 2.0) as f32);
        }
    }
    debug_assert_eq!(positions.len() / 3, base as usize + extra);
    control.progress(0.4);
    let count = mesh.triangle_count();
    let mut indices = Vec::with_capacity(mesh.indices.len() + extra * 12);
    let mut origin = Vec::with_capacity(count + extra * 4);
    for (t, &v) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        if t % CHECK_EVERY == 0 {
            control.check()?;
            control.progress(0.4 + 0.6 * t as f64 / count as f64);
        }
        let m: [u32; 3] = std::array::from_fn(|k| {
            let e = edges.edge_of((3 * t + k) as u32);
            if e != NONE && split[e as usize] {
                mid[e as usize]
            } else {
                NONE
            }
        });
        let before = indices.len();
        match m.iter().filter(|&&x| x != NONE).count() {
            0 => indices.extend_from_slice(&v),
            3 => indices.extend_from_slice(&[
                v[0], m[0], m[2], //
                v[1], m[1], m[0], //
                v[2], m[2], m[1], //
                m[0], m[1], m[2],
            ]),
            1 => {
                let k = m.iter().position(|&x| x != NONE).unwrap_or(0);
                let (next, other) = (v[(k + 1) % 3], v[(k + 2) % 3]);
                indices.extend_from_slice(&[v[k], m[k], other, m[k], next, other]);
            }
            _ => {
                // The unsplit edge is k; the corner across it is the tip of the two split edges.
                let k = m.iter().position(|&x| x == NONE).unwrap_or(0);
                let tip = (k + 2) % 3;
                let left = (tip + 2) % 3;
                let right = (tip + 1) % 3;
                indices.extend_from_slice(&[
                    v[tip], m[tip], m[left], //
                    v[left], m[left], v[right], //
                    m[left], m[tip], v[right],
                ]);
            }
        }
        origin.resize(origin.len() + (indices.len() - before) / 3, t as u32);
    }
    control.progress(1.0);
    Ok(Some(Split {
        mesh: Mesh { positions, indices },
        origin,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::topology::Edges;
    use crate::repair::{RepairError, silent};

    fn tri(positions: &[f32], indices: &[u32]) -> Mesh {
        Mesh {
            positions: positions.to_vec(),
            indices: indices.to_vec(),
        }
    }

    fn open_edges(m: &Mesh) -> usize {
        let edges = Edges::new(m);
        (0..edges.len() as u32)
            .filter(|&e| edges.degree(e) == 1)
            .count()
    }

    #[test]
    fn subdividing_splits_a_triangle_into_four_at_its_midpoints() {
        let face = tri(&[0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0], &[0, 1, 2]);
        let out = silent(|c| subdivide(&face, &[], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.message, "Added 3 triangles.");
        assert_eq!(out.origin, Origin::Map(vec![0; 4]));
        assert_eq!(out.select, Some(Vec::new()));
        // Midpoints in edge order: (0,1), (0,2), (1,2).
        assert_eq!(
            out.mesh.positions[9..],
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]
        );
        assert_eq!(out.mesh.indices, [0, 3, 4, 1, 5, 3, 2, 4, 5, 3, 5, 4]);
        // Each piece faces the way the triangle did, and they cover it exactly.
        assert_eq!(stats(&out.mesh).area, stats(&face).area);
    }

    #[test]
    fn subdividing_a_selection_splits_its_neighbours_along_the_shared_edges() {
        let pair = tri(
            &[0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0],
            &[0, 1, 2, 1, 3, 2],
        );
        let out = silent(|c| subdivide(&pair, &[0], c)).unwrap();
        assert_eq!(out.message, "Added 4 triangles.");
        assert_eq!(out.mesh.vertex_count(), 7);
        assert_eq!(out.origin, Origin::Map(vec![0, 0, 0, 0, 1, 1]));
        // No T-junction: the only open edges are the outline's, the selected triangle's two split
        // in two.
        assert_eq!(open_edges(&out.mesh), 6);
    }

    #[test]
    fn subdividing_a_closed_part_keeps_it_closed() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let out = silent(|c| subdivide(&cube, &[], c)).unwrap();
        assert_eq!(out.mesh.triangle_count(), 48);
        let s = stats(&out.mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert_eq!(s.volume, 8000.0);
        // Half of it: the border between selected and unselected stays closed too.
        let half = silent(|c| subdivide(&cube, &[0, 1, 2, 3, 4, 5], c)).unwrap();
        let s = stats(&half.mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert_eq!(s.volume, 8000.0);
    }

    #[test]
    fn a_point_goes_in_the_middle_of_each_selected_triangle() {
        let cube = make_box([0.0; 3], [2.0; 3]);
        let out = silent(|c| add_point(&cube, &[0], c)).unwrap();
        assert_eq!(out.message, "Added 1 point.");
        assert_eq!(out.mesh.triangle_count(), 14);
        assert_eq!(out.mesh.vertex_count(), 9);
        let [a, b, c] = [0, 1, 2].map(|k| cube.vertex(cube.indices[k]));
        let centre: [f32; 3] =
            std::array::from_fn(|k| ((a[k] as f64 + b[k] as f64 + c[k] as f64) / 3.0) as f32);
        assert_eq!(out.mesh.vertex(8), centre);
        let [i, j, k] = [cube.indices[0], cube.indices[1], cube.indices[2]];
        assert_eq!(out.mesh.indices[..9], [i, j, 8, j, k, 8, k, i, 8]);
        assert_eq!(out.mesh.indices[9..], cube.indices[3..]);
        let mut origin = vec![0, 0, 0];
        origin.extend(1..12);
        assert_eq!(out.origin, Origin::Map(origin));
        assert_eq!(out.select, Some(Vec::new()));
        assert_eq!(stats(&out.mesh).volume, 8.0);
        let none = silent(|c| add_point(&cube, &[], c)).unwrap();
        assert!(!none.changed);
        assert_eq!(none.message, "No triangles are selected.");
    }

    #[test]
    fn a_cancelled_split_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        let cube = make_box([0.0; 3], [2.0; 3]);
        assert_eq!(
            subdivide(&cube, &[], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(
            add_point(&cube, &[0], &mut control),
            Err(RepairError::Cancelled)
        );
    }
}
