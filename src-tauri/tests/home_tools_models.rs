//! Platform statistics, Measure and projects with measurements on your own model files, through the
//! jobs their commands run. Skipped unless pointed at models:
//!
//! ```sh
//! TRICKY3D_MODELS="a.stl|b.3mf" cargo test --release --manifest-path src-tauri/Cargo.toml --test home_tools_models -- --ignored --nocapture
//! ```

mod common;

use std::path::Path;
use std::time::Instant;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{OutMesh, decode, encode};
use tricky3d_lib::formats::import_file;
use tricky3d_lib::jobs::{self, RepairJobs};
use tricky3d_lib::measure::{Feature, MeasureCache, Op, evaluate};
use tricky3d_lib::mesh::{Matrix, Mesh, linear_determinant, stats};
use tricky3d_lib::progress::Progress;

/// Rotated 30° about Z and moved onto the platform.
fn placed() -> Matrix {
    let (s, c) = 30f64.to_radians().sin_cos();
    [
        c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 100.0, 100.0, 20.0, 1.0,
    ]
}

fn bundle_of(meta: Map<String, Value>, parts: &[(&str, &Mesh, Matrix, Option<&str>)]) -> Vec<u8> {
    let out: Vec<OutMesh> = parts
        .iter()
        .map(|&(name, mesh, matrix, measurements)| {
            let mut info = Map::new();
            info.insert("name".into(), json!(name));
            info.insert("matrix".into(), json!(matrix.to_vec()));
            if let Some(m) = measurements {
                info.insert("measurements".into(), json!(m));
            }
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

/// The biggest triangle: a click on its centre lands well inside it.
fn biggest_triangle(mesh: &Mesh) -> (u32, [f64; 3]) {
    let mut best = (0u32, 0.0f64, [0.0; 3]);
    for (t, c) in mesh.indices.chunks(3).enumerate() {
        let [a, b, d] = [0, 1, 2].map(|k| mesh.vertex(c[k]).map(f64::from));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let area = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if area > best.1 {
            best = (
                t as u32,
                area,
                [0, 1, 2].map(|k| (a[k] + b[k] + d[k]) / 3.0),
            );
        }
    }
    (best.0, best.2)
}

#[test]
#[ignore = "reads model files named in TRICKY3D_MODELS"]
fn statistics_measure_and_projects_on_model_files() {
    let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
        eprintln!("TRICKY3D_MODELS is not set");
        return;
    };
    let dir = temp_dir("home-tools-models");
    for path in list.split('|').filter(|p| !p.is_empty()) {
        let name = Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let t0 = Instant::now();
        let mesh = import_file(Path::new(path), true, &mut Progress::silent())
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .parts
            .remove(0)
            .mesh;
        let imported = t0.elapsed();
        let local = stats(&mesh);
        let m = placed();

        // Platform statistics: the numbers the Information panel shows.
        let mut meta = Map::new();
        meta.insert("job".into(), json!(1));
        meta.insert("platform".into(), json!([400.0, 400.0, 400.0]));
        meta.insert("selected".into(), json!([0]));
        let t0 = Instant::now();
        let reply = jobs::platform_stats(
            decode(&bundle_of(meta, &[(&name, &mesh, m, None)])).unwrap(),
            &RepairJobs::default(),
            &mut |_| {},
        )
        .unwrap();
        let statistics = t0.elapsed();
        let row = &reply.parts[0];
        assert_eq!(row.triangles, local.triangles, "{name}");
        assert_eq!(row.points, local.vertices, "{name}");
        if local.open_edges == 0 {
            assert_eq!(
                row.volume,
                Some((linear_determinant(&m) * local.volume).abs()),
                "{name}"
            );
        } else {
            assert_eq!(row.volume, None, "{name}");
        }
        assert!(
            (row.area - local.area).abs() <= local.area * 1e-9,
            "{name}: {} vs {}",
            row.area,
            local.area
        );

        // Measure: the first pick builds the mesh index, the next ones reuse it.
        let cache = MeasureCache::default();
        let (t, centre) = biggest_triangle(&mesh);
        let hit = tricky3d_lib::mesh::transform_point(&m, centre);
        let pick = |kind: &str, send: bool| {
            let mut meta = Map::new();
            for (k, v) in [
                ("meshKey", json!(1)),
                ("kind", json!(kind)),
                ("triangle", json!(t)),
                ("hit", json!(hit)),
                ("matrix", json!(m.to_vec())),
                ("owner", json!(m.to_vec())),
                ("snap", json!("auto")),
                ("tolerance", json!(0.2)),
                ("clip", json!([])),
            ] {
                meta.insert(k.into(), v);
            }
            let empty = Mesh::default();
            let bytes = bundle_of(
                meta,
                &[("part", if send { &mesh } else { &empty }, m, None)],
            );
            let t0 = Instant::now();
            let reply = jobs::measure_pick(decode(&bytes).unwrap(), &cache);
            (reply, t0.elapsed())
        };
        let (first, first_time) = pick("point", true);
        let first = first.unwrap();
        let (edge, edge_time) = pick("edge", false);
        let (surface, surface_time) = pick("surface", false);
        let (wall, wall_time) = pick("wall", false);
        let edge = edge.unwrap();
        let surface = surface.unwrap();
        let thickness = wall.as_ref().ok().map(|w| {
            evaluate(Op::Thickness, std::slice::from_ref(&w.feature), &m)
                .unwrap()
                .value
        });
        let d = evaluate(
            Op::Distance,
            &[first.feature.clone(), edge.feature.clone()],
            &m,
        )
        .unwrap();
        assert!(d.value.is_finite());
        assert!(matches!(surface.feature, Feature::Surface { .. }));

        // A project with a measurement on the part, saved and opened again.
        let measurements = serde_json::to_string(&json!([{ "id": "m1", "tool": "point-edge", "features": [first.feature, edge.feature], "axes": [false, false, false], "diameter": false }])).unwrap();
        let file = dir.join(format!("{name}.3mf"));
        let mut meta = Map::new();
        meta.insert("path".into(), json!(file.to_string_lossy()));
        meta.insert("platform".into(), json!([400.0, 400.0, 400.0]));
        let t0 = Instant::now();
        jobs::save_project(
            decode(&bundle_of(meta, &[(&name, &mesh, m, Some(&measurements))])).unwrap(),
        )
        .unwrap();
        let saved = t0.elapsed();
        let t0 = Instant::now();
        let opened = decode(&jobs::open_project(&file, &mut Progress::silent()).unwrap()).unwrap();
        let reopened = t0.elapsed();
        assert_eq!(
            opened.meshes[0]
                .info
                .get("measurements")
                .and_then(Value::as_str),
            Some(measurements.as_str()),
            "{name}"
        );

        println!(
            "{name}: {} triangles; import {:.0} ms, statistics {:.0} ms, first pick {:.0} ms, edge {:.1} ms, surface {:.1} ms, wall {:.1} ms ({}), point to edge {:.3} mm, save {:.0} ms, open {:.0} ms; status closed {} oriented {} holes {}",
            local.triangles,
            imported.as_secs_f64() * 1e3,
            statistics.as_secs_f64() * 1e3,
            first_time.as_secs_f64() * 1e3,
            edge_time.as_secs_f64() * 1e3,
            surface_time.as_secs_f64() * 1e3,
            wall_time.as_secs_f64() * 1e3,
            thickness.map_or_else(|| wall.err().unwrap_or_default(), |v| format!("{v:.3} mm")),
            d.value,
            saved.as_secs_f64() * 1e3,
            reopened.as_secs_f64() * 1e3,
            row.closed,
            row.oriented,
            row.holes,
        );
    }
}
