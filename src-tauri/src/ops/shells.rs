//! Split shells into parts: connected components of a triangle mesh.

use super::signed_volume;
use crate::mesh::Mesh;
use crate::repair::topology::{Edges, Link, components};

/// The shells of a mesh (`Link::Shell`: triangles joined across edges with two sides running
/// opposite ways, as the reference tool splits them), each a mesh of its own. A vertex that
/// several shells use (where they touch at a corner or meet along a non-manifold edge) goes into
/// each of them. Each shell keeps only the vertices its triangles use, in their original relative
/// order, and its triangles in their original order. Sorted by |signed volume| largest first, ties
/// in order of first triangle.
///
/// No welding on purpose: Merge parts concatenates without welding, so splitting a merged part over
/// its own indices gives back exactly the meshes that went in, even where two of them touch.
pub fn split_shells(mesh: &Mesh) -> Vec<Mesh> {
    let (shell_of, count) = components(mesh, &Edges::new(mesh), Link::Shell);
    if count <= 1 {
        return if count == 0 {
            Vec::new()
        } else {
            vec![super::compact(&mesh.positions, mesh.indices.clone())]
        };
    }
    let tris = mesh.indices.as_chunks::<3>().0;
    let key = |s: u32, v: u32| (s as u64) << 32 | v as u64;
    // Every (shell, vertex) pair once, shell by shell and each shell's vertices ascending, so a
    // vertex's place among its shell's pairs is its index in that shell.
    let mut used: Vec<u64> = tris
        .iter()
        .zip(&shell_of)
        .flat_map(|(t, &s)| t.map(|v| key(s, v)))
        .collect();
    used.sort_unstable();
    used.dedup();
    let mut start = vec![0usize; count + 1];
    let mut shells: Vec<Mesh> = (0..count).map(|_| Mesh::default()).collect();
    for &k in &used {
        let (s, v) = ((k >> 32) as usize, k as u32 as usize);
        start[s + 1] += 1;
        shells[s]
            .positions
            .extend_from_slice(&mesh.positions[v * 3..v * 3 + 3]);
    }
    for s in 0..count {
        start[s + 1] += start[s];
    }
    for (t, &s) in tris.iter().zip(&shell_of) {
        let own = &used[start[s as usize]..start[s as usize + 1]];
        shells[s as usize]
            .indices
            .extend(t.map(|v| own.binary_search(&key(s, v)).unwrap() as u32));
    }

    let mut sized: Vec<(f64, Mesh)> = shells
        .into_iter()
        .map(|m| (signed_volume(&m).abs(), m))
        .collect();
    // Stable, and NaN cannot occur: positions passed `Mesh::check`.
    sized.sort_by(|a, b| b.0.total_cmp(&a.0));
    sized.into_iter().map(|(_, m)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;

    #[test]
    fn shells_sorted_by_volume_keep_their_own_order() {
        let small = make_box([0.0; 3], [1.0; 3]);
        let big = make_box([5.0; 3], [8.0; 3]);
        let merged = concat(&[&small, &big]).unwrap();
        let shells = split_shells(&merged);
        assert_eq!(shells, vec![big, small]);
    }

    #[test]
    fn equal_volumes_keep_first_triangle_order_and_unused_vertices_go() {
        let a = make_box([0.0; 3], [1.0; 3]);
        let b = make_box([5.0; 3], [6.0; 3]);
        let mut merged = concat(&[&a, &b]).unwrap();
        merged.positions.extend_from_slice(&[99.0, 99.0, 99.0]);
        let shells = split_shells(&merged);
        assert_eq!(shells, vec![a, b]);
    }

    #[test]
    fn touching_shells_stay_apart_without_welding() {
        // Two cubes sharing a face position but not vertex indices are two shells.
        let a = make_box([0.0; 3], [1.0; 3]);
        let b = make_box([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        let shells = split_shells(&concat(&[&a, &b]).unwrap());
        assert_eq!(shells.len(), 2);
        assert_eq!(stats(&shells[0]).volume, 1.0);
    }

    #[test]
    fn shells_split_at_shared_corners_and_non_manifold_edges() {
        // Two cubes sharing one corner vertex index: each gets its own copy of it.
        let a = make_box([0.0; 3], [1.0; 3]);
        let mut glued = concat(&[&a, &make_box([1.0; 3], [2.0; 3])]).unwrap();
        for i in &mut glued.indices {
            if *i == 8 {
                *i = 6;
            }
        }
        let shells = split_shells(&glued);
        assert_eq!(shells.len(), 2);
        assert_eq!(shells[0], a);
        assert!(
            shells
                .iter()
                .all(|m| m.vertex_count() == 8 && stats(m).volume == 1.0)
        );
        // A fin on one of the box's edges is a shell of its own, sharing that edge's vertices.
        let mut finned = a.clone();
        finned.positions.extend_from_slice(&[0.5, -1.0, -1.0]);
        finned.indices.extend_from_slice(&[0, 1, 8]);
        let shells = split_shells(&finned);
        assert_eq!(shells.len(), 2);
        assert_eq!(shells[0], a);
        assert_eq!(shells[1].indices, vec![0, 1, 2]);
        assert_eq!(&shells[1].positions[..6], &a.positions[..6]);
    }

    #[test]
    fn interleaved_triangles_and_inside_out_shells() {
        // Triangles of two shells alternate; the second shell is inside out (negative volume), so
        // it sorts by its absolute volume.
        let a = make_box([0.0; 3], [1.0; 3]);
        let mut b = make_box([5.0; 3], [7.0; 3]);
        for t in b.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        let mut mixed = Mesh {
            positions: [a.positions.clone(), b.positions.clone()].concat(),
            indices: Vec::new(),
        };
        for k in 0..12 {
            mixed
                .indices
                .extend_from_slice(&a.indices[k * 3..k * 3 + 3]);
            mixed
                .indices
                .extend(b.indices[k * 3..k * 3 + 3].iter().map(|i| i + 8));
        }
        let shells = split_shells(&mixed);
        assert_eq!(shells, vec![b, a]);
    }

    #[test]
    fn one_shell_and_no_triangles() {
        let a = make_box([0.0; 3], [1.0; 3]);
        assert_eq!(split_shells(&a), vec![a]);
        assert!(split_shells(&Mesh::default()).is_empty());
        // A single non-manifold or open shell is still one shell.
        let mut open = make_box([0.0; 3], [1.0; 3]);
        open.indices.truncate(30);
        assert_eq!(split_shells(&open).len(), 1);
    }
}
