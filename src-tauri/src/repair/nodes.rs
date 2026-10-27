//! Home › Repair › Move nodes: puts one vertex where the user dragged it. The window picks the
//! vertex and shows the drag; this checks the request, moves the vertex and says what the move did
//! to the triangles round it. The triangles themselves stay as they are.

use super::exact::{collinear, dot, normal, vertex};
use super::{Control, Origin, Outcome, RepairError, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// One vertex and where it goes (world space, mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeMove {
    pub vertex: u32,
    pub to: [f64; 3],
}

/// Moves the vertex `step.vertex` to `step.to` (rounded to f32, as every position is). Fails when
/// the vertex is no corner of any triangle or the position is not a finite point. Warns about the
/// triangles round it that now face the other way or have no area left; the selection stays.
pub fn move_node(mesh: &Mesh, step: &NodeMove, control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let v = step.vertex;
    let around: Vec<u32> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, t)| t.contains(&v))
        .map(|(t, _)| t as u32)
        .collect();
    if (v as usize) >= mesh.vertex_count() || around.is_empty() {
        return Err(RepairError::Failed(
            "The point to move is not part of the working copy any more.".into(),
        ));
    }
    if !step
        .to
        .iter()
        .all(|x| x.is_finite() && x.abs() < f32::MAX as f64)
    {
        return Err(RepairError::Failed(
            "The point cannot go there: its position is not a number.".into(),
        ));
    }
    let to = step.to.map(|x| x as f32);
    if to == mesh.vertex(v) {
        return Ok(Outcome::unchanged(mesh, "The point did not move."));
    }
    let mut out = mesh.clone();
    out.positions[v as usize * 3..v as usize * 3 + 3].copy_from_slice(&to);
    let (mut turned, mut flat) = (0u64, 0u64);
    for &t in &around {
        let corners = |m: &Mesh| {
            let i = t as usize * 3;
            [0, 1, 2].map(|k| vertex(m, m.indices[i + k]))
        };
        let [a, b, c] = corners(&out);
        if collinear(a, b, c) {
            flat += 1;
            continue;
        }
        let [p, q, r] = corners(mesh);
        // A triangle that had no area faced no way, so it cannot have turned.
        if !collinear(p, q, r) && dot(normal(p, q, r), normal(a, b, c)) < 0.0 {
            turned += 1;
        }
    }
    control.progress(1.0);
    let mut warnings = Vec::new();
    let triangles = |n: u64| {
        if n == 1 {
            "1 triangle".to_string()
        } else {
            format!("{} triangles", fmt_count(n))
        }
    };
    if turned > 0 {
        let verb = if turned == 1 { "faces" } else { "face" };
        warnings.push(format!(
            "{} round the point now {verb} the other way.",
            triangles(turned)
        ));
    }
    if flat > 0 {
        let verb = if flat == 1 { "has" } else { "have" };
        warnings.push(format!(
            "{} round the point {verb} no area now.",
            triangles(flat)
        ));
    }
    let [x, y, z] = to;
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Same,
        message: format!("Moved the point to {x:.4}, {y:.4}, {z:.4}."),
        warnings,
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
    use crate::repair::silent;

    fn moved(mesh: &Mesh, vertex: u32, to: [f64; 3]) -> RepairResult<Outcome> {
        silent(|c| move_node(mesh, &NodeMove { vertex, to }, c))
    }

    #[test]
    fn the_vertex_moves_and_its_triangles_follow() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let out = moved(&cube, 0, [-5.0, -5.0, -5.0]).unwrap();
        assert!(out.changed);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.select, None);
        assert_eq!(out.mesh.indices, cube.indices);
        assert_eq!(out.mesh.vertex(0), [-5.0, -5.0, -5.0]);
        assert_eq!(out.mesh.positions[3..], cube.positions[3..]);
        assert_eq!(out.message, "Moved the point to -5.0000, -5.0000, -5.0000.");
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let s = stats(&out.mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert!(s.volume > 8000.0);
    }

    #[test]
    fn a_move_through_the_part_warns_about_turned_and_flat_triangles() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        // Corner 0 pushed through to the far side: the triangles round it face inwards now.
        let through = moved(&cube, 0, [30.0, 30.0, 30.0]).unwrap();
        assert!(
            through
                .warnings
                .iter()
                .any(|w| w.ends_with("round the point now face the other way.")),
            "{:?}",
            through.warnings
        );
        // Onto the bottom face's diagonal: the bottom triangles that use it have no area.
        let onto = moved(&cube, 0, [20.0, 20.0, 0.0]).unwrap();
        assert!(
            onto.warnings
                .iter()
                .any(|w| w.ends_with("round the point have no area now.")
                    || w.ends_with("round the point has no area now.")),
            "{:?}",
            onto.warnings
        );
    }

    #[test]
    fn bad_requests_fail_and_no_move_changes_nothing() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        assert!(matches!(
            moved(&cube, 99, [0.0; 3]),
            Err(RepairError::Failed(_))
        ));
        assert!(matches!(
            moved(&cube, 0, [f64::NAN, 0.0, 0.0]),
            Err(RepairError::Failed(_))
        ));
        assert!(matches!(
            moved(&cube, 0, [1e300, 0.0, 0.0]),
            Err(RepairError::Failed(_))
        ));
        // A vertex no triangle uses.
        let mut loose = cube.clone();
        loose.positions.extend_from_slice(&[1.0, 1.0, 1.0]);
        assert!(matches!(
            moved(&loose, 8, [2.0; 3]),
            Err(RepairError::Failed(_))
        ));
        let same = moved(&cube, 0, [0.0; 3]).unwrap();
        assert!(!same.changed);
        assert_eq!(same.message, "The point did not move.");
        // Rounding to f32 back onto the old position is no move either.
        let rounded = moved(&cube, 0, [1e-12, 0.0, 0.0]).unwrap();
        assert!(rounded.changed, "1e-12 is a distinct f32");
        let back = moved(&cube, 6, [20.000000000001, 20.0, 20.0]).unwrap();
        assert!(!back.changed);
    }

    #[test]
    fn a_cancelled_move_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        let cube = make_box([0.0; 3], [20.0; 3]);
        let step = NodeMove {
            vertex: 0,
            to: [1.0; 3],
        };
        assert_eq!(
            move_node(&cube, &step, &mut control),
            Err(RepairError::Cancelled)
        );
    }
}
