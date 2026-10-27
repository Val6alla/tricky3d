//! Selection edits from the ribbon and the context menu: Remove triangle (delete the selected
//! triangles) and Flip selected.

use super::topology::keep_triangles;
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// Deletes the selected triangles, and the vertices only they used. The remaining triangles keep
/// their order.
pub fn remove_selected(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    let (selected, count) = selected_mask(mesh, selection);
    if count == 0 {
        return Ok(Outcome::unchanged(mesh, NOTHING_SELECTED));
    }
    let keep: Vec<bool> = selected.iter().map(|&s| !s).collect();
    let (out, kept) = keep_triangles(mesh, &keep);
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(kept),
        message: format!("Removed {}.", triangles(count)),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// Turns the selected triangles to face the other way (swaps their second and third corners).
pub fn flip_selected(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    let (selected, count) = selected_mask(mesh, selection);
    if count == 0 {
        return Ok(Outcome::unchanged(mesh, NOTHING_SELECTED));
    }
    let mut out = mesh.clone();
    for (t, _) in out
        .indices
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(&selected)
        .filter(|(_, s)| **s)
    {
        t.swap(1, 2);
    }
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Same,
        message: format!("Flipped {}.", triangles(count)),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

pub(super) const NOTHING_SELECTED: &str = "No triangles are selected.";

/// One flag per triangle, set for the selected ones, and how many are set. Ids past the last
/// triangle are ignored and a repeated id counts once.
pub(super) fn selected_mask(mesh: &Mesh, selection: &[u32]) -> (Vec<bool>, usize) {
    let mut mask = vec![false; mesh.triangle_count()];
    let mut count = 0;
    for &t in selection {
        if let Some(slot) = mask.get_mut(t as usize)
            && !*slot
        {
            *slot = true;
            count += 1;
        }
    }
    (mask, count)
}

/// "1 triangle", "1,204 triangles".
pub(super) fn triangles(n: usize) -> String {
    let unit = if n == 1 { "triangle" } else { "triangles" };
    format!("{} {unit}", fmt_count(n as u64))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::{Control, RepairError, remap_selection, silent};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    #[test]
    fn removing_the_selection_deletes_exactly_those_triangles() {
        let mesh = cube();
        // The -z face (triangles 0 and 1) and one +x triangle.
        let out = silent(|c| remove_selected(&mesh, &[0, 1, 8], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.message, "Removed 3 triangles.");
        let kept: Vec<u32> = (0..12).filter(|t| ![0, 1, 8].contains(t)).collect();
        assert_eq!(out.origin, Origin::Map(kept.clone()));
        assert_eq!(out.mesh.triangle_count(), 9);
        // Every kept triangle has the same corners as before, in order.
        for (n, &t) in kept.iter().enumerate() {
            for k in 0..3 {
                let before = mesh.vertex(mesh.indices[t as usize * 3 + k]);
                let after = out.mesh.vertex(out.mesh.indices[n * 3 + k]);
                assert_eq!(before, after);
            }
        }
        // One hole: the open bottom joined with the missing +x triangle along edge 1-2.
        let s = stats(&out.mesh);
        assert_eq!(s.open_edges, 5);
        // The selection itself is gone; another selection follows its triangles.
        assert!(remap_selection(&[0, 1, 8], &out.origin, 9).is_empty());
        assert_eq!(remap_selection(&[2, 9], &out.origin, 9), vec![0, 6]);
    }

    #[test]
    fn removing_every_triangle_leaves_an_empty_mesh() {
        let mesh = cube();
        let all: Vec<u32> = (0..12).collect();
        let out = silent(|c| remove_selected(&mesh, &all, c)).unwrap();
        assert_eq!(out.mesh, Mesh::default());
        assert_eq!(out.message, "Removed 12 triangles.");
    }

    #[test]
    fn vertices_only_removed_triangles_used_go() {
        let mut mesh = cube();
        mesh.positions.extend_from_slice(&[50.0, 50.0, 50.0]);
        mesh.indices.extend_from_slice(&[8, 1, 2]);
        let out = silent(|c| remove_selected(&mesh, &[12], c)).unwrap();
        assert_eq!(out.mesh, cube());
        assert_eq!(out.message, "Removed 1 triangle.");
    }

    #[test]
    fn flipping_turns_only_the_selected_triangles() {
        let mesh = cube();
        let out = silent(|c| flip_selected(&mesh, &[2, 3], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.message, "Flipped 2 triangles.");
        assert_eq!(out.mesh.positions, mesh.positions);
        for t in 0..12 {
            let a = &mesh.indices[t * 3..t * 3 + 3];
            let b = &out.mesh.indices[t * 3..t * 3 + 3];
            if t == 2 || t == 3 {
                assert_eq!(b, [a[0], a[2], a[1]]);
            } else {
                assert_eq!(a, b);
            }
        }
        // The +z face turned: the volume loses twice its contribution (2 × 20 × 400 / 3).
        assert!((stats(&out.mesh).volume - (8000.0 - 2.0 * 8000.0 / 3.0)).abs() < 1e-9);
        // Flipping the same triangles again restores the cube.
        let back = silent(|c| flip_selected(&out.mesh, &[2, 3], c)).unwrap();
        assert_eq!(back.mesh, mesh);
    }

    #[test]
    fn flipping_everything_turns_the_cube_inside_out() {
        let all: Vec<u32> = (0..12).collect();
        let out = silent(|c| flip_selected(&cube(), &all, c)).unwrap();
        assert_eq!(stats(&out.mesh).volume, -8000.0);
        assert_eq!(out.message, "Flipped 12 triangles.");
    }

    #[test]
    fn an_empty_selection_changes_nothing() {
        let mesh = cube();
        for out in [
            silent(|c| remove_selected(&mesh, &[], c)).unwrap(),
            silent(|c| flip_selected(&mesh, &[], c)).unwrap(),
        ] {
            assert!(!out.changed);
            assert_eq!(out.origin, Origin::Same);
            assert_eq!(out.mesh, mesh);
            assert_eq!(out.message, "No triangles are selected.");
        }
    }

    #[test]
    fn ids_past_the_end_are_ignored_and_repeats_count_once() {
        let mesh = cube();
        let out = silent(|c| flip_selected(&mesh, &[4, 4, 99], c)).unwrap();
        assert_eq!(out.message, "Flipped 1 triangle.");
        let out = silent(|c| remove_selected(&mesh, &[12, 4000], c)).unwrap();
        assert!(!out.changed);
    }

    #[test]
    fn a_cancelled_edit_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            remove_selected(&cube(), &[0], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(
            flip_selected(&cube(), &[0], &mut control),
            Err(RepairError::Cancelled)
        );
    }
}
