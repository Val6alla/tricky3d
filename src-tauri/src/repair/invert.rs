//! Invert negative shells: turn every closed shell whose signed volume is negative. Ignores the
//! selection.

use super::tiny_shells::{measure_shells, plural};
use super::{Control, Origin, Outcome, RepairResult};
use crate::mesh::Mesh;

/// Turns every triangle of each closed shell (triangles joined through shared vertices whose edges
/// all have exactly two sides) with negative volume. Open shells have no volume to go by, and a
/// flat closed shell (a double-sided sheet) none worth trusting, so they stay as they are.
///
/// Turning swaps a triangle's second and third corners. A shell's volume is summed from its first
/// corner, which stays put, so a second run sums the same terms exactly negated: positive, and
/// nothing more turns.
pub fn run(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    let _ = selection;
    control.check()?;
    let (shells, shell_of) = measure_shells(mesh, &mut control.sub(0.0, 0.9))?;
    let negative: Vec<bool> = shells.iter().map(|s| s.closed && s.volume < 0.0).collect();
    let count = negative.iter().filter(|&&n| n).count();
    if count == 0 {
        return Ok(Outcome::unchanged(mesh, "No negative shells to invert."));
    }
    let mut out = mesh.clone();
    let mut turned = 0;
    for (t, &s) in out.indices.as_chunks_mut::<3>().0.iter_mut().zip(&shell_of) {
        if negative[s as usize] {
            t.swap(1, 2);
            turned += 1;
        }
    }
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Same,
        message: format!(
            "Inverted {} ({}).",
            plural(count, "shell", "shells"),
            plural(turned, "triangle", "triangles")
        ),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::{Control, RepairError, silent};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn inside_out(mut mesh: Mesh) -> Mesh {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        mesh
    }

    /// Runs the action, checks it turned whole shells only (each triangle as it was or with its
    /// second and third corners swapped) and that a second run changes nothing.
    fn invert(mesh: &Mesh) -> Outcome {
        let out = silent(|c| run(mesh, &[], c)).unwrap();
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh.positions, mesh.positions);
        for (a, b) in mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .zip(out.mesh.indices.as_chunks::<3>().0)
        {
            assert!(a == b || *b == [a[0], a[2], a[1]]);
        }
        let again = silent(|c| run(&out.mesh, &[], c)).unwrap();
        assert!(!again.changed, "second run: {}", again.message);
        out
    }

    #[test]
    fn an_outward_cube_is_unchanged() {
        let out = invert(&cube());
        assert!(!out.changed);
        assert_eq!(out.message, "No negative shells to invert.");
        assert_eq!(out.mesh, cube());
        assert!(!invert(&Mesh::default()).changed);
    }

    #[test]
    fn an_inside_out_cube_turns_outward() {
        let out = invert(&inside_out(cube()));
        assert!(out.changed);
        assert_eq!(out.message, "Inverted 1 shell (12 triangles).");
        assert_eq!(out.mesh, cube());
        assert_eq!(stats(&out.mesh).volume, 8000.0);
    }

    #[test]
    fn only_closed_negative_shells_turn() {
        let mut open = inside_out(make_box([40.0; 3], [50.0; 3]));
        open.indices.truncate(30);
        let sheet = Mesh {
            positions: vec![60.0, 0.0, 0.0, 61.0, 0.0, 0.0, 60.0, 1.0, 0.0],
            indices: vec![0, 1, 2, 0, 2, 1],
        };
        let hollow_inner = inside_out(make_box([5.0; 3], [10.0; 3]));
        let m = concat(&[
            &cube(),
            &hollow_inner,
            &open,
            &sheet,
            &inside_out(make_box([30.0; 3], [33.0; 3])),
        ])
        .unwrap();
        let out = invert(&m);
        assert_eq!(out.message, "Inverted 2 shells (24 triangles).");
        // The cavity (12..24) and the far box (24 + 10 + 2 = 36..48) turned; the open shell and
        // the flat sheet did not.
        for t in 0..m.triangle_count() {
            let turned = out.mesh.indices[t * 3..t * 3 + 3] != m.indices[t * 3..t * 3 + 3];
            let want = (12..24).contains(&t) || (36..48).contains(&t);
            assert_eq!(turned, want, "triangle {t}");
        }
    }

    #[test]
    fn shells_joined_at_a_vertex_turn_together() {
        // Two inside-out cubes sharing a corner vertex are one closed shell.
        let mut glued = concat(&[
            &inside_out(cube()),
            &inside_out(make_box([20.0; 3], [40.0; 3])),
        ])
        .unwrap();
        for i in &mut glued.indices {
            if *i == 8 {
                *i = 6;
            }
        }
        let out = invert(&glued);
        assert_eq!(out.message, "Inverted 1 shell (24 triangles).");
        assert_eq!(stats(&out.mesh).volume, 16000.0);
    }

    #[test]
    fn a_shell_with_a_non_manifold_edge_is_not_closed() {
        let mut m = inside_out(cube());
        m.positions.extend_from_slice(&[10.0, -10.0, -10.0]);
        m.indices.extend_from_slice(&[0, 1, 8]);
        assert!(!invert(&m).changed);
    }

    #[test]
    fn tiny_shells_far_away_keep_their_sign() {
        let far = inside_out(make_box([1e7; 3], [1e7 + 1.0; 3]));
        let tiny = inside_out(make_box([12345.0; 3], [12345.001; 3]));
        let m = concat(&[&far, &tiny]).unwrap();
        let out = invert(&m);
        assert_eq!(out.message, "Inverted 2 shells (24 triangles).");
    }

    #[test]
    fn random_shells_end_up_with_positive_volume() {
        let mut rng = Rng(21);
        for _ in 0..40 {
            let mut parts = Vec::new();
            for _ in 0..1 + rng.below(6) {
                let o = [0, 1, 2].map(|_| rng.int(-40, 40) as f32 * 0.5);
                let mut part = if rng.chance(0.5) {
                    make_box(o, [0, 1, 2].map(|k| o[k] + rng.int(1, 9) as f32))
                } else {
                    let mut t = torus(5.0, 2.0, 5 + rng.below(6) as usize, 5);
                    crate::ops::translate(&mut t, o.map(f64::from));
                    t
                };
                if rng.chance(0.5) {
                    part = inside_out(part);
                }
                parts.push(part);
            }
            let refs: Vec<&Mesh> = parts.iter().collect();
            let m = concat(&refs).unwrap();
            let out = invert(&m);
            let (rows, _) = crate::repair::shells::shell_table(&out.mesh);
            let total: f64 = rows.iter().map(|r| r.volume.unwrap()).sum();
            // Every shell faces outward now, so the signed total is the sum of their sizes.
            assert!((stats(&out.mesh).volume - total).abs() <= 1e-9 * total);
        }
    }

    #[test]
    fn progress_rises_and_cancel_stops() {
        let m = inside_out(cube());
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
    }

    /// Timings on a 1,002,528-triangle inside-out torus with 5,000 inside-out boxes, then on the
    /// real models listed in TRICKY3D_MODELS (paths separated by `;`):
    /// `cargo test --release --lib repair::invert -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let big = inside_out(torus(40.0, 15.0, 708, 708));
        let boxes: Vec<Mesh> = (0..5_000)
            .map(|i| {
                let (x, y) = ((i % 100) as f32, (i / 100) as f32);
                inside_out(make_box([x, y, 80.0], [x + 0.5, y + 0.5, 80.5]))
            })
            .collect();
        let mut all: Vec<&Mesh> = vec![&big];
        all.extend(boxes.iter());
        let m = concat(&all).unwrap();
        time_one("inside-out torus + 5,000 boxes", &m);
        for (name, mesh) in crate::repair::flipped::testing::real_models() {
            time_one(&name, &mesh);
        }
    }

    fn time_one(name: &str, m: &Mesh) {
        let started = Instant::now();
        let out = silent(|c| run(m, &[], c)).unwrap();
        let took = started.elapsed().as_secs_f64();
        let started = Instant::now();
        let again = silent(|c| run(&out.mesh, &[], c)).unwrap();
        let second = started.elapsed().as_secs_f64();
        println!(
            "{name}: {} triangles; invert negative shells {:.0} ms ({}), again {:.0} ms ({})",
            m.triangle_count(),
            took * 1000.0,
            out.message,
            second * 1000.0,
            again.message
        );
        assert!(!again.changed);
    }
}
