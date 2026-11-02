//! Hollow through the jobs the window runs: the cavity preview, Auto holes, Apply with plugs, a
//! preview cancelled by the next one, and a preview that is no longer the one kept.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Bundle, OutMesh, decode, encode};
use tricky3d_lib::jobs::{self, CANCELLED, HollowCache, RepairJobs};
use tricky3d_lib::mesh::{IDENTITY, Mesh, make_box, stats};

fn request(meta: Value, mesh: Option<&Mesh>) -> Bundle {
    let meta: Map<String, Value> = meta.as_object().unwrap().clone();
    let meshes: Vec<OutMesh> = mesh
        .map(|m| {
            let mut info = Map::new();
            info.insert("name".into(), json!("cube"));
            info.insert("matrix".into(), json!(IDENTITY.to_vec()));
            OutMesh {
                info,
                positions: &m.positions,
                indices: &m.indices,
                edges: None,
            }
        })
        .into_iter()
        .collect();
    decode(&encode(&meta, &meshes)).unwrap()
}

fn settings(thickness: f64) -> Value {
    json!({"thickness": thickness, "accuracy": 0.5, "smoothness": 1})
}

fn preview(jobs: &RepairJobs, cache: &HollowCache, job: u64, key: u64, t: f64) -> Bundle {
    let cube = make_box([0.0; 3], [40.0; 3]);
    let bytes = jobs::hollow_preview(
        request(
            json!({"job": job, "key": key, "settings": settings(t)}),
            Some(&cube),
        ),
        jobs,
        cache,
        &mut |_| {},
    )
    .unwrap();
    decode(&bytes).unwrap()
}

#[test]
fn preview_auto_holes_and_apply_with_plugs() {
    let jobs = RepairJobs::default();
    let cache = HollowCache::default();
    let answer = preview(&jobs, &cache, 1, 7, 2.0);
    assert_eq!(answer.meta["key"], json!(7));
    assert_eq!(answer.meta["partVolume"], json!(64_000.0));
    let cavity = answer.meta["cavityVolume"].as_f64().unwrap();
    assert!(
        (cavity - 36f64.powi(3)).abs() < 0.01 * 36f64.powi(3),
        "{cavity}"
    );
    assert_eq!(answer.meta["thin"], Value::Null);
    assert!((stats(&answer.meshes[0].mesh).volume - cavity).abs() < 1e-6);
    // Auto holes: two, at the floor.
    let template = json!({"at": [0.0, 0.0, 0.0], "normal": [0.0, 0.0, -1.0], "diameter": 3.0,
                          "taper": 5.0, "flare": 0.3});
    let placed = decode(
        &jobs::hollow_holes(
            request(
                json!({"key": 7, "holes": [], "auto": {"count": 2, "template": template}}),
                None,
            ),
            &cache,
        )
        .unwrap(),
    )
    .unwrap();
    let holes = placed.meta["holes"].as_array().unwrap().clone();
    assert_eq!(holes.len(), 2);
    assert!(
        placed.meta["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["ok"] == json!(true))
    );
    assert_eq!(placed.meshes.len(), 2);
    assert!(placed.meshes.iter().all(|m| m.mesh.triangle_count() > 0));
    // A third hole in solid wall is refused, and Apply leaves it out.
    let mut with_bad = holes.clone();
    with_bad.push(
        json!({"at": [1.0, 1.0, 0.0], "normal": [0.0, 0.0, -1.0], "diameter": 1.0,
                         "taper": 0.0, "flare": 0.0}),
    );
    let applied = decode(
        &jobs::hollow_apply(
            request(
                json!({"job": 2, "key": 7, "holes": with_bad, "plugs": 0.15}),
                None,
            ),
            &jobs,
            &cache,
            &mut |_| {},
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(applied.meta["skipped"][0]["index"], json!(2));
    assert_eq!(applied.meta["plugs"], json!(2));
    assert_eq!(applied.meshes.len(), 3);
    let hollow = &applied.meshes[0];
    assert!(hollow.info.get("stats").is_some());
    let s = stats(&hollow.mesh);
    assert_eq!((s.open_edges, s.non_manifold_edges, s.shells), (0, 0, 1));
    assert!(s.volume < 64_000.0 - cavity);
    for plug in &applied.meshes[1..] {
        let p = stats(&plug.mesh);
        assert!(
            p.volume > 0.0 && p.open_edges == 0 && p.shells == 1,
            "{p:?}"
        );
    }
    assert!(jobs.is_idle());
}

#[test]
fn a_new_preview_cancels_the_one_running_and_a_stale_key_is_refused() {
    let jobs = RepairJobs::default();
    let cache = HollowCache::default();
    let cube = make_box([0.0; 3], [40.0; 3]);
    // The window cancels the running preview as soon as the thickness changes (here: at its first
    // progress event), then starts the next.
    let stop = AtomicBool::new(false);
    let first = jobs::hollow_preview(
        request(
            json!({"job": 10, "key": 1, "settings": settings(2.0)}),
            Some(&cube),
        ),
        &jobs,
        &cache,
        &mut |_| {
            if !stop.swap(true, Ordering::Relaxed) {
                jobs.cancel(10);
            }
        },
    );
    assert_eq!(first.unwrap_err(), CANCELLED);
    // Nothing was kept from the cancelled run.
    let none = jobs::hollow_holes(request(json!({"key": 1, "holes": []}), None), &cache);
    assert!(
        none.unwrap_err()
            .starts_with("The cavity changed meanwhile")
    );
    let answer = preview(&jobs, &cache, 11, 2, 3.0);
    let cavity = answer.meta["cavityVolume"].as_f64().unwrap();
    assert!(
        (cavity - 34f64.powi(3)).abs() < 0.01 * 34f64.powi(3),
        "{cavity}"
    );
    // Holes for the old key are refused once the new preview is kept.
    let stale = jobs::hollow_holes(request(json!({"key": 1, "holes": []}), None), &cache);
    assert!(stale.is_err());
    assert!(jobs::hollow_holes(request(json!({"key": 2, "holes": []}), None), &cache).is_ok());
    // A cancel before the run starts.
    jobs.cancel(12);
    let early = jobs::hollow_preview(
        request(
            json!({"job": 12, "key": 3, "settings": settings(2.0)}),
            Some(&cube),
        ),
        &jobs,
        &cache,
        &mut |_| {},
    );
    assert_eq!(early.unwrap_err(), CANCELLED);
    assert!(jobs.is_idle());
}

#[test]
fn an_open_part_and_bad_settings_are_refused() {
    let jobs = RepairJobs::default();
    let cache = HollowCache::default();
    let mut open = make_box([0.0; 3], [40.0; 3]);
    open.indices.truncate(open.indices.len() - 3);
    let answer = decode(
        &jobs::hollow_preview(
            request(
                json!({"job": 1, "key": 1, "settings": settings(2.0)}),
                Some(&open),
            ),
            &jobs,
            &cache,
            &mut |_| {},
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(answer.meta["refused"]["closed"], json!(false));
    assert!(answer.meshes.is_empty());
    let cube = make_box([0.0; 3], [40.0; 3]);
    let err = jobs::hollow_preview(
        request(
            json!({"job": 2, "key": 2, "settings": {"thickness": 2.0, "accuracy": 1.5,
                                                          "smoothness": 0}}),
            Some(&cube),
        ),
        &jobs,
        &cache,
        &mut |_| {},
    )
    .unwrap_err();
    assert!(
        err.starts_with("Enter an accuracy of at most half the thickness"),
        "{err}"
    );
    assert!(jobs.is_idle());
}
