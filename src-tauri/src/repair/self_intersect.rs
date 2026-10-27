//! Detect self-intersections (highlight only), Split-off self-intersections (move the intersecting
//! triangles into a shell of their own) and Remove self-intersections (delete them, leaving holes
//! for a later Close all holes). With a selection, only pairs with a selected triangle count.
//!
//! Triangles intersect by tritri's rule: some point of one lies in the relative interior of the
//! other (crossing, piercing, or overlapping with positive area in a shared plane; touching along
//! boundaries does not count). Both triangles of every pair that counts are intersecting
//! triangles.

use super::topology::{NONE, keep_triangles};
use super::tritri::intersecting_pairs;
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};
use crate::ops::compact;

/// Highlights the intersecting triangles and changes nothing.
pub fn detect(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    let found = intersecting(mesh, selection, control)?;
    let message = if found.is_empty() {
        NONE_FOUND.to_string()
    } else {
        format!("Found {}.", triangles(found.len()))
    };
    let mut out = Outcome::unchanged(mesh, message);
    out.highlight = Some(found);
    Ok(out)
}

/// Gives the intersecting triangles their own copies of their vertices (shared among themselves)
/// and moves them after the others, so they form shells of their own and the rest keeps its order.
/// They still intersect each other, so a second run finds them again; when they are already split
/// off like that, nothing changes.
pub fn split_off(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    let found = intersecting(mesh, selection, control)?;
    if found.is_empty() {
        return Ok(Outcome::unchanged(mesh, NONE_FOUND));
    }
    let count = mesh.triangle_count();
    let mut hit = vec![false; count];
    for &t in &found {
        hit[t as usize] = true;
    }
    let corners = mesh.indices.as_chunks::<3>().0;
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut origin = Vec::with_capacity(count);
    for (t, c) in corners.iter().enumerate().filter(|&(t, _)| !hit[t]) {
        indices.extend_from_slice(c);
        origin.push(t as u32);
    }
    // The copies go after the original vertices; originals that only split-off triangles used
    // drop out when the mesh is compacted.
    let mut positions = mesh.positions.clone();
    let mut copy = vec![NONE; mesh.vertex_count()];
    for &t in &found {
        for &v in &corners[t as usize] {
            if copy[v as usize] == NONE {
                copy[v as usize] = (positions.len() / 3) as u32;
                positions.extend_from_slice(&mesh.vertex(v));
            }
            indices.push(copy[v as usize]);
        }
        origin.push(t);
    }
    let out = compact(&positions, indices);
    control.progress(1.0);
    if out == *mesh {
        return Ok(Outcome::unchanged(
            mesh,
            format!("{} already split off.", triangles_are(found.len())),
        ));
    }
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(origin),
        message: format!("Split off {}.", triangles(found.len())),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// Deletes the intersecting triangles, and the vertices only they used.
pub fn remove(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    let found = intersecting(mesh, selection, control)?;
    if found.is_empty() {
        return Ok(Outcome::unchanged(mesh, NONE_FOUND));
    }
    let mut keep = vec![true; mesh.triangle_count()];
    for &t in &found {
        keep[t as usize] = false;
    }
    let (out, kept) = keep_triangles(mesh, &keep);
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(kept),
        message: format!("Removed {}.", triangles(found.len())),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

const NONE_FOUND: &str = "No self-intersections found.";

/// Both triangles of every intersecting pair, or with a selection of every pair with a selected
/// triangle. Sorted.
fn intersecting(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Vec<u32>> {
    control.check()?;
    let count = mesh.triangle_count();
    let selected: Option<Vec<bool>> = (!selection.is_empty()).then(|| {
        let mut mask = vec![false; count];
        for &t in selection {
            if let Some(m) = mask.get_mut(t as usize) {
                *m = true;
            }
        }
        mask
    });
    let pairs = intersecting_pairs(mesh, &mut control.sub(0.0, 0.9))?;
    let mut hit = vec![false; count];
    for (i, j) in pairs {
        if selected
            .as_ref()
            .is_none_or(|s| s[i as usize] || s[j as usize])
        {
            hit[i as usize] = true;
            hit[j as usize] = true;
        }
    }
    Ok((0..count as u32).filter(|&t| hit[t as usize]).collect())
}

/// "1 self-intersecting triangle", "1,204 self-intersecting triangles".
fn triangles(n: usize) -> String {
    let unit = if n == 1 { "triangle" } else { "triangles" };
    format!("{} self-intersecting {unit}", fmt_count(n as u64))
}

/// "The 1 self-intersecting triangle is", "The 12 self-intersecting triangles are".
fn triangles_are(n: usize) -> String {
    format!("The {} {}", triangles(n), if n == 1 { "is" } else { "are" })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::exact::testing::torus;
    use crate::repair::status::analyze;
    use crate::repair::topology::{Edges, Link, components};
    use crate::repair::{RepairError, remap_selection, silent, tritri};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    /// Two 20 mm cubes in one part, the second moved 10 mm in x: they cut through each other, and
    /// their bottom, front, back and top faces overlap where they share a plane.
    fn two_cubes() -> Mesh {
        concat(&[&cube(), &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0])]).unwrap()
    }

    /// make_box's faces: 0 −z, 1 +z, 2 −y, 3 +y, 4 +x, 5 −x, two triangles each. The first
    /// cube's −x face (triangles 10 and 11) lies at x = 0, outside the second cube.
    const OUTSIDE: u32 = 10;

    fn pairs(mesh: &Mesh) -> Vec<(u32, u32)> {
        silent(|c| tritri::intersecting_pairs(mesh, c)).unwrap()
    }

    fn all_in_pairs(mesh: &Mesh) -> Vec<u32> {
        let mut t: Vec<u32> = pairs(mesh).iter().flat_map(|&(i, j)| [i, j]).collect();
        t.sort_unstable();
        t.dedup();
        t
    }

    #[test]
    fn a_clean_cube_has_nothing_to_find() {
        let m = cube();
        for out in [
            silent(|c| detect(&m, &[], c)).unwrap(),
            silent(|c| split_off(&m, &[], c)).unwrap(),
            silent(|c| remove(&m, &[], c)).unwrap(),
        ] {
            assert!(!out.changed);
            assert_eq!(out.mesh, m);
            assert_eq!(out.origin, Origin::Same);
            assert_eq!(out.message, "No self-intersections found.");
        }
        // Detect still reports its (empty) highlight, so an old one clears.
        let out = silent(|c| detect(&m, &[], c)).unwrap();
        assert_eq!(out.highlight, Some(Vec::new()));
    }

    #[test]
    fn detect_highlights_both_triangles_of_every_pair() {
        let m = two_cubes();
        let expected = all_in_pairs(&m);
        assert!(!expected.is_empty());
        let out = silent(|c| detect(&m, &[], c)).unwrap();
        assert!(!out.changed);
        assert_eq!(out.mesh, m);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.highlight.as_ref(), Some(&expected));
        assert_eq!(
            out.message,
            format!(
                "Found {} self-intersecting triangles.",
                fmt_count(expected.len() as u64)
            )
        );
        // The first cube's +x face and the second's −x face lie inside the other cube and cross
        // its faces; the first cube's −x face and the second's +x face touch nothing.
        for t in [8, 9, 12 + 10, 12 + 11] {
            assert!(expected.contains(&t), "triangle {t}");
        }
        for t in [OUTSIDE, OUTSIDE + 1, 12 + 8, 12 + 9] {
            assert!(!expected.contains(&t), "triangle {t}");
        }
    }

    #[test]
    fn with_a_selection_only_pairs_with_a_selected_triangle_count() {
        let m = two_cubes();
        let all = pairs(&m);
        // Select one triangle of the first cube's +x face.
        let selected = 8u32;
        let out = silent(|c| detect(&m, &[selected], c)).unwrap();
        let mut expected: Vec<u32> = all
            .iter()
            .filter(|&&(i, j)| i == selected || j == selected)
            .flat_map(|&(i, j)| [i, j])
            .collect();
        expected.sort_unstable();
        expected.dedup();
        assert!(expected.len() > 1);
        assert!(expected.len() < all_in_pairs(&m).len());
        assert_eq!(out.highlight, Some(expected.clone()));
        // A selected triangle in no pair finds nothing.
        assert!(!all.iter().any(|&(i, j)| i == OUTSIDE || j == OUTSIDE));
        let quiet = silent(|c| detect(&m, &[OUTSIDE], c)).unwrap();
        assert_eq!(quiet.highlight, Some(Vec::new()));
        assert_eq!(quiet.message, "No self-intersections found.");
        // Remove with the same selection deletes exactly those triangles.
        let removed = silent(|c| remove(&m, &[selected], c)).unwrap();
        assert_eq!(
            removed.mesh.triangle_count(),
            m.triangle_count() - expected.len()
        );
        let Origin::Map(kept) = &removed.origin else {
            panic!("triangles were removed")
        };
        assert!(kept.iter().all(|t| !expected.contains(t)));
        // Ids past the last triangle are ignored.
        let far = silent(|c| detect(&m, &[selected, 4000], c)).unwrap();
        assert_eq!(far.highlight, Some(expected));
    }

    #[test]
    fn split_off_moves_the_intersecting_triangles_to_shells_of_their_own() {
        let m = two_cubes();
        let found = all_in_pairs(&m);
        let out = silent(|c| split_off(&m, &[], c)).unwrap();
        assert!(out.changed);
        assert_eq!(
            out.message,
            format!(
                "Split off {} self-intersecting triangles.",
                fmt_count(found.len() as u64)
            )
        );
        assert_eq!(out.mesh.triangle_count(), m.triangle_count());
        let Origin::Map(origin) = &out.origin else {
            panic!("triangles were reordered")
        };
        // The others first in their order, then the split-off ones in theirs.
        let kept = m.triangle_count() - found.len();
        let others: Vec<u32> = (0..m.triangle_count() as u32)
            .filter(|t| !found.contains(t))
            .collect();
        assert_eq!(origin[..kept], others[..]);
        assert_eq!(origin[kept..], found[..]);
        // Every triangle keeps its corners' positions.
        for (n, &t) in origin.iter().enumerate() {
            for k in 0..3 {
                assert_eq!(
                    out.mesh.vertex(out.mesh.indices[n * 3 + k]),
                    m.vertex(m.indices[t as usize * 3 + k])
                );
            }
        }
        // The split-off triangles share no vertex with the others, but share theirs among
        // themselves: the copies of one cube's corners stay joined.
        let first: Vec<u32> = out.mesh.indices[..kept * 3].to_vec();
        assert!(
            out.mesh.indices[kept * 3..]
                .iter()
                .all(|v| !first.contains(v))
        );
        let edges = Edges::new(&out.mesh);
        let (shell, shells) = components(&out.mesh, &edges, Link::Vertex);
        let (rest, split) = (&shell[..kept], &shell[kept..]);
        assert!(split.iter().all(|s| !rest.contains(s)));
        assert!(shells < m.triangle_count(), "copies are shared");
        // No vertex is left that nothing uses.
        let mut used = vec![false; out.mesh.vertex_count()];
        for &v in &out.mesh.indices {
            used[v as usize] = true;
        }
        assert!(used.iter().all(|&u| u));
        // The surface itself is the same: same area, same volume.
        let (a, b) = (stats(&m), stats(&out.mesh));
        assert!((a.area - b.area).abs() < 1e-9 && (a.volume - b.volume).abs() < 1e-9);
        // The selection follows the triangles.
        let position = others.iter().position(|&t| t == OUTSIDE).unwrap() as u32;
        let after = remap_selection(&[found[0], OUTSIDE], &out.origin, out.mesh.triangle_count());
        assert_eq!(after, vec![position, kept as u32]);
    }

    #[test]
    fn splitting_off_twice_changes_nothing_the_second_time() {
        let once = silent(|c| split_off(&two_cubes(), &[], c)).unwrap();
        let twice = silent(|c| split_off(&once.mesh, &[], c)).unwrap();
        assert!(!twice.changed, "{}", twice.message);
        assert_eq!(twice.mesh, once.mesh);
        assert_eq!(twice.origin, Origin::Same);
        let found = all_in_pairs(&once.mesh).len();
        assert_eq!(
            twice.message,
            format!(
                "The {} self-intersecting triangles are already split off.",
                fmt_count(found as u64)
            )
        );
    }

    #[test]
    fn remove_deletes_them_and_leaves_holes() {
        let m = two_cubes();
        let found = all_in_pairs(&m);
        let out = silent(|c| remove(&m, &[], c)).unwrap();
        assert_eq!(
            out.message,
            format!(
                "Removed {} self-intersecting triangles.",
                fmt_count(found.len() as u64)
            )
        );
        assert_eq!(out.mesh.triangle_count(), m.triangle_count() - found.len());
        let Origin::Map(origin) = &out.origin else {
            panic!("triangles were removed")
        };
        assert!(origin.iter().all(|t| !found.contains(t)));
        assert!(pairs(&out.mesh).is_empty());
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert!(s.border_edges > 0 && s.holes > 0, "{s:?}");
        // Running it again finds nothing.
        let again = silent(|c| remove(&out.mesh, &[], c)).unwrap();
        assert!(!again.changed);
        assert_eq!(again.message, "No self-intersections found.");
    }

    #[test]
    fn touching_does_not_count_but_overlapping_faces_do() {
        // A second cube touching the first only along an edge (x = 20, y = 20), and a third
        // touching it only at the corner (20, 20, 20).
        let edge = make_box([20.0, 20.0, 0.0], [40.0, 40.0, 20.0]);
        let corner = make_box([20.0; 3], [40.0; 3]);
        for other in [&edge, &corner] {
            let m = concat(&[&cube(), other]).unwrap();
            let out = silent(|c| detect(&m, &[], c)).unwrap();
            assert_eq!(out.highlight, Some(Vec::new()));
        }
        // Stacked face to face at x = 20, the two square faces lie on each other with positive
        // area: that counts, and only those four triangles.
        let stacked = concat(&[&cube(), &make_box([20.0, 0.0, 0.0], [40.0, 20.0, 20.0])]).unwrap();
        let out = silent(|c| detect(&stacked, &[], c)).unwrap();
        assert_eq!(out.highlight, Some(vec![8, 9, 12 + 10, 12 + 11]));
    }

    #[test]
    fn a_shell_folded_through_itself_is_found() {
        // One shell: a cube whose top corner vertex 6 is pulled down through the bottom face, so
        // its three triangles there pierce the bottom.
        let mut m = cube();
        m.positions[6 * 3 + 2] = -5.0;
        m.positions[6 * 3] = 15.0;
        m.positions[6 * 3 + 1] = 15.0;
        let s = silent(|c| analyze(&m, c)).unwrap().status;
        assert_eq!(s.shells, 1);
        let out = silent(|c| detect(&m, &[], c)).unwrap();
        let found = out.highlight.unwrap();
        assert!(!found.is_empty());
        let removed = silent(|c| remove(&m, &[], c)).unwrap();
        assert!(pairs(&removed.mesh).is_empty());
    }

    #[test]
    fn a_cancelled_run_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        let m = two_cubes();
        assert_eq!(detect(&m, &[], &mut control), Err(RepairError::Cancelled));
        assert_eq!(
            split_off(&m, &[], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(remove(&m, &[], &mut control), Err(RepairError::Cancelled));
    }

    /// `cargo test --release --lib repair::self_intersect -- --ignored --nocapture`: 730k
    /// triangles in 27 overlapping tori, like the developer's merged CAD parts, and a clean torus.
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
        type Act = fn(&Mesh, &[u32], &mut Control) -> RepairResult<Outcome>;
        let actions: [(&str, Act); 3] = [
            ("detect", detect),
            ("split off", split_off),
            ("remove", remove),
        ];
        for (name, action) in actions {
            let started = Instant::now();
            let out = silent(|c| action(&m, &[], c)).unwrap();
            println!(
                "{name}: {} triangles in {:.3} s: {}",
                m.triangle_count(),
                started.elapsed().as_secs_f64(),
                out.message
            );
        }
        let clean = torus(30.0, 8.0, 520, 700);
        let started = Instant::now();
        let out = silent(|c| detect(&clean, &[], c)).unwrap();
        println!(
            "detect on a clean torus: {} triangles in {:.3} s: {}",
            clean.triangle_count(),
            started.elapsed().as_secs_f64(),
            out.message
        );
    }

    /// Real files, with timings: `TRICKY3D_MODELS="a.stl;b.stl" cargo test --release --lib
    /// repair::self_intersect::tests::real_models -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_models() {
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        type Act = fn(&Mesh, &[u32], &mut Control) -> RepairResult<Outcome>;
        let actions: [(&str, Act); 3] = [
            ("detect", detect),
            ("split off", split_off),
            ("remove", remove),
        ];
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            for part in &imported.parts {
                let m = &part.mesh;
                for (name, action) in actions {
                    let started = Instant::now();
                    let out = silent(|c| action(m, &[], c)).unwrap();
                    println!(
                        "{path}: {name}: {} triangles in {:.3} s: {}",
                        m.triangle_count(),
                        started.elapsed().as_secs_f64(),
                        out.message
                    );
                    if name == "remove" {
                        assert!(pairs(&out.mesh).is_empty());
                    }
                }
            }
        }
    }
}
