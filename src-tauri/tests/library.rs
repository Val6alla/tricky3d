//! Part library › Primitive end to end: every primitive at its defaults is built through the job the
//! `library_primitive` command runs, saved in a project, opened again unchanged, and run through
//! Default and Extended repair, which find nothing to repair.

mod common;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{BlobRef, OutMesh, decode, encode_with_extra};
use tricky3d_lib::jobs::{self, CANCELLED, RepairJobs};
use tricky3d_lib::library;
use tricky3d_lib::mesh::{IDENTITY, Mesh, stats};
use tricky3d_lib::progress::Progress;
use tricky3d_lib::project::{self, PartOut};
use tricky3d_lib::repair::defaults::{DEFAULT_ID, EXTENDED_ID, builtin_script};

/// The mesh and stats library_primitive sends back.
fn build(id: &str, jobs: &RepairJobs, job: u64) -> (Mesh, Map<String, Value>) {
    let bytes =
        jobs::library_primitive(id, &Map::new(), job, jobs).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut bundle = decode(&bytes).unwrap();
    let m = bundle.meshes.remove(0);
    (m.mesh, m.info)
}

/// What Extended or Default repair changed on `mesh`, run as the window runs it.
fn repair(script: &str, mesh: &Mesh, jobs: &RepairJobs, job: u64) -> bool {
    let steps = serde_json::to_value(&builtin_script(script).unwrap().actions).unwrap();
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("label".into(), json!(script));
    meta.insert("actions".into(), steps);
    meta.insert("withStatus".into(), json!(true));
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
    let answer = decode(&jobs::repair_run(request, jobs, &mut |_| {}).unwrap()).unwrap();
    answer.meta["changed"].as_bool().unwrap()
}

/// Every triangle as its corners' coordinate bits, starting from its smallest corner, sorted: the
/// same for two meshes with the same triangles facing the same way, however their vertices are numbered.
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
fn every_primitive_saves_reopens_and_needs_no_repair() {
    let jobs = RepairJobs::default();
    let dir = temp_dir("library");
    let mut job = 100;
    for primitive in library::catalog() {
        job += 3;
        let (mesh, info) = build(primitive.id, &jobs, job);
        let local = stats(&mesh);
        assert_eq!(info["stats"]["openEdges"], json!(0), "{}", primitive.id);
        assert_eq!(
            info["stats"]["volume"].as_f64(),
            Some(local.volume),
            "{}",
            primitive.id
        );

        let file = dir.join(format!("{}.3mf", primitive.id));
        let part = PartOut {
            name: primitive.label,
            matrix: IDENTITY,
            color: None,
            visible: true,
            transparent: false,
            locked: false,
            measurements: Some("[]"),
            mesh: &mesh,
        };
        project::save_file(&file, [200.0; 3], &[part]).unwrap();
        let reopened = project::open_file(&file, &mut Progress::silent()).unwrap();
        assert_eq!(reopened.parts.len(), 1);
        assert_eq!(reopened.parts[0].name, primitive.label);
        assert_eq!(reopened.parts[0].measurements.as_deref(), Some("[]"));
        // Opening welds the vertices in the order the file uses them, so the triangles are compared by position.
        assert_eq!(
            triangles(&reopened.parts[0].mesh),
            triangles(&mesh),
            "{} changed in the project",
            primitive.id
        );

        for (k, script) in [DEFAULT_ID, EXTENDED_ID].into_iter().enumerate() {
            assert!(
                !repair(script, &reopened.parts[0].mesh, &jobs, job + 1 + k as u64),
                "{} needed {script}",
                primitive.id
            );
        }
    }
    assert!(jobs.is_idle());
}

#[test]
fn a_cancelled_build_fails_with_cancelled_and_a_bad_value_with_its_sentence() {
    let jobs = RepairJobs::default();
    jobs.cancel(9);
    assert_eq!(
        jobs::library_primitive("sphere", &Map::new(), 9, &jobs).unwrap_err(),
        CANCELLED
    );
    let mut params = Map::new();
    params.insert("radius".into(), json!(0));
    assert_eq!(
        jobs::library_primitive("cylinder", &params, 10, &jobs).unwrap_err(),
        "Radius must be from 0.001 mm to 10000 mm."
    );
    assert!(jobs.is_idle());
}
