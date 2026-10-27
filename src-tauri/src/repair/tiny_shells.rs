//! Remove tiny shells: delete shells (triangles joined through shared vertices) below a size
//! threshold, by the chosen detection mode. Ignores the selection.
//!
//! Also measures shells for Invert negative shells.

use super::topology::{Edges, Link, NONE, components, keep_triangles};
use super::{Control, Origin, Outcome, RepairResult, TinyShellMode, TinyShellParams};
use crate::mesh::{Mesh, cross, dot, fmt_count, sub};

/// Triangles between cancel checks.
const CHECK_EVERY: usize = 1 << 16;

/// The warning when the largest shell is tiny too.
const LARGEST_KEPT: &str = "The largest shell is below the minimum size too and was kept. If the \
                            part was modelled in metres, scale it by 1000.";

/// Deletes the shells the mode calls tiny: closed shells whose volume is below the minimum volume
/// and open shells whose area is below the minimum area (the default), every shell below the
/// minimum volume, or every shell below the minimum area. "Below" is strict, so a threshold of 0
/// deletes nothing. The largest shell (by area) always stays, so a part modelled in metres is not
/// deleted whole. The remaining triangles keep their order.
pub fn run(
    mesh: &Mesh,
    selection: &[u32],
    params: &TinyShellParams,
    control: &mut Control,
) -> RepairResult<Outcome> {
    let _ = selection;
    control.check()?;
    let (shells, shell_of) = measure_shells(mesh, &mut control.sub(0.0, 0.8))?;
    let mut tiny: Vec<bool> = shells.iter().map(|s| is_tiny(s, params)).collect();
    let mut warnings = Vec::new();
    let largest = (0..shells.len())
        .rev()
        .max_by(|&a, &b| shells[a].area.total_cmp(&shells[b].area));
    if let Some(l) = largest.filter(|&l| tiny[l]) {
        tiny[l] = false;
        warnings.push(LARGEST_KEPT.to_string());
    }
    let count = tiny.iter().filter(|&&t| t).count();
    if count == 0 {
        let mut out = Outcome::unchanged(mesh, "No tiny shells to remove.");
        out.warnings = warnings;
        return Ok(out);
    }
    control.check()?;
    let keep: Vec<bool> = shell_of.iter().map(|&s| !tiny[s as usize]).collect();
    let removed = mesh.triangle_count() - keep.iter().filter(|&&k| k).count();
    let (out, kept) = keep_triangles(mesh, &keep);
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(kept),
        message: format!(
            "Removed {} ({}).",
            plural(count, "tiny shell", "tiny shells"),
            plural(removed, "triangle", "triangles")
        ),
        warnings,
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

fn is_tiny(shell: &Shell, params: &TinyShellParams) -> bool {
    let small_volume = shell.volume.abs() < params.min_volume;
    let small_area = shell.area < params.min_area;
    match params.mode {
        TinyShellMode::ClosedVolumeOpenArea => {
            if shell.closed {
                small_volume
            } else {
                small_area
            }
        }
        TinyShellMode::Volume => small_volume,
        TinyShellMode::Area => small_area,
    }
}

/// One shell's size.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Shell {
    pub triangles: usize,
    /// Every edge of the shell has exactly two sides.
    pub closed: bool,
    /// mm³, signed: positive when the shell faces outward. Exactly 0 for a flat shell (a
    /// double-sided sheet, say), whose sum is rounding noise far below the size of its terms.
    pub volume: f64,
    /// mm².
    pub area: f64,
}

/// The shells (components over shared vertices, numbered in order of first triangle) with their
/// sizes, and the shell of each triangle.
///
/// A shell's volume is summed from one of its own vertices with a compensated sum, so a tiny
/// shell far from the origin keeps its precision: a closed shell's first corner, an open shell's
/// lowest border vertex. Seen from a point on a hole's rim, a flat hole adds no volume, so an
/// open shell whose holes are flat measures as if they were closed (a cube missing a face
/// measures as the whole cube); that gives "volume" a meaning for open shells too.
pub(super) fn measure_shells(
    mesh: &Mesh,
    control: &mut Control,
) -> RepairResult<(Vec<Shell>, Vec<u32>)> {
    control.check()?;
    let edges = Edges::new(mesh);
    control.progress(0.5);
    control.check()?;
    let (shell_of, count) = components(mesh, &edges, Link::Vertex);
    control.progress(0.7);
    control.check()?;
    // Every side of an edge belongs to the same shell (they share its two vertices). Edges come
    // in order of lower vertex, so the first border edge met gives a shell's lowest border vertex.
    let mut closed = vec![true; count];
    let mut origin = vec![NONE; count];
    for e in 0..edges.len() as u32 {
        let sides = edges.sides(e);
        if sides.len() != 2 {
            let s = shell_of[(sides[0] / 3) as usize] as usize;
            closed[s] = false;
            if sides.len() == 1 && origin[s] == NONE {
                origin[s] = edges.ends(e)[0];
            }
        }
    }
    drop(edges);
    control.progress(0.8);
    // Six times the volume as (sum, correction), the sum of the terms' sizes, twice the area.
    let mut volume = vec![(0.0f64, 0.0f64); count];
    let mut magnitude = vec![0.0f64; count];
    let mut area = vec![0.0f64; count];
    let mut triangles = vec![0usize; count];
    for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        if t % CHECK_EVERY == 0 {
            control.check()?;
        }
        let s = shell_of[t] as usize;
        if origin[s] == NONE {
            origin[s] = c[0];
        }
        let o = mesh.point(origin[s]);
        let [a, b, d] = c.map(|v| sub(mesh.point(v), o));
        let v = dot(a, cross(b, d));
        add(&mut volume[s], v);
        magnitude[s] += v.abs();
        let n = cross(sub(b, a), sub(d, a));
        area[s] += dot(n, n).sqrt();
        triangles[s] += 1;
    }
    let shells = (0..count)
        .map(|s| {
            let v = volume[s].0 + volume[s].1;
            Shell {
                triangles: triangles[s],
                closed: closed[s],
                volume: if v.abs() <= 1e-12 * magnitude[s] {
                    0.0
                } else {
                    v / 6.0
                },
                area: area[s] / 2.0,
            }
        })
        .collect();
    control.progress(1.0);
    Ok((shells, shell_of))
}

/// Neumaier's compensated sum: adds `x` to (sum, correction).
fn add(acc: &mut (f64, f64), x: f64) {
    let sum = acc.0 + x;
    acc.1 += if acc.0.abs() >= x.abs() {
        (acc.0 - sum) + x
    } else {
        (x - sum) + acc.0
    };
    acc.0 = sum;
}

/// "1 tiny shell", "1,204 triangles".
pub(super) fn plural(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        fmt_count(n as u64),
        if n == 1 { one } else { many }
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::defaults::TINY_SHELLS;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::{RepairError, remap_selection, silent};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn inside_out(mut mesh: Mesh) -> Mesh {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        mesh
    }

    /// A box of 0.01 mm³ beside the 20 mm cube, as in the cube-speck fixture.
    fn speck() -> Mesh {
        let side = 0.01f64.cbrt() as f32;
        make_box([25.0, 0.0, 0.0], [25.0 + side, side, side])
    }

    fn params(mode: TinyShellMode, min_volume: f64, min_area: f64) -> TinyShellParams {
        TinyShellParams {
            mode,
            min_volume,
            min_area,
        }
    }

    fn remove(mesh: &Mesh, p: TinyShellParams) -> Outcome {
        let out = silent(|c| run(mesh, &[], &p, c)).unwrap();
        // Running it again on its own output changes nothing.
        let again = silent(|c| run(&out.mesh, &[], &p, c)).unwrap();
        assert!(!again.changed, "second run: {}", again.message);
        assert_eq!(again.mesh, out.mesh);
        out
    }

    fn measure(mesh: &Mesh) -> (Vec<Shell>, Vec<u32>) {
        silent(|c| measure_shells(mesh, c)).unwrap()
    }

    /// A single triangle of the given area in the plane z = `z`.
    fn triangle(x: f32, area: f32, z: f32) -> Mesh {
        let side = (2.0 * area).sqrt();
        Mesh {
            positions: vec![x, 0.0, z, x + side, 0.0, z, x, side, z],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn the_default_keeps_the_speck_and_a_larger_minimum_removes_it() {
        let m = concat(&[&cube(), &speck()]).unwrap();
        let kept = remove(&m, TINY_SHELLS);
        assert!(!kept.changed);
        assert_eq!(kept.message, "No tiny shells to remove.");
        assert_eq!(kept.mesh, m);
        let out = remove(
            &m,
            TinyShellParams {
                min_volume: 0.1,
                ..TINY_SHELLS
            },
        );
        assert!(out.changed);
        assert_eq!(out.message, "Removed 1 tiny shell (12 triangles).");
        assert_eq!(out.mesh, cube());
        assert_eq!(out.origin, Origin::Map((0..12).collect()));
        assert_eq!(stats(&out.mesh).volume, 8000.0);
    }

    #[test]
    fn the_largest_shell_stays_even_when_it_is_tiny() {
        // A part modelled in metres: a 0.02 m cube (0.000008 mm³) and a smaller speck.
        let part = make_box([0.0; 3], [0.02; 3]);
        let alone = remove(&part, TINY_SHELLS);
        assert!(!alone.changed);
        assert_eq!(alone.mesh, part);
        assert_eq!(
            alone.warnings,
            [
                "The largest shell is below the minimum size too and was kept. If the part was \
              modelled in metres, scale it by 1000."
            ]
        );
        let with_speck = concat(&[&part, &make_box([0.05; 3], [0.051; 3])]).unwrap();
        let out = remove(&with_speck, TINY_SHELLS);
        assert_eq!(out.message, "Removed 1 tiny shell (12 triangles).");
        assert_eq!(out.mesh, part);
        assert_eq!(out.warnings.len(), 1);
        // A normal part with a speck says nothing more.
        let normal = remove(
            &concat(&[&cube(), &speck()]).unwrap(),
            TinyShellParams {
                min_volume: 0.1,
                ..TINY_SHELLS
            },
        );
        assert!(normal.warnings.is_empty());
    }

    #[test]
    fn shells_are_measured_from_their_own_vertices() {
        let (shells, shell_of) = measure(&concat(&[&cube(), &speck()]).unwrap());
        assert_eq!(shell_of, [vec![0; 12], vec![1; 12]].concat());
        assert_eq!(
            shells[0],
            Shell {
                triangles: 12,
                closed: true,
                volume: 8000.0,
                area: 2400.0,
            }
        );
        assert!(shells[1].closed);
        assert!((shells[1].volume - 0.01).abs() < 1e-7, "{:?}", shells[1]);
        // Inside out: negative.
        assert_eq!(measure(&inside_out(cube())).0[0].volume, -8000.0);
        // A 1 mm cube 10 km out and a one-f32-step cube 12 m out keep their volumes.
        let far = measure(&make_box([1e7; 3], [1e7 + 1.0; 3])).0;
        assert_eq!(far[0].volume, 1.0);
        let tiny = make_box([12345.0; 3], [12345.001; 3]);
        let v = measure(&tiny).0[0].volume;
        assert!((v - stats(&tiny).volume).abs() < 1e-9 * stats(&tiny).volume.abs());
        assert!(v > 0.0);
        // Nothing has no shells.
        assert_eq!(measure(&Mesh::default()), (vec![], vec![]));
    }

    #[test]
    fn an_open_shell_measures_from_its_rim() {
        // Missing its top face (flat): the whole cube's volume, whichever face is missing.
        for face in 0..6 {
            let mut open = cube();
            open.indices.drain(face * 6..face * 6 + 6);
            let s = &measure(&open).0[0];
            assert!(!s.closed);
            assert_eq!(s.volume, 8000.0, "face {face}");
            assert_eq!(s.area, 2000.0);
        }
        // A flat open sheet has no volume.
        let s = &measure(&triangle(0.0, 5.0, 3.0)).0[0];
        assert_eq!((s.closed, s.volume), (false, 0.0));
        assert!((s.area - 5.0).abs() < 1e-5);
        // A double-sided sheet is closed but flat.
        let sheet = Mesh {
            positions: vec![1.0, 2.0, 3.0, 4.0, 2.5, 3.0, 1.5, 7.0, 3.25],
            indices: vec![0, 1, 2, 0, 2, 1],
        };
        let s = &measure(&sheet).0[0];
        assert_eq!((s.closed, s.volume), (true, 0.0));
    }

    #[test]
    fn non_manifold_shells_are_not_closed() {
        // A fin on a cube edge: one shell, three sides on edge 0-1.
        let mut m = cube();
        m.positions.extend_from_slice(&[10.0, -10.0, -10.0]);
        m.indices.extend_from_slice(&[0, 1, 8]);
        let (shells, _) = measure(&m);
        assert_eq!(shells.len(), 1);
        assert!(!shells[0].closed);
    }

    #[test]
    fn the_default_mode_judges_closed_shells_by_volume_and_open_ones_by_area() {
        // A thin closed slab (large area, 0.00005 mm³), an open speck triangle (0.00005 mm²), an
        // open flat sheet (no volume, 4 mm²), and the cube.
        let slab = make_box([30.0, 0.0, 0.0], [30.5, 0.1, 0.001]);
        let parts = [
            cube(),
            slab.clone(),
            triangle(40.0, 0.00005, 0.0),
            triangle(50.0, 4.0, 0.0),
        ];
        let refs: Vec<&Mesh> = parts.iter().collect();
        let m = concat(&refs).unwrap();
        let out = remove(&m, TINY_SHELLS);
        assert_eq!(out.message, "Removed 2 tiny shells (13 triangles).");
        let keep: Vec<u32> = (0..12).chain([25]).collect();
        assert_eq!(out.origin, Origin::Map(keep));
        // By volume alone the flat sheet goes too, by area alone only the speck triangle.
        let by_volume = remove(&m, params(TinyShellMode::Volume, 0.0001, 0.0001));
        assert_eq!(by_volume.message, "Removed 3 tiny shells (14 triangles).");
        assert_eq!(by_volume.mesh, cube());
        let by_area = remove(&m, params(TinyShellMode::Area, 0.0001, 0.0001));
        assert_eq!(by_area.message, "Removed 1 tiny shell (1 triangle).");
        assert_eq!(by_area.mesh.triangle_count(), 25);
        // A large open shell with plenty of volume stays even by volume.
        let mut open = cube();
        open.indices.truncate(30);
        let both = concat(&[&open, &speck()]).unwrap();
        let out = remove(&both, params(TinyShellMode::Volume, 0.1, 0.0));
        assert_eq!(out.mesh, open);
    }

    #[test]
    fn thresholds_are_strict() {
        let m = concat(&[&cube(), &make_box([30.0; 3], [32.0; 3])]).unwrap();
        // Exactly 8 mm³ and 24 mm²: not below.
        assert!(!remove(&m, params(TinyShellMode::Volume, 8.0, 0.0)).changed);
        assert!(!remove(&m, params(TinyShellMode::Area, 0.0, 24.0)).changed);
        assert!(remove(&m, params(TinyShellMode::Volume, 8.000001, 0.0)).changed);
        assert!(remove(&m, params(TinyShellMode::Area, 0.0, 24.000001)).changed);
        // Zero thresholds remove nothing, not even a flat shell.
        let flat = concat(&[&cube(), &triangle(40.0, 1.0, 0.0)]).unwrap();
        for mode in [
            TinyShellMode::ClosedVolumeOpenArea,
            TinyShellMode::Volume,
            TinyShellMode::Area,
        ] {
            assert!(!remove(&flat, params(mode, 0.0, 0.0)).changed);
        }
    }

    #[test]
    fn inside_out_shells_count_by_size() {
        let m = concat(&[&cube(), &inside_out(make_box([30.0; 3], [31.0; 3]))]).unwrap();
        let out = remove(&m, params(TinyShellMode::ClosedVolumeOpenArea, 2.0, 0.0));
        assert_eq!(out.mesh, cube());
    }

    #[test]
    fn everything_tiny_leaves_the_largest_and_the_selection_follows() {
        let m = concat(&[&speck(), &cube(), &speck()]).unwrap();
        let all = remove(&m, params(TinyShellMode::Area, 0.0, 1e6));
        assert_eq!(all.mesh, cube());
        assert_eq!(all.message, "Removed 2 tiny shells (24 triangles).");
        let out = remove(
            &m,
            TinyShellParams {
                min_volume: 0.1,
                ..TINY_SHELLS
            },
        );
        // Selected: a speck triangle and two cube triangles; the cube's are now 0..12.
        assert_eq!(
            remap_selection(&[3, 12, 20], &out.origin, out.mesh.triangle_count()),
            vec![0, 8]
        );
    }

    #[test]
    fn shells_joined_at_a_vertex_count_as_one() {
        // A speck sharing a corner with the cube is part of the cube's shell.
        let mut glued = concat(&[&cube(), &make_box([20.0; 3], [20.1; 3])]).unwrap();
        for i in &mut glued.indices {
            if *i == 8 {
                *i = 6;
            }
        }
        let out = remove(
            &glued,
            TinyShellParams {
                min_volume: 0.1,
                ..TINY_SHELLS
            },
        );
        assert!(!out.changed);
    }

    #[test]
    fn random_shell_soups_measure_like_the_shells_tab() {
        // The Shells tab measures closed shells the same way (from the first corner); the volumes
        // and areas must agree to rounding.
        let mut rng = Rng(9);
        for _ in 0..30 {
            let mut parts = Vec::new();
            for _ in 0..1 + rng.below(5) {
                let o = [0, 1, 2].map(|_| rng.int(-50, 50) as f32 * 0.37);
                let mut part = if rng.chance(0.5) {
                    make_box(o, [0, 1, 2].map(|k| o[k] + rng.int(1, 9) as f32 * 0.11))
                } else {
                    let mut t = torus(3.0, 1.0, 5 + rng.below(5) as usize, 4);
                    crate::ops::translate(&mut t, o.map(f64::from));
                    t
                };
                if rng.chance(0.3) {
                    part = inside_out(part);
                }
                parts.push(part);
            }
            let refs: Vec<&Mesh> = parts.iter().collect();
            let m = concat(&refs).unwrap();
            let (ours, of) = measure(&m);
            let (rows, row_of) = crate::repair::shells::shell_table(&m);
            assert_eq!(of, row_of);
            assert_eq!(ours.len(), rows.len());
            for (s, r) in ours.iter().zip(&rows) {
                assert_eq!(s.closed, r.watertight);
                assert_eq!(s.triangles as u64, r.triangles);
                assert!((s.area - r.area).abs() <= 1e-9 * r.area);
                let v = r.volume.unwrap();
                assert!((s.volume.abs() - v).abs() <= 1e-9 * v, "{s:?} {r:?}");
            }
        }
    }

    #[test]
    fn progress_rises_and_cancel_stops() {
        let m = concat(&[&cube(), &speck()]).unwrap();
        let p = TinyShellParams {
            min_volume: 0.1,
            ..TINY_SHELLS
        };
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            run(&m, &[], &p, &mut control).unwrap();
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
        assert_eq!(seen.last(), Some(&1.0));
        let stop = AtomicBool::new(true);
        let mut control = Control::new(&stop, |_| {});
        assert_eq!(run(&m, &[], &p, &mut control), Err(RepairError::Cancelled));
    }

    /// Timings on a 1,002,528-triangle torus with 20,000 specks, then on the real models listed in
    /// TRICKY3D_MODELS (paths separated by `;`):
    /// `cargo test --release --lib repair::tiny_shells -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let big = torus(40.0, 15.0, 708, 708);
        let specks: Vec<Mesh> = (0..20_000)
            .map(|i| {
                let x = (i % 200) as f32;
                let y = (i / 200) as f32;
                make_box([x, y, 80.0], [x + 0.01, y + 0.01, 80.01])
            })
            .collect();
        let mut all: Vec<&Mesh> = vec![&big];
        all.extend(specks.iter());
        let m = concat(&all).unwrap();
        time_one("torus + 20,000 specks", &m);
        for (name, mesh) in crate::repair::flipped::testing::real_models() {
            time_one(&name, &mesh);
        }
    }

    fn time_one(name: &str, m: &Mesh) {
        let started = Instant::now();
        let out = silent(|c| run(m, &[], &TINY_SHELLS, c)).unwrap();
        let took = started.elapsed().as_secs_f64();
        let started = Instant::now();
        let again = silent(|c| run(&out.mesh, &[], &TINY_SHELLS, c)).unwrap();
        let second = started.elapsed().as_secs_f64();
        println!(
            "{name}: {} triangles; remove tiny shells {:.0} ms ({}), again {:.0} ms ({})",
            m.triangle_count(),
            took * 1000.0,
            out.message,
            second * 1000.0,
            again.message
        );
        assert!(!again.changed);
    }
}
