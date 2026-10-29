//! Generate shell on your own model files, through the job the `shell_generate` command runs. Skipped
//! unless pointed at models; meant for scripts/corpus/capped.ps1, one model and one kind a run:
//!
//! ```sh
//! TRICKY3D_MODELS="a.stl|b.stl" TRICKY3D_SHELL_KIND=hollow-part \
//!   cargo test --release --manifest-path src-tauri/Cargo.toml --test shell_models -- --ignored --nocapture
//! ```
//!
//! TRICKY3D_SHELL_THICKNESS and TRICKY3D_SHELL_ACCURACY (mm) set the shell; without them the
//! thickness is a 40th of the part's box diagonal (TRICKY3D_SHELL_SHARE sets another share), 0.2
//! to 4 mm, and the accuracy a third of it,
//! coarser where the part is so big that the grid would pass 150 million points.
//! TRICKY3D_SHELL_REPAIR=default (or extended) runs that repair script on the part first, as the
//! window runs it, for a model Generate shell would refuse. Each model prints
//! one line, `SHELL {json}`: the outcome, the times, and the result checked again here (closed,
//! oriented, crossings among the triangles the run made).

mod common;

use std::path::Path;
use std::time::Instant;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{BlobRef, OutMesh, decode, encode, encode_with_extra};
use tricky3d_lib::formats::import_file;
use tricky3d_lib::jobs::{self, RepairJobs};
use tricky3d_lib::mesh::{IDENTITY, Mesh, bounds, stats};
use tricky3d_lib::progress::Progress;
use tricky3d_lib::repair::defaults::{DEFAULT_ID, EXTENDED_ID, builtin_script};
use tricky3d_lib::repair::{silent, status, tritri};

/// Most grid points the automatic accuracy allows.
const POINTS: f64 = 1.5e8;

fn env_mm(key: &str) -> Option<f64> {
    std::env::var(key)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
}

/// The part after the repair script `script` ("default" or "extended"), run as the window runs it.
fn repaired(mesh: Mesh, script: &str) -> Mesh {
    let id = if script == "extended" {
        EXTENDED_ID
    } else {
        DEFAULT_ID
    };
    let steps = serde_json::to_value(&builtin_script(id).unwrap().actions).unwrap();
    let mut meta = Map::new();
    meta.insert("job".into(), json!(1u64));
    meta.insert("label".into(), json!(script));
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
        mesh
    }
}

/// The thickness and accuracy for a part of this box size.
fn shell_size(size: [f64; 3]) -> (f64, f64) {
    let diagonal = size.iter().map(|x| x * x).sum::<f64>().sqrt();
    let share = env_mm("TRICKY3D_SHELL_SHARE").unwrap_or(40.0);
    let mut t = env_mm("TRICKY3D_SHELL_THICKNESS").unwrap_or((diagonal / share).clamp(0.2, 4.0));
    let mut h = env_mm("TRICKY3D_SHELL_ACCURACY").unwrap_or(t / 3.0);
    let volume: f64 = size.iter().map(|x| x + 2.0 * t).product();
    h = h.max((volume / POINTS).cbrt()).max(0.01);
    if h > t / 2.0 {
        t = 2.0 * h;
    }
    (t, h)
}

fn one_model(path: &Path, kind: &str, job: u64) -> Value {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let start = Instant::now();
    let imported = match import_file(path, true, &mut Progress::silent()) {
        Ok(i) => i,
        Err(e) => {
            return json!({"model": name, "kind": kind, "outcome": "import-error", "error": e});
        }
    };
    let meshes: Vec<&Mesh> = imported.parts.iter().map(|p| &p.mesh).collect();
    let mesh = tricky3d_lib::ops::concat(&meshes).unwrap();
    let import_seconds = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let script = std::env::var("TRICKY3D_SHELL_REPAIR").ok();
    let mesh = match &script {
        Some(script) => repaired(mesh, script),
        None => mesh,
    };
    let repair_seconds = script.is_some().then(|| start.elapsed().as_secs_f64());
    let Some(b) = bounds(&mesh) else {
        return json!({"model": name, "kind": kind, "outcome": "empty-input"});
    };
    let size = [0, 1, 2].map(|k| (b[1][k] - b[0][k]) as f64);
    let (thickness, accuracy) = shell_size(size);
    let settings =
        json!({"kind": kind, "thickness": thickness, "accuracy": accuracy, "smooth": false});
    let mut info = Map::new();
    info.insert("name".into(), json!(name));
    info.insert("matrix".into(), json!(IDENTITY.to_vec()));
    let out = [OutMesh {
        info,
        positions: &mesh.positions,
        indices: &mesh.indices,
        edges: None,
    }];
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("settings".into(), settings);
    let request = decode(&encode(&meta, &out)).unwrap();
    let jobs = RepairJobs::default();
    let start = Instant::now();
    let mut stages: Vec<(String, f64)> = Vec::new();
    let result = jobs::shell_generate(request, &jobs, &mut |p| {
        if let Some(stage) = &p.stage
            && stages.last().is_none_or(|(s, _)| s != stage)
        {
            stages.push((stage.clone(), start.elapsed().as_secs_f64()));
        }
    });
    let seconds = start.elapsed().as_secs_f64();
    let mut row = json!({
        "model": name,
        "kind": kind,
        "triangles": mesh.triangle_count(),
        "area": stats(&mesh).area,
        "size": size,
        "thickness": thickness,
        "accuracy": accuracy,
        "importSeconds": import_seconds,
        "repair": script,
        "repairSeconds": repair_seconds,
        "seconds": seconds,
        "stages": stages,
    });
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            row["outcome"] = json!("error");
            row["error"] = json!(e);
            return row;
        }
    };
    let answer = decode(&bytes).unwrap();
    if let Some(refused) = answer.meta.get("refused") {
        row["outcome"] = json!("refused");
        row["refused"] = refused.clone();
        return row;
    }
    let warnings = answer.meta["results"][0]["warnings"].clone();
    row["warnings"] = warnings;
    let Some(made) = answer.meshes.into_iter().next() else {
        row["outcome"] = json!("empty");
        return row;
    };
    let mut result = made.mesh;
    // Checked again here, the hollowing shell the right way out.
    if kind == "hollowing-shell" {
        for t in result.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
    let start = Instant::now();
    let s = silent(|c| status::analyze(&result, c)).unwrap().status;
    let first_new = if kind.starts_with("hollow-part") {
        mesh.triangle_count() as u32
    } else {
        0
    };
    let pairs = silent(|c| tritri::intersecting_pairs(&result, c)).unwrap();
    let new_crossings = pairs
        .iter()
        .filter(|&&(a, b)| a >= first_new || b >= first_new)
        .count();
    row["checkSeconds"] = json!(start.elapsed().as_secs_f64());
    row["resultTriangles"] = json!(result.triangle_count());
    row["volume"] = json!(stats(&result).volume);
    row["inputVolume"] = json!(stats(&mesh).volume);
    row["closed"] = json!(s.closed);
    row["oriented"] = json!(s.oriented);
    row["newCrossings"] = json!(new_crossings);
    row["oldCrossings"] = json!(pairs.len() - new_crossings);
    let sound = s.closed && s.oriented && new_crossings == 0;
    row["outcome"] = json!(if sound { "made" } else { "unsound" });
    row
}

#[test]
#[ignore]
fn shell_on_model_files() {
    let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
        eprintln!("TRICKY3D_MODELS is not set: nothing to do");
        return;
    };
    let kind = std::env::var("TRICKY3D_SHELL_KIND").unwrap_or_else(|_| "hollow-part".into());
    for (n, path) in list.split('|').filter(|p| !p.trim().is_empty()).enumerate() {
        let row = one_model(Path::new(path.trim()), &kind, n as u64 + 1);
        println!("SHELL {row}");
        assert_ne!(row["outcome"], json!("unsound"), "{row}");
    }
}

/// The automatic size rule keeps the grid under its budget and the accuracy under half the
/// thickness.
#[test]
fn automatic_shell_sizes() {
    let (t, h) = shell_size([20.0; 3]);
    assert_close(t, (1200.0f64).sqrt() / 40.0, 1e-12, "thickness");
    assert_close(h, t / 3.0, 1e-12, "accuracy");
    // A big part: the accuracy grows to keep the grid in budget, and the thickness with it.
    let (t, h) = shell_size([2000.0, 1000.0, 500.0]);
    assert!(t >= 4.0 && h > 4.0 / 3.0, "{t} {h}");
    let points: f64 = [2008.0, 1008.0, 508.0].iter().map(|x| x / h).product();
    assert!(points <= POINTS * 1.0001, "{points}");
    assert!(h <= t / 2.0);
}
