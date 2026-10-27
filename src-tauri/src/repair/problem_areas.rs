//! Remove problem areas: delete the triangles that are degenerate, intersect another triangle, or
//! touch a non-manifold edge, so the holes can be closed again. With a selection, only selected
//! problem triangles go.

use super::defaults::{DEGENERATE, effective_degenerate_tolerance};
use super::status::degenerate_triangles;
use super::topology::{Edges, keep_triangles};
use super::tritri::intersecting_triangles;
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// Why a triangle is a problem, as bits.
const DEGENERATE_FACE: u8 = 1;
const INTERSECTING: u8 = 2;
const NON_MANIFOLD: u8 = 4;

/// Deletes every problem triangle (with a selection, every selected one) and the vertices only
/// they used: the degenerate ones at the Remove degenerate faces default tolerance (the Status
/// tab's count), both triangles of every intersecting pair, and those with a side on an edge of
/// three or more triangles. The others keep their order. Deleting triangles makes none of those
/// problems, so a second run finds nothing.
pub fn run(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let count = mesh.triangle_count();
    let mut why = vec![0u8; count];
    let tolerance = effective_degenerate_tolerance(&DEGENERATE);
    for t in degenerate_triangles(mesh, tolerance) {
        why[t as usize] |= DEGENERATE_FACE;
    }
    control.progress(0.05);
    for t in intersecting_triangles(mesh, &mut control.sub(0.05, 0.85))? {
        why[t as usize] |= INTERSECTING;
    }
    control.check()?;
    let edges = Edges::new(mesh);
    for e in 0..edges.len() as u32 {
        if edges.degree(e) >= 3 {
            for &s in edges.sides(e) {
                why[(s / 3) as usize] |= NON_MANIFOLD;
            }
        }
    }
    drop(edges);
    if !selection.is_empty() {
        let mut selected = vec![false; count];
        for &t in selection {
            if let Some(s) = selected.get_mut(t as usize) {
                *s = true;
            }
        }
        for (w, s) in why.iter_mut().zip(&selected) {
            if !s {
                *w = 0;
            }
        }
    }
    control.progress(0.95);
    let removed = why.iter().filter(|&&w| w != 0).count();
    if removed == 0 {
        return Ok(Outcome::unchanged(mesh, "No problem areas found."));
    }
    let keep: Vec<bool> = why.iter().map(|&w| w == 0).collect();
    let (out, kept) = keep_triangles(mesh, &keep);
    let tally = |bit: u8| why.iter().filter(|&&w| w & bit != 0).count();
    let reasons: Vec<String> = [
        (tally(DEGENERATE_FACE), "degenerate"),
        (tally(INTERSECTING), "self-intersecting"),
        (tally(NON_MANIFOLD), "at non-manifold edges"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{} {what}", fmt_count(*n as u64)))
    .collect();
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(kept),
        message: format!("Removed {} ({}).", triangles(removed), reasons.join(", ")),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// "1 triangle", "1,204 triangles".
fn triangles(n: usize) -> String {
    let unit = if n == 1 { "triangle" } else { "triangles" };
    format!("{} {unit}", fmt_count(n as u64))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;
    use crate::repair::exact::testing::torus;
    use crate::repair::status::analyze;
    use crate::repair::{RepairError, remap_selection, silent, tritri};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn act(mesh: &Mesh, selection: &[u32]) -> Outcome {
        silent(|c| run(mesh, selection, c)).unwrap()
    }

    /// The cube with its top face as a fan round a point 0.0002 mm from the front edge: one
    /// degenerate cap, triangle 10 (the cube-sliver fixture).
    fn cube_sliver() -> Mesh {
        let mut m = cube();
        m.indices.drain(6..12);
        m.positions.extend_from_slice(&[10.0, 0.0, 20.0]);
        for [a, b] in [[4, 5], [5, 6], [6, 7], [7, 4]] {
            m.indices.extend_from_slice(&[a, b, 8]);
        }
        m
    }

    /// The cube with a fin on its bottom front edge 0-1: that edge has three triangles.
    fn finned() -> Mesh {
        let mut m = cube();
        m.positions.extend_from_slice(&[10.0, -10.0, -10.0]);
        m.indices.extend_from_slice(&[0, 1, 8]);
        m
    }

    fn two_cubes() -> Mesh {
        concat(&[&cube(), &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0])]).unwrap()
    }

    fn intersecting(mesh: &Mesh) -> Vec<u32> {
        silent(|c| tritri::intersecting_triangles(mesh, c)).unwrap()
    }

    #[test]
    fn a_clean_cube_has_no_problem_areas() {
        let m = cube();
        let out = act(&m, &[]);
        assert!(!out.changed);
        assert_eq!(out.mesh, m);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.message, "No problem areas found.");
    }

    #[test]
    fn a_degenerate_sliver_goes_and_leaves_a_hole() {
        let m = cube_sliver();
        let out = act(&m, &[]);
        assert_eq!(out.message, "Removed 1 triangle (1 degenerate).");
        let kept: Vec<u32> = (0..14).filter(|&t| t != 10).collect();
        assert_eq!(out.origin, Origin::Map(kept));
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert_eq!((s.holes, s.border_edges, s.degenerate_triangles), (1, 3, 0));
        assert!(!act(&out.mesh, &[]).changed);
    }

    #[test]
    fn intersecting_triangles_go_in_pairs() {
        let m = two_cubes();
        let found = intersecting(&m);
        assert!(!found.is_empty());
        let out = act(&m, &[]);
        assert_eq!(
            out.message,
            format!(
                "Removed {} ({} self-intersecting).",
                triangles(found.len()),
                fmt_count(found.len() as u64)
            )
        );
        assert_eq!(out.mesh.triangle_count(), m.triangle_count() - found.len());
        assert!(intersecting(&out.mesh).is_empty());
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert!(s.holes > 0 && s.non_manifold_edges == 0, "{s:?}");
        let again = act(&out.mesh, &[]);
        assert!(!again.changed, "{}", again.message);
    }

    #[test]
    fn triangles_on_a_non_manifold_edge_go() {
        let m = finned();
        let out = act(&m, &[]);
        // The fin and the two cube triangles on edge 0-1: the bottom (0, 2, 1) and the front
        // (0, 1, 5).
        assert_eq!(
            out.message,
            "Removed 3 triangles (3 at non-manifold edges)."
        );
        let Origin::Map(kept) = &out.origin else {
            panic!("triangles were removed")
        };
        for t in [0, 4, 12] {
            assert!(!kept.contains(&t), "triangle {t} stayed");
        }
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert_eq!(s.non_manifold_edges, 0);
        assert_eq!(s.holes, 1);
        assert!(!act(&out.mesh, &[]).changed);
    }

    #[test]
    fn every_kind_at_once_is_counted_by_reason() {
        // The sliver cube, the finned cube and the two interpenetrating cubes side by side.
        let mut sliver = cube_sliver();
        crate::ops::translate(&mut sliver, [100.0, 0.0, 0.0]);
        let mut fin = finned();
        crate::ops::translate(&mut fin, [200.0, 0.0, 0.0]);
        let two = two_cubes();
        let m = concat(&[&two, &sliver, &fin]).unwrap();
        let crossing = intersecting(&two).len();
        let out = act(&m, &[]);
        assert_eq!(
            out.message,
            format!(
                "Removed {} (1 degenerate, {crossing} self-intersecting, 3 at non-manifold \
                 edges).",
                triangles(crossing + 4)
            )
        );
        assert!(!act(&out.mesh, &[]).changed);
    }

    #[test]
    fn with_a_selection_only_selected_problem_triangles_go() {
        let m = finned();
        // Select the fin (12) and a good triangle (5): only the fin goes.
        let out = act(&m, &[5, 12]);
        assert_eq!(out.message, "Removed 1 triangle (1 at non-manifold edges).");
        assert_eq!(out.origin, Origin::Map((0..12).collect()));
        assert_eq!(
            remap_selection(&[5, 12], &out.origin, out.mesh.triangle_count()),
            vec![5]
        );
        // Nothing selected is a problem: nothing changes.
        let out = act(&m, &[5, 6, 4000]);
        assert!(!out.changed);
        assert_eq!(out.message, "No problem areas found.");
    }

    #[test]
    fn a_cancelled_run_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            run(&two_cubes(), &[], &mut control),
            Err(RepairError::Cancelled)
        );
    }

    /// `cargo test --release --lib repair::problem_areas -- --ignored --nocapture`: 730k triangles
    /// in 27 overlapping tori, and a clean torus.
    #[test]
    #[ignore]
    fn timing_on_730k_triangles() {
        let one = torus(30.0, 8.0, 150, 90);
        let mut parts = Vec::new();
        for k in 0..27 {
            let mut t = one.clone();
            crate::ops::translate(
                &mut t,
                [
                    (k % 3) as f64 * 25.0,
                    (k / 3 % 3) as f64 * 25.0,
                    (k / 9) as f64 * 12.0,
                ],
            );
            parts.push(t);
        }
        let m = concat(&parts.iter().collect::<Vec<_>>()).unwrap();
        for mesh in [&m, &torus(30.0, 8.0, 520, 700)] {
            let started = Instant::now();
            let out = act(mesh, &[]);
            println!(
                "remove problem areas: {} triangles in {:.3} s: {}",
                mesh.triangle_count(),
                started.elapsed().as_secs_f64(),
                out.message
            );
        }
    }

    /// Real files, with timings: `TRICKY3D_MODELS="a.stl;b.stl" cargo test --release --lib
    /// repair::problem_areas::tests::real_models -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_models() {
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            for part in &imported.parts {
                let m = &part.mesh;
                let started = Instant::now();
                let out = act(m, &[]);
                let took = started.elapsed().as_secs_f64();
                let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
                println!(
                    "{path}: {} triangles in {took:.3} s: {} Then: {} non-manifold edges, {} \
                     degenerate, {} holes.",
                    m.triangle_count(),
                    out.message,
                    s.non_manifold_edges,
                    s.degenerate_triangles,
                    s.holes
                );
                assert_eq!(s.non_manifold_edges, 0);
                assert!(intersecting(&out.mesh).is_empty());
                assert!(!act(&out.mesh, &[]).changed);
            }
        }
    }
}
