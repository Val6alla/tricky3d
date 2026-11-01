//! Modify's mesh tools end to end, through the job the `modify_run` command runs: the answer's
//! counts, mesh and origin, refusals, cancel, and the big-model timings in the report.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Blob, BlobRef, Bundle, OutMesh, decode, encode_with_extra};
use tricky3d_lib::jobs::{self, CANCELLED, RepairJobs};
use tricky3d_lib::mesh::{Mesh, stats};

fn request(job: u64, tool: Value, mesh: &Mesh, selection: &[u32]) -> Bundle {
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("tool".into(), tool);
    let out = OutMesh {
        info: Map::new(),
        positions: &mesh.positions,
        indices: &mesh.indices,
        edges: None,
    };
    decode(&encode_with_extra(
        &meta,
        &[out],
        &[vec![("selection", BlobRef::U32(selection))]],
    ))
    .unwrap()
}

fn run(bundle: Bundle, jobs: &RepairJobs) -> Result<Bundle, String> {
    jobs::modify_run(bundle, jobs, &mut |_| {}).map(|bytes| decode(&bytes).unwrap())
}

fn sphere(r: f64, segments: u32, rings: u32) -> Mesh {
    let params = json!({"radius": r, "segments": segments, "rings": rings});
    tricky3d_lib::library::generate("sphere", params.as_object().unwrap()).unwrap()
}

fn reduce(target: u64) -> Value {
    json!({"tool": "reduce", "target": target, "lowest": false, "maxDeviation": 0.05,
           "maxEdge": 100.0, "selectedOnly": false, "fast": false})
}

#[test]
fn a_reduction_answers_its_counts_mesh_and_origin() {
    let jobs = RepairJobs::default();
    // 25,280 triangles: a quarter of them still keeps within 0.05 mm of a 20 mm sphere.
    let ball = sphere(20.0, 160, 80);
    let n = ball.triangle_count();
    let reply = run(request(1, reduce(n as u64 / 4), &ball, &[]), &jobs).unwrap();
    assert_eq!(reply.meta["changed"], json!(true));
    assert_eq!(reply.meta["before"], json!(n));
    let after = reply.meta["after"].as_u64().unwrap() as usize;
    assert!(after <= n / 4, "{after}");
    let out = &reply.meshes[0];
    assert_eq!(out.mesh.triangle_count(), after);
    assert!(out.info.get("stats").is_some());
    let Some(Blob::U32(origin)) = out.extra("origin") else {
        panic!("no origin");
    };
    assert_eq!(origin.len(), after);
    assert!(
        reply.meta["message"]
            .as_str()
            .unwrap()
            .starts_with("Reduced")
    );
    assert!(jobs.is_idle());
}

#[test]
fn smoothing_and_refining_answer_through_the_job_and_invert_turns_the_part() {
    let jobs = RepairJobs::default();
    let cube = cube(20.0);
    let smooth = json!({"tool": "smooth", "strength": 0.5, "iterations": 3, "keepEdges": true,
                        "edgeAngle": 30.0, "selectedOnly": false, "keepVolume": true});
    // A 12-triangle cube has only corners: nothing may move.
    let reply = run(request(1, smooth, &cube, &[]), &jobs).unwrap();
    assert_eq!(reply.meta["changed"], json!(false));
    assert_eq!(reply.meshes[0].mesh.triangle_count(), 0);
    let refine = json!({"tool": "refine", "maxEdge": 5.0, "selectedOnly": false});
    let reply = run(request(2, refine, &cube, &[]), &jobs).unwrap();
    assert_eq!(reply.meta["changed"], json!(true));
    assert!((stats(&reply.meshes[0].mesh).volume - 8000.0).abs() < 1e-6);
    let reply = run(request(3, json!({"tool": "invert"}), &cube, &[]), &jobs).unwrap();
    assert_eq!(stats(&reply.meshes[0].mesh).volume, -8000.0);
    assert!(reply.meshes[0].extra("origin").is_none());
}

#[test]
fn remesh_z_compensation_extrude_and_offsets_answer_through_the_job() {
    let jobs = RepairJobs::default();
    let ball = sphere(10.0, 48, 24);
    let remesh = json!({"tool": "remesh", "method": "uniform", "edge": 1.5, "largeEdge": 6.0,
                        "smoothing": 0.0, "keepEdges": true, "edgeAngle": 40.0, "fast": false,
                        "selectedOnly": false});
    let reply = run(request(1, remesh, &ball, &[]), &jobs).unwrap();
    assert_eq!(reply.meta["changed"], json!(true));
    let Some(Blob::U32(origin)) = reply.meshes[0].extra("origin") else {
        panic!("no origin");
    };
    // Every triangle is new: a selection does not carry over.
    assert!(origin.iter().all(|&t| t == u32::MAX));
    let cube = cube(20.0);
    let zcomp = json!({"tool": "z-compensation", "distance": 0.2, "angle": 20.0});
    let reply = run(request(2, zcomp, &cube, &[]), &jobs).unwrap();
    let b = tricky3d_lib::mesh::bounds(&reply.meshes[0].mesh).unwrap();
    assert_eq!(b[0][2], 0.2, "{b:?}");
    // The top two triangles of the cube.
    let top: Vec<u32> = (0..cube.triangle_count() as u32)
        .filter(|&t| {
            cube.indices[3 * t as usize..3 * t as usize + 3]
                .iter()
                .all(|&v| cube.vertex(v)[2] == b[1][2])
        })
        .collect();
    assert_eq!(top.len(), 2);
    let extrude = json!({"tool": "extrude", "distance": 5.0, "direction": "normal", "walls": true});
    let reply = run(request(3, extrude, &cube, &top), &jobs).unwrap();
    assert!((stats(&reply.meshes[0].mesh).volume - 20.0 * 20.0 * 25.0).abs() < 1e-6);
    let offset = json!({"tool": "offset-selection", "distance": 1.0, "transition": 0.0});
    let reply = run(request(4, offset, &cube, &top), &jobs).unwrap();
    assert!((stats(&reply.meshes[0].mesh).volume - 20.0 * 20.0 * 21.0).abs() < 1e-6);
    let mut open = cube.clone();
    open.indices.truncate(open.indices.len() - 6);
    let holes = json!({"tool": "offset-holes", "distance": 1.0, "selectedOnly": false});
    let reply = run(request(5, holes, &open, &[]), &jobs).unwrap();
    // One face open (10 triangles left): a band of 2 triangles along each of its 4 edges.
    assert_eq!(reply.meshes[0].mesh.triangle_count(), 10 + 8);
    // Extrude needs a selection.
    let extrude = json!({"tool": "extrude", "distance": 5.0, "direction": "z", "walls": false});
    assert!(
        run(request(6, extrude, &cube, &[]), &jobs)
            .unwrap_err()
            .starts_with("Select the triangles")
    );
    assert!(jobs.is_idle());
}

#[test]
fn bad_requests_are_refused_with_a_sentence() {
    let jobs = RepairJobs::default();
    let cube = cube(20.0);
    // Selected triangles only with nothing selected.
    let only = json!({"tool": "refine", "maxEdge": 5.0, "selectedOnly": true});
    let err = run(request(1, only, &cube, &[]), &jobs).unwrap_err();
    assert!(
        err.starts_with("Select the triangles to refine first"),
        "{err}"
    );
    // A selection that does not fit.
    let refine = json!({"tool": "refine", "maxEdge": 5.0, "selectedOnly": true});
    assert!(run(request(2, refine, &cube, &[40]), &jobs).is_err());
    // Settings out of range, and a tool that does not exist.
    let bad = json!({"tool": "reduce", "target": 10, "lowest": false, "maxDeviation": 0.0,
                     "maxEdge": 10.0, "selectedOnly": false, "fast": false});
    assert!(
        run(request(3, bad, &cube, &[]), &jobs)
            .unwrap_err()
            .contains("Max. deformation")
    );
    assert!(run(request(4, json!({"tool": "melt"}), &cube, &[]), &jobs).is_err());
    assert!(jobs.is_idle());
}

#[test]
fn a_cancel_before_or_during_the_run_leaves_nothing_behind() {
    let jobs = RepairJobs::default();
    let ball = sphere(20.0, 200, 100);
    jobs.cancel(1);
    assert_eq!(
        run(request(1, reduce(1000), &ball, &[]), &jobs).unwrap_err(),
        CANCELLED
    );
    // Cancelled from the first progress event on.
    let stop = AtomicBool::new(false);
    let result = jobs::modify_run(request(2, reduce(1000), &ball, &[]), &jobs, &mut |_| {
        if !stop.swap(true, Ordering::Relaxed) {
            jobs.cancel(2);
        }
    });
    assert_eq!(result.unwrap_err(), CANCELLED);
    assert!(jobs.is_idle());
}

/// The big-model timings in the report: one tool on one model file, through the job the app
/// runs. Run in release through scripts/corpus/capped.ps1:
///   TRICKY3D_MODEL=path TRICKY3D_TOOL=reduce|smooth|remesh [TRICKY3D_REPAIR=extended]
///   cargo test --release --test modify big_model_report -- --ignored --nocapture
/// With TRICKY3D_MODEL=sphere it makes a sphere of about a million triangles instead and checks
/// the reduction to a tenth against mesh compare.
#[test]
#[ignore = "needs a big model; run through scripts/corpus/capped.ps1"]
fn big_model_report() {
    use std::time::Instant;
    let model = std::env::var("TRICKY3D_MODEL").expect("TRICKY3D_MODEL");
    let tool = std::env::var("TRICKY3D_TOOL").unwrap_or_else(|_| "reduce".into());
    let jobs = RepairJobs::default();
    let started = Instant::now();
    let mesh = if model == "sphere" {
        sphere(20.0, 1000, 500)
    } else {
        let imported = tricky3d_lib::formats::import_file(
            std::path::Path::new(&model),
            true,
            &mut tricky3d_lib::progress::Progress::silent(),
        )
        .expect("import");
        let meshes: Vec<&Mesh> = imported.parts.iter().map(|p| &p.mesh).collect();
        tricky3d_lib::ops::concat(&meshes).unwrap()
    };
    println!(
        "read {} triangles in {:.2} s",
        mesh.triangle_count(),
        started.elapsed().as_secs_f64()
    );
    let mesh = match std::env::var("TRICKY3D_REPAIR").as_deref() {
        Ok("extended") => {
            let t = Instant::now();
            let repaired = repair_extended(&mesh);
            println!(
                "extended repair: {} triangles in {:.2} s",
                repaired.triangle_count(),
                t.elapsed().as_secs_f64()
            );
            repaired
        }
        _ => mesh,
    };
    let n = mesh.triangle_count() as u64;
    let settings = match tool.as_str() {
        "reduce" if model == "sphere" => reduce(n / 10),
        "reduce" => {
            json!({"tool": "reduce", "target": n / 2, "lowest": false, "maxDeviation": 0.05,
                           "maxEdge": 100.0, "selectedOnly": false, "fast": false})
        }
        "smooth" => json!({"tool": "smooth", "strength": 0.5, "iterations": 10, "keepEdges": true,
                           "edgeAngle": 40.0, "selectedOnly": false, "keepVolume": true}),
        other => remesh_settings(other, &mesh),
    };
    let t = Instant::now();
    let reply = run(request(1, settings, &mesh, &[]), &jobs);
    let seconds = t.elapsed().as_secs_f64();
    match reply {
        Ok(reply) => {
            let out = &reply.meshes[0].mesh;
            println!(
                "{tool}: {} → {} triangles in {seconds:.2} s; {}",
                n, reply.meta["after"], reply.meta["message"]
            );
            if model == "sphere" {
                let d = deviation(&mesh, out);
                let (v0, v1) = (stats(&mesh).volume, stats(out).volume);
                println!("deviation {d:.5} mm; volume {v0:.2} → {v1:.2} mm³");
                assert!(d <= 0.05, "deviation {d}");
            }
        }
        Err(e) => println!("{tool}: refused after {seconds:.2} s: {e}"),
    }
}

/// The settings for a remesh method, with the edge length that keeps about as many triangles
/// (an equilateral triangle of side L covers √3/4 L²), or TRICKY3D_EDGE.
fn remesh_settings(method: &str, mesh: &Mesh) -> Value {
    let s = stats(mesh);
    let edge = std::env::var("TRICKY3D_EDGE")
        .ok()
        .and_then(|e| e.parse::<f64>().ok())
        .unwrap_or_else(|| (4.0 * s.area / (3f64.sqrt() * s.triangles as f64)).sqrt());
    println!("edge length {edge:.3} mm");
    json!({"tool": "remesh", "method": method, "edge": edge, "largeEdge": 4.0 * edge,
           "smoothing": 0.0, "keepEdges": true, "edgeAngle": 40.0, "fast": false,
           "selectedOnly": false})
}

/// The part after Extended repair, run as the window runs it (src-tauri/tests/shell_models.rs).
fn repair_extended(mesh: &Mesh) -> Mesh {
    use tricky3d_lib::repair::defaults::{EXTENDED_ID, builtin_script};
    let steps = serde_json::to_value(&builtin_script(EXTENDED_ID).unwrap().actions).unwrap();
    let mut meta = Map::new();
    meta.insert("job".into(), json!(1u64));
    meta.insert("label".into(), json!("extended"));
    meta.insert("actions".into(), steps);
    meta.insert("withStatus".into(), json!(false));
    let out = [OutMesh {
        info: Map::new(),
        positions: &mesh.positions,
        indices: &mesh.indices,
        edges: None,
    }];
    let request = decode(&encode_with_extra(
        &meta,
        &out,
        &[vec![("selection", BlobRef::U32(&[]))]],
    ))
    .unwrap();
    let answer =
        decode(&jobs::repair_run(request, &RepairJobs::default(), &mut |_| {}).unwrap()).unwrap();
    if answer.meta["changed"].as_bool() == Some(true) {
        answer.meshes.into_iter().next().unwrap().mesh
    } else {
        mesh.clone()
    }
}

fn deviation(a: &Mesh, b: &Mesh) -> f64 {
    use tricky3d_lib::analysis::compare::{CompareSettings, Direction, compare};
    use tricky3d_lib::arrange::World;
    use tricky3d_lib::mesh::IDENTITY;
    let s = CompareSettings {
        direction: Direction::Both,
        align: false,
    };
    let c = tricky3d_lib::repair::silent(|c| {
        compare(&World::new(a, &IDENTITY), &World::new(b, &IDENTITY), &s, c)
    })
    .unwrap();
    c.forward
        .unwrap()
        .0
        .max_unsigned
        .max(c.backward.unwrap().0.max_unsigned)
}
