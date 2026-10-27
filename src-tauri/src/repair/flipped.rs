//! Fix flipped triangles: make the orientation consistent within each shell, then turn each closed
//! shell to face outward. Turns the triangles the Status tab counts as inverted (orient::inverted),
//! plus inward-facing shells inside other shells, which the status takes as cavities; with a
//! selection, only selected ones.

use super::topology::{Edges, orientation_patches};
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// Turns the inverted triangles; with a selection, only the selected ones among them. Turning some
/// of a patch's inverted triangles leaves its reference winding as it was (for a closed patch the
/// winding with positive volume, for an open one the majority's, which only grows), so the others
/// stay inverted and a second run with the same selection turns nothing.
pub fn run(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let mut wrong = inverted(mesh, &mut control.sub(0.0, 0.9))?;
    if !selection.is_empty() {
        let mut chosen = vec![false; wrong.len()];
        for &t in selection {
            if let Some(slot) = chosen.get_mut(t as usize) {
                *slot = true;
            }
        }
        for (w, c) in wrong.iter_mut().zip(chosen) {
            *w &= c;
        }
    }
    if !wrong.contains(&true) {
        let message = if selection.is_empty() {
            "No flipped triangles to fix."
        } else {
            "None of the selected triangles is flipped."
        };
        return Ok(Outcome::unchanged(mesh, message));
    }
    let (out, turned) = turn(mesh, &wrong);
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Same,
        message: format!(
            "Flipped {} {}.",
            fmt_count(turned as u64),
            if turned == 1 { "triangle" } else { "triangles" }
        ),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// The mesh with every inverted triangle turned (orient::inverted) and how many turned.
///
/// Turning swaps a triangle's second and third corners, so its first corner stays where it was.
/// Run again, orient::inverted then walks the same patches from the same first triangles and sums
/// each closed patch's volume from the same point, every term exactly negated where a triangle
/// turned: each patch already has its reference winding, and nothing more turns.
pub fn orient(mesh: &Mesh, control: &mut Control) -> RepairResult<(Mesh, usize)> {
    let wrong = inverted(mesh, &mut control.sub(0.0, 0.9))?;
    let out = turn(mesh, &wrong);
    control.progress(1.0);
    Ok(out)
}

/// orient::inverted, with the edges and orientation patches it needs.
fn inverted(mesh: &Mesh, control: &mut Control) -> RepairResult<Vec<bool>> {
    control.check()?;
    let edges = Edges::new(mesh);
    control.progress(0.4);
    control.check()?;
    let patches = orientation_patches(mesh, &edges);
    control.progress(0.6);
    super::orient::inverted(
        mesh,
        &edges,
        &patches,
        super::orient::Nested::Outward,
        &mut control.sub(0.6, 1.0),
    )
}

/// The mesh with the marked triangles' second and third corners swapped, and how many there were.
fn turn(mesh: &Mesh, wrong: &[bool]) -> (Mesh, usize) {
    let mut out = mesh.clone();
    let mut turned = 0;
    for (t, _) in out
        .indices
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(wrong)
        .filter(|(_, w)| **w)
    {
        t.swap(1, 2);
        turned += 1;
    }
    (out, turned)
}

/// Helpers the timing tests of the repair actions share.
#[cfg(test)]
pub(super) mod testing {
    use crate::mesh::Mesh;

    /// The parts of the files in TRICKY3D_MODELS, with their names.
    pub fn real_models() -> Vec<(String, Mesh)> {
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap_or_else(|e| panic!("{path}: {e}"));
            let name = std::path::Path::new(path)
                .file_name()
                .map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
            for part in imported.parts {
                out.push((name.clone(), part.mesh));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::status::{RepairStatus, analyze};
    use crate::repair::topology::keep_triangles;
    use crate::repair::{RepairError, silent};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn flip(mesh: &mut Mesh, t: usize) {
        mesh.indices.swap(t * 3 + 1, t * 3 + 2);
    }

    fn inside_out(mut mesh: Mesh) -> Mesh {
        for t in 0..mesh.triangle_count() {
            flip(&mut mesh, t);
        }
        mesh
    }

    fn fix(mesh: &Mesh, selection: &[u32]) -> Outcome {
        silent(|c| run(mesh, selection, c)).unwrap()
    }

    fn status(mesh: &Mesh) -> RepairStatus {
        silent(|c| analyze(mesh, c)).unwrap().status
    }

    /// The Status tab's inverted triangles, sorted.
    fn inverted_list(mesh: &Mesh) -> Vec<u32> {
        silent(|c| analyze(mesh, c)).unwrap().inverted
    }

    /// The triangles the action turns (the status's, plus inward shells inside other shells).
    fn turned_list(mesh: &Mesh) -> Vec<u32> {
        let wrong = silent(|c| inverted(mesh, c)).unwrap();
        (0..wrong.len() as u32)
            .filter(|&t| wrong[t as usize])
            .collect()
    }

    /// What every run must give: exactly `turned_list` turned (each one's first corner kept),
    /// nothing else touched, nothing the status counts left, and a second run that changes nothing.
    fn check_fix(mesh: &Mesh) -> Mesh {
        let wrong = turned_list(mesh);
        let out = fix(mesh, &[]);
        assert_eq!(out.changed, !wrong.is_empty());
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh.positions, mesh.positions);
        for t in 0..mesh.triangle_count() {
            let a = &mesh.indices[t * 3..t * 3 + 3];
            let b = &out.mesh.indices[t * 3..t * 3 + 3];
            if wrong.binary_search(&(t as u32)).is_ok() {
                assert_eq!(b, [a[0], a[2], a[1]], "triangle {t}");
            } else {
                assert_eq!(a, b, "triangle {t}");
            }
        }
        assert!(inverted_list(&out.mesh).is_empty());
        let again = fix(&out.mesh, &[]);
        assert!(
            !again.changed,
            "the second run turned more: {}",
            again.message
        );
        assert_eq!(again.mesh, out.mesh);
        // orient() is the same turn.
        let (oriented, turned) = silent(|c| orient(mesh, c)).unwrap();
        assert_eq!((oriented, turned), (out.mesh.clone(), wrong.len()));
        out.mesh
    }

    #[test]
    fn a_clean_cube_is_unchanged() {
        let m = cube();
        let out = fix(&m, &[]);
        assert!(!out.changed);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh, m);
        assert_eq!(out.message, "No flipped triangles to fix.");
        let (same, turned) = silent(|c| orient(&m, c)).unwrap();
        assert_eq!((same, turned), (m, 0));
        // Nothing at all is fine too.
        assert!(!fix(&Mesh::default(), &[]).changed);
    }

    #[test]
    fn three_flipped_triangles_turn_back() {
        let mut m = cube();
        for t in [2, 5, 8] {
            flip(&mut m, t);
        }
        let out = fix(&m, &[]);
        assert_eq!(out.message, "Flipped 3 triangles.");
        assert_eq!(out.mesh, cube());
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented);
        assert_eq!(s.volume, 8000.0);
        check_fix(&m);
    }

    #[test]
    fn an_inside_out_cube_turns_outward() {
        let m = inside_out(cube());
        assert_eq!(stats(&m).volume, -8000.0);
        let out = fix(&m, &[]);
        assert_eq!(out.message, "Flipped 12 triangles.");
        assert_eq!(stats(&out.mesh).volume, 8000.0);
        assert!(status(&out.mesh).oriented);
        check_fix(&m);
        // One triangle is "Flipped 1 triangle.".
        let mut one = cube();
        flip(&mut one, 7);
        assert_eq!(fix(&one, &[]).message, "Flipped 1 triangle.");
    }

    #[test]
    fn every_closed_shell_ends_up_facing_outward() {
        let outer = cube();
        let inner = make_box([5.0; 3], [10.0; 3]);
        // A hollow part's inward inner shell turns out, like every closed shell (orient.rs),
        // although the status does not count it.
        let hollow = concat(&[&outer, &inside_out(inner.clone())]).unwrap();
        assert!(inverted_list(&hollow).is_empty());
        assert_eq!(turned_list(&hollow), (12..24).collect::<Vec<u32>>());
        let fixed = check_fix(&hollow);
        assert_eq!(stats(&fixed).volume, 8000.0 + 125.0);
        // Embedded and separate shells, some inside out, some with stray triangles.
        let mut many = concat(&[
            &inside_out(outer.clone()),
            &inner,
            &inside_out(make_box([30.0; 3], [33.0; 3])),
            &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0]),
        ])
        .unwrap();
        for t in [1, 13, 14, 30, 40, 41, 47] {
            flip(&mut many, t);
        }
        let fixed = check_fix(&many);
        assert_eq!(stats(&fixed).volume, 8000.0 + 125.0 + 27.0 + 8000.0);
    }

    #[test]
    fn open_shells_follow_their_majority() {
        let mut open = cube();
        open.indices.truncate(30);
        for t in [3, 8] {
            flip(&mut open, t);
        }
        let out = fix(&open, &[]);
        assert_eq!(out.message, "Flipped 2 triangles.");
        let mut want = cube();
        want.indices.truncate(30);
        assert_eq!(out.mesh, want);
        check_fix(&open);
    }

    /// A Möbius strip of `n` quads, with a tab glued to its border against its winding as
    /// triangle 0 (as in orient.rs's tests).
    fn mobius_with_tab(n: u32) -> Mesh {
        let mut positions = Vec::new();
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = ((a / 2.0).cos(), (a / 2.0).sin());
            for w in [1.0f32, -1.0] {
                let r = 4.0 + w * c;
                positions.extend_from_slice(&[r * a.cos(), r * a.sin(), w * s]);
            }
        }
        positions.extend_from_slice(&[4.0, 0.0, -3.0]);
        let mut indices = vec![1, 3, 2 * n];
        for i in 0..n {
            let (t, b) = (2 * i, 2 * i + 1);
            let (nt, nb) = if i + 1 < n {
                (2 * i + 2, 2 * i + 3)
            } else {
                (1, 0)
            };
            indices.extend_from_slice(&[t, b, nb, t, nb, nt]);
        }
        Mesh { positions, indices }
    }

    #[test]
    fn a_mobius_strip_settles_after_one_run() {
        for n in [5u32, 6, 7, 12] {
            let mut m = mobius_with_tab(n);
            for t in (1..m.triangle_count()).step_by(3) {
                flip(&mut m, t);
            }
            let fixed = check_fix(&m);
            assert!(
                !status(&fixed).oriented,
                "a Möbius strip never agrees everywhere"
            );
        }
    }

    #[test]
    fn with_a_selection_only_selected_inverted_triangles_turn() {
        let mut m = cube();
        for t in [2, 7, 11] {
            flip(&mut m, t);
        }
        // 3 is selected but faces the right way; 2 is inverted but not selected.
        let out = fix(&m, &[3, 7, 11, 500]);
        assert_eq!(out.message, "Flipped 2 triangles.");
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(inverted_list(&out.mesh), vec![2]);
        let mut want = cube();
        flip(&mut want, 2);
        assert_eq!(out.mesh, want);
        // The same selection again finds nothing; the rest still gets fixed without one.
        let again = fix(&out.mesh, &[3, 7, 11, 500]);
        assert!(!again.changed);
        assert_eq!(again.message, "None of the selected triangles is flipped.");
        assert_eq!(fix(&out.mesh, &[]).mesh, cube());
        // Even when most of a closed shell is selected and inverted, the volume decides.
        let mut most = cube();
        for t in 0..8 {
            flip(&mut most, t);
        }
        let out = fix(&most, &[0, 1, 2, 3]);
        assert_eq!(inverted_list(&out.mesh), vec![4, 5, 6, 7]);
    }

    #[test]
    fn random_shells_are_fixed_in_one_run() {
        // Boxes and tori, some open, some inside out, some touching or crossing, with random
        // triangles turned: one run leaves nothing inverted and the second changes nothing.
        let mut rng = Rng(5);
        for round in 0..60 {
            let mut parts = Vec::new();
            for _ in 0..1 + rng.below(4) {
                let o = [0, 1, 2].map(|_| rng.int(-10, 10) as f32);
                let mut part = if rng.chance(0.5) {
                    make_box(o, [0, 1, 2].map(|k| o[k] + rng.int(1, 8) as f32))
                } else {
                    let mut t = torus(6.0, 2.0, 6 + rng.below(6) as usize, 5);
                    crate::ops::translate(&mut t, o.map(f64::from));
                    t
                };
                if rng.chance(0.3) {
                    part = inside_out(part);
                }
                if rng.chance(0.3) {
                    let keep: Vec<bool> = (0..part.triangle_count())
                        .map(|_| !rng.chance(0.15))
                        .collect();
                    part = keep_triangles(&part, &keep).0;
                }
                parts.push(part);
            }
            let refs: Vec<&Mesh> = parts.iter().collect();
            let mut m = concat(&refs).unwrap();
            for t in 0..m.triangle_count() {
                if rng.chance(0.2) {
                    flip(&mut m, t);
                }
            }
            let fixed = check_fix(&m);
            // Every closed shell ends up with positive volume.
            let s = status(&fixed);
            assert_eq!(s.inverted_triangles, 0, "round {round}");
        }
    }

    #[test]
    fn progress_rises_and_cancel_stops() {
        let mut m = cube();
        flip(&mut m, 4);
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            run(&m, &[], &mut control).unwrap();
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
        assert_eq!(seen.last(), Some(&1.0));
        let stop = AtomicBool::new(true);
        let mut control = Control::new(&stop, |_| {});
        assert_eq!(run(&m, &[], &mut control), Err(RepairError::Cancelled));
        assert_eq!(orient(&m, &mut control), Err(RepairError::Cancelled));
    }

    /// Timings on a 1,002,528-triangle torus with every 7th triangle turned plus 200 inside-out
    /// boxes, then on the real models listed in TRICKY3D_MODELS (paths separated by `;`):
    /// `cargo test --release --lib repair::flipped -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let mut m = torus(40.0, 15.0, 708, 708);
        for t in (0..m.triangle_count()).step_by(7) {
            flip(&mut m, t);
        }
        let boxes: Vec<Mesh> = (0..200)
            .map(|i| {
                let x = i as f32 * 2.0;
                inside_out(make_box([x, 0.0, 60.0], [x + 1.0, 1.0, 61.0]))
            })
            .collect();
        let mut all: Vec<&Mesh> = vec![&m];
        all.extend(boxes.iter());
        let m = concat(&all).unwrap();
        time_one("torus + 200 inside-out boxes", &m);
        for (name, mesh) in super::testing::real_models() {
            time_one(&name, &mesh);
        }
    }

    fn time_one(name: &str, m: &Mesh) {
        let started = Instant::now();
        let out = fix(m, &[]);
        let first = started.elapsed().as_secs_f64();
        let started = Instant::now();
        let again = fix(&out.mesh, &[]);
        let second = started.elapsed().as_secs_f64();
        println!(
            "{name}: {} triangles; fix flipped {:.0} ms ({}), again {:.0} ms ({})",
            m.triangle_count(),
            first * 1000.0,
            out.message,
            second * 1000.0,
            again.message
        );
        assert!(!again.changed);
        assert!(inverted_list(&out.mesh).is_empty());
    }
}
