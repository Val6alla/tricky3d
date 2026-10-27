//! Repair mode's IPC work in jobs.rs, end to end and as the window calls it: repair_run
//! with Sanitize and the selection edits (selections, reports, unchanged runs, bad requests,
//! cancelling, progress), repair_check, repair_shells, the catalog, the scripts file, and script
//! import and export, including the hand-written `.repair` fixture of Extended repair.

mod common;

use std::path::Path;
use std::time::Instant;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Blob, BlobRef, Bundle, OutMesh, decode, encode_with_extra};
use tricky3d_lib::formats::import_file;
use tricky3d_lib::jobs::{self, RepairJobs, RepairProgress};
use tricky3d_lib::mesh::{Mesh, make_box, stats};
use tricky3d_lib::ops::concat;
use tricky3d_lib::progress::Progress;
use tricky3d_lib::repair::defaults::{self, EXTENDED_ID, builtin_script};
use tricky3d_lib::repair::scripts::{self, Script, Step, StoredScripts, parse_action};

// ---------------------------------------------------------------------------------------------
// Helpers

fn cube() -> Mesh {
    make_box([0.0; 3], [20.0; 3])
}

/// The cube without its -z face (triangles 0 and 1): one hole of four edges.
fn open_cube() -> Mesh {
    let mut mesh = cube();
    mesh.indices.drain(0..6);
    mesh
}

/// Every triangle with its own three vertices, as an STL reader that does not weld gives it.
fn soup(mesh: &Mesh) -> Mesh {
    let mut positions = Vec::new();
    for &i in &mesh.indices {
        positions.extend_from_slice(&mesh.vertex(i));
    }
    Mesh {
        positions,
        indices: (0..mesh.indices.len() as u32).collect(),
    }
}

fn step(action: &str, params: Value) -> Value {
    json!({ "type": action, "enabled": true, "params": params })
}

fn sanitize() -> Value {
    step("sanitize_duplicate_nodes", json!({}))
}

/// A bundle as the window encodes it, read back the way the command reads it.
fn through_the_window(
    meta: Map<String, Value>,
    meshes: &[&Mesh],
    extra: &[Vec<(&str, BlobRef)>],
) -> Bundle {
    let out: Vec<OutMesh> = meshes
        .iter()
        .map(|m| OutMesh {
            info: Map::new(),
            positions: &m.positions,
            indices: &m.indices,
            edges: None,
        })
        .collect();
    decode(&encode_with_extra(&meta, &out, extra)).unwrap()
}

fn run_meta(job: u64, label: &str, steps: Value, with_status: bool) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("label".into(), json!(label));
    meta.insert("actions".into(), steps);
    meta.insert("withStatus".into(), json!(with_status));
    meta
}

/// The repair_run request src/repair/ipc.ts sends.
fn request(
    job: u64,
    label: &str,
    steps: Value,
    with_status: bool,
    mesh: &Mesh,
    selection: &[u32],
) -> Bundle {
    through_the_window(
        run_meta(job, label, steps, with_status),
        &[mesh],
        &[vec![("selection", BlobRef::U32(selection))]],
    )
}

/// A repair_run answer: the raw header (to see which fields were sent), the meta, mesh 0 and its
/// extra arrays.
struct Answer {
    header: Value,
    meta: Map<String, Value>,
    mesh: Mesh,
    extra: Vec<(String, Blob)>,
}

impl Answer {
    fn read(bytes: &[u8]) -> Answer {
        let length = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&bytes[4..4 + length]).unwrap();
        let mut bundle = decode(bytes).unwrap();
        assert_eq!(bundle.meshes.len(), 1);
        let m = bundle.meshes.remove(0);
        Answer {
            header,
            meta: bundle.meta,
            mesh: m.mesh,
            extra: m.extra,
        }
    }

    fn entry(&self) -> &Value {
        &self.header["meshes"][0]
    }

    fn blob(&self, name: &str) -> Option<&Blob> {
        self.extra.iter().find(|(n, _)| n == name).map(|(_, b)| b)
    }

    fn u32s(&self, name: &str) -> Vec<u32> {
        match self.blob(name) {
            Some(Blob::U32(v)) => v.clone(),
            other => panic!("{name} should be u32 values, got {other:?}"),
        }
    }

    fn f32s(&self, name: &str) -> Vec<f32> {
        match self.blob(name) {
            Some(Blob::F32(v)) => v.clone(),
            other => panic!("{name} should be f32 values, got {other:?}"),
        }
    }

    fn reports(&self) -> &Vec<Value> {
        self.meta["reports"].as_array().unwrap()
    }

    fn messages(&self) -> Vec<&str> {
        self.reports()
            .iter()
            .map(|r| r["message"].as_str().unwrap())
            .collect()
    }
}

/// Runs a request as the command does, with the progress events it sent.
fn run(jobs: &RepairJobs, bundle: Bundle) -> (Result<Answer, String>, Vec<RepairProgress>) {
    let mut events = Vec::new();
    let result = jobs::repair_run(bundle, jobs, &mut |p| events.push(p.clone()));
    (result.map(|bytes| Answer::read(&bytes)), events)
}

fn run_ok(jobs: &RepairJobs, bundle: Bundle) -> Answer {
    run(jobs, bundle).0.unwrap()
}

fn run_err(jobs: &RepairJobs, bundle: Bundle) -> String {
    match run(jobs, bundle).0 {
        Ok(_) => panic!("the run should fail"),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------------------------
// repair_run

#[test]
fn a_changed_run_sends_the_new_mesh_with_stats_edges_reports_and_selection() {
    let jobs = RepairJobs::default();
    let (result, events) = run(
        &jobs,
        request(
            1,
            "Sanitize duplicate nodes",
            json!([sanitize()]),
            false,
            &soup(&cube()),
            &[3, 7],
        ),
    );
    let a = result.unwrap();
    assert_eq!(a.meta["changed"], true);
    assert_eq!(a.meta["status"], Value::Null);
    assert_eq!(a.meta["warnings"], json!([]));
    let report = &a.reports()[0];
    assert_eq!(a.reports().len(), 1);
    assert_eq!(report["action"], "sanitize_duplicate_nodes");
    assert_eq!(report["label"], "Sanitize duplicate nodes");
    assert_eq!(report["message"], "Merged 28 nodes.");
    assert_eq!(report["warnings"], json!([]));
    assert_eq!(report["changed"], true);
    assert!(report["millis"].is_u64());
    // The welded cube, with its stats and (no) problem edges for meshCache.
    assert_eq!((a.mesh.vertex_count(), a.mesh.triangle_count()), (8, 12));
    let s = stats(&a.mesh);
    assert_eq!(
        (s.volume, s.open_edges, s.non_manifold_edges),
        (8000.0, 0, 0)
    );
    assert_eq!(a.entry()["stats"]["triangles"], 12);
    assert_eq!(a.entry()["stats"]["vertices"], 8);
    assert_eq!(a.entry()["stats"]["openEdges"], 0);
    assert_eq!(a.entry()["edges"][1], 0);
    // Merging moves no triangle, so the selection stays; nothing else comes back.
    assert_eq!(a.u32s("selection"), vec![3, 7]);
    for name in [
        "border",
        "nonManifold",
        "inverted",
        "degenerate",
        "intersecting",
    ] {
        assert!(a.blob(name).is_none(), "{name}");
    }
    assert_eq!(
        events,
        vec![RepairProgress {
            job: 1,
            step: 0,
            steps: 1,
            label: "Sanitize duplicate nodes".into(),
            detail: None,
            local: 0.0,
            fraction: 0.0,
        }]
    );
    assert!(jobs.is_idle());
}

#[test]
fn an_unchanged_run_sends_no_arrays_and_echoes_the_selection() {
    let jobs = RepairJobs::default();
    let a = run_ok(
        &jobs,
        request(
            2,
            "Sanitize duplicate nodes",
            json!([step(
                "sanitize_duplicate_nodes",
                json!({ "tolerance": 0.001 })
            )]),
            false,
            &cube(),
            &[0, 11],
        ),
    );
    assert_eq!(a.meta["changed"], false);
    assert_eq!(a.mesh, Mesh::default());
    assert!(a.entry().get("stats").is_none());
    assert!(a.entry().get("edges").is_none());
    assert_eq!(a.u32s("selection"), vec![0, 11]);
    assert_eq!(a.messages(), ["No duplicate nodes to merge."]);
    assert_eq!(a.reports()[0]["changed"], false);
}

#[test]
fn removing_the_selection_deletes_it_and_leaves_no_selection() {
    let jobs = RepairJobs::default();
    let a = run_ok(
        &jobs,
        request(
            3,
            "Remove selected triangles",
            json!([step("remove_selected_triangles", json!({}))]),
            false,
            &cube(),
            &[0, 1],
        ),
    );
    assert_eq!(a.meta["changed"], true);
    assert_eq!(a.mesh, open_cube());
    assert_eq!(a.messages(), ["Removed 2 triangles."]);
    assert!(a.u32s("selection").is_empty());
    assert_eq!(a.entry()["stats"]["openEdges"], 4);
    // Four open edges: two corners each.
    assert_eq!(a.entry()["edges"][1], 24);
}

#[test]
fn the_selection_follows_every_step_of_a_run() {
    // The cube, a triangle with a repeated corner (12) and a copy of triangle 1 (13).
    let mut mesh = cube();
    let copy = mesh.indices[3..6].to_vec();
    mesh.indices.extend_from_slice(&[0, 0, 1]);
    mesh.indices.extend_from_slice(&copy);
    let jobs = RepairJobs::default();
    let a = run_ok(
        &jobs,
        request(
            4,
            "Flip, then sanitize",
            json!([step("flip_selected_triangles", json!({})), sanitize()]),
            false,
            &mesh,
            &[1, 12, 13],
        ),
    );
    assert_eq!(
        a.messages(),
        ["Flipped 3 triangles.", "Removed 1 degenerate triangle."]
    );
    // Flipping keeps every triangle; Sanitize drops triangle 12, so the copy becomes 12.
    assert_eq!(a.u32s("selection"), vec![1, 12]);
    assert_eq!(a.mesh.triangle_count(), 13);
    let flipped = [copy[0], copy[2], copy[1]];
    assert_eq!(a.mesh.indices[3..6], flipped);
    assert_eq!(a.mesh.indices[36..39], flipped);
    assert_eq!(a.meta["selected"], false);
}

#[test]
fn a_home_edit_is_one_run_that_leaves_nothing_selected() {
    let jobs = RepairJobs::default();
    let (result, events) = run(
        &jobs,
        request(
            5,
            "Subdivide triangles",
            json!([step("subdivide_triangles", json!({}))]),
            true,
            &cube(),
            &[0, 1],
        ),
    );
    let a = result.unwrap();
    assert_eq!(a.meta["changed"], true);
    assert_eq!(a.meta["selected"], true);
    // The two selected triangles split in four, the four beside them in two.
    assert_eq!(a.messages(), ["Added 10 triangles."]);
    assert_eq!(a.reports()[0]["action"], "subdivide_triangles");
    assert_eq!(a.reports()[0]["label"], "Subdivide triangles");
    assert!(a.u32s("selection").is_empty());
    assert_eq!(a.mesh.triangle_count(), 22);
    assert_eq!(a.meta["status"]["closed"], true);
    assert_eq!(events[0].label, "Subdivide triangles");
    assert!(jobs.is_idle());
}

#[test]
fn a_search_sets_the_selection_and_leaves_the_mesh() {
    let jobs = RepairJobs::default();
    let search = || json!([step("select_problem_triangles", json!({}))]);
    // The open cube's damaged triangles are the four round its hole.
    let a = run_ok(
        &jobs,
        request(6, "Select by criteria", search(), false, &open_cube(), &[9]),
    );
    assert_eq!(a.meta["changed"], false);
    assert_eq!(a.meta["selected"], true);
    assert_eq!(a.mesh, Mesh::default());
    assert_eq!(a.messages(), ["Selected 4 problem triangles."]);
    assert_eq!(a.u32s("selection").len(), 4);
    // Nothing found: the selection comes back as it went.
    let a = run_ok(
        &jobs,
        request(7, "Select by criteria", search(), false, &cube(), &[9]),
    );
    assert_eq!(a.meta["selected"], false);
    assert_eq!(a.u32s("selection"), vec![9]);
    // Find overlapping triangles leaves the crossing ones highlighted too.
    let two = concat(&[&cube(), &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0])]).unwrap();
    let a = run_ok(
        &jobs,
        request(
            8,
            "Find overlapping triangles",
            json!([step("find_overlaps", json!({}))]),
            false,
            &two,
            &[],
        ),
    );
    assert_eq!(a.meta["selected"], true);
    assert!(!a.u32s("intersecting").is_empty());
    assert_eq!(a.u32s("selection"), a.u32s("intersecting"));
}

#[test]
fn only_the_home_searches_and_edits_set_the_selection() {
    // Every other action leaves the selection following its triangles, as before the Home actions
    // existed: run each on a damaged part with a selection and look at `selected`.
    let home = [
        "close_holes_curved",
        "bridge_gap",
        "grow_hole_band",
        "subdivide_triangles",
        "add_triangle",
        "add_point",
        "cut_surface",
        "select_problem_triangles",
        "find_overlaps",
    ];
    let damaged = concat(&[
        &open_cube(),
        &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0]),
    ])
    .unwrap();
    let jobs = RepairJobs::default();
    for (n, spec) in defaults::catalog()
        .actions
        .iter()
        .filter(|a| a.implemented)
        .enumerate()
    {
        let id = json!(spec.id);
        // The two interactive tools need what the window sends them.
        let params = match id.as_str().unwrap() {
            "move_node" => json!({ "vertex": 0, "x": -1.0, "y": -1.0, "z": -1.0 }),
            "cut_surface" => json!({ "points": [] }),
            _ => json!({}),
        };
        let a = run_ok(
            &jobs,
            request(
                100 + n as u64,
                spec.label,
                json!([step(id.as_str().unwrap(), params)]),
                false,
                &damaged,
                &[2, 3],
            ),
        );
        if !home.contains(&id.as_str().unwrap()) {
            assert_eq!(a.meta["selected"], false, "{id}");
        }
    }
}

#[test]
fn the_cut_line_is_routed_on_the_working_copy_sent_once() {
    // A 20 mm cube with its faces split 16 ways: a closed line round the top face, 2 mm in.
    let mut cube = make_box([0.0; 3], [20.0; 3]);
    for _ in 0..2 {
        let out = tricky3d_lib::repair::silent(|c| {
            tricky3d_lib::repair::refine::subdivide(&cube, &[], c)
        })
        .unwrap();
        cube = out.mesh;
    }
    // The top triangle under each point.
    let on_top = |x: f64, y: f64| {
        (0..cube.triangle_count())
            .find(|&t| {
                let c: Vec<[f32; 3]> = (0..3)
                    .map(|k| cube.vertex(cube.indices[t * 3 + k]))
                    .collect();
                c.iter().all(|p| p[2] == 20.0) && {
                    let (xs, ys): (Vec<f64>, Vec<f64>) =
                        c.iter().map(|p| (p[0] as f64, p[1] as f64)).unzip();
                    let s = |a: usize, b: usize| {
                        (xs[b] - xs[a]) * (y - ys[a]) - (ys[b] - ys[a]) * (x - xs[a])
                    };
                    let (d0, d1, d2) = (s(0, 1), s(1, 2), s(2, 0));
                    (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
                }
            })
            .unwrap()
    };
    let points: Vec<Value> = [(2.3, 2.1), (17.6, 2.4), (17.2, 17.9), (2.6, 17.4)]
        .iter()
        .map(|&(x, y)| json!([x, y, 20.0, on_top(x, y)]))
        .collect();
    let meta = |key: u64| {
        let mut m = Map::new();
        m.insert("meshKey".into(), json!(key));
        m.insert("points".into(), json!(points));
        m.insert("closed".into(), json!(true));
        m.insert("snap".into(), json!(0.0));
        m
    };
    let nothing = Mesh {
        positions: Vec::new(),
        indices: Vec::new(),
    };
    let cache = jobs::CutCache::default();
    let read = |bytes: Vec<u8>| Answer::read(&bytes);
    // The first time with the working copy, then with the key alone.
    let a =
        read(jobs::repair_cut_path(through_the_window(meta(7), &[&cube], &[]), &cache).unwrap());
    let pieces: Vec<u64> = a.meta["pieces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_u64().unwrap())
        .collect();
    assert_eq!(pieces.len(), 4);
    assert_eq!(a.f32s("path").len() as u64, 3 * pieces.iter().sum::<u64>());
    assert!(a.f32s("path").chunks(3).all(|p| p[2] == 20.0));
    assert_eq!(a.meta["spans"], json!([[0, 1], [1, 2], [2, 3], [3, 0]]));
    let b =
        read(jobs::repair_cut_path(through_the_window(meta(7), &[&nothing], &[]), &cache).unwrap());
    assert_eq!(b.meta, a.meta);
    assert_eq!(b.f32s("path"), a.f32s("path"));
    // Another key, or after a reset: the working copy is needed.
    let missing =
        |key: u64| jobs::repair_cut_path(through_the_window(meta(key), &[&nothing], &[]), &cache);
    assert_eq!(missing(8).unwrap_err(), jobs::MESH_NEEDED);
    cache.clear();
    assert_eq!(missing(7).unwrap_err(), jobs::MESH_NEEDED);
    // The cut along the same line, as the window runs it, cuts along the line drawn.
    let mut step = Map::new();
    step.insert("type".into(), json!("cut_surface"));
    step.insert("params".into(), Value::Object(meta(7)));
    let jobs_table = RepairJobs::default();
    let cut = run_ok(
        &jobs_table,
        request(1, "Cut surfaces", json!([step]), true, &cube, &[]),
    );
    let message = cut.reports()[0]["message"].as_str().unwrap().to_string();
    assert!(
        message.ends_with("What the line encloses is a shell of its own now."),
        "{message}"
    );
    assert_eq!(
        cut.meta["status"]["shells"],
        json!(2),
        "{:?}",
        cut.meta["status"]
    );
    let edges: u64 = message.split(' ').nth(5).unwrap().parse().unwrap();
    assert_eq!(cut.meta["status"]["borderEdges"], json!(2 * edges));
    assert_eq!(cut.meta["selected"], json!(true));
    assert!(cut.u32s("selection").is_empty());
}

#[test]
fn the_home_actions_stay_out_of_scripts() {
    // In the catalog for their labels and the selection they need, but not in the script
    // editor's dropdown.
    let catalog = defaults::catalog();
    for (id, needs) in [
        ("close_holes_curved", false),
        ("bridge_gap", false),
        ("grow_hole_band", false),
        ("subdivide_triangles", false),
        ("add_triangle", false),
        ("add_point", true),
        ("move_node", false),
        ("cut_surface", false),
        ("select_problem_triangles", false),
        ("find_overlaps", false),
    ] {
        let spec = catalog
            .actions
            .iter()
            .find(|a| json!(a.id) == json!(id))
            .unwrap_or_else(|| panic!("{id}"));
        assert!(spec.implemented && !spec.in_scripts, "{id}");
        assert_eq!(spec.needs_selection, needs, "{id}");
    }
}

#[test]
fn disabled_steps_are_skipped_even_when_they_could_not_run() {
    let jobs = RepairJobs::default();
    let steps = json!([
        { "type": "remesh", "enabled": false },
        sanitize(),
        { "type": "stitch_triangles", "enabled": false, "params": { "tolerance": -1 } },
    ]);
    let (result, events) = run(
        &jobs,
        request(5, "Mixed", steps, false, &soup(&cube()), &[]),
    );
    let a = result.unwrap();
    assert_eq!(a.messages(), ["Merged 28 nodes."]);
    assert!(events.iter().all(|e| e.steps == 1));
}

#[test]
fn a_bad_step_fails_the_run_before_anything_runs() {
    let jobs = RepairJobs::default();
    let cases = [
        (
            json!([
                sanitize(),
                step("stitch_triangles", json!({ "tolerance": -1 }))
            ]),
            "My script",
            "Stitch triangles: the tolerance must be a number of 0 mm or more (step 2 of My script).",
        ),
        (
            json!([sanitize(), step("remesh", json!({}))]),
            "My script",
            "Remesh is not available yet (step 2 of My script).",
        ),
        (
            json!([step("remesh", json!({}))]),
            "Remesh",
            "Remesh is not available yet.",
        ),
        (
            json!([step("remove_tiny_shells", json!({ "mode": "everything" }))]),
            "Remove tiny shells",
            "Remove tiny shells: the tiny-shell detection must be one of the choices tricky3D offers.",
        ),
        (
            json!([sanitize(), { "type": "teleport" }]),
            "My script",
            "Could not run My script: step 2 is \"teleport\", which tricky3D does not know.",
        ),
        (
            json!(["close_all_holes"]),
            "My script",
            "Could not run My script: step 1 is damaged.",
        ),
        (
            json!([{ "type": "stitch_triangles", "params": 5 }]),
            "My script",
            "Could not run My script: step 1 is damaged.",
        ),
    ];
    for (n, (steps, label, want)) in cases.into_iter().enumerate() {
        let (result, events) = run(
            &jobs,
            request(10 + n as u64, label, steps, false, &soup(&cube()), &[]),
        );
        assert_eq!(result.err().as_deref(), Some(want));
        assert!(events.is_empty(), "nothing ran: {events:?}");
    }
    assert!(jobs.is_idle());
}

#[test]
fn damaged_requests_are_refused() {
    let jobs = RepairJobs::default();
    let steps = || json!([sanitize()]);
    let broken = Mesh {
        positions: vec![0.0; 9],
        indices: vec![0, 1, 5],
    };
    assert_eq!(
        run_err(&jobs, request(1, "Fix", steps(), false, &broken, &[])),
        "Could not run Fix: the working copy is damaged (a triangle refers to a vertex that does not exist)."
    );
    let selection_error = "Could not run Fix: the selection does not fit the working copy.";
    for selection in [&[3u32, 1][..], &[1, 1], &[12]] {
        assert_eq!(
            run_err(&jobs, request(1, "Fix", steps(), false, &cube(), selection)),
            selection_error
        );
    }
    let mesh = cube();
    let floats = through_the_window(
        run_meta(1, "Fix", steps(), false),
        &[&mesh],
        &[vec![("selection", BlobRef::F32(&[1.0]))]],
    );
    assert_eq!(run_err(&jobs, floats), selection_error);

    let damaged = "Could not run the repair: the request from the window is damaged.";
    let mut no_job = run_meta(1, "Fix", steps(), false);
    no_job.remove("job");
    let mut negative = run_meta(1, "Fix", steps(), false);
    negative.insert("job".into(), json!(-1));
    let mut not_a_list = run_meta(1, "Fix", steps(), false);
    not_a_list.insert("actions".into(), json!({ "type": "fix_flipped_triangles" }));
    let mut odd_status = run_meta(1, "Fix", steps(), false);
    odd_status.insert("withStatus".into(), json!("yes"));
    for meta in [no_job, negative, not_a_list, odd_status] {
        assert_eq!(
            run_err(&jobs, through_the_window(meta, &[&mesh], &[])),
            damaged
        );
    }
    let two = through_the_window(run_meta(1, "Fix", steps(), false), &[&mesh, &mesh], &[]);
    assert_eq!(run_err(&jobs, two), damaged);
    let none = through_the_window(run_meta(1, "Fix", steps(), false), &[], &[]);
    assert_eq!(run_err(&jobs, none), damaged);

    // Without a label the messages still read as sentences.
    let mut unnamed = run_meta(1, "", steps(), false);
    unnamed.remove("label");
    assert_eq!(
        run_err(&jobs, through_the_window(unnamed, &[&broken], &[])),
        "Could not run the repair: the working copy is damaged (a triangle refers to a vertex that does not exist)."
    );
    assert!(jobs.is_idle());
}

#[test]
fn a_selection_may_be_left_out_and_a_run_may_have_no_actions() {
    let jobs = RepairJobs::default();
    let mesh = cube();
    let bundle = through_the_window(run_meta(30, "Nothing", json!([]), false), &[&mesh], &[]);
    let (result, events) = run(&jobs, bundle);
    let a = result.unwrap();
    assert_eq!(a.meta["changed"], false);
    assert!(a.reports().is_empty());
    assert!(a.u32s("selection").is_empty());
    assert_eq!(a.meta["status"], Value::Null);
    assert!(events.is_empty());
    // Missing "actions" and "withStatus" mean none and false.
    let mut meta = run_meta(31, "Nothing", json!([]), false);
    meta.remove("actions");
    meta.remove("withStatus");
    let a = run_ok(&jobs, through_the_window(meta, &[&mesh], &[]));
    assert_eq!(a.meta["changed"], false);
}

#[test]
fn a_run_cancelled_midway_fails_with_cancelled_and_unregisters() {
    let jobs = RepairJobs::default();
    let bundle = request(
        7,
        "Three",
        json!([sanitize(), sanitize(), sanitize()]),
        false,
        &soup(&cube()),
        &[],
    );
    let mut events = Vec::new();
    let result = jobs::repair_run(bundle, &jobs, &mut |p| {
        events.push(p.clone());
        // The user presses Cancel as the second action starts.
        if p.step == 1 {
            jobs.cancel(7);
        }
    });
    assert_eq!(result.err().as_deref(), Some("cancelled"));
    assert_eq!(events.last().map(|e| e.step), Some(1));
    assert!(jobs.is_idle());
    // The next run is not affected.
    let a = run_ok(
        &jobs,
        request(8, "One", json!([sanitize()]), false, &soup(&cube()), &[]),
    );
    assert_eq!(a.meta["changed"], true);
}

#[test]
fn a_cancel_that_comes_before_its_run_still_cancels_it() {
    let jobs = RepairJobs::default();
    jobs.cancel(9);
    assert!(!jobs.is_idle());
    assert_eq!(
        run_err(
            &jobs,
            request(9, "One", json!([sanitize()]), false, &soup(&cube()), &[])
        ),
        "cancelled"
    );
    assert!(jobs.is_idle());
}

#[test]
fn cancels_of_finished_or_unknown_runs_change_nothing() {
    let jobs = RepairJobs::default();
    let one = || json!([sanitize()]);
    run_ok(&jobs, request(10, "One", one(), false, &soup(&cube()), &[]));
    // After the run ended, and for a number below every job started: ignored.
    jobs.cancel(10);
    jobs.cancel(4);
    assert!(jobs.is_idle());
    let a = run_ok(&jobs, request(4, "One", one(), false, &soup(&cube()), &[]));
    assert_eq!(a.meta["changed"], true);
    // An early cancel whose run never comes is dropped when a later job starts.
    jobs.cancel(20);
    run_ok(&jobs, request(22, "One", one(), false, &soup(&cube()), &[]));
    assert!(jobs.is_idle());
    // Cancelling while nothing runs does not affect the next job.
    let a = run_ok(&jobs, request(23, "One", one(), false, &soup(&cube()), &[]));
    assert_eq!(a.meta["changed"], true);
}

#[test]
fn progress_covers_every_step_over_the_whole_run_and_never_goes_back() {
    let jobs = RepairJobs::default();
    let (result, events) = run(
        &jobs,
        request(
            40,
            "Three",
            json!([
                sanitize(),
                step("flip_selected_triangles", json!({})),
                sanitize()
            ]),
            false,
            &soup(&cube()),
            &[],
        ),
    );
    result.unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e.job == 40 && e.steps == 3));
    assert!(events.windows(2).all(|w| w[1].fraction >= w[0].fraction));
    assert!(events.iter().all(|e| (0.0..=1.0).contains(&e.fraction)));
    for (step, label) in [
        "Sanitize duplicate nodes",
        "Flip selected triangles",
        "Sanitize duplicate nodes",
    ]
    .into_iter()
    .enumerate()
    {
        // Each action's start is always sent, at its share of the run.
        let first = events.iter().find(|e| e.step == step).unwrap();
        assert_eq!(first.label, label);
        assert_eq!(first.fraction, step as f64 / 3.0);
    }
}

#[test]
fn an_update_computes_the_status_and_its_highlights() {
    let jobs = RepairJobs::default();
    let (result, events) = run(
        &jobs,
        request(50, "Update", json!([]), true, &open_cube(), &[2]),
    );
    let a = result.unwrap();
    assert!(events.is_empty());
    assert_eq!(a.meta["changed"], false);
    assert_eq!(a.mesh, Mesh::default());
    let status = &a.meta["status"];
    assert_eq!(status["closed"], false);
    assert_eq!(status["borderEdges"], 4);
    assert_eq!(status["nonManifoldEdges"], 0);
    assert_eq!(status["holes"], 1);
    assert_eq!(status["triangles"], 10);
    assert_eq!(status["shells"], 1);
    assert_eq!(a.u32s("selection"), vec![2]);
    assert_highlights_match(&a);
    // Every border segment lies on the open bottom.
    let border = a.f32s("border");
    assert!(border.chunks(3).all(|p| p[2] == 0.0));

    let a = run_ok(&jobs, request(51, "Update", json!([]), true, &cube(), &[]));
    let status = &a.meta["status"];
    assert_eq!(status["closed"], true);
    assert_eq!(status["oriented"], true);
    assert_eq!(status["borderEdges"], 0);
    assert_eq!(status["holes"], 0);
    assert_highlights_match(&a);
}

/// The highlight arrays agree with the status counts they belong to.
fn assert_highlights_match(a: &Answer) {
    let status = &a.meta["status"];
    let count = |key: &str| status[key].as_u64().unwrap() as usize;
    assert_eq!(a.f32s("border").len(), 6 * count("borderEdges"));
    assert_eq!(a.f32s("nonManifold").len(), 6 * count("nonManifoldEdges"));
    assert_eq!(a.u32s("inverted").len(), count("invertedTriangles"));
    assert_eq!(a.u32s("degenerate").len(), count("degenerateTriangles"));
}

#[test]
fn a_status_after_a_change_describes_the_result() {
    let jobs = RepairJobs::default();
    let a = run_ok(
        &jobs,
        request(
            52,
            "Sanitize",
            json!([sanitize()]),
            true,
            &soup(&cube()),
            &[],
        ),
    );
    assert_eq!(a.meta["changed"], true);
    let status = &a.meta["status"];
    assert_eq!(status["closed"], true);
    assert_eq!(status["triangles"], 12);
    assert_eq!(status["vertices"], 8);
    assert_eq!(status["volume"], 8000.0);
    assert_highlights_match(&a);
}

// ---------------------------------------------------------------------------------------------
// repair_check, repair_shells, repair_catalog

#[test]
fn repair_check_reports_closed_and_holes_per_mesh() {
    let (closed, open) = (cube(), open_cube());
    let checks =
        jobs::repair_check(through_the_window(Map::new(), &[&closed, &open], &[])).unwrap();
    let json = serde_json::to_value(&checks).unwrap();
    assert_eq!(
        json[0],
        json!({ "closed": true, "oriented": true, "holes": 0, "borderEdges": 0,
                "nonManifoldEdges": 0, "invertedTriangles": 0 })
    );
    assert_eq!(json[1]["closed"], false);
    assert_eq!(json[1]["holes"], 1);
    assert_eq!(json[1]["borderEdges"], 4);
    assert_eq!(json[1]["nonManifoldEdges"], 0);
    let broken = Mesh {
        positions: vec![0.0; 3],
        indices: vec![0, 0, 1],
    };
    assert_eq!(
        jobs::repair_check(through_the_window(Map::new(), &[&broken], &[])).unwrap_err(),
        "Could not check the part: its data is damaged (a triangle refers to a vertex that does not exist)."
    );
}

#[test]
fn repair_shells_lists_rows_and_the_shell_of_each_triangle() {
    let small = make_box([50.0, 0.0, 0.0], [60.0, 10.0, 10.0]);
    let both = concat(&[&cube(), &small]).unwrap();
    let bytes = jobs::repair_shells(through_the_window(Map::new(), &[&both], &[])).unwrap();
    let a = Answer::read(&bytes);
    assert_eq!(a.mesh, Mesh::default());
    let rows = a.meta["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for (row, (volume, area)) in rows.iter().zip([(8000.0, 2400.0), (1000.0, 600.0)]) {
        assert_eq!(row["triangles"], 12);
        assert_eq!(row["watertight"], true);
        assert_eq!(row["volume"], volume);
        assert_eq!(row["bboxVolume"], volume);
        assert_eq!(row["area"], area);
    }
    let mut want = vec![0u32; 12];
    want.extend([1u32; 12]);
    assert_eq!(a.u32s("shellOf"), want);
    let mesh = cube();
    assert_eq!(
        jobs::repair_shells(through_the_window(Map::new(), &[&mesh, &mesh], &[])).unwrap_err(),
        "Could not list the shells: the request from the window is damaged."
    );
}

#[test]
fn the_catalog_lists_every_action_and_the_builtin_scripts() {
    let json = serde_json::to_value(jobs::repair_catalog()).unwrap();
    assert_eq!(json["actions"].as_array().unwrap().len(), 36);
    let names: Vec<&str> = json["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Simple repair", "Default repair", "Extended repair"]
    );
    assert_eq!(json["scripts"][2]["builtIn"], true);
    assert_eq!(json["actions"][0]["inScripts"], true);
}

// ---------------------------------------------------------------------------------------------
// The scripts file

fn own_script(id: &str, name: &str) -> Script {
    Script {
        id: id.into(),
        name: name.into(),
        built_in: false,
        actions: vec![
            serde_json::from_value(step("stitch_triangles", json!({ "tolerance": 0.05 }))).unwrap(),
            Step {
                action: tricky3d_lib::repair::ActionId::CloseAllHoles,
                enabled: false,
                params: Map::new(),
            },
        ],
    }
}

#[test]
fn scripts_save_and_load_through_the_config_folder() {
    let dir = temp_dir("repair-scripts");
    assert_eq!(jobs::load_scripts(&dir), Ok(None));
    // Saving creates the folder.
    let config = dir.join("app.tricky3d.desktop");
    let mut builtin = builtin_script(EXTENDED_ID).unwrap();
    builtin.name = "Changed".into();
    let data = StoredScripts {
        scripts: vec![own_script("own.1", "Mine"), builtin],
        order: vec!["own.1".into(), "builtin.simple".into()],
    };
    jobs::save_scripts(&config, &data).unwrap();
    let back = jobs::load_scripts(&config).unwrap().unwrap();
    // Built-in scripts are never stored.
    assert_eq!(back.scripts, vec![own_script("own.1", "Mine")]);
    assert_eq!(back.order, data.order);
    // Saving again replaces the file whole and leaves no temporary file behind.
    let next = StoredScripts {
        scripts: vec![own_script("own.2", "Second ü \"quoted\"")],
        order: Vec::new(),
    };
    jobs::save_scripts(&config, &next).unwrap();
    assert_eq!(jobs::load_scripts(&config).unwrap(), Some(next));
    let files: Vec<String> = std::fs::read_dir(&config)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, [jobs::SCRIPTS_FILE]);
}

#[test]
fn a_damaged_scripts_file_is_reported_not_replaced() {
    let dir = temp_dir("repair-scripts-damaged");
    let file = dir.join(jobs::SCRIPTS_FILE);
    std::fs::write(&file, "{ \"scripts\": [ oops").unwrap();
    let err = jobs::load_scripts(&dir).unwrap_err();
    assert!(
        err.starts_with(
            "Could not read your repair scripts: repair-scripts.json in the settings folder is damaged ("
        ),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{ \"scripts\": [ oops"
    );
    // A script naming an action tricky3D does not know is damage too.
    std::fs::write(
        &file,
        r#"{ "scripts": [{ "id": "own.1", "name": "x", "actions": [{ "type": "teleport" }] }] }"#,
    )
    .unwrap();
    assert!(jobs::load_scripts(&dir).is_err());
}

// ---------------------------------------------------------------------------------------------
// Script files: the reference tool's .repair and our .json

/// Every step with all of its action's parameters, the defaults filling the gaps.
fn normalized(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .map(|s| {
            let mut params = s.params.clone();
            for spec in defaults::params_of(s.action) {
                params
                    .entry(spec.id.to_string())
                    .or_insert(spec.default.clone());
            }
            Step {
                params,
                ..s.clone()
            }
        })
        .collect()
}

fn assert_is_extended_repair(script: &Script) {
    let builtin = builtin_script(EXTENDED_ID).unwrap();
    assert_eq!(script.name, "Extended repair");
    assert!(!script.built_in);
    assert!(script.id.starts_with("own."));
    assert_eq!(normalized(&script.actions), normalized(&builtin.actions));
    assert_eq!(script.actions, builtin.actions);
    let parsed: Vec<_> = script
        .actions
        .iter()
        .map(|s| parse_action(s).unwrap())
        .collect();
    let want: Vec<_> = builtin
        .actions
        .iter()
        .map(|s| parse_action(s).unwrap())
        .collect();
    assert_eq!(parsed, want);
}

#[test]
fn the_extended_repair_fixture_imports_as_the_builtin_script() {
    let path = fixture("repair/extended-repair.repair");
    let imported = jobs::import_script(&path).unwrap();
    assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
    assert_is_extended_repair(&imported.script);

    // The same list with LF and with CRLF line ends, whatever the checkout did to the file.
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    let crlf = text.replace('\n', "\r\n");
    assert!(crlf.contains("</type>\r\n") && !text.contains('\r'));
    for variant in [&text, &crlf] {
        let imported = scripts::import_repair_xml(variant).unwrap();
        assert!(imported.warnings.is_empty());
        assert_is_extended_repair(&imported.script);
    }
}

#[test]
fn scripts_export_and_import_through_files() {
    let dir = temp_dir("repair-script-files");
    let builtin = builtin_script(EXTENDED_ID).unwrap();
    let path = dir.join(jobs::script_file_name(&builtin.name));
    assert!(path.ends_with("Extended repair.json"));
    jobs::export_script(&path, &builtin).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"format\": \"tricky3d-repair-script\""));
    let back = jobs::import_script(&path).unwrap();
    assert!(back.warnings.is_empty());
    assert_is_extended_repair(&back.script);

    let own = own_script("own.7", "Mine");
    let own_path = dir.join("mine.json");
    jobs::export_script(&own_path, &own).unwrap();
    let back = jobs::import_script(&own_path).unwrap().script;
    assert_eq!((back.name.as_str(), back.built_in), ("Mine", false));
    assert_eq!(normalized(&back.actions), normalized(&own.actions));
    assert_ne!(back.id, own.id);
}

#[test]
fn script_files_are_told_apart_by_extension_or_content() {
    let dir = temp_dir("repair-script-kinds");
    let xml = std::fs::read_to_string(fixture("repair/extended-repair.repair")).unwrap();
    let json_text = scripts::export_json(&builtin_script(EXTENDED_ID).unwrap());
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    // No extension, or another one: the first character decides.
    for (name, bytes) in [
        ("list", xml.as_bytes()),
        ("script", json_text.as_bytes()),
        ("list.txt", xml.as_bytes()),
        ("LIST.REPAIR", xml.as_bytes()),
        ("Script.JSON", json_text.as_bytes()),
    ] {
        let imported = jobs::import_script(&write(name, bytes)).unwrap();
        assert_is_extended_repair(&imported.script);
    }
    // UTF-16 with a byte order mark, as some Windows programs write XML, and UTF-8 with one.
    let mut le = vec![0xFF, 0xFE];
    let mut be = vec![0xFE, 0xFF];
    for unit in xml.encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
        be.extend_from_slice(&unit.to_be_bytes());
    }
    let mut bom = b"\xEF\xBB\xBF".to_vec();
    bom.extend_from_slice(json_text.as_bytes());
    for (name, bytes) in [("le.repair", le), ("be.repair", be), ("bom.json", bom)] {
        let imported = jobs::import_script(&write(name, &bytes)).unwrap();
        assert_is_extended_repair(&imported.script);
    }
}

#[test]
fn files_that_are_not_scripts_are_refused_with_their_name() {
    let dir = temp_dir("repair-script-bad");
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let cases: [(&str, Vec<u8>, &str); 7] = [
        ("notes.txt", b"hello".to_vec(), "it is not a repair script"),
        (
            "binary.json",
            vec![0xFF, 0x00, 0xC3],
            "it is not a text file",
        ),
        (
            "odd.repair",
            vec![0xFF, 0xFE, b'<'],
            "it is not a text file",
        ),
        (
            "broken.repair",
            b"<repairlist><repair></repairlist>".to_vec(),
            "its repair list is not valid XML",
        ),
        (
            "part.repair",
            b"solid x\nendsolid x\n".to_vec(),
            "it is not a repair list",
        ),
        ("empty.json", Vec::new(), "it is not valid JSON"),
        (
            "model.repair",
            b"<amf></amf>".to_vec(),
            "it is not a repair list",
        ),
    ];
    for (name, bytes, reason) in cases {
        let err = jobs::import_script(&write(name, &bytes)).unwrap_err();
        let start = format!("Could not import \"{name}\": {reason}");
        assert!(err.starts_with(&start) && err.ends_with('.'), "{err}");
    }
    assert_eq!(
        jobs::import_script(&dir.join("missing.json")).unwrap_err(),
        "Could not import \"missing.json\": the file does not exist any more."
    );
    let big = write("big.json", &vec![b' '; (16 << 20) + 1]);
    assert_eq!(
        jobs::import_script(&big).unwrap_err(),
        "Could not import \"big.json\": it is too large to be a repair script."
    );
}

#[test]
fn export_file_names_are_safe_on_every_system() {
    for (name, want) in [
        ("Extended repair", "Extended repair.json"),
        ("a/b\\c:d*e?f\"g<h>i|j", "a_b_c_d_e_f_g_h_i_j.json"),
        ("  spaced. . ", "spaced.json"),
        ("", "Repair script.json"),
        ("...", "Repair script.json"),
        ("tab\there", "tab_here.json"),
        ("CON", "CON_.json"),
        ("con.backup", "con_.backup.json"),
        ("COM1", "COM1_.json"),
        ("lpt9.x", "lpt9_.x.json"),
        ("COM0", "COM0.json"),
        ("Console", "Console.json"),
        ("Ünïcödé 名前", "Ünïcödé 名前.json"),
    ] {
        assert_eq!(jobs::script_file_name(name), want, "{name:?}");
    }
    let long = jobs::script_file_name(&"é".repeat(300));
    assert!(long.len() <= 125 && long.ends_with(".json"), "{long}");
}

// ---------------------------------------------------------------------------------------------
// Real models

/// `TRICKY3D_REAL_MODELS="a.stl;b.obj" cargo test --release --test repair_scripts -- --ignored
/// --nocapture real_models`: repair_run's round trip on real files (all parts merged into one
/// working copy), with timings.
#[test]
#[ignore]
fn real_models() {
    let Ok(list) = std::env::var("TRICKY3D_REAL_MODELS") else {
        eprintln!("Set TRICKY3D_REAL_MODELS to one or more mesh files separated by ';'.");
        return;
    };
    let jobs = RepairJobs::default();
    for (n, path) in list.split(';').filter(|p| !p.trim().is_empty()).enumerate() {
        let started = Instant::now();
        let imported = import_file(Path::new(path.trim()), false, &mut Progress::silent()).unwrap();
        let parts: Vec<&Mesh> = imported.parts.iter().map(|p| &p.mesh).collect();
        let mesh = concat(&parts).unwrap();
        let s = stats(&mesh);
        println!(
            "{path}: {} triangles, {} vertices, {} shells, {} open edges, {} non-manifold edges; import {:.0} ms",
            s.triangles,
            s.vertices,
            s.shells,
            s.open_edges,
            s.non_manifold_edges,
            ms(started)
        );
        let every_tenth: Vec<u32> = (0..mesh.triangle_count() as u32).step_by(10).collect();
        let job = 1000 * (n as u64 + 1);
        for (k, (label, steps, selection)) in [
            ("No actions", json!([]), &[][..]),
            ("Sanitize duplicate nodes", json!([sanitize()]), &[][..]),
            (
                "Sanitize, tolerance 0",
                json!([step("sanitize_duplicate_nodes", json!({ "tolerance": 0 }))]),
                &[][..],
            ),
            (
                "Flip selected (every 10th)",
                json!([step("flip_selected_triangles", json!({}))]),
                &every_tenth[..],
            ),
            (
                "Remove selected (every 10th)",
                json!([step("remove_selected_triangles", json!({}))]),
                &every_tenth[..],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let started = Instant::now();
            let bundle = request(job + k as u64, label, steps, false, &mesh, selection);
            let decoded = ms(started);
            let started = Instant::now();
            let bytes = jobs::repair_run(bundle, &jobs, &mut |_| {}).unwrap();
            let ran = ms(started);
            let a = Answer::read(&bytes);
            println!(
                "  {label}: encode+decode request {decoded:.0} ms, repair_run {ran:.0} ms (answer {:.1} MB): {:?}",
                bytes.len() as f64 / 1e6,
                a.messages()
            );
        }
    }
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
