//! Smooth triangles: one smoothing step over the selection, or over the whole part. It was PR #1's
//! Move point (src/repair/edits.ts there, then a Home-only action) and moves the same points to
//! the same f32 positions.

use super::edit::selected_mask;
use super::topology::{Edges, side_ends};
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// Vertices between cancel checks.
const CHECK_EVERY: usize = 1 << 16;

/// Moves each point inside the selection (with none, of the whole part) to the average of its
/// neighbours (one step, every point from the old positions). A point is inside when every
/// triangle round it is selected and none of its edges is open or non-manifold; the others stay
/// where they are. The triangles and the selection do not change.
pub fn smooth_triangles(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    let (mut selected, count) = selected_mask(mesh, selection);
    if count == 0 {
        selected.fill(true);
    }
    let verts = mesh.vertex_count();
    let mut incident = vec![0u32; verts];
    let mut picked = vec![0u32; verts];
    for (t, corners) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        for &v in corners {
            incident[v as usize] += 1;
            if selected[t] {
                picked[v as usize] += 1;
            }
        }
    }
    let edges = Edges::new(mesh);
    control.progress(0.3);
    control.check()?;
    let mut fixed = vec![false; verts];
    // Each vertex's neighbours in edge order: the order the window summed them in, which decides
    // the last bit of the average.
    let mut start = vec![0usize; verts + 1];
    for e in 0..edges.len() as u32 {
        let [lo, hi] = edges.ends(e);
        if edges.degree(e) != 2 {
            fixed[lo as usize] = true;
            fixed[hi as usize] = true;
        }
        start[lo as usize + 1] += 1;
        start[hi as usize + 1] += 1;
    }
    for v in 0..verts {
        start[v + 1] += start[v];
    }
    let mut fill = start[..verts].to_vec();
    let mut neighbours = vec![0u32; start[verts]];
    for e in 0..edges.len() as u32 {
        let (a, b) = side_ends(mesh, edges.sides(e)[0]);
        neighbours[fill[a as usize]] = b;
        fill[a as usize] += 1;
        neighbours[fill[b as usize]] = a;
        fill[b as usize] += 1;
    }
    drop(fill);
    control.progress(0.6);
    let mut positions = mesh.positions.clone();
    let mut moved = 0usize;
    for v in 0..verts {
        if v % CHECK_EVERY == 0 {
            control.check()?;
        }
        let around = &neighbours[start[v]..start[v + 1]];
        if picked[v] == 0 || picked[v] != incident[v] || fixed[v] || around.is_empty() {
            continue;
        }
        let mut sum = [0.0f64; 3];
        for &u in around {
            let p = mesh.vertex(u);
            for k in 0..3 {
                sum[k] += p[k] as f64;
            }
        }
        let target = sum.map(|s| (s / around.len() as f64) as f32);
        if target != mesh.vertex(v as u32) {
            positions[v * 3..v * 3 + 3].copy_from_slice(&target);
            moved += 1;
        }
    }
    control.progress(1.0);
    if moved == 0 {
        return Ok(Outcome::unchanged(mesh, "No interior point to smooth."));
    }
    let unit = if moved == 1 { "point" } else { "points" };
    Ok(Outcome {
        mesh: Mesh {
            positions,
            indices: mesh.indices.clone(),
        },
        changed: true,
        origin: Origin::Same,
        message: format!("Smoothed {} {unit}.", fmt_count(moved as u64)),
        warnings: Vec::new(),
        concerns: Vec::new(),
        highlight: None,
        select: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::{RepairError, silent};

    /// Four triangles round vertex 0 (at (1, 0, 0), off centre) with an open rim 1-2-3-4.
    fn fan() -> Mesh {
        Mesh {
            positions: vec![
                1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0, 0.0,
            ],
            indices: vec![0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1],
        }
    }

    #[test]
    fn an_inner_point_moves_to_the_average_of_its_neighbours() {
        let out = silent(|c| smooth_triangles(&fan(), &[0, 1, 2, 3], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.message, "Smoothed 1 point.");
        assert_eq!(out.origin, Origin::Same);
        // The selection stays, so another click smooths the same triangles again.
        assert_eq!(out.select, None);
        assert_eq!(out.mesh.vertex(0), [0.0, 0.0, 0.0]);
        // The rim is open, so its points stay.
        assert_eq!(out.mesh.positions[3..], fan().positions[3..]);
        assert_eq!(out.mesh.indices, fan().indices);
    }

    #[test]
    fn a_point_with_an_unselected_triangle_stays() {
        let out = silent(|c| smooth_triangles(&fan(), &[0, 1, 2], c)).unwrap();
        assert!(!out.changed);
        assert_eq!(out.message, "No interior point to smooth.");
    }

    #[test]
    fn without_a_selection_the_whole_part_is_smoothed() {
        let none = silent(|c| smooth_triangles(&fan(), &[], c)).unwrap();
        let all = silent(|c| smooth_triangles(&fan(), &[0, 1, 2, 3], c)).unwrap();
        assert!(none.changed);
        assert_eq!(none, all);
    }

    #[test]
    fn smoothing_a_whole_closed_part_moves_every_point_once() {
        let cube = make_box([0.0; 3], [2.0; 3]);
        let all: Vec<u32> = (0..12).collect();
        let out = silent(|c| smooth_triangles(&cube, &all, c)).unwrap();
        assert_eq!(out.message, "Smoothed 8 points.");
        let s = stats(&out.mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert!(s.volume > 0.0 && s.volume < 8.0, "{}", s.volume);
        // From the old positions, not ones moved earlier in the same step: corner 0's neighbours
        // are all still corners of the cube.
        let corner = out.mesh.vertex(0);
        assert!(
            corner.iter().all(|&x| (0.0..=2.0).contains(&x)),
            "{corner:?}"
        );
    }

    #[test]
    fn a_cancelled_smoothing_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            smooth_triangles(&fan(), &[0, 1, 2, 3], &mut control),
            Err(RepairError::Cancelled)
        );
    }
}
