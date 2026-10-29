//! Generate shell end to end, through the job the `shell_generate` command runs: parts placed by
//! their matrices, one result per part with progress over all of them, parts that are not closed
//! refused before anything runs, cancel at any point, and a hollow part saved in a project and
//! opened again.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Bundle, OutMesh, decode, encode};
use tricky3d_lib::jobs::{self, CANCELLED, RepairJobs, ShellProgress};
use tricky3d_lib::mesh::{IDENTITY, Matrix, Mesh, bake, bounds, stats};
use tricky3d_lib::progress::Progress;
use tricky3d_lib::project::{self, PartOut};

fn request(settings: Value, job: u64, parts: &[(&str, &Mesh, Matrix)]) -> Bundle {
    let out: Vec<OutMesh> = parts
        .iter()
        .map(|&(name, mesh, matrix)| {
            let mut info = Map::new();
            info.insert("name".into(), json!(name));
            info.insert("matrix".into(), json!(matrix.to_vec()));
            OutMesh {
                info,
                positions: &mesh.positions,
                indices: &mesh.indices,
                edges: None,
            }
        })
        .collect();
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("settings".into(), settings);
    decode(&encode(&meta, &out)).unwrap()
}

fn hollow(thickness: f64, accuracy: f64) -> Value {
    json!({"kind": "hollow-part", "thickness": thickness, "accuracy": accuracy, "smooth": false})
}

/// A result mesh with its info.
type Made = (Map<String, Value>, Mesh);

/// The answer's meta, and each mesh with its info.
fn answer(bytes: &[u8]) -> (Map<String, Value>, Vec<Made>) {
    let b = decode(bytes).unwrap();
    (
        b.meta,
        b.meshes.into_iter().map(|m| (m.info, m.mesh)).collect(),
    )
}

fn run(bundle: Bundle, jobs: &RepairJobs) -> Result<Vec<u8>, String> {
    jobs::shell_generate(bundle, jobs, &mut |_| {})
}

#[test]
fn a_moved_and_turned_part_gets_its_shell_over_it_in_world_space() {
    let jobs = RepairJobs::default();
    let part = cube(20.0);
    let placed = trs([60.0, 40.0, 10.0], 30.0, 0.0, [1.0; 3]);
    let bytes = run(
        request(hollow(2.0, 0.5), 1, &[("Box", &part, placed)]),
        &jobs,
    )
    .unwrap();
    let (meta, meshes) = answer(&bytes);
    assert_eq!(
        meta["results"],
        json!([{"source": 0, "made": true, "warnings": []}])
    );
    assert_eq!(meshes.len(), 1);
    let (info, mesh) = &meshes[0];
    assert_eq!(info["source"], json!(0));
    // Measured like an import: the stats the info panel shows.
    assert_eq!(
        info["stats"]["volume"].as_f64().unwrap(),
        stats(mesh).volume
    );
    assert_eq!(info["stats"]["shells"], json!(2));
    // The cavity's cut-off edges make it weigh a little more than 8000 − 16³.
    assert_close(stats(mesh).volume, 3904.0 + 4.0, 5.0, "volume");
    // Exactly over the original: its outer surface is the original's, placed.
    let world = bake(&part, &placed);
    assert_eq!(bounds(mesh), bounds(&world));
    assert!(jobs.is_idle());
}

#[test]
fn several_parts_give_one_result_each_with_progress_over_all() {
    let jobs = RepairJobs::default();
    let (a, b) = (cube(20.0), cube(10.0));
    let moved = trs([40.0, 0.0, 0.0], 0.0, 0.0, [1.0; 3]);
    let mut events: Vec<ShellProgress> = Vec::new();
    let outer = json!({"kind": "outer-offset", "thickness": 1.0, "accuracy": 0.5, "smooth": true});
    let bytes = jobs::shell_generate(
        request(outer, 2, &[("A", &a, IDENTITY), ("B", &b, moved)]),
        &jobs,
        &mut |p| events.push(p.clone()),
    )
    .unwrap();
    let (meta, meshes) = answer(&bytes);
    assert_eq!(meta["results"].as_array().unwrap().len(), 2);
    assert_eq!(
        meshes
            .iter()
            .map(|(i, _)| i["source"].clone())
            .collect::<Vec<_>>(),
        [json!(0), json!(1)]
    );
    // Thicker by the thickness all round (smoothing may take a little off the outermost points).
    let grown = bounds(&meshes[1].1).unwrap();
    assert_close(
        grown[0][0] as f64,
        39.0,
        0.15,
        "outer offset reaches 1 mm out",
    );
    assert_close(
        grown[1][2] as f64,
        11.0,
        0.15,
        "outer offset reaches 1 mm up",
    );
    assert!(events.iter().all(|e| e.job == 2 && e.parts == 2));
    assert!(events.windows(2).all(|w| w[1].fraction >= w[0].fraction));
    assert_eq!(events.first().unwrap().part, 0);
    assert_eq!(events.last().unwrap().part, 1);
    for stage in [
        "Checking the part",
        "Finding the inside",
        "Measuring distances",
        "Building the surface",
        "Smoothing",
        "Reducing triangles",
        "Checking the result",
    ] {
        assert!(
            events.iter().any(|e| e.stage.as_deref() == Some(stage)),
            "{stage}"
        );
    }
}

#[test]
fn a_part_that_is_not_closed_is_refused_before_anything_runs() {
    let jobs = RepairJobs::default();
    let mut open = cube(20.0);
    open.indices.truncate(open.indices.len() - 3);
    let mut events = 0;
    let bytes = jobs::shell_generate(
        request(
            hollow(2.0, 0.5),
            3,
            &[("Good", &cube(20.0), IDENTITY), ("Open", &open, IDENTITY)],
        ),
        &jobs,
        &mut |_| events += 1,
    )
    .unwrap();
    let (meta, meshes) = answer(&bytes);
    assert!(meshes.is_empty());
    assert!(meta.get("results").is_none());
    let refused = meta["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["source"], json!(1));
    assert_eq!(refused[0]["closed"], json!(false));
    assert_eq!(refused[0]["holes"], json!(1));
    // Inside out counts too.
    let mut inside_out = cube(20.0);
    for t in inside_out.indices.chunks_mut(3) {
        t.swap(1, 2);
    }
    let (meta, _) = answer(
        &run(
            request(hollow(2.0, 0.5), 4, &[("Turned", &inside_out, IDENTITY)]),
            &jobs,
        )
        .unwrap(),
    );
    assert_eq!(meta["refused"][0]["oriented"], json!(false));
    assert!(jobs.is_idle());
}

#[test]
fn a_cancel_stops_it_with_cancelled_at_any_step() {
    let jobs = RepairJobs::default();
    let part = cube(20.0);
    // Before it starts.
    jobs.cancel(100);
    let early = run(
        request(hollow(2.0, 0.5), 100, &[("A", &part, IDENTITY)]),
        &jobs,
    );
    assert_eq!(early, Err(CANCELLED.to_string()));
    // When each step starts.
    for (n, stage) in [
        "Checking the part",
        "Finding the inside",
        "Measuring distances",
        "Building the surface",
        "Reducing triangles",
        "Checking the result",
    ]
    .into_iter()
    .enumerate()
    {
        let job = 200 + n as u64;
        let fired = AtomicBool::new(false);
        let result = jobs::shell_generate(
            request(hollow(2.0, 0.5), job, &[("A", &part, IDENTITY)]),
            &jobs,
            &mut |p| {
                if p.stage.as_deref() == Some(stage) && !fired.swap(true, Ordering::Relaxed) {
                    jobs.cancel(job);
                }
            },
        );
        assert!(fired.load(Ordering::Relaxed), "{stage} was never reported");
        assert_eq!(result, Err(CANCELLED.to_string()), "cancelled at {stage}");
        assert!(jobs.is_idle());
    }
}

#[test]
fn settings_out_of_range_fail_with_a_sentence() {
    let jobs = RepairJobs::default();
    let bad = json!({"kind": "inner-offset", "thickness": 2.0, "accuracy": 1.5, "smooth": false});
    let result = run(request(bad, 5, &[("A", &cube(20.0), IDENTITY)]), &jobs);
    assert_eq!(
        result,
        Err("Enter an accuracy of at most half the thickness (1 mm).".to_string())
    );
    let unknown = json!({"kind": "melt", "thickness": 2.0, "accuracy": 0.5, "smooth": false});
    let result = run(request(unknown, 6, &[("A", &cube(20.0), IDENTITY)]), &jobs);
    assert_eq!(
        result,
        Err("Could not generate the shell: the request from the window is damaged.".to_string())
    );
}

/// The triangles as corner positions, rotated to start at the least, sorted.
fn triangles(mesh: &Mesh) -> Vec<[[u32; 3]; 3]> {
    let mut out: Vec<[[u32; 3]; 3]> = mesh
        .indices
        .chunks(3)
        .map(|t| {
            let c = [0, 1, 2].map(|k| mesh.vertex(t[k]).map(f32::to_bits));
            let first = (0..3).min_by_key(|&k| c[k]).unwrap();
            [c[first], c[(first + 1) % 3], c[(first + 2) % 3]]
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_hollow_part_saves_and_opens_again_as_3mf() {
    let jobs = RepairJobs::default();
    let improved =
        json!({"kind": "hollow-part-improved", "thickness": 2.0, "accuracy": 0.5, "smooth": false});
    let (_, meshes) = answer(
        &run(
            request(improved, 7, &[("Box", &cube(20.0), IDENTITY)]),
            &jobs,
        )
        .unwrap(),
    );
    let mesh = &meshes[0].1;
    let dir = temp_dir("shell-3mf");
    let file = dir.join("hollow.3mf");
    let part = PartOut {
        name: "Box (hollow)",
        matrix: IDENTITY,
        color: Some("#a0b6a9"),
        visible: true,
        transparent: false,
        locked: false,
        measurements: None,
        source: None,
        mesh,
    };
    project::save_file(&file, [200.0; 3], &[part]).unwrap();
    let reopened = project::open_file(&file, &mut Progress::silent()).unwrap();
    assert_eq!(reopened.parts.len(), 1);
    assert_eq!(reopened.parts[0].name, "Box (hollow)");
    let back = &reopened.parts[0].mesh;
    assert_eq!(triangles(back), triangles(mesh));
    let (s, t) = (stats(back), stats(mesh));
    assert_eq!(s.shells, 2);
    assert_eq!(s.volume, t.volume);
    assert_close(s.volume, 3904.0, 0.5, "improved hollow cube");
}
