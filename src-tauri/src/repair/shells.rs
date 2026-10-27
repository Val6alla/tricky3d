//! The Shells tab: one row per shell, and the shell of each triangle so a row can select it.

use serde::Serialize;

use super::topology::{Edges, Link, components, is_consistent};
use crate::mesh::{Mesh, cross, dot, sub};

/// `ShellRow` in src/repair/types.ts. Lengths in mm, areas in mm², volumes in mm³ (the tab shows
/// cm² and cm³).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRow {
    pub triangles: u64,
    pub area: f64,
    pub bbox_volume: f64,
    /// |signed volume| for a watertight shell; None for an open one, whose volume means nothing.
    pub volume: Option<f64>,
    /// Every edge of the shell has exactly two triangles of the shell.
    pub watertight: bool,
}

/// One row per shell (`Link::Shell`: triangles joined across edges with two sides running opposite
/// ways, as the reference tool counts them), in order of first triangle, and
/// the row of each triangle. A shell's volume is measured from its first corner, so a tiny shell
/// far from the origin still gets a meaningful number.
pub fn shell_table(mesh: &Mesh) -> (Vec<ShellRow>, Vec<u32>) {
    let edges = Edges::new(mesh);
    let (shell_of, count) = components(mesh, &edges, Link::Shell);
    // Area and volume are summed twice and six times over, then divided once, as src/mesh.rs does.
    struct Sums {
        triangles: u64,
        area: f64,
        volume: f64,
        origin: [f64; 3],
        lo: [f64; 3],
        hi: [f64; 3],
        watertight: bool,
    }
    let mut sums: Vec<Sums> = (0..count)
        .map(|_| Sums {
            triangles: 0,
            area: 0.0,
            volume: 0.0,
            origin: [0.0; 3],
            lo: [f64::INFINITY; 3],
            hi: [f64::NEG_INFINITY; 3],
            watertight: true,
        })
        .collect();
    for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let s = &mut sums[shell_of[t] as usize];
        let corners = c.map(|v| mesh.point(v));
        if s.triangles == 0 {
            s.origin = corners[0];
        }
        s.triangles += 1;
        let [a, b, d] = corners;
        let n = cross(sub(b, a), sub(d, a));
        s.area += dot(n, n).sqrt();
        let [a, b, d] = corners.map(|p| sub(p, s.origin));
        s.volume += dot(a, cross(b, d));
        for p in corners {
            s.lo = [0, 1, 2].map(|k| s.lo[k].min(p[k]));
            s.hi = [0, 1, 2].map(|k| s.hi[k].max(p[k]));
        }
    }
    // A shell is closed when every edge of it joins it to itself: an edge that joins nothing
    // (open, non-manifold or turned) leaves each shell it touches open there.
    for e in 0..edges.len() as u32 {
        if !is_consistent(mesh, &edges, e) {
            for &side in edges.sides(e) {
                sums[shell_of[(side / 3) as usize] as usize].watertight = false;
            }
        }
    }
    let rows = sums
        .into_iter()
        .map(|s| ShellRow {
            triangles: s.triangles,
            area: s.area / 2.0,
            bbox_volume: (0..3).map(|k| s.hi[k] - s.lo[k]).product(),
            volume: s.watertight.then_some(s.volume.abs() / 6.0),
            watertight: s.watertight,
        })
        .collect();
    (rows, shell_of)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;

    #[test]
    fn a_cube_and_a_speck_are_two_rows() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        // 0.01 mm³ speck: a box 0.1 × 0.2 × 0.5 mm.
        let speck = make_box([30.0, 30.0, 30.0], [30.1, 30.2, 30.5]);
        let (rows, shell_of) = shell_table(&concat(&[&cube, &speck]).unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            ShellRow {
                triangles: 12,
                area: 2400.0,
                bbox_volume: 8000.0,
                volume: Some(8000.0),
                watertight: true,
            }
        );
        assert_eq!(rows[1].triangles, 12);
        // The speck's sides as f32 stores them (30.1 and 30.2 round).
        let [x, y, z] = [30.1f32, 30.2, 30.5].map(|v| v as f64 - 30.0);
        assert!((rows[1].volume.unwrap() - x * y * z).abs() < 1e-12);
        assert!((rows[1].bbox_volume - x * y * z).abs() < 1e-12);
        assert!((rows[1].area - 2.0 * (x * y + x * z + y * z)).abs() < 1e-12);
        assert!((rows[1].volume.unwrap() - 0.01).abs() < 1e-7);
        assert_eq!(shell_of, [vec![0; 12], vec![1; 12]].concat());
    }

    #[test]
    fn open_and_inside_out_shells() {
        let mut open = make_box([0.0; 3], [20.0; 3]);
        open.indices.truncate(30);
        let mut inside_out = make_box([50.0; 3], [60.0; 3]);
        for t in inside_out.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        let (rows, _) = shell_table(&concat(&[&open, &inside_out]).unwrap());
        assert_eq!((rows[0].triangles, rows[0].watertight), (10, false));
        assert_eq!(rows[0].volume, None);
        assert_eq!(rows[0].bbox_volume, 8000.0);
        assert_eq!(rows[1].volume, Some(1000.0));
        assert!(rows[1].watertight);
    }

    #[test]
    fn rows_follow_first_triangles_and_edges() {
        // Triangles of two cubes interleaved, the second cube first.
        let a = make_box([0.0; 3], [1.0; 3]);
        let b = make_box([5.0; 3], [7.0; 3]);
        let mut m = Mesh {
            positions: [a.positions.clone(), b.positions.clone()].concat(),
            indices: Vec::new(),
        };
        for k in 0..12 {
            m.indices
                .extend(b.indices[k * 3..k * 3 + 3].iter().map(|i| i + 8));
            m.indices.extend_from_slice(&a.indices[k * 3..k * 3 + 3]);
        }
        let (rows, shell_of) = shell_table(&m);
        assert_eq!(rows[0].volume, Some(8.0));
        assert_eq!(rows[1].volume, Some(1.0));
        assert!(
            shell_of
                .iter()
                .enumerate()
                .all(|(t, &s)| s == (t % 2) as u32)
        );
        // Two closed cubes sharing a corner vertex are two shells, both watertight.
        let mut glued = concat(&[&a, &make_box([1.0; 3], [2.0; 3])]).unwrap();
        for i in &mut glued.indices {
            if *i == 8 {
                *i = 6;
            }
        }
        let (rows, _) = shell_table(&glued);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.watertight && r.volume == Some(1.0)));
    }

    #[test]
    fn non_manifold_shells_are_not_watertight_and_nothing_has_no_rows() {
        // A fin on one of the box's edges: the fin is a shell of its own, and the box, still one
        // shell across its other edges, is open along that edge.
        let mut m = make_box([0.0; 3], [20.0; 3]);
        m.positions.extend_from_slice(&[10.0, -10.0, -10.0]);
        m.indices.extend_from_slice(&[0, 1, 8]);
        let (rows, shell_of) = shell_table(&m);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].triangles, rows[1].triangles), (12, 1));
        assert_eq!(shell_of[12], 1);
        assert!(rows.iter().all(|r| !r.watertight && r.volume.is_none()));
        assert_eq!(shell_table(&Mesh::default()), (vec![], vec![]));
    }

    #[test]
    fn a_tiny_shell_far_away_keeps_its_volume() {
        // 1 mm³ at 10 km: measured from the origin, rounding would swamp it.
        let m = make_box([1e7; 3], [1e7 + 1.0; 3]);
        let (rows, _) = shell_table(&m);
        assert_eq!(rows[0].volume, Some(1.0));
    }
}
