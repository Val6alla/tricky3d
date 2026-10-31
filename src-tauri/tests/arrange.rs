//! Arrange end to end, through the job the `arrange` command runs: parts placed by their matrices,
//! the answer's matrices applied to them, and cancel before and during a run.

mod common;

use std::cell::Cell;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Bundle, OutMesh, decode, encode};
use tricky3d_lib::jobs::{self, CANCELLED, RepairJobs};
use tricky3d_lib::mesh::{Matrix, Mesh, make_box};

/// A part of a request: its mesh, its matrix, and whether the action works on it.
type Part<'a> = (&'a Mesh, Matrix, bool);

fn request(job: u64, action: &str, settings: Value, parts: &[Part]) -> Bundle {
    let out: Vec<OutMesh> = parts
        .iter()
        .enumerate()
        .map(|(i, &(mesh, matrix, target))| {
            let mut info = Map::new();
            info.insert("name".into(), json!(format!("Part {i}")));
            info.insert("matrix".into(), json!(matrix.to_vec()));
            info.insert("target".into(), json!(target));
            OutMesh {
                info,
                positions: &mesh.positions,
                indices: &mesh.indices,
                edges: None,
            }
        })
        .collect();
    let mut meta = match settings {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    meta.insert("job".into(), json!(job));
    meta.insert("action".into(), json!(action));
    meta.insert("platform".into(), json!([200.0, 200.0, 200.0]));
    decode(&encode(&meta, &out)).unwrap()
}

fn run(bundle: Bundle, jobs: &RepairJobs) -> Result<Map<String, Value>, String> {
    jobs::arrange(bundle, jobs, &mut |_| {}).map(|bytes| decode(&bytes).unwrap().meta)
}

fn matrices(meta: &Map<String, Value>, key: &str) -> Vec<Matrix> {
    meta[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let v: Vec<f64> = serde_json::from_value(m.clone()).unwrap();
            Matrix::try_from(v.as_slice()).unwrap()
        })
        .collect()
}

/// The world box of `mesh` placed by `m`.
fn world_box(mesh: &Mesh, m: &Matrix) -> [[f64; 3]; 2] {
    let w = tricky3d_lib::arrange::World::new(mesh, m);
    w.bounds().unwrap()
}

#[test]
fn align_left_and_drop_through_the_job_are_exact() {
    let cube = cube(10.0);
    let a = trs([12.345, 30.0, 4.0], 0.0, 0.0, [1.0; 3]);
    let b = trs([77.7, 60.0, 9.5], 33.0, 20.0, [1.0; 3]);
    let parts = [(&cube, a, true), (&cube, b, true)];
    let jobs = RepairJobs::default();
    let meta = run(request(1, "align", json!({"align": "left"}), &parts), &jobs).unwrap();
    let out = matrices(&meta, "matrices");
    let left = world_box(&cube, &a)[0][0].min(world_box(&cube, &b)[0][0]);
    for m in &out {
        assert_close(world_box(&cube, m)[0][0], left, 1e-3, "left side");
    }
    let meta = run(request(2, "align", json!({"align": "drop"}), &parts), &jobs).unwrap();
    for m in matrices(&meta, "matrices") {
        assert_close(world_box(&cube, &m)[0][2], 0.0, 1e-3, "on the platform");
    }
    assert!(jobs.is_idle());
}

#[test]
fn fill_the_platform_keeps_clear_of_the_other_parts() {
    let block = make_box([0.0; 3], [20.0, 10.0, 5.0]);
    let other = cube(30.0);
    let parts = [
        (&block, trs([50.0, 50.0, 0.0], 0.0, 0.0, [1.0; 3]), true),
        (&other, trs([0.0, 0.0, 0.0], 0.0, 0.0, [1.0; 3]), false),
    ];
    let settings = json!({"duplicate": {"mode": "fill", "spacing": 5.0, "wall": 0.0, "limit": 10000, "stack": false}});
    let meta = run(
        request(1, "duplicate", settings, &parts),
        &RepairJobs::default(),
    )
    .unwrap();
    let copies = meta["copies"].as_array().unwrap();
    // 8 × 13 places on the empty platform; those within 5 mm of the 30 mm cube are skipped:
    // columns at x < 35 (two) in rows at y < 35 (three).
    assert_eq!(copies.len() + 1, 8 * 13 - 2 * 3);
    let other_box = world_box(&other, &parts[1].1);
    let moved = matrices(&meta, "matrices");
    let mut placed = vec![moved[0]];
    for c in copies {
        let v: Vec<f64> = serde_json::from_value(c["matrix"].clone()).unwrap();
        placed.push(Matrix::try_from(v.as_slice()).unwrap());
    }
    for m in placed {
        let b = world_box(&block, &m);
        assert!(
            b[1][0] <= 200.0 + 1e-9
                && b[1][1] <= 200.0 + 1e-9
                && b[0][0] >= -1e-9
                && b[0][1] >= -1e-9
        );
        let apart = (0..2).any(|k| b[0][k] >= other_box[1][k] + 5.0 - 1e-9);
        assert!(apart, "{b:?} is within 5 mm of the other part");
    }
}

#[test]
fn a_cancel_before_or_during_a_run_answers_cancelled() {
    let torus = torus(64, 32, 30.0, 8.0);
    let parts = [(
        &torus,
        trs([100.0, 100.0, 20.0], 10.0, 35.0, [1.0; 3]),
        true,
    )];
    let jobs = RepairJobs::default();
    jobs.cancel(5);
    let early = run(
        request(5, "orient", json!({"goal": "least-support"}), &parts),
        &jobs,
    );
    assert_eq!(early, Err(CANCELLED.to_string()));
    for at in [0.0, 0.3, 0.9] {
        let jobs = RepairJobs::default();
        let sent = Cell::new(false);
        let answer = jobs::arrange(
            request(6, "orient", json!({"goal": "lowest-height"}), &parts),
            &jobs,
            &mut |p| {
                if p.fraction >= at && !sent.get() {
                    sent.set(true);
                    jobs.cancel(6);
                }
            },
        );
        assert_eq!(answer, Err(CANCELLED.to_string()), "cancelled at {at}");
        assert!(jobs.is_idle());
    }
    // Run to the end, the same request lays the torus on its side's flat ring: 16 mm tall.
    let meta = run(
        request(7, "orient", json!({"goal": "lowest-height"}), &parts),
        &RepairJobs::default(),
    )
    .unwrap();
    let best = &meta["candidates"][0];
    assert_close(
        best["height"].as_f64().unwrap(),
        16.0,
        0.05,
        "lowest height",
    );
}

#[test]
fn lay_flat_through_the_job_puts_the_face_on_the_platform() {
    let block = make_box([0.0; 3], [20.0, 10.0, 5.0]);
    let m = trs([80.0, 80.0, 40.0], 25.0, 50.0, [1.0; 3]);
    let parts = [(&block, m, true)];
    let meta = run(
        request(1, "lay-flat-auto", json!({}), &parts),
        &RepairJobs::default(),
    )
    .unwrap();
    let out = matrices(&meta, "matrices");
    let b = world_box(&block, &out[0]);
    assert_close(b[0][2], 0.0, 1e-9, "on the platform");
    assert_close(b[1][2] - b[0][2], 5.0, 1e-9, "on its largest face");
}

// ---------------------------------------------------------------------------------------------
// Packing

use std::time::Instant;

use tricky3d_lib::arrange::World;
use tricky3d_lib::library::{self, Category, Input};

/// The four methods with the dialog's defaults (`fast` turns performance up for quick test runs).
fn methods(fast: bool) -> Vec<(&'static str, Value)> {
    let p = if fast { 9 } else { 5 };
    vec![
        (
            "3D Bounding box",
            json!({"method": "bounding-box", "distance": 1.0, "voxel": 5.0, "wall": 0.0, "floor": 0.0, "ceiling": 0.0, "stack": true}),
        ),
        (
            "2D Scanline",
            json!({"method": "scanline2d", "performance": p, "align": true, "zSteps": 12, "wall": 4.0, "spacing": 1, "voxel": 1.0}),
        ),
        (
            "3D Scanline",
            json!({"method": "scanline3d", "performance": p, "voxel": 2.0, "zSteps": 12, "flip": true, "wall": 0.0, "floor": 0.0, "ceiling": 0.0}),
        ),
        (
            "3D Monte Carlo",
            json!({"method": "monte-carlo", "performance": p, "distance": 1.0, "rotation": "z", "wall": 0.0, "floor": 0.0, "ceiling": 0.0, "seed": 1}),
        ),
    ]
}

/// Five items of each Part library category that build with their defaults: 20 mixed shapes,
/// standing where the library puts them (centred on the origin, on z = 0).
fn library_shapes() -> Vec<(String, Mesh)> {
    let mut out = Vec::new();
    for category in [
        Category::Primitive,
        Category::Design,
        Category::Mathematical,
        Category::Quality,
    ] {
        let mut n = 0;
        for item in library::catalog() {
            if n == 5
                || item.category != category
                || item.soon.is_some()
                || item.input != Input::None
            {
                continue;
            }
            if let Ok(mesh) = library::generate(item.id, &Map::new())
                && mesh.triangle_count() > 0
                && mesh.triangle_count() <= 200_000
            {
                out.push((item.id.to_string(), mesh));
                n += 1;
            }
        }
    }
    out
}

/// What one pack did, for the report table.
struct Packed {
    parts: usize,
    fits: usize,
    /// Packed parts whose lowest point is more than 1 mm above the platform: on top of others.
    stacked: usize,
    height: f64,
    density: f64,
    seconds: f64,
}

/// Packs `parts` (all targets) on a 200 mm cube through the job, which also checks the meshes for
/// collisions, and checks every packed part is on the platform and every other part beside it.
fn pack_and_check(
    parts: &[(String, Mesh, Matrix)],
    settings: &Value,
    platform: f64,
) -> Result<Packed, String> {
    let list: Vec<Part> = parts.iter().map(|(_, m, x)| (m, *x, true)).collect();
    let mut bundle = request(1, "pack", json!({"pack": settings}), &list);
    bundle
        .meta
        .insert("platform".into(), json!([platform, platform, platform]));
    let start = Instant::now();
    let meta = run(bundle, &RepairJobs::default())?;
    let seconds = start.elapsed().as_secs_f64();
    let fits: Vec<bool> = serde_json::from_value(meta["fits"].clone()).unwrap();
    let mut stacked = 0;
    for ((_, mesh, _), (m, fit)) in parts
        .iter()
        .zip(matrices(&meta, "matrices").iter().zip(&fits))
    {
        let b = world_box(mesh, m);
        if *fit && b[0][2] > 1.0 {
            stacked += 1;
        }
        if *fit {
            for k in 0..3 {
                assert!(
                    b[0][k] >= -1e-6 && b[1][k] <= platform + 1e-6,
                    "{settings}: off the platform {b:?}"
                );
            }
        } else {
            assert!(
                b[0][0] > platform,
                "{settings}: a part that does not fit was left on the platform"
            );
        }
    }
    Ok(Packed {
        parts: parts.len(),
        fits: fits.iter().filter(|&&f| f).count(),
        stacked,
        height: meta["height"].as_f64().unwrap(),
        density: meta["density"].as_f64().unwrap(),
        seconds,
    })
}

#[test]
fn every_method_packs_twenty_library_shapes_without_collisions() {
    let shapes = library_shapes();
    assert_eq!(
        shapes.len(),
        20,
        "{:?}",
        shapes.iter().map(|s| &s.0).collect::<Vec<_>>()
    );
    // Scattered over and beside the platform, some overlapping, as a user might leave them.
    let parts: Vec<(String, Mesh, Matrix)> = shapes
        .into_iter()
        .enumerate()
        .map(|(i, (id, mesh))| {
            (
                id,
                mesh,
                trs(
                    [30.0 * (i % 7) as f64, 25.0 * (i / 7) as f64, 0.0],
                    17.0 * i as f64,
                    0.0,
                    [1.0; 3],
                ),
            )
        })
        .collect();
    for (name, settings) in methods(true) {
        let packed =
            pack_and_check(&parts, &settings, 200.0).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(packed.fits >= 10, "{name}: only {} of 20 fit", packed.fits);
        println!(
            "| {name} | {} | {} | {:.1} | {:.1} % | {:.2} |",
            packed.parts,
            packed.fits,
            packed.height,
            packed.density * 100.0,
            packed.seconds
        );
    }
}

#[test]
fn a_locked_part_never_moves_and_the_rest_pack_round_it() {
    let block = make_box([0.0; 3], [20.0, 20.0, 20.0]);
    let wall = make_box([0.0; 3], [10.0, 180.0, 60.0]);
    let fixed = trs([95.0, 10.0, 0.0], 0.0, 0.0, [1.0; 3]);
    let mut parts: Vec<Part> = (0..12)
        .map(|i| {
            (
                &block,
                trs([5.0 * i as f64, 0.0, 30.0], 0.0, 0.0, [1.0; 3]),
                true,
            )
        })
        .collect();
    parts.push((&wall, fixed, false));
    for (name, settings) in methods(true) {
        let meta = run(
            request(1, "pack", json!({"pack": settings}), &parts),
            &RepairJobs::default(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        // Only the targets get matrices; the locked wall is not in the answer at all.
        assert_eq!(matrices(&meta, "matrices").len(), 12, "{name}");
        let wall_box = world_box(&wall, &fixed);
        for m in matrices(&meta, "matrices") {
            let b = world_box(&block, &m);
            let apart = (0..3).any(|k| b[0][k] >= wall_box[1][k] || b[1][k] <= wall_box[0][k]);
            assert!(apart, "{name}: a block went into the locked wall: {b:?}");
        }
    }
}

#[test]
fn a_pack_cancelled_at_any_point_changes_nothing() {
    let shapes = library_shapes();
    let parts: Vec<Part> = shapes
        .iter()
        .take(8)
        .map(|(_, m)| (m, trs([0.0; 3], 0.0, 0.0, [1.0; 3]), true))
        .collect();
    for (name, settings) in methods(true) {
        for at in [0.0, 0.2, 0.5, 0.8, 0.95] {
            let jobs = RepairJobs::default();
            let sent = Cell::new(false);
            let answer = jobs::arrange(
                request(3, "pack", json!({"pack": settings}), &parts),
                &jobs,
                &mut |p| {
                    if p.fraction >= at && !sent.get() {
                        sent.set(true);
                        jobs.cancel(3);
                    }
                },
            );
            assert_eq!(
                answer,
                Err(CANCELLED.to_string()),
                "{name} cancelled at {at}"
            );
            assert!(jobs.is_idle());
        }
    }
}

#[test]
fn monte_carlo_through_the_job_gives_the_same_answer_for_the_same_seed() {
    let shapes = library_shapes();
    let parts: Vec<Part> = shapes
        .iter()
        .take(10)
        .map(|(_, m)| (m, trs([0.0; 3], 0.0, 0.0, [1.0; 3]), true))
        .collect();
    let mc = &methods(true)[3].1;
    let a = run(
        request(1, "pack", json!({"pack": mc}), &parts),
        &RepairJobs::default(),
    )
    .unwrap();
    let b = run(
        request(2, "pack", json!({"pack": mc}), &parts),
        &RepairJobs::default(),
    )
    .unwrap();
    assert_eq!(a, b);
}

/// The corpus run behind the report's tables: every method on the 20 library shapes, or on 50
/// Thingi10K models (closed, one shell, 500 to 50,000 triangles, the lowest file ids), each scaled
/// so its longest side is 20 to 60 mm. Run in release through scripts/corpus/capped.ps1, one
/// method at a time:
///
///   TRICKY3D_PACK_SET=library|thingi TRICKY3D_PACK_METHOD=0..3 TRICKY3D_PACK_PLATFORM=<mm, 200>
///   TRICKY3D_THINGI=<folder with geometry_data.csv and Thingi10K/raw_meshes>
///   cargo test --release --test arrange pack_report -- --ignored --nocapture
#[test]
#[ignore = "needs the model corpus; run through scripts/corpus/capped.ps1"]
fn pack_report() {
    let set = std::env::var("TRICKY3D_PACK_SET").unwrap_or_else(|_| "library".into());
    let method: usize = std::env::var("TRICKY3D_PACK_METHOD").map_or(0, |m| m.parse().unwrap());
    let parts: Vec<(String, Mesh, Matrix)> = if set == "thingi" {
        thingi_models()
    } else {
        library_shapes()
            .into_iter()
            .map(|(id, mesh)| (id, mesh, IDENTITY_M))
            .collect()
    };
    let platform: f64 =
        std::env::var("TRICKY3D_PACK_PLATFORM").map_or(200.0, |p| p.parse().unwrap());
    let (name, settings) = methods(false).swap_remove(method);
    match pack_and_check(&parts, &settings, platform) {
        Ok(p) => println!(
            "PACK|{set}|{platform}|{name}|{}|{}|{}|{:.1}|{:.2}|{:.2}|no collisions",
            p.parts,
            p.fits,
            p.stacked,
            p.height,
            p.density * 100.0,
            p.seconds
        ),
        Err(e) => println!("PACK|{set}|{platform}|{name}|{}|-|-|-|-|-|{e}", parts.len()),
    }
}

const IDENTITY_M: Matrix = tricky3d_lib::mesh::IDENTITY;

fn thingi_models() -> Vec<(String, Mesh, Matrix)> {
    let root =
        std::path::PathBuf::from(std::env::var("TRICKY3D_THINGI").expect("set TRICKY3D_THINGI"));
    let table = std::fs::read_to_string(root.join("geometry_data.csv")).unwrap();
    let mut lines = table.lines();
    let head: Vec<&str> = lines.next().unwrap().split(',').collect();
    let col = |name: &str| head.iter().position(|h| *h == name).unwrap();
    let (id, faces, shells, border, solid) = (
        col("file_id"),
        col("num_faces"),
        col("num_connected_components"),
        col("num_boundary_edges"),
        col("solid"),
    );
    let mut ids: Vec<u64> = lines
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let n: u64 = f.get(faces)?.parse().ok()?;
            (f.get(solid)? == &"1"
                && f.get(shells)? == &"1"
                && f.get(border)? == &"0"
                && (500..=50_000).contains(&n))
            .then(|| f.get(id)?.parse().ok())
            .flatten()
        })
        .collect();
    ids.sort_unstable();
    let mut out = Vec::new();
    for id in ids {
        if out.len() == 50 {
            break;
        }
        let path = root
            .join("Thingi10K")
            .join("raw_meshes")
            .join(format!("{id}.stl"));
        let Ok(imported) = tricky3d_lib::formats::import_file(
            &path,
            true,
            &mut tricky3d_lib::progress::Progress::silent(),
        ) else {
            continue;
        };
        let Some(part) = imported.parts.into_iter().next() else {
            continue;
        };
        let w = World::new(&part.mesh, &IDENTITY_M);
        let Some(b) = w.bounds() else {
            continue;
        };
        let longest = (0..3).map(|k| b[1][k] - b[0][k]).fold(0.0, f64::max);
        if longest <= 0.0 {
            continue;
        }
        let s = (20.0 + (id % 41) as f64) / longest;
        let mut m = [0.0; 16];
        m[0] = s;
        m[5] = s;
        m[10] = s;
        m[15] = 1.0;
        out.push((id.to_string(), part.mesh, m));
    }
    out
}

/// Brief 16: packs the 50 Thingi10K models of `pack_report` with one method, then runs Collision
/// detection (the Analyze tab's check, minimum distance 0) on the result: it must find nothing.
/// Same environment as `pack_report`, run through scripts/corpus/capped.ps1.
#[test]
#[ignore = "needs the model corpus; run through scripts/corpus/capped.ps1"]
fn collision_check_of_a_packed_platform() {
    let parts = thingi_models();
    let method: usize = std::env::var("TRICKY3D_PACK_METHOD").map_or(0, |m| m.parse().unwrap());
    let (name, settings) = methods(false).swap_remove(method);
    let list: Vec<Part> = parts.iter().map(|(_, m, x)| (m, *x, true)).collect();
    let meta = run(
        request(1, "pack", json!({"pack": settings}), &list),
        &RepairJobs::default(),
    )
    .unwrap();
    let packed = matrices(&meta, "matrices");
    let fits: Vec<bool> = serde_json::from_value(meta["fits"].clone()).unwrap();
    let mut out = Vec::new();
    for (((_, mesh, _), m), fit) in parts.iter().zip(&packed).zip(&fits) {
        if *fit {
            let mut info = Map::new();
            info.insert("name".into(), json!("part"));
            info.insert("matrix".into(), json!(m.to_vec()));
            out.push(OutMesh {
                info,
                positions: &mesh.positions,
                indices: &mesh.indices,
                edges: None,
            });
        }
    }
    let mut check = Map::new();
    check.insert("job".into(), json!(2));
    check.insert("kind".into(), json!("collisions"));
    check.insert("settings".into(), json!({"minDistance": 0.0}));
    let start = Instant::now();
    let reply = decode(
        &jobs::analyze_part(
            decode(&encode(&check, &out)).unwrap(),
            &RepairJobs::default(),
            &mut |_| {},
        )
        .unwrap(),
    )
    .unwrap();
    let found = reply.meta["result"]["pairs"].as_array().unwrap().len();
    println!(
        "COLLISIONS|{name}|{} parts|{found} pairs|{:.2} s",
        out.len(),
        start.elapsed().as_secs_f64()
    );
    assert_eq!(found, 0);
}
