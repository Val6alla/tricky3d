//! The Home tab's local edits and selections (Home › Repair) on a part the size of a real model:
//! 718,301 triangles with 900 holes and half of it selected. Each runs as the window runs it (one
//! repair run of one action), must leave a sound mesh, and must stop when cancelled part way.
//! `cargo test --release --test home_edits -- --nocapture` prints how long each took.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use common::torus;
use tricky3d_lib::mesh::{Mesh, stats};
use tricky3d_lib::repair::nodes::NodeMove;
use tricky3d_lib::repair::status;
use tricky3d_lib::repair::surface_cut::{self, CutLine, CutPoint};
use tricky3d_lib::repair::topology::{Edges, Link, components, keep_triangles};
use tricky3d_lib::repair::{Action, RepairError, ScriptOutcome, defaults, run_script};

/// Quads round the torus each way.
const N: usize = 600;

/// The torus with the first triangle of every 20th quad each way taken out (899 holes of three
/// edges; the 900th falls inside the block) and a block of 20 × 20 quads taken out of its top
/// (one curved hole of 80 edges), no two holes touching.
fn holed() -> Mesh {
    let whole = torus(N, N, 10.0, 3.0);
    let keep: Vec<bool> = (0..whole.triangle_count())
        .map(|t| {
            let (quad, first) = (t / 2, t % 2 == 0);
            let (i, j) = (quad / N, quad % N);
            let single = first && i % 20 == 0 && j % 20 == 0;
            let block = (305..325).contains(&i) && (145..165).contains(&j);
            !(single || block)
        })
        .collect();
    keep_triangles(&whole, &keep).0
}

/// The triangles of the quads in the first half round the torus: those whose corners all lie on
/// its first N / 2 + 1 rings (keep_triangles keeps every vertex here, so the numbers hold).
fn half(mesh: &Mesh) -> Vec<u32> {
    let rings = ((N / 2 + 1) * N) as u32;
    (0..mesh.triangle_count() as u32)
        .filter(|&t| {
            mesh.indices[t as usize * 3..t as usize * 3 + 3]
                .iter()
                .all(|&v| v < rings)
        })
        .collect()
}

fn run(action: Action, mesh: &Mesh, selection: &[u32]) -> (ScriptOutcome, f64) {
    let label = defaults::label_of(action.id()).to_string();
    let started = Instant::now();
    let out = run_script(
        &[(action, label)],
        mesh,
        selection,
        &AtomicBool::new(false),
        |_, _| {},
        |_, _| {},
        |_, _| {},
    )
    .unwrap();
    (out, started.elapsed().as_secs_f64())
}

/// Runs `action` and cancels it as soon as it first reports progress.
fn cancelled(action: Action, mesh: &Mesh, selection: &[u32]) -> Result<ScriptOutcome, RepairError> {
    let cancel = AtomicBool::new(false);
    run_script(
        &[(action, String::new())],
        mesh,
        selection,
        &cancel,
        |_, _| {},
        |_, _| cancel.store(true, Ordering::Relaxed),
        |_, _| {},
    )
}

fn assert_sound(name: &str, out: &ScriptOutcome) {
    out.mesh
        .check()
        .unwrap_or_else(|e| panic!("{name}: damaged mesh: {e}"));
    let s = stats(&out.mesh);
    assert_eq!(s.non_manifold_edges, 0, "{name}");
}

#[test]
fn every_home_edit_on_a_big_part_with_a_big_selection() {
    let mesh = holed();
    let selection = half(&mesh);
    let before = stats(&mesh);
    assert_eq!(mesh.triangle_count(), 2 * N * N - 899 - 800);
    assert!(mesh.triangle_count() > 700_000);
    assert!(selection.len() > 350_000);
    assert_eq!(before.open_edges, 899 * 3 + 80);
    println!(
        "{} triangles, {} selected, {} open edges",
        mesh.triangle_count(),
        selection.len(),
        before.open_edges
    );

    let report = |name: &str, out: &ScriptOutcome, took: f64| {
        println!(
            "{name:<28} {took:>7.3} s  {} -> {} triangles: {}",
            mesh.triangle_count(),
            out.mesh.triangle_count(),
            out.reports[0].message
        );
    };

    // Splits: the area stays, the selection is left empty.
    for (action, name) in [
        (Action::SubdivideTriangles, "Subdivide triangles"),
        (Action::AddPoint, "Add point"),
    ] {
        let (out, took) = run(action, &mesh, &selection);
        report(name, &out, took);
        assert!(
            out.changed && out.selected && out.selection.is_empty(),
            "{name}"
        );
        assert!(out.mesh.triangle_count() > mesh.triangle_count() + selection.len() / 2);
        assert_sound(name, &out);
        let area = stats(&out.mesh).area;
        assert!(
            (area - before.area).abs() < 1e-6 * before.area,
            "{name}: {area}"
        );
    }
    // Every open edge of a selected triangle is split in two: those of the 450 small holes in the
    // first half.
    let (out, _) = run(Action::SubdivideTriangles, &mesh, &selection);
    assert_eq!(stats(&out.mesh).open_edges, before.open_edges + 450 * 3);

    let (out, took) = run(Action::SmoothTriangles, &mesh, &selection);
    report("Smooth triangles", &out, took);
    assert!(out.changed && !out.selected);
    assert_eq!(out.selection, selection);
    assert_eq!(out.mesh.indices, mesh.indices);
    assert_sound("Smooth triangles", &out);

    // Open edges.
    let (out, took) = run(Action::AddTriangle, &mesh, &[]);
    report("Add triangle", &out, took);
    assert_eq!(out.reports[0].message, "Added 899 triangles.");
    assert_eq!(stats(&out.mesh).open_edges, 80);
    assert_sound("Add triangle", &out);

    let (out, took) = run(Action::GrowHoleBand, &mesh, &selection);
    report("Grow a band", &out, took);
    assert_eq!(
        out.reports[0].message,
        "Added a band of 2,700 triangles along 450 open loops."
    );
    assert_sound("Grow a band", &out);

    let (out, took) = run(Action::BridgeGap, &mesh, &[]);
    report("Bridge a gap", &out, took);
    assert_eq!(out.reports[0].message, "Bridged a gap with 2 triangles.");
    assert_sound("Bridge a gap", &out);

    let (out, took) = run(Action::CloseHolesCurved, &mesh, &[]);
    report("Close holes along curvature", &out, took);
    assert!(out.reports[0].message.starts_with("Closed 900 holes"));
    assert!(
        out.reports[0].warnings.is_empty(),
        "{:?}",
        out.reports[0].warnings
    );
    let s = stats(&out.mesh);
    assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
    let whole = stats(&torus(N, N, 10.0, 3.0));
    assert!(
        (s.volume - whole.volume).abs() < 1e-3 * whole.volume,
        "{}",
        s.volume
    );

    // Selections: the mesh stays.
    let (out, took) = run(Action::SelectProblemTriangles, &mesh, &selection);
    report("Select by criteria", &out, took);
    assert!(!out.changed && out.selected);
    assert!(
        out.selection.len() >= 899 * 3 + 80,
        "{}",
        out.selection.len()
    );
    let (out, took) = run(Action::FindOverlaps, &mesh, &[]);
    report("Find overlapping triangles", &out, took);
    assert!(!out.changed && !out.selected);
    assert_eq!(out.reports[0].message, "No overlapping triangles found.");
}

/// Vertex (i, j) of the torus: the j-th of N round the tube on the i-th ring of N round the
/// middle, as a clicked point on the first triangle of its quad.
fn torus_point(mesh: &Mesh, i: usize, j: usize) -> CutPoint {
    let v = (i % N) * N + (j % N);
    let p = mesh.vertex(v as u32);
    CutPoint {
        at: p.map(f64::from),
        triangle: (2 * v) as u32,
    }
}

/// A closed line of four points over the top of the whole torus, a quarter of the way round it
/// and a fifth of the way round its tube: hundreds of triangles in between.
fn big_loop(mesh: &Mesh, snap: f64) -> CutLine {
    CutLine {
        points: vec![
            torus_point(mesh, 100, 90),
            torus_point(mesh, 250, 90),
            torus_point(mesh, 250, 210),
            torus_point(mesh, 100, 210),
        ],
        options: surface_cut::CutOptions {
            snap,
            closed: true,
            ..surface_cut::OPTIONS
        },
    }
}

#[test]
fn cut_surfaces_and_move_nodes_on_a_big_part() {
    let mesh = torus(N, N, 10.0, 3.0);
    let before = stats(&mesh);
    assert_eq!(mesh.triangle_count(), 2 * N * N);
    for snap in [0.5, 0.0] {
        for smooth in [false, true] {
            let mut line = big_loop(&mesh, snap);
            line.options.smooth = smooth;
            let name = format!("Cut surfaces, snap {snap}, smooth {smooth}");
            let (out, took) = run(Action::CutSurface(line), &mesh, &[]);
            println!(
                "{name:<36} {took:>7.3} s  {} -> {} triangles: {}",
                mesh.triangle_count(),
                out.mesh.triangle_count(),
                out.reports[0].message
            );
            assert!(
                out.changed && out.selected && out.selection.is_empty(),
                "{name}"
            );
            assert_sound(&name, &out);
            assert!(
                out.reports[0].warnings.is_empty(),
                "{name}: {:?}",
                out.reports[0].warnings
            );
            // Every edge of the closed line is two open edges now, and what it encloses is a shell
            // of its own.
            let message = &out.reports[0].message;
            let edges: u64 = message
                .split(' ')
                .nth(5)
                .unwrap()
                .replace(',', "")
                .parse()
                .unwrap();
            assert!(edges > 300, "{name}: {message}");
            let s = stats(&out.mesh);
            assert_eq!(s.open_edges, 2 * edges, "{name}");
            let e = Edges::new(&out.mesh);
            assert_eq!(components(&out.mesh, &e, Link::Shell).1, 2, "{name}");
            assert!(
                (s.area - before.area).abs() < 1e-6 * before.area,
                "{name}: {}",
                s.area
            );
            // No new edge shorter than Remove degenerate faces' tolerance, at any snap distance.
            let degenerate =
                status::degenerate_triangles(&out.mesh, defaults::DEGENERATE.tolerance);
            assert!(
                degenerate.is_empty(),
                "{name}: {} degenerate faces",
                degenerate.len()
            );
        }
    }

    // A vertex six triangles share, pulled out by 1 mm along x.
    let v = (175 * N + 150) as u32;
    let p = mesh.vertex(v).map(f64::from);
    let step = NodeMove {
        vertex: v,
        to: [p[0] + 1.0, p[1], p[2]],
    };
    let (out, took) = run(Action::MoveNode(step), &mesh, &[]);
    println!(
        "{:<36} {took:>7.3} s: {}",
        "Move nodes", out.reports[0].message
    );
    assert!(out.changed && !out.selected);
    assert_eq!(out.mesh.indices, mesh.indices);
    let moved: Vec<usize> = (0..mesh.vertex_count())
        .filter(|&u| out.mesh.vertex(u as u32) != mesh.vertex(u as u32))
        .collect();
    assert_eq!(moved, vec![v as usize]);
    let around = mesh.indices.iter().filter(|&&c| c == v).count();
    assert_eq!(around, 6);
    assert_sound("Move nodes", &out);
}

#[test]
fn every_home_edit_stops_when_cancelled() {
    let mesh = holed();
    let selection = half(&mesh);
    let whole = torus(N, N, 10.0, 3.0);
    for (action, selected) in [
        (Action::SubdivideTriangles, true),
        (Action::AddPoint, true),
        (Action::SmoothTriangles, true),
        (Action::CutSurface(big_loop(&whole, 0.5)), false),
        (Action::AddTriangle, false),
        (Action::GrowHoleBand, true),
        (Action::BridgeGap, false),
        (Action::CloseHolesCurved, false),
        (Action::SelectProblemTriangles, false),
        (Action::FindOverlaps, false),
    ] {
        let chosen: &[u32] = if selected { &selection } else { &[] };
        let started = Instant::now();
        // The cut needs a part without holes in its way.
        let part = match action {
            Action::CutSurface(_) => &whole,
            _ => &mesh,
        };
        let result = cancelled(action.clone(), part, chosen);
        assert_eq!(
            result.err(),
            Some(RepairError::Cancelled),
            "{:?} did not stop",
            action
        );
        println!(
            "{:<24} stopped after {:.3} s",
            format!("{action:?}"),
            started.elapsed().as_secs_f64()
        );
    }
}
