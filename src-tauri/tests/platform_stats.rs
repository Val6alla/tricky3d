//! Home › Platform statistics through the job the `platform_stats` command runs: the request the
//! window sends, the numbers per part, the totals for all and for the selected parts, and Cancel.

mod common;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{OutMesh, decode, encode};
use tricky3d_lib::formats::import_file;
use tricky3d_lib::jobs::{self, CANCELLED, RepairJobs};
use tricky3d_lib::mesh::{IDENTITY, Matrix, Mesh, linear_determinant, stats};
use tricky3d_lib::progress::Progress;

fn request(
    job: u64,
    platform: [f64; 3],
    selected: &[usize],
    parts: &[(&str, &Mesh, Matrix)],
) -> Vec<u8> {
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("platform".into(), json!(platform));
    meta.insert("selected".into(), json!(selected));
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
    encode(&meta, &out)
}

fn run(bytes: Vec<u8>, jobs: &RepairJobs) -> Result<jobs::StatsReply, String> {
    let mut fractions = Vec::new();
    let reply = jobs::platform_stats(decode(&bytes).unwrap(), jobs, &mut |p| {
        fractions.push(p.fraction)
    });
    if reply.is_ok() {
        assert_eq!(fractions.last(), Some(&1.0));
        assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
    }
    reply
}

/// Rotation by 30° about Z, scale 1.5, then a move.
fn turned() -> Matrix {
    let (s, c) = 30f64.to_radians().sin_cos();
    let k = 1.5;
    [
        k * c,
        k * s,
        0.0,
        0.0, //
        -k * s,
        k * c,
        0.0,
        0.0, //
        0.0,
        0.0,
        k,
        0.0, //
        100.0,
        90.0,
        15.0,
        1.0,
    ]
}

#[test]
fn every_part_gets_its_row_and_the_platform_its_totals() {
    let sphere = import_file(
        &fixture("icosphere_20k.stl"),
        false,
        &mut Progress::silent(),
    )
    .unwrap()
    .parts
    .remove(0)
    .mesh;
    let local = stats(&sphere);
    let open = open_box();
    let cube = cube(20.0);
    let mut at = IDENTITY;
    at[12] = 5.0;
    at[13] = 5.0;
    let m = turned();
    let parts = [
        ("Sphere", &sphere, m),
        ("Open", &open, IDENTITY),
        ("Cube", &cube, at),
    ];
    let reply = run(
        request(7, [200.0, 200.0, 200.0], &[2], &parts),
        &RepairJobs::default(),
    )
    .unwrap();

    // The sphere: volume and area under the matrix, as the Information panel works them out.
    let s = &reply.parts[0];
    assert_eq!([s.triangles, s.points], [local.triangles, local.vertices]);
    assert_eq!(s.edges, local.triangles * 3 / 2);
    assert_eq!(
        s.volume,
        Some((linear_determinant(&m) * local.volume).abs())
    );
    let k2 = 1.5f64 * 1.5;
    assert!((s.area - local.area * k2).abs() <= local.area * k2 * 1e-12);
    assert!(s.closed && s.oriented);

    let o = &reply.parts[1];
    assert_eq!(o.volume, None);
    assert!(!o.closed);
    assert!(o.holes >= 1);

    let c = &reply.parts[2];
    assert_eq!(
        (c.min, c.max),
        (Some([5.0, 5.0, 0.0]), Some([25.0, 25.0, 20.0]))
    );

    // All parts: one has holes, so the total volume is unknown.
    assert_eq!(reply.all.parts, 3);
    assert_eq!(reply.all.volume, None);
    // The cube alone (the selection).
    assert_eq!(reply.selected.parts, 1);
    assert_eq!(reply.selected.volume, Some(8000.0));
    assert_eq!(reply.selected.top, Some(20.0));
    assert!((reply.selected.packing_density.unwrap() - 100.0 * 8000.0 / 8e6).abs() < 1e-12);
    assert!(
        (reply.selected.with_object_height.unwrap() - 100.0 * 8000.0 / (200.0 * 200.0 * 20.0))
            .abs()
            < 1e-12
    );
}

/// A 10 mm box with its top missing: 10 triangles, one hole.
fn open_box() -> Mesh {
    let mut mesh = cube(10.0);
    mesh.indices.drain(6..12);
    mesh
}

#[test]
fn cancel_stops_the_run_and_a_damaged_request_says_so() {
    let jobs = RepairJobs::default();
    let cube = cube(10.0);
    // A cancel that arrives before the run starts is kept for it.
    jobs.cancel(41);
    let reply = run(
        request(41, [100.0; 3], &[], &[("Cube", &cube, IDENTITY)]),
        &jobs,
    );
    assert_eq!(reply.unwrap_err(), CANCELLED);
    assert!(jobs.is_idle());

    let mut meta = Map::new();
    meta.insert("job".into(), json!(42));
    meta.insert("platform".into(), json!([100.0, 0.0, 100.0]));
    meta.insert("selected".into(), Value::Array(vec![]));
    let error =
        jobs::platform_stats(decode(&encode(&meta, &[])).unwrap(), &jobs, &mut |_| {}).unwrap_err();
    assert_eq!(
        error,
        "Could not work out the platform statistics: the request from the window is damaged."
    );
}

#[test]
fn export_to_csv_writes_the_text_as_it_came() {
    let dir = temp_dir("stats-csv");
    let file = dir.join("statistics.csv");
    let text = "Part,ID\r\n\"Bracket, left\",1\r\nÜberteil,2\r\n";
    jobs::export_text(&file, text).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    // A second export replaces the file.
    jobs::export_text(&file, "Part\r\n").unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Part\r\n");
    let error = jobs::export_text(&dir.join("missing").join("x.csv"), text).unwrap_err();
    assert_eq!(
        error,
        "Could not write \"x.csv\": the folder does not exist."
    );
}
