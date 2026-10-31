//! Part analyses end to end, through the job the `analyze_part` command runs: the part placed by its
//! matrix, the answer's result and drawing, cancel, and analyses kept through project save and open.

mod common;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Blob, Bundle, OutMesh, decode, encode};
use tricky3d_lib::jobs::{self, CANCELLED, RepairJobs};
use tricky3d_lib::mesh::{IDENTITY, Matrix, Mesh, make_box};

fn request(job: u64, kind: &str, settings: Value, parts: &[(&Mesh, Matrix)]) -> Bundle {
    let out: Vec<OutMesh> = parts
        .iter()
        .enumerate()
        .map(|(i, &(mesh, matrix))| {
            let mut info = Map::new();
            info.insert("name".into(), json!(format!("Part {i}")));
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
    meta.insert("kind".into(), json!(kind));
    meta.insert("settings".into(), settings);
    decode(&encode(&meta, &out)).unwrap()
}

fn run(bundle: Bundle, jobs: &RepairJobs) -> Result<Bundle, String> {
    jobs::analyze_part(bundle, jobs, &mut |_| {}).map(|bytes| decode(&bytes).unwrap())
}

fn moved(x: f64, y: f64, z: f64) -> Matrix {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

#[test]
fn each_kind_answers_its_numbers_and_drawing() {
    let jobs = RepairJobs::default();
    let block = make_box([0.0; 3], [20.0, 10.0, 5.0]);
    let at = moved(50.0, 60.0, 0.0);

    let standard = run(request(1, "standard", Value::Null, &[(&block, at)]), &jobs).unwrap();
    let r = &standard.meta["result"];
    assert_eq!(r["volume"], json!(1000.0));
    assert_eq!(r["min"], json!([50.0, 60.0, 0.0]));
    assert_eq!(r["closed"], json!(true));

    let gravity = run(request(2, "gravity", Value::Null, &[(&block, at)]), &jobs).unwrap();
    let c: Vec<f64> = serde_json::from_value(gravity.meta["result"]["centre"].clone()).unwrap();
    for (got, want) in c.iter().zip([60.0, 65.0, 2.5]) {
        assert!((got - want).abs() < 1e-9, "{c:?}");
    }

    let shadow = run(request(3, "shadow", Value::Null, &[(&block, at)]), &jobs).unwrap();
    assert!((shadow.meta["result"]["area"].as_f64().unwrap() - 200.0).abs() < 1e-9);
    // The shadow's triangles lie on the platform, and its outline is the box's footprint.
    let carrier = &shadow.meshes[0];
    assert!(carrier.mesh.positions.chunks(3).all(|p| p[2] == 0.0));
    let Some(Blob::F32(lines)) = carrier.extra("lines") else {
        panic!("no outline");
    };
    assert_eq!(lines.len(), 4 * 6);

    let skin = run(
        request(
            4,
            "skin",
            json!({"upskins": true, "upAngle": 0.0, "downskins": true, "downAngle": 0.0, "minComponent": 10.0, "filterSmall": false}),
            &[(&block, at)],
        ),
        &jobs,
    )
    .unwrap();
    assert_eq!(skin.meta["result"]["up"]["area"], json!(200.0));
    let Some(Blob::U32(classes)) = skin.meshes[0].extra("classes") else {
        panic!("no classes");
    };
    assert_eq!(classes.len(), 12);

    // Lifted 10 mm: its whole bottom needs support, 200 mm² × 10 mm.
    let support = run(
        request(
            5,
            "support",
            json!({"angle": 45.0}),
            &[(&block, moved(0.0, 0.0, 10.0))],
        ),
        &jobs,
    )
    .unwrap();
    assert!((support.meta["result"]["volume"].as_f64().unwrap() - 2000.0).abs() < 1e-9);
    assert!(!support.meshes[0].mesh.positions.is_empty());
    assert!(jobs.is_idle());
}

#[test]
fn shadow_area_says_why_an_open_part_cannot_have_one() {
    let jobs = RepairJobs::default();
    let mut open = make_box([0.0; 3], [10.0; 3]);
    open.indices.drain(0..6);
    let err = run(
        request(1, "shadow", Value::Null, &[(&open, IDENTITY)]),
        &jobs,
    )
    .unwrap_err();
    assert!(
        err.contains("closed, sound mesh") && err.contains("1 hole"),
        "{err}"
    );
}

#[test]
fn a_cancel_before_or_during_the_run_leaves_nothing_behind() {
    let jobs = RepairJobs::default();
    let ball = torus(400, 200, 30.0, 10.0);
    jobs.cancel(7);
    assert_eq!(
        run(
            request(7, "shadow", Value::Null, &[(&ball, IDENTITY)]),
            &jobs
        )
        .err(),
        Some(CANCELLED.to_string())
    );
    let during = jobs::analyze_part(
        request(8, "shadow", Value::Null, &[(&ball, IDENTITY)]),
        &jobs,
        &mut |p| {
            if p.fraction > 0.0 {
                jobs.cancel(8);
            }
        },
    );
    assert_eq!(during, Err(CANCELLED.to_string()));
    assert!(jobs.is_idle());
}

#[test]
fn bad_settings_are_refused() {
    let jobs = RepairJobs::default();
    let block = cube(10.0);
    for (kind, settings) in [
        ("support", json!({"angle": 120.0})),
        (
            "skin",
            json!({"upskins": true, "upAngle": 95.0, "downskins": true, "downAngle": 45.0, "minComponent": 0.0, "filterSmall": false}),
        ),
        ("skin", Value::Null),
        ("nothing", Value::Null),
    ] {
        assert!(
            run(request(1, kind, settings, &[(&block, IDENTITY)]), &jobs).is_err(),
            "{kind}"
        );
    }
}

#[test]
fn analyses_travel_with_their_part_through_save_and_open() {
    let dir = temp_dir("analysis-project");
    let file = dir.join("analysed.3mf");
    let block = cube(20.0);
    let json_text = r#"[{"id":"a1","kind":"shadow","settings":null,"result":{"area":400},"matrix":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"drawing":{"lines":"AAAAAA=="}}]"#;
    let mut meta = Map::new();
    meta.insert("path".into(), json!(file.to_string_lossy()));
    meta.insert("platform".into(), json!([200.0, 200.0, 200.0]));
    let infos = [("Analysed", Some(json_text)), ("Plain", None)].map(|(name, analyses)| {
        let mut info = Map::new();
        info.insert("name".into(), json!(name));
        if let Some(a) = analyses {
            info.insert("analyses".into(), json!(a));
            info.insert("colors".into(), json!("AAAA"));
        }
        info
    });
    let out: Vec<OutMesh> = infos
        .iter()
        .map(|info| OutMesh {
            info: info.clone(),
            positions: &block.positions,
            indices: &block.indices,
            edges: None,
        })
        .collect();
    jobs::save_project(decode(&encode(&meta, &out)).unwrap()).unwrap();
    let opened = decode(
        &jobs::open_project(&file, &mut tricky3d_lib::progress::Progress::silent()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        opened.meshes[0]
            .info
            .get("analyses")
            .and_then(Value::as_str),
        Some(json_text)
    );
    assert_eq!(opened.meshes[1].info.get("analyses"), None);
    // The coloured part of a wall thickness keeps its triangle colours.
    assert_eq!(
        opened.meshes[0].info.get("colors").and_then(Value::as_str),
        Some("AAAA")
    );
    assert_eq!(opened.meshes[1].info.get("colors"), None);
}

#[test]
fn wall_thickness_answers_a_thickness_per_triangle_and_the_thinnest_spot() {
    let jobs = RepairJobs::default();
    let plate = make_box([0.0; 3], [40.0, 40.0, 3.0]);
    let reply = run(
        request(
            1,
            "wall",
            json!({"threshold": 4.0, "acceptance": 5.0, "stopEarly": false}),
            &[(&plate, moved(10.0, 0.0, 0.0))],
        ),
        &jobs,
    )
    .unwrap();
    let r = &reply.meta["result"];
    assert_eq!(r["passed"], json!(false));
    assert!((r["minThickness"].as_f64().unwrap() - 3.0).abs() < 1e-9);
    let from: Vec<f64> = serde_json::from_value(r["minFrom"].clone()).unwrap();
    assert!(from[0] >= 10.0, "world coordinates: {from:?}");
    let Some(Blob::F32(values)) = reply.meshes[0].extra("values") else {
        panic!("no thicknesses");
    };
    assert_eq!(values.len(), 12);
    for bad in [
        json!({"threshold": 0.0, "acceptance": 5.0, "stopEarly": false}),
        json!({"threshold": 1.0, "acceptance": 101.0, "stopEarly": false}),
    ] {
        assert!(run(request(2, "wall", bad, &[(&plate, IDENTITY)]), &jobs).is_err());
    }
}

#[test]
fn compare_answers_both_directions_with_a_refined_copy_each_and_changes_no_part() {
    let jobs = RepairJobs::default();
    let a = make_box([0.0; 3], [20.0; 3]);
    let b = make_box([0.0; 3], [20.0; 3]);
    let a_before = a.clone();
    let reply = run(
        request(
            1,
            "compare",
            json!({"direction": "both", "align": false}),
            &[(&a, IDENTITY), (&b, moved(0.0, 0.0, 1.0))],
        ),
        &jobs,
    )
    .unwrap();
    let r = &reply.meta["result"];
    // The comparison is 1 mm higher: the reference's top is 1 mm under it, its bottom 1 mm out.
    assert!((r["forward"]["maxPositive"].as_f64().unwrap() - 1.0).abs() < 1e-6);
    assert!((r["forward"]["maxNegative"].as_f64().unwrap() + 1.0).abs() < 1e-6);
    assert!(r["backward"].is_object() && r["fit"].is_null());
    let roles: Vec<&str> = reply.meshes[1..]
        .iter()
        .map(|m| m.info["surface"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["reference", "comparison"]);
    for m in &reply.meshes[1..] {
        let Some(Blob::F32(values)) = m.extra("values") else {
            panic!("no distances");
        };
        assert_eq!(values.len(), m.mesh.positions.len() / 3);
        assert!(m.mesh.indices.len() / 3 > 12, "refined");
    }
    assert_eq!(a, a_before);
    // One way only.
    let one = run(
        request(
            2,
            "compare",
            json!({"direction": "comparison-to-reference", "align": false}),
            &[(&a, IDENTITY), (&b, IDENTITY)],
        ),
        &jobs,
    )
    .unwrap();
    assert!(one.meta["result"]["forward"].is_null());
    assert_eq!(one.meshes.len(), 2);
    // Two parts are needed.
    assert!(
        run(
            request(
                3,
                "compare",
                json!({"direction": "both", "align": false}),
                &[(&a, IDENTITY)]
            ),
            &jobs
        )
        .is_err()
    );
}

#[test]
fn a_cancelled_compare_leaves_nothing_behind() {
    let jobs = RepairJobs::default();
    let ring = torus(200, 100, 30.0, 10.0);
    let during = jobs::analyze_part(
        request(
            9,
            "compare",
            json!({"direction": "both", "align": true}),
            &[(&ring, IDENTITY), (&ring, moved(1.0, 2.0, 0.0))],
        ),
        &jobs,
        &mut |p| {
            if p.fraction > 0.05 {
                jobs.cancel(9);
            }
        },
    );
    assert_eq!(during, Err(CANCELLED.to_string()));
    assert!(jobs.is_idle());
}

fn pairs(reply: &Bundle) -> Vec<(u64, u64, String, f64)> {
    reply.meta["result"]["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["a"].as_u64().unwrap(),
                p["b"].as_u64().unwrap(),
                p["kind"].as_str().unwrap().to_string(),
                p["distance"].as_f64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn collision_detection_finds_touching_overlapping_and_too_close_parts() {
    let jobs = RepairJobs::default();
    let block = make_box([0.0; 3], [10.0; 3]);
    let check = |at: Matrix, limit: f64| {
        run(
            request(
                1,
                "collisions",
                json!({ "minDistance": limit }),
                &[(&block, IDENTITY), (&block, at)],
            ),
            &jobs,
        )
        .unwrap()
    };
    // Touching face to face counts, as in Packing.
    let touching = check(moved(10.0, 0.0, 0.0), 0.0);
    assert_eq!(pairs(&touching), vec![(0, 1, "collide".to_string(), 0.0)]);
    let overlapping = check(moved(5.0, 5.0, 0.0), 0.0);
    assert_eq!(pairs(&overlapping).len(), 1);
    // The crossing triangles are drawn.
    assert!(!overlapping.meshes[0].mesh.positions.is_empty());
    // 0.5 mm apart: clear with no minimum distance, too close with 1 mm.
    assert!(pairs(&check(moved(10.5, 0.0, 0.0), 0.0)).is_empty());
    let near = pairs(&check(moved(10.5, 0.0, 0.0), 1.0));
    assert_eq!(near.len(), 1);
    assert_eq!(near[0].2, "near");
    assert!((near[0].3 - 0.5).abs() < 1e-9, "{near:?}");
    // 1.5 mm apart is far enough.
    assert!(pairs(&check(moved(11.5, 0.0, 0.0), 1.0)).is_empty());
    assert!(
        run(
            request(
                2,
                "collisions",
                json!({ "minDistance": -1.0 }),
                &[(&block, IDENTITY)]
            ),
            &jobs
        )
        .is_err()
    );
}

#[test]
fn interlocking_and_z_removability_through_the_job() {
    let jobs = RepairJobs::default();
    let bottom = make_box([0.0; 3], [20.0, 20.0, 10.0]);
    let top = make_box([5.0, 5.0, 10.0], [15.0, 15.0, 20.0]);
    let r = run(
        request(
            1,
            "interlock",
            Value::Null,
            &[(&bottom, IDENTITY), (&top, IDENTITY)],
        ),
        &jobs,
    )
    .unwrap();
    assert_eq!(
        r.meta["result"],
        json!({"rounds": [[1], [0]], "trapped": []})
    );
    assert!(
        run(
            request(2, "interlock", Value::Null, &[(&bottom, IDENTITY)]),
            &jobs
        )
        .is_err()
    );
    let r = run(
        request(3, "removability", Value::Null, &[(&bottom, IDENTITY)]),
        &jobs,
    )
    .unwrap();
    assert_eq!(r.meta["result"]["removable"], json!(true));
    let Some(Blob::U32(classes)) = r.meshes[0].extra("classes") else {
        panic!("no classes");
    };
    assert_eq!(classes.len(), 12);
}

/// The big-model timings in the report: one analysis on one model file, through the job the app
/// runs. Run in release through scripts/corpus/capped.ps1, one kind at a time:
///
///   TRICKY3D_BIG=<model file>
///   TRICKY3D_KIND=standard|gravity|shadow|support|skin|wall|compare|removability|collisions
///   cargo test --release --test analysis big_model_report -- --ignored --nocapture
///
/// Collision detection checks the model against a copy of itself moved by a tenth of its length
/// along X, so the two cross.
#[test]
#[ignore = "needs a big model; run through scripts/corpus/capped.ps1"]
fn big_model_report() {
    let path = std::path::PathBuf::from(std::env::var("TRICKY3D_BIG").expect("set TRICKY3D_BIG"));
    let kind = std::env::var("TRICKY3D_KIND").unwrap_or_else(|_| "standard".into());
    let read = std::time::Instant::now();
    let imported = tricky3d_lib::formats::import_file(
        &path,
        true,
        &mut tricky3d_lib::progress::Progress::silent(),
    )
    .unwrap();
    let mesh = imported.parts.into_iter().next().unwrap().mesh;
    let read = read.elapsed().as_secs_f64();
    let b = tricky3d_lib::mesh::bounds(&mesh).unwrap();
    let length = (b[1][0] - b[0][0]) as f64;
    let (settings, parts): (Value, Vec<(&Mesh, Matrix)>) = match kind.as_str() {
        "wall" => (
            json!({"threshold": 0.4, "acceptance": 5.0, "stopEarly": false}),
            vec![(&mesh, IDENTITY)],
        ),
        "skin" => (
            json!({"upskins": true, "upAngle": 45.0, "downskins": true, "downAngle": 45.0, "minComponent": 10.0, "filterSmall": false}),
            vec![(&mesh, IDENTITY)],
        ),
        "collisions" => (
            json!({"minDistance": 0.0}),
            vec![(&mesh, IDENTITY), (&mesh, moved(length / 10.0, 0.0, 0.0))],
        ),
        "support" => (json!({"angle": 45.0}), vec![(&mesh, IDENTITY)]),
        // Against itself, moved 1 mm up, both ways.
        "compare" => (
            json!({"direction": "both", "align": false}),
            vec![(&mesh, IDENTITY), (&mesh, moved(0.0, 0.0, 1.0))],
        ),
        _ => (Value::Null, vec![(&mesh, IDENTITY)]),
    };
    let start = std::time::Instant::now();
    let reply = run(
        request(1, &kind, settings.clone(), &parts),
        &RepairJobs::default(),
    )
    .unwrap();
    let seconds = start.elapsed().as_secs_f64();
    // Again until two seconds have passed, so the capped runner (which samples memory every
    // 250 ms) sees the peak; each run frees what the one before used.
    let mut runs = 1;
    while start.elapsed().as_secs_f64() < 2.0 {
        run(
            request(1, &kind, settings.clone(), &parts),
            &RepairJobs::default(),
        )
        .unwrap();
        runs += 1;
    }
    let mean = start.elapsed().as_secs_f64() / runs as f64;
    let r = &reply.meta["result"];
    let summary = match kind.as_str() {
        "wall" => format!(
            "passed {} thin {:.2} % min {:.3} mm",
            r["passed"],
            r["thinPercent"].as_f64().unwrap(),
            r["minThickness"].as_f64().unwrap_or(f64::NAN)
        ),
        "collisions" => format!("{} pairs", r["pairs"].as_array().unwrap().len()),
        "compare" => format!(
            "largest above {:.3} below {:.3} mm, {} points",
            r["forward"]["maxPositive"].as_f64().unwrap(),
            r["forward"]["maxNegative"].as_f64().unwrap(),
            r["forward"]["points"]
        ),
        "standard" => format!(
            "{} triangles, {} shells, closed {}",
            r["triangles"], r["shells"], r["closed"]
        ),
        _ => r.to_string().chars().take(120).collect(),
    };
    println!(
        "BIG|{}|{kind}|{} triangles|read {read:.2} s|analysis {seconds:.2} s (mean of {runs}: {mean:.2} s)|{summary}",
        path.file_name().unwrap().to_string_lossy(),
        mesh.triangle_count(),
    );
}
