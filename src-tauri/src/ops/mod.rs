//! Mesh operations behind Split shells, Merge parts, Cut and Generate shell. They work on world-space
//! meshes (the jobs bake each part's matrix first) and never touch Tauri types.

pub(crate) mod collapse;
pub mod cut;
pub mod shell;
pub mod shells;

use crate::mesh::Mesh;

/// Concatenates meshes without welding: each mesh keeps its vertex order, later indices are offset.
/// Merge parts relies on this keeping every shell exactly as it was, so Split shells gives the
/// parts back. Errors with a reason clause when the result would exceed u32 vertex/index limits.
pub fn concat(meshes: &[&Mesh]) -> Result<Mesh, String> {
    let vertices: usize = meshes.iter().map(|m| m.vertex_count()).sum();
    let corners: usize = meshes.iter().map(|m| m.indices.len()).sum();
    // The same limits as `Mesh::check`: every index and the index count must fit below u32::MAX.
    if vertices >= u32::MAX as usize || corners >= u32::MAX as usize {
        return Err("the merged part would have more triangles than tricky3D can handle".into());
    }
    let mut out = Mesh {
        positions: Vec::with_capacity(vertices * 3),
        indices: Vec::with_capacity(corners),
    };
    for m in meshes {
        let base = out.vertex_count() as u32;
        out.positions.extend_from_slice(&m.positions);
        out.indices.extend(m.indices.iter().map(|i| i + base));
    }
    Ok(out)
}

/// Moves every vertex by `offset` (f64 maths, one rounding to f32 per coordinate).
pub fn translate(mesh: &mut Mesh, offset: [f64; 3]) {
    for p in mesh.positions.as_chunks_mut::<3>().0 {
        for k in 0..3 {
            p[k] = (p[k] as f64 + offset[k]) as f32;
        }
    }
}

/// The mesh reduced to the vertices `indices` use, in their original order, with the indices
/// renumbered to match. Every operation that drops triangles ends with this.
pub(crate) fn compact(positions: &[f32], indices: Vec<u32>) -> Mesh {
    const UNUSED: u32 = u32::MAX;
    let mut remap = vec![UNUSED; positions.len() / 3];
    for &i in &indices {
        remap[i as usize] = 0;
    }
    let mut out = Vec::new();
    let mut next = 0u32;
    for (v, slot) in remap.iter_mut().enumerate() {
        if *slot != UNUSED {
            *slot = next;
            next += 1;
            out.extend_from_slice(&positions[v * 3..v * 3 + 3]);
        }
    }
    let mut indices = indices;
    for i in &mut indices {
        *i = remap[*i as usize];
    }
    Mesh {
        positions: out,
        indices,
    }
}

/// Signed volume in mm³ (positive for a closed mesh facing outwards), the same sum as the info
/// panel's.
pub(crate) fn signed_volume(mesh: &Mesh) -> f64 {
    use crate::mesh::{cross, dot};
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| dot(mesh.point(t[0]), cross(mesh.point(t[1]), mesh.point(t[2]))))
        .sum::<f64>()
        / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;

    #[test]
    fn concat_offsets_later_indices_and_keeps_vertex_order() {
        let a = make_box([0.0; 3], [1.0; 3]);
        let b = make_box([5.0; 3], [6.0; 3]);
        let m = concat(&[&a, &b]).unwrap();
        assert_eq!(m.positions[..24], a.positions[..]);
        assert_eq!(m.positions[24..], b.positions[..]);
        assert_eq!(m.indices[..36], a.indices[..]);
        assert!(
            m.indices[36..]
                .iter()
                .zip(&b.indices)
                .all(|(x, y)| *x == y + 8)
        );
        assert_eq!(concat(&[]).unwrap(), Mesh::default());
    }

    #[test]
    fn translate_rounds_once_from_f64() {
        let mut m = Mesh {
            positions: vec![0.1, 1.0, -2.0],
            indices: vec![],
        };
        translate(&mut m, [0.2, 0.0, 1e-9]);
        assert_eq!(m.positions, vec![(0.1f32 as f64 + 0.2) as f32, 1.0, -2.0]);
    }

    #[test]
    fn compact_drops_unused_vertices_in_order() {
        let positions = [0.0, 0.0, 0.0, 9.0, 9.0, 9.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let m = compact(&positions, vec![3, 0, 2]);
        assert_eq!(
            m.positions,
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(m.indices, vec![2, 0, 1]);
    }
}
