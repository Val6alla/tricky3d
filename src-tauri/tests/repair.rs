//! The repair actions and the built-in scripts on the committed fixtures in
//! tests/fixtures/repair/, read through the importer like a user's file. Regenerate the fixtures
//! with `cargo test --manifest-path src-tauri/Cargo.toml --test repair -- --ignored write_fixtures`.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use tricky3d_lib::formats::{ExportFormat, NamedMesh, import_file, write_export};
use tricky3d_lib::mesh::{Mesh, make_box};
use tricky3d_lib::ops::concat;
use tricky3d_lib::progress::Progress;
use tricky3d_lib::repair::defaults::{
    self, DEFAULT_ID, EXTENDED_ID, SIMPLE_ID, builtin_script, builtin_scripts,
};
use tricky3d_lib::repair::scripts::{Script, parse_action};
use tricky3d_lib::repair::status::{RepairStatus, analyze};
use tricky3d_lib::repair::{
    Action, RepairError, ScriptOutcome, TinyShellParams, run_action, run_script, silent, tritri,
};

// ---------------------------------------------------------------------------------------------
// Fixtures

const FIXTURES: [&str; 8] = [
    "cube-open-face.stl",
    "cube-3-flipped.stl",
    "cube-double-triangle.stl",
    "cube-crack.stl",
    "cube-speck.stl",
    "two-cubes.stl",
    "cube-sliver.stl",
    "cube-inside-out.stl",
];

fn flip(mesh: &mut Mesh, triangle: usize) {
    mesh.indices.swap(triangle * 3 + 1, triangle * 3 + 2);
}

fn cube20() -> Mesh {
    make_box([0.0; 3], [20.0; 3])
}

/// make_box's faces come in the order −z, +z, −y, +y, +x, −x, two triangles each.
fn without_face(mut mesh: Mesh, face: usize) -> Mesh {
    mesh.indices.drain(face * 6..face * 6 + 6);
    mesh
}

fn moved(mut mesh: Mesh, dx: f32) -> Mesh {
    for p in mesh.positions.as_chunks_mut::<3>().0 {
        p[0] += dx;
    }
    mesh
}

/// Each fixture as the generator builds it.
fn build_fixture(name: &str) -> Mesh {
    match name {
        // A 20 mm cube with its bottom face removed: one hole of 4 edges.
        "cube-open-face.stl" => without_face(cube20(), 0),
        // Three triangles on three different faces turned round.
        "cube-3-flipped.stl" => {
            let mut m = cube20();
            for t in [2, 5, 8] {
                flip(&mut m, t);
            }
            m
        }
        // One triangle twice.
        "cube-double-triangle.stl" => {
            let mut m = cube20();
            let copy = m.indices[12..15].to_vec();
            m.indices.extend(copy);
            m
        }
        // The cube cut at x = 10 into two open halves, the right one moved 0.005 mm: a crack.
        "cube-crack.stl" => {
            let left = without_face(make_box([0.0; 3], [10.0, 20.0, 20.0]), 4);
            let right = moved(
                without_face(make_box([10.0, 0.0, 0.0], [20.0; 3]), 5),
                0.005,
            );
            concat(&[&left, &right]).unwrap()
        }
        // The cube and, beside it, a speck of 0.01 mm³.
        "cube-speck.stl" => {
            let side = 0.01f64.cbrt() as f32;
            let speck = make_box([25.0, 0.0, 0.0], [25.0 + side, side, side]);
            concat(&[&cube20(), &speck]).unwrap()
        }
        // Two 20 mm cubes in one part, the second moved 10 mm in X: they overlap by half.
        "two-cubes.stl" => {
            let second = make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0]);
            concat(&[&cube20(), &second]).unwrap()
        }
        // The top face as a fan around a point on its front edge: one sliver without area.
        "cube-sliver.stl" => {
            let mut m = without_face(cube20(), 1);
            m.positions.extend_from_slice(&[10.0, 0.0, 20.0]);
            let apex = 8;
            for [a, b] in [[4, 5], [5, 6], [6, 7], [7, 4]] {
                m.indices.extend_from_slice(&[a, b, apex]);
            }
            m
        }
        // Every triangle facing inward.
        "cube-inside-out.stl" => {
            let mut m = cube20();
            for t in 0..12 {
                flip(&mut m, t);
            }
            m
        }
        _ => panic!("unknown fixture {name}"),
    }
}

fn repair_fixture(name: &str) -> std::path::PathBuf {
    fixture("repair").join(name)
}

#[test]
#[ignore = "writes the committed fixtures"]
fn write_fixtures() {
    std::fs::create_dir_all(fixture("repair")).unwrap();
    for name in FIXTURES {
        let mesh = build_fixture(name);
        let title = name.trim_end_matches(".stl");
        let bytes = write_export(
            std::io::Cursor::new(Vec::new()),
            ExportFormat::StlAscii,
            title,
            &[NamedMesh {
                name: title,
                mesh: &mesh,
            }],
        )
        .unwrap()
        .into_inner();
        std::fs::write(repair_fixture(name), bytes).unwrap();
    }
}

/// A fixture read through the importer (welded like any imported file).
fn load(name: &str) -> Mesh {
    let imported = import_file(&repair_fixture(name), true, &mut Progress::silent())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(imported.parts.len(), 1, "{name}");
    imported.parts.into_iter().next().unwrap().mesh
}

#[test]
fn committed_fixtures_match_the_generator() {
    for name in FIXTURES {
        let built = build_fixture(name);
        let loaded = load(name);
        assert_eq!(
            loaded.triangle_count(),
            built.triangle_count(),
            "{name}: triangle count"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn status(mesh: &Mesh) -> RepairStatus {
    silent(|c| analyze(mesh, c)).unwrap().status
}

fn self_intersections(mesh: &Mesh) -> usize {
    silent(|c| tritri::intersecting_pairs(mesh, c))
        .unwrap()
        .len()
}

fn act(action: Action, mesh: &Mesh) -> Mesh {
    let outcome = silent(|c| run_action(&action, mesh, &[], c)).unwrap();
    outcome.mesh
}

fn steps(script: &Script) -> Vec<(Action, String)> {
    script
        .actions
        .iter()
        .filter(|s| s.enabled)
        .map(|s| {
            (
                parse_action(s).unwrap(),
                defaults::label_of(s.action).to_string(),
            )
        })
        .collect()
}

fn run(script: &Script, mesh: &Mesh) -> ScriptOutcome {
    let cancel = AtomicBool::new(false);
    run_script(
        &steps(script),
        mesh,
        &[],
        &cancel,
        |_, _| {},
        |_, _| {},
        |_, _| {},
    )
    .unwrap_or_else(|e| panic!("{}: {e:?}", script.name))
}

fn script(id: &str) -> Script {
    builtin_script(id).unwrap()
}

/// Volume in cm³ to three decimals, as the Status and Information panels show it.
fn cm3(volume_mm3: f64) -> String {
    format!("{:.3}", volume_mm3 / 1000.0)
}

/// What Extended repair must leave on every fixture.
fn assert_printable(name: &str, mesh: &Mesh) {
    let s = status(mesh);
    assert!(s.closed, "{name}: not closed: {s:?}");
    assert!(s.oriented, "{name}: not oriented: {s:?}");
    assert_eq!(s.border_edges, 0, "{name}");
    assert_eq!(s.holes, 0, "{name}");
    assert_eq!(s.inverted_triangles, 0, "{name}");
    assert_eq!(s.non_manifold_edges, 0, "{name}");
    assert!(s.volume > 0.0, "{name}: volume {}", s.volume);
    assert_eq!(
        self_intersections(mesh),
        0,
        "{name}: self-intersections left"
    );
}

// ---------------------------------------------------------------------------------------------
// Each single action fixes its own fixture

#[test]
fn close_holes_fills_the_open_face() {
    let mesh = load("cube-open-face.stl");
    let before = status(&mesh);
    assert_eq!(
        (before.holes, before.border_edges, before.closed),
        (1, 4, false)
    );
    for action in [Action::CloseAllHoles, Action::CloseTrivialHoles] {
        let fixed = act(action.clone(), &mesh);
        let s = status(&fixed);
        assert_eq!((s.holes, s.border_edges), (0, 0), "{action:?}");
        assert!(s.closed && s.oriented, "{action:?}: {s:?}");
        assert_eq!(cm3(s.volume), "8.000", "{action:?}");
    }
}

#[test]
fn fix_flipped_turns_the_three_triangles_back() {
    let mesh = load("cube-3-flipped.stl");
    assert_eq!(status(&mesh).inverted_triangles, 3);
    let s = status(&act(Action::FixFlippedTriangles, &mesh));
    assert_eq!(s.inverted_triangles, 0);
    assert!(s.oriented && s.closed);
    assert_eq!(cm3(s.volume), "8.000");
}

#[test]
fn remove_double_triangles_drops_the_copy() {
    let mesh = load("cube-double-triangle.stl");
    assert_eq!(mesh.triangle_count(), 13);
    assert!(!status(&mesh).closed);
    let fixed = act(Action::RemoveDoubleTriangles, &mesh);
    assert_eq!(fixed.triangle_count(), 12);
    let s = status(&fixed);
    assert!(s.closed && s.oriented, "{s:?}");
}

#[test]
fn stitch_closes_the_crack() {
    let mesh = load("cube-crack.stl");
    let before = status(&mesh);
    assert_eq!(before.shells, 2);
    assert_eq!(before.holes, 2);
    let fixed = act(Action::StitchTriangles(defaults::STITCH), &mesh);
    let s = status(&fixed);
    assert_eq!((s.shells, s.border_edges, s.holes), (1, 0, 0), "{s:?}");
    assert!(s.closed && s.oriented, "{s:?}");
}

#[test]
fn remove_tiny_shells_deletes_the_speck_only_above_its_threshold() {
    let mesh = load("cube-speck.stl");
    assert_eq!(status(&mesh).shells, 2);
    // The scripts' default (0.0001 mm³) keeps a 0.01 mm³ speck.
    let kept = act(Action::RemoveTinyShells(defaults::TINY_SHELLS), &mesh);
    assert_eq!(status(&kept).shells, 2);
    let params = TinyShellParams {
        min_volume: 0.1,
        ..defaults::TINY_SHELLS
    };
    let s = status(&act(Action::RemoveTinyShells(params), &mesh));
    assert_eq!(s.shells, 1);
    assert_eq!(cm3(s.volume), "8.000");
}

#[test]
fn wrap_unites_the_interpenetrating_cubes() {
    let mesh = load("two-cubes.stl");
    let before = status(&mesh);
    assert_eq!(before.shells, 2);
    assert!(self_intersections(&mesh) > 0);
    let fixed = act(Action::WrapPartSurface, &mesh);
    assert_printable("two-cubes wrapped", &fixed);
    let s = status(&fixed);
    assert_eq!(s.shells, 1);
    assert_eq!(cm3(s.volume), "12.000");
    // The original surface stays where it was: every vertex lies on the outer box's planes.
    for p in fixed.positions.as_chunks::<3>().0 {
        let on = [
            p[0] == 0.0 || p[0] == 30.0,
            p[1] == 0.0 || p[1] == 20.0,
            p[2] == 0.0 || p[2] == 20.0,
        ];
        assert!(on.iter().any(|&b| b), "vertex {p:?} is off the box surface");
    }
}

#[test]
fn remove_degenerate_faces_collapses_the_sliver() {
    let mesh = load("cube-sliver.stl");
    assert!(status(&mesh).degenerate_triangles >= 1);
    let fixed = act(Action::RemoveDegenerateFaces(defaults::DEGENERATE), &mesh);
    let s = status(&fixed);
    assert_eq!(s.degenerate_triangles, 0, "{s:?}");
    assert!(s.closed && s.oriented, "{s:?}");
    assert_eq!(cm3(s.volume), "8.000");
}

#[test]
fn fix_flipped_turns_the_inside_out_cube_outward() {
    let mesh = load("cube-inside-out.stl");
    let before = status(&mesh);
    assert_eq!(cm3(before.volume), "-8.000");
    assert!(!before.oriented);
    let s = status(&act(Action::FixFlippedTriangles, &mesh));
    assert_eq!(cm3(s.volume), "8.000");
    assert!(s.oriented && s.closed);
}

// ---------------------------------------------------------------------------------------------
// Built-in scripts

#[test]
fn extended_repair_makes_every_fixture_printable() {
    let extended = script(EXTENDED_ID);
    for name in FIXTURES {
        let result = run(&extended, &load(name));
        assert_printable(name, &result.mesh);
        let s = status(&result.mesh);
        match name {
            "two-cubes.stl" => {
                assert_eq!(s.shells, 1, "{name}");
                assert_eq!(cm3(s.volume), "12.000", "{name}");
            }
            "cube-inside-out.stl" => assert_eq!(cm3(s.volume), "8.000", "{name}"),
            "cube-speck.stl" => assert_eq!(s.shells, 2, "{name}: the speck is above 0.0001 mm³"),
            // The right half stays 0.005 mm further out once the crack is stitched shut.
            "cube-crack.stl" => {
                assert_eq!(s.shells, 1, "{name}");
                assert_eq!(cm3(s.volume), "8.002", "{name}");
            }
            _ => {
                assert_eq!(s.shells, 1, "{name}");
                assert_eq!(cm3(s.volume), "8.000", "{name}");
            }
        }
    }
}

#[test]
fn simple_and_default_repair_do_not_wrap() {
    let mesh = load("two-cubes.stl");
    for id in [SIMPLE_ID, DEFAULT_ID] {
        let result = run(&script(id), &mesh);
        let s = status(&result.mesh);
        assert_eq!(s.shells, 2, "{id}");
        assert!(s.closed && s.oriented, "{id}: {s:?}");
    }
}

#[test]
fn running_a_script_twice_changes_nothing_the_second_time() {
    for script in builtin_scripts() {
        for name in FIXTURES {
            let first = run(&script, &load(name));
            let second = run(&script, &first.mesh);
            assert!(
                !second.changed,
                "{} on {name}: the second run changed {:?}",
                script.name,
                second
                    .reports
                    .iter()
                    .filter(|r| r.changed)
                    .map(|r| &r.label)
                    .collect::<Vec<_>>()
            );
            assert_eq!(second.mesh, first.mesh, "{} on {name}", script.name);
        }
    }
}

#[test]
fn a_cancelled_script_stops_and_returns_nothing() {
    let mesh = load("two-cubes.stl");
    let cancel = AtomicBool::new(false);
    let steps = steps(&script(EXTENDED_ID));
    let mut reached = Vec::new();
    let result = run_script(
        &steps,
        &mesh,
        &[],
        &cancel,
        |step, _| {
            reached.push(step);
            if step == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        |_, _| {},
        |_, _| {},
    );
    assert_eq!(result, Err(RepairError::Cancelled));
    assert_eq!(reached, vec![0, 1, 2, 3]);
}
