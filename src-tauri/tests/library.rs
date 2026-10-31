//! Part library end to end, through the job the `library_build` command runs: every primitive at
//! its defaults, and one item of each other category, is saved in a project, opened again
//! unchanged, and run through Default and Extended repair, which find nothing to repair. Items made
//! from the selection come back in world space; pictures arrive as grey levels; a build reports
//! progress and can be cancelled; values out of range are refused with a sentence.

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

/// The request the window sends: meta {id, params, job, preview}, the selected parts, and a picture.
fn request(
    id: &str,
    params: Value,
    job: u64,
    parts: &[(&Mesh, [f64; 16])],
    picture: Option<(usize, usize, Vec<f32>)>,
) -> tricky3d_lib::bundle::Bundle {
    let mut meta = Map::new();
    meta.insert("id".into(), json!(id));
    meta.insert("params".into(), params);
    meta.insert("job".into(), json!(job));
    meta.insert("preview".into(), json!(false));
    let mut out: Vec<OutMesh> = parts
        .iter()
        .map(|(mesh, matrix)| {
            let mut info = Map::new();
            info.insert("name".into(), json!("part"));
            info.insert("matrix".into(), json!(matrix.to_vec()));
            OutMesh {
                info,
                positions: &mesh.positions,
                indices: &mesh.indices,
                edges: None,
            }
        })
        .collect();
    let mut extra = vec![Vec::new(); out.len()];
    let gray;
    if let Some((w, h, g)) = picture {
        gray = g;
        let mut info = Map::new();
        info.insert("picture".into(), json!(true));
        info.insert("width".into(), json!(w));
        info.insert("height".into(), json!(h));
        out.push(OutMesh {
            info,
            positions: &[],
            indices: &[],
            edges: None,
        });
        extra.push(vec![("gray", BlobRef::F32(&gray))]);
    }
    decode(&encode_with_extra(&meta, &out, &extra)).unwrap()
}

/// The mesh and info library_build sends back.
fn build_with(id: &str, params: Value, jobs: &RepairJobs, job: u64) -> (Mesh, Map<String, Value>) {
    let bytes = jobs::library_build(request(id, params, job, &[], None), jobs, &mut |_| {})
        .unwrap_or_else(|e| panic!("{id}: {e}"));
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
fn every_primitive_and_one_item_of_each_category_saves_reopens_and_needs_no_repair() {
    let jobs = RepairJobs::default();
    let dir = temp_dir("library");
    let mut job = 100;
    let picked = [
        "qr_code",
        "gear",
        "menger",
        "superellipsoid",
        "tensile",
        "flexure",
    ];
    for primitive in library::catalog()
        .into_iter()
        .filter(|i| i.category == library::Category::Primitive || picked.contains(&i.id))
    {
        job += 3;
        let params = if primitive.id == "menger" {
            json!({"level": 2})
        } else {
            json!({})
        };
        let (mesh, info) = build_with(primitive.id, params, &jobs, job);
        assert_eq!(info["world"], json!(false));
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
            priority: None,
            rotation: None,
            measurements: Some("[]"),
            analyses: None,
            colors: None,
            source: None,
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
        jobs::library_build(
            request("sphere", json!({}), 9, &[], None),
            &jobs,
            &mut |_| {}
        )
        .unwrap_err(),
        CANCELLED
    );
    assert_eq!(
        jobs::library_build(
            request("cylinder", json!({"radius": 0}), 10, &[], None),
            &jobs,
            &mut |_| {}
        )
        .unwrap_err(),
        "Radius must be from 0.001 mm to 10000 mm."
    );
    // A long build reports its progress as it goes.
    let mut fractions = Vec::new();
    jobs::library_build(
        request("gyroid", json!({"accuracy": 0.5}), 11, &[], None),
        &jobs,
        &mut |p| fractions.push(p.fraction),
    )
    .unwrap();
    assert!(!fractions.is_empty() && fractions.windows(2).all(|w| w[0] <= w[1]));
    assert!(jobs.is_idle());
}

#[test]
fn parts_made_from_the_selection_come_back_where_it_is() {
    let jobs = RepairJobs::default();
    let part = cube(20.0);
    let placed = trs([80.0, 70.0, 5.0], 30.0, 0.0, [1.0; 3]);
    let bytes = jobs::library_build(
        request("convex_hull", json!({}), 20, &[(&part, placed)], None),
        &jobs,
        &mut |_| {},
    )
    .unwrap();
    let answer = decode(&bytes).unwrap();
    assert_eq!(answer.meshes[0].info["world"], json!(true));
    assert_same_shape(
        &answer.meshes[0].mesh,
        &tricky3d_lib::mesh::bake(&part, &placed),
        "hull of a box",
    );
    // Without a selection there is nothing to make it from.
    let err = jobs::library_build(
        request("base_plate", json!({}), 21, &[], None),
        &jobs,
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err, "Select the part to make the base plate from first.");
}

#[test]
fn a_picture_from_the_window_shapes_the_relief() {
    let jobs = RepairJobs::default();
    // 4 × 2 pixels: the left half black, the right half white.
    let gray = vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0];
    let params = json!({"width": 40, "detail": 40, "base": 1, "height": 4});
    let bytes = jobs::library_build(
        request("heightmap", params, 30, &[], Some((4, 2, gray))),
        &jobs,
        &mut |_| {},
    )
    .unwrap();
    let mesh = decode(&bytes).unwrap().meshes.remove(0).mesh;
    let b = tricky3d_lib::mesh::bounds(&mesh).unwrap();
    assert!((b[1][0] - b[0][0] - 40.0).abs() < 1e-4 && (b[1][1] - b[0][1] - 20.0).abs() < 1e-4);
    // Dark stands high on the left, light stays at the base on the right.
    let height_at = |left: bool| {
        mesh.positions
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|p| if left { p[0] < -15.0 } else { p[0] > 15.0 })
            .map(|p| p[2])
            .fold(0.0f32, f32::max)
    };
    assert!((height_at(true) - 5.0).abs() < 1e-4);
    assert!((height_at(false) - 1.0).abs() < 1e-4);
    // A damaged picture is refused.
    let err = jobs::library_build(
        request("heightmap", json!({}), 31, &[], Some((5, 2, vec![0.5; 8]))),
        &jobs,
        &mut |_| {},
    )
    .unwrap_err();
    assert!(
        err.ends_with("the request from the window is damaged."),
        "{err}"
    );
}
