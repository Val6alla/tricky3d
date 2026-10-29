//! Known answers for Generate shell. Volumes are compared with the exact solid's; the tolerance of
//! each test says where its error comes from and how it scales with the accuracy (grid step) h.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

use super::*;
use crate::mesh::{Welder, make_box, stats};
use crate::repair::topology::{Edges, Link, components};
use crate::repair::{silent, status, tritri};

fn settings(kind: ShellKind, thickness: f64, accuracy: f64) -> ShellSettings {
    ShellSettings {
        kind,
        thickness,
        accuracy,
        smooth: false,
    }
}

fn run(mesh: &Mesh, s: &ShellSettings) -> Shell {
    silent(|c| generate(mesh, s, c)).unwrap_or_else(|e| panic!("{s:?}: {e:?}"))
}

fn made(mesh: &Mesh, s: &ShellSettings) -> Mesh {
    run(mesh, s).mesh.expect("a result")
}

fn cube() -> Mesh {
    make_box([0.0; 3], [20.0; 3])
}

/// The Part library's sphere, finer than its default so the facets stay well below the tolerance.
fn sphere(r: f64) -> Mesh {
    let params = json!({"radius": r, "segments": 256, "rings": 128});
    crate::library::generate("sphere", params.as_object().unwrap()).unwrap()
}

/// A prism along y from 0 to `depth` over a polygon in (x, z) given with its triangles.
fn prism(outline: &[[f32; 2]], caps: &[[usize; 3]], depth: f32) -> Mesh {
    let mut w = Welder::new(64);
    let at = |p: [f32; 2], y: f32| [p[0], y, p[1]];
    for c in caps {
        let [a, b, d] = c.map(|i| outline[i]);
        w.triangle(at(a, 0.0), at(b, 0.0), at(d, 0.0));
        w.triangle(at(a, depth), at(d, depth), at(b, depth));
    }
    for i in 0..outline.len() {
        let (p, q) = (outline[i], outline[(i + 1) % outline.len()]);
        w.triangle(at(p, 0.0), at(q, depth), at(q, 0.0));
        w.triangle(at(p, 0.0), at(p, depth), at(q, depth));
    }
    let mut m = w.finish().unwrap();
    if stats(&m).volume < 0.0 {
        m = turned(m);
    }
    m
}

/// A 20 mm block with a fin 2 mm thick and 10 mm tall on top, 20 mm deep.
fn finned_block() -> Mesh {
    let outline = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 20.0],
        [11.0, 20.0],
        [11.0, 30.0],
        [9.0, 30.0],
        [9.0, 20.0],
        [0.0, 20.0],
    ];
    let caps = [
        [0, 1, 2],
        [0, 2, 3],
        [0, 3, 6],
        [0, 6, 7],
        [6, 3, 4],
        [6, 4, 5],
    ];
    prism(&outline, &caps, 20.0)
}

/// Closed, oriented and free of crossings, with `shells` shells.
fn assert_sound(mesh: &Mesh, shells: u64) {
    let s = silent(|c| status::analyze(mesh, c)).unwrap().status;
    assert!(s.closed && s.oriented, "{s:?}");
    assert_eq!(s.shells, shells, "{s:?}");
    let crossing = silent(|c| tritri::intersecting_pairs(mesh, c)).unwrap();
    assert!(crossing.is_empty(), "{} crossing pairs", crossing.len());
}

/// Signed volume of each shell, largest first by size.
fn shell_volumes(mesh: &Mesh) -> Vec<f64> {
    let edges = Edges::new(mesh);
    let (of, count) = components(mesh, &edges, Link::Shell);
    let mut volumes = vec![0.0; count];
    for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let [a, b, c] = tri.map(|v| exact::vertex(mesh, v));
        volumes[of[t] as usize] += exact::dot(a, exact::cross(b, c)) / 6.0;
    }
    volumes.sort_by(|a, b| b.abs().total_cmp(&a.abs()));
    volumes
}

/// The volume error of `mesh` against `exact`, checked against `c` h² mm³ (on the side `sign`
/// says: +1 only more, −1 only less, 0 either; the other side gets 0.5 mm³ for rounding).
fn assert_error(mesh: &Mesh, exact: f64, h: f64, c: f64, sign: i32) -> f64 {
    let error = stats(mesh).volume - exact;
    let bound = c * h * h;
    let (lo, hi) = match sign {
        1 => (-0.5, bound),
        -1 => (-bound, 0.5),
        _ => (-bound, bound),
    };
    assert!(
        (lo..=hi).contains(&error),
        "volume {} against {exact}: off by {error}, allowed {lo}..{hi} at h {h}",
        exact + error
    );
    error
}

// The volume tests run at these grid steps (the dialog's Accuracy). Each error falls about as h²:
// halving the step quarters it. The tolerances are c h² mm³, with c half as large again as the
// error measured at h = 0.5 (brief 12 report), and the measured errors in each test's comment.
const STEPS: [f64; 2] = [0.5, 0.25];

/// Cube 20 mm, Hollow part 2 mm: 8000 − 16³ = 3904 mm³. Marching tetrahedra cuts the cavity's 12
/// edges off by up to a step, so the cavity comes out smaller: the result only ever weighs more.
/// Measured +5.8, +1.5, +0.4 mm³ at h = 0.5, 0.25, 0.125: c = 36.
#[test]
fn cube_hollow_part() {
    for h in STEPS {
        let out = made(&cube(), &settings(ShellKind::HollowPart, 2.0, h));
        assert_sound(&out, 2);
        let volumes = shell_volumes(&out);
        assert!((volumes[0] - 8000.0).abs() < 1e-6, "{volumes:?}");
        // The inner shell faces into the cavity: negative volume.
        assert!(volumes[1] < 0.0, "{volumes:?}");
        assert_error(&out, 3904.0, h, 36.0, 1);
    }
}

/// Cube 20 mm, Inner offset 2 mm: a 16 mm cube, 4096 mm³, its edges cut off by up to a step.
/// Measured −5.8, −1.5, −0.4 mm³: c = 36, only less.
#[test]
fn cube_inner_offset() {
    for h in STEPS {
        let out = made(&cube(), &settings(ShellKind::InnerOffset, 2.0, h));
        assert_sound(&out, 1);
        assert_error(&out, 4096.0, h, 36.0, -1);
    }
}

/// Cube 20 mm, Outer offset 2 mm: the cube, slabs on its faces, quarter cylinders on its edges and
/// eighth spheres on its corners: 8000 + 6·400·2 + 12·20·π·4/4 + 4/3·π·8 = 13 587.5 mm³. The
/// rounded parts are flat triangles across a curve, so a little is missing. Measured −10.1, −2.9,
/// −0.7 mm³: c = 60, only less.
#[test]
fn cube_outer_offset() {
    let expected = 8000.0 + 4800.0 + 12.0 * 20.0 * PI + 4.0 / 3.0 * PI * 8.0;
    assert!((expected - 13587.5).abs() < 0.1);
    for h in STEPS {
        let out = made(&cube(), &settings(ShellKind::OuterOffset, 2.0, h));
        assert_sound(&out, 1);
        assert_error(&out, expected, h, 60.0, -1);
        // Rounded, not sharp: every vertex lies the thickness from the cube, give or take the
        // clearance that keeps a vertex off its grid edge's ends (5 % of a diagonal step).
        let reach = out
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| {
                exact::length(
                    p.map(|x| (x as f64 - 10.0).abs() - 10.0)
                        .map(|x| x.max(0.0)),
                )
            })
            .fold(0.0f64, f64::max);
        assert!(reach < 2.0 + 0.05 * 3f64.sqrt() * h, "{reach}");
    }
}

/// Sphere R 10 mm, Inner offset 2 mm: a sphere of R 8, 2144.66 mm³. Flat triangles across a curve
/// leave a little out; so do the input's own facets (1.05 mm³ for this 256-segment sphere).
/// Measured −10.3, −3.9, −1.5 mm³: c = 80, only less.
#[test]
fn sphere_inner_offset() {
    let expected = 4.0 / 3.0 * PI * 512.0;
    for h in STEPS {
        let out = made(&sphere(10.0), &settings(ShellKind::InnerOffset, 2.0, h));
        assert_sound(&out, 1);
        assert_error(&out, expected, h, 80.0, -1);
    }
}

/// Sphere R 10 mm, Hollow part 2 mm: 4188.79 − 2144.66 = 2044.13 mm³, the inner offset's error
/// with its sign turned, and the input's facets (the original part keeps them). Measured +9.2,
/// +2.9, +0.5 mm³: c = 80.
#[test]
fn sphere_hollow_part() {
    let expected = 4.0 / 3.0 * PI * (1000.0 - 512.0);
    let input = sphere(10.0);
    let facets = 4.0 / 3.0 * PI * 1000.0 - stats(&input).volume;
    assert!(facets > 0.0 && facets < 1.5, "{facets}");
    for h in STEPS {
        let out = made(&input, &settings(ShellKind::HollowPart, 2.0, h));
        assert_sound(&out, 2);
        assert_error(&out, expected, h, 80.0, 0);
    }
}

/// Hollow part, improved rebuilds the cube's cavity with its sharp edges: within 0.5 mm³ of 3904
/// at both steps (measured +0.17, +0.01, +0.00), where the plain method is off by 5.8 at h = 0.5.
#[test]
fn improved_hollow_part_keeps_sharp_edges() {
    for h in STEPS {
        let out = made(&cube(), &settings(ShellKind::HollowPartImproved, 2.0, h));
        assert_sound(&out, 2);
        let error = assert_error(&out, 3904.0, h, 2.0, 0);
        assert!(error.abs() < 0.5, "{error}");
    }
    let plain = made(&cube(), &settings(ShellKind::HollowPart, 2.0, 0.5));
    assert!(stats(&plain).volume - 3904.0 > 3.0);
}

/// A plate 3 mm thick has no inside deeper than 2 mm: no result, and a warning.
#[test]
fn a_thin_plate_vanishes_with_a_warning() {
    let plate = make_box([0.0; 3], [40.0, 30.0, 3.0]);
    for kind in [
        ShellKind::InnerOffset,
        ShellKind::HollowingShell,
        ShellKind::HollowPart,
        ShellKind::HollowPartImproved,
    ] {
        let shell = run(&plate, &settings(kind, 2.0, 0.5));
        assert!(shell.mesh.is_none(), "{kind:?}");
        assert_eq!(shell.warnings.len(), 1, "{kind:?}");
        assert!(
            shell.warnings[0].contains("thinner than 4 mm"),
            "{:?}",
            shell.warnings
        );
    }
}

/// The fin is thinner than twice the thickness: the inner offset keeps the block and warns about
/// the top of the part.
#[test]
fn a_thin_fin_is_left_out_with_where_it_was() {
    let part = finned_block();
    assert_sound(&part, 1);
    for kind in [ShellKind::InnerOffset, ShellKind::HollowPart] {
        let shell = run(&part, &settings(kind, 2.0, 0.5));
        let out = shell.mesh.expect("the block's offset");
        assert_sound(&out, if kind == ShellKind::HollowPart { 2 } else { 1 });
        assert_eq!(shell.warnings.len(), 1, "{:?}", shell.warnings);
        let w = &shell.warnings[0];
        assert!(
            w.contains("thinner than 4 mm") && w.contains("at the top"),
            "{w}"
        );
        // The box named covers the fin, not the block.
        assert!(w.contains("Z 20 to 30"), "{w}");
    }
    // A cube has nothing to warn about.
    assert!(
        run(&cube(), &settings(ShellKind::InnerOffset, 2.0, 0.5))
            .warnings
            .is_empty()
    );
}

/// Every kind gives a closed, oriented result without crossings; the hollowing shell is the inner
/// offset turned inside out.
#[test]
fn every_kind_is_sound() {
    let part = finned_block();
    for kind in [
        ShellKind::HollowPart,
        ShellKind::InnerOffset,
        ShellKind::OuterOffset,
        ShellKind::HollowingShell,
        ShellKind::HollowPartImproved,
    ] {
        for smooth in [false, true] {
            let s = ShellSettings {
                smooth,
                ..settings(kind, 1.5, 0.5)
            };
            let out = made(&part, &s);
            let shells = if kind == ShellKind::HollowPart || kind == ShellKind::HollowPartImproved {
                2
            } else {
                1
            };
            // Status calls a lone inside-out shell inverted: the hollowing shell is checked the
            // right way out.
            match kind {
                ShellKind::HollowingShell => assert_sound(&turned(out.clone()), shells),
                _ => assert_sound(&out, shells),
            }
            let v = stats(&out).volume;
            match kind {
                ShellKind::HollowingShell => assert!(v < 0.0, "{kind:?} {v}"),
                _ => assert!(v > 0.0, "{kind:?} {v}"),
            }
        }
    }
    let inner = made(&cube(), &settings(ShellKind::InnerOffset, 2.0, 0.5));
    let hollowing = made(&cube(), &settings(ShellKind::HollowingShell, 2.0, 0.5));
    assert!((stats(&inner).volume + stats(&hollowing).volume).abs() < 1e-6);
}

/// A cavity thinner than a grid step: the plain method leaves what the grid happens to catch, the
/// improved one leaves none of it, so the wall there is never thinner than asked.
#[test]
fn improved_hollow_part_drops_slivers_of_cavity() {
    // Walls 4.1 mm thick: the cavity would be 0.1 mm thick, a fifth of a step.
    let plate = make_box([0.0; 3], [30.0, 20.0, 4.1]);
    let s = settings(ShellKind::HollowPartImproved, 2.0, 0.5);
    let shell = run(&plate, &s);
    if let Some(out) = &shell.mesh {
        let volumes = shell_volumes(out);
        assert!(volumes[1..].iter().all(|v| v.abs() > 1.0), "{volumes:?}");
    }
}

#[test]
fn settings_are_checked() {
    let ok = settings(ShellKind::HollowPart, 2.0, 0.5);
    assert_eq!(settings_problem(&ok), None);
    for (t, h) in [
        (0.0, 0.1),
        (-1.0, 0.1),
        (f64::NAN, 0.1),
        (2000.0, 1.0),
        (2.0, 0.001),
        (2.0, 1.5),
    ] {
        let s = settings(ShellKind::HollowPart, t, h);
        assert!(settings_problem(&s).is_some(), "{t} {h}");
        assert!(silent(|c| generate(&cube(), &s, c)).is_err());
    }
    assert_eq!(
        settings_problem(&settings(ShellKind::HollowPart, 2.0, 1.5)).unwrap(),
        "Enter an accuracy of at most half the thickness (1 mm)."
    );
}

/// A cancel at any point stops the run with Cancelled.
#[test]
fn a_cancel_stops_it_anywhere() {
    for kind in [
        ShellKind::HollowPart,
        ShellKind::OuterOffset,
        ShellKind::HollowPartImproved,
    ] {
        for at in [0.0, 0.1, 0.3, 0.6, 0.8, 0.9, 0.99] {
            let cancel = AtomicBool::new(false);
            let mut control = Control::new(&cancel, |f| {
                if f >= at {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
            let result = generate(&sphere(10.0), &settings(kind, 2.0, 0.5), &mut control);
            assert_eq!(result, Err(RepairError::Cancelled), "{kind:?} at {at}");
        }
    }
}

#[test]
fn sides_and_messages() {
    let part = [[0.0; 3], [20.0, 20.0, 30.0]];
    assert_eq!(side([10.0, 10.0, 25.0], &part), "at the top of the part");
    assert_eq!(side([10.0, 10.0, 2.0], &part), "at the bottom of the part");
    assert_eq!(
        side([1.0, 10.0, 15.0], &part),
        "toward the −X side of the part"
    );
    assert_eq!(
        side([10.0, 18.0, 15.0], &part),
        "toward the +Y side of the part"
    );
    assert_eq!(side([11.0, 9.0, 16.0], &part), "in the middle of the part");
    // Thin walls over most of the part get no box; a few get theirs, to a tenth of a millimetre.
    let all = ThinWalls {
        share: 0.41,
        bounds: [[0.5, 0.0, 1.0], [19.5, 20.0, 29.0]],
        centre: [10.0, 10.0, 12.0],
    };
    assert_eq!(
        thin_message(ShellKind::HollowPart, 2.0, &all, &part),
        "Walls thinner than 4 mm (twice the shell thickness) stay solid, with no cavity: about 41 % \
         of the surface, all over the part."
    );
    let fin = ThinWalls {
        share: 0.0049,
        bounds: [[9.04, 0.0, 20.0], [10.96, 20.0, 30.0]],
        centre: [10.0, 10.0, 25.0],
    };
    assert_eq!(
        thin_message(ShellKind::InnerOffset, 1.5, &fin, &part),
        "Walls thinner than 3 mm (twice the shell thickness) are left out of the inner offset: about \
         1 % of the surface, at the top of the part (X 9 to 11, Y 0 to 20, Z 20 to 30 mm)."
    );
    assert_eq!(round_mm(4.0), "4");
    assert_eq!(round_mm(0.125), "0.125");
    assert_eq!(round_mm(2.50), "2.5");
}

#[test]
fn estimates_grow_with_finer_steps_and_refuse_far_too_fine_ones() {
    let part = EstimatePart {
        size: [20.0; 3],
        area: 2400.0,
        triangles: 12.0,
    };
    let coarse = estimate(&[part], &settings(ShellKind::HollowPart, 2.0, 0.5));
    let fine = estimate(&[part], &settings(ShellKind::HollowPart, 2.0, 0.1));
    // Five times finer: about 5³ times the points, less the fixed padding.
    assert!(fine.points > coarse.points * 50.0);
    assert!(fine.seconds > coarse.seconds && fine.megabytes > coarse.megabytes);
    assert!(!coarse.slow && !coarse.heavy && coarse.refused.is_none());
    let huge = EstimatePart {
        size: [2000.0; 3],
        area: 2.4e7,
        triangles: 1e6,
    };
    let far = estimate(&[huge], &settings(ShellKind::HollowPart, 2.0, 0.01));
    assert!(far.slow && far.heavy && far.refused.is_some());
}

/// The known-answer table at three accuracies, for the report:
/// `cargo test --release --lib ops::shell::tests::volume_table -- --ignored --nocapture`.
#[test]
#[ignore]
fn volume_table() {
    let cases: [(&str, Mesh, ShellKind, f64); 6] = [
        (
            "cube 20, hollow part",
            cube(),
            ShellKind::HollowPart,
            3904.0,
        ),
        (
            "cube 20, inner offset",
            cube(),
            ShellKind::InnerOffset,
            4096.0,
        ),
        (
            "cube 20, outer offset",
            cube(),
            ShellKind::OuterOffset,
            8000.0 + 4800.0 + 12.0 * 20.0 * PI + 4.0 / 3.0 * PI * 8.0,
        ),
        (
            "sphere 10, inner offset",
            sphere(10.0),
            ShellKind::InnerOffset,
            4.0 / 3.0 * PI * 512.0,
        ),
        (
            "sphere 10, hollow part",
            sphere(10.0),
            ShellKind::HollowPart,
            4.0 / 3.0 * PI * 488.0,
        ),
        (
            "cube 20, hollow improved",
            cube(),
            ShellKind::HollowPartImproved,
            3904.0,
        ),
    ];
    for (name, mesh, kind, expected) in cases {
        for h in [0.5, 0.25, 0.125] {
            for smooth in [false, true] {
                let s = ShellSettings {
                    smooth,
                    ..settings(kind, 2.0, h)
                };
                let start = std::time::Instant::now();
                let out = made(&mesh, &s);
                let v = stats(&out).volume;
                println!(
                    "{name:26} h {h:5} smooth {smooth:5}: {v:10.2} mm³ (exact {expected:9.2}, off {:+7.2}), {:7} triangles, {:.2} s",
                    v - expected,
                    out.triangle_count(),
                    start.elapsed().as_secs_f64()
                );
            }
        }
    }
}

/// Two cubes 1 mm apart, offset 0.4 mm outward: the offsets face each other across a 0.2 mm gap.
/// Their vertices there are left alone by the tidying; those far from the gap are not.
#[test]
fn vertices_across_a_narrow_gap_are_left_alone() {
    let both = crate::ops::concat(&[
        &make_box([0.0; 3], [5.0; 3]),
        &make_box([6.0, 0.0, 0.0], [11.0, 5.0, 5.0]),
    ])
    .unwrap();
    let surface = Surface::new(&both);
    let bounds = [[-1.0, -1.0, -1.0], [12.0, 6.0, 6.0]];
    let made = silent(|c| offset_surface(&surface, &bounds, Offset::Outer(0.4), 0.1, false, c))
        .unwrap()
        .unwrap();
    let locked = super::tidy::near_other_sheets(&made, 0.6, &AtomicBool::new(false));
    let at = |v: usize| exact::vertex(&made, v as u32);
    let in_gap = (0..locked.len()).filter(|&v| {
        let p = at(v);
        (5.3..=5.7).contains(&p[0]) && (0.5..=4.5).contains(&p[1]) && (0.5..=4.5).contains(&p[2])
    });
    assert!(in_gap.clone().count() > 100);
    assert!(
        in_gap.clone().all(|v| locked[v]),
        "a vertex facing the gap is free"
    );
    let far = (0..locked.len()).filter(|&v| at(v)[0] < 4.0 || at(v)[0] > 7.0);
    assert!(
        far.clone().all(|v| !locked[v]),
        "a vertex far from the gap is locked"
    );
}
