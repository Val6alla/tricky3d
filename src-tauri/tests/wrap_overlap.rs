//! Extended repair on one part made of many closed, outward-facing shells that overlap heavily,
//! like a CAD model whose parts were joined with Merge parts: cylinders through boxes, angled
//! plates through cylinders, coaxial cylinders of the same radius over the same length, and caps
//! lying in one plane. Wrap part surface must turn it into one closed shell.

mod common;

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{BlobRef, Bundle, OutMesh, decode, encode_with_extra};
use tricky3d_lib::jobs::{self, RepairJobs};
use tricky3d_lib::mesh::{Mesh, Welder, stats};
use tricky3d_lib::ops::concat;
use tricky3d_lib::repair::ActionId;
use tricky3d_lib::repair::defaults::{EXTENDED_ID, builtin_script};

type P3 = [f64; 3];

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: P3) -> P3 {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    a.map(|x| x / n)
}

fn f32s(p: P3) -> [f32; 3] {
    p.map(|x| x as f32)
}

/// A closed cylinder from `a` to `b` facing out: `segments` round, `rows` along, the caps fanned
/// from their centres, the ring starting at angle `phase`.
fn cylinder(a: P3, b: P3, radius: f64, segments: usize, rows: usize, phase: f64) -> Mesh {
    let d = sub(b, a);
    let axis = unit(d);
    let helper = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = unit(cross(axis, helper));
    let v = cross(axis, u);
    let point = |i: usize, j: usize| -> [f32; 3] {
        let t = i as f64 / rows as f64;
        let angle = phase + TAU * (j % segments) as f64 / segments as f64;
        let (s, c) = angle.sin_cos();
        f32s([0, 1, 2].map(|k| a[k] + d[k] * t + radius * (c * u[k] + s * v[k])))
    };
    let mut w = Welder::new(0);
    for i in 0..rows {
        for j in 0..segments {
            let (p, q, r, s) = (
                point(i, j),
                point(i, j + 1),
                point(i + 1, j + 1),
                point(i + 1, j),
            );
            w.triangle(p, q, r);
            w.triangle(p, r, s);
        }
    }
    let (bottom, top) = (f32s(a), f32s(b));
    for j in 0..segments {
        w.triangle(bottom, point(0, j + 1), point(0, j));
        w.triangle(top, point(rows, j), point(rows, j + 1));
    }
    w.finish().unwrap()
}

/// A closed box with half-sizes `half` and `n` × `n` quads per face, turned by `degrees` about
/// `axis` and centred at `center`, facing out.
fn slab(center: P3, half: P3, axis: P3, degrees: f64, n: usize) -> Mesh {
    let k = unit(axis);
    let (s, c) = degrees.to_radians().sin_cos();
    // Rodrigues' rotation: proper, so the faces keep facing out.
    let turn = |p: P3| -> P3 {
        let kxp = cross(k, p);
        let kdp = k[0] * p[0] + k[1] * p[1] + k[2] * p[2];
        [0, 1, 2].map(|i| p[i] * c + kxp[i] * s + k[i] * kdp * (1.0 - c))
    };
    let place = |p: P3| {
        let r = turn([p[0] * half[0], p[1] * half[1], p[2] * half[2]]);
        f32s([r[0] + center[0], r[1] + center[1], r[2] + center[2]])
    };
    let lerp = |i: usize| -1.0 + 2.0 * i as f64 / n as f64;
    let mut w = Welder::new(0);
    for a in 0..3 {
        let (b, cc) = ((a + 1) % 3, (a + 2) % 3);
        for side in [-1.0, 1.0] {
            for i in 0..n {
                for j in 0..n {
                    let corner = |di: usize, dj: usize| -> [f32; 3] {
                        let mut p = [0.0; 3];
                        p[a] = side;
                        p[b] = lerp(i + di);
                        p[cc] = lerp(j + dj);
                        place(p)
                    };
                    let (p, q, r, t) = (corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1));
                    if side > 0.0 {
                        w.triangle(p, q, r);
                        w.triangle(p, r, t);
                    } else {
                        w.triangle(p, r, q);
                        w.triangle(p, t, r);
                    }
                }
            }
        }
    }
    w.finish().unwrap()
}

/// A made-up machine on a pedestal, built from cylinders and slabs: 25 closed shells that overlap
/// each other.
/// `detail` scales every segment and row count (1.0: about 560k triangles).
fn synthetic_gun(detail: f64) -> Vec<Mesh> {
    let n = |count: usize| ((count as f64 * detail).round() as usize).max(3);
    let rows = |count: usize| ((count as f64 * detail).round() as usize).max(1);
    let x = [1.0, 0.0, 0.0];
    let y = [0.0, 1.0, 0.0];
    let z = [0.0, 0.0, 1.0];
    let mut parts = vec![
        // Pedestal: the column and its flange share the bottom plane.
        cylinder(
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 40.0],
            6.0,
            n(720),
            rows(40),
            0.0,
        ),
        cylinder([0.0, 0.0, 0.0], [0.0, 0.0, 3.0], 14.0, n(720), rows(2), 0.0),
        // Column head and the trunnion tube through it, with a cap over each tube end.
        cylinder(
            [0.0, 0.0, 38.0],
            [0.0, 0.0, 44.0],
            7.0,
            n(720),
            rows(6),
            0.01,
        ),
        cylinder(
            [-12.0, 0.0, 41.0],
            [12.0, 0.0, 41.0],
            3.0,
            n(512),
            rows(48),
            0.0,
        ),
        cylinder(
            [-13.0, 0.0, 41.0],
            [-10.0, 0.0, 41.0],
            3.5,
            n(512),
            rows(4),
            0.0,
        ),
        cylinder(
            [10.0, 0.0, 41.0],
            [13.0, 0.0, 41.0],
            3.5,
            n(512),
            rows(4),
            0.0,
        ),
        // Cradle.
        slab([0.0, 0.0, 44.5], [5.0, 4.0, 3.0], z, 0.0, n(40)),
        // Barrel, jacket, and a spring sleeve of the jacket's radius over part of its length:
        // their round faces lie exactly on each other there.
        cylinder(
            [-40.0, 0.0, 46.0],
            [50.0, 0.0, 46.0],
            1.2,
            n(256),
            rows(300),
            0.0,
        ),
        cylinder(
            [-8.0, 0.0, 46.0],
            [25.0, 0.0, 46.0],
            2.0,
            n(256),
            rows(80),
            0.0,
        ),
        cylinder(
            [-8.0, 0.0, 46.0],
            [10.0, 0.0, 46.0],
            2.0,
            n(256),
            rows(40),
            0.0,
        ),
        cylinder(
            [48.0, 0.0, 46.0],
            [54.0, 0.0, 46.0],
            1.8,
            n(256),
            rows(12),
            0.0,
        ),
        // Ammo drum, slightly tilted, through the jacket and the cradle, with its hub.
        cylinder(
            [0.5, 0.0, 47.0],
            [0.0, 0.3, 51.5],
            7.0,
            n(720),
            rows(8),
            0.0,
        ),
        cylinder(
            [0.45, 0.0, 46.5],
            [-0.05, 0.33, 52.5],
            2.0,
            n(128),
            rows(6),
            0.0,
        ),
        // Shield: a tilted main plate the barrel passes through, two wings and a top lip.
        slab([-6.0, 0.0, 52.0], [0.4, 20.0, 14.0], y, 8.0, n(60)),
        slab([-3.0, 21.0, 52.0], [0.4, 6.0, 14.0], z, 30.0, n(40)),
        slab([-3.0, -21.0, 52.0], [0.4, 6.0, 14.0], z, -30.0, n(40)),
        slab([-4.5, 0.0, 65.5], [2.0, 20.5, 0.4], x, 0.0, n(40)),
        // Shoulder rest arms and pads, and a handle through each pad.
        cylinder(
            [-4.0, 3.0, 44.0],
            [-18.0, 7.0, 40.0],
            0.8,
            n(96),
            rows(30),
            0.0,
        ),
        cylinder(
            [-4.0, -3.0, 44.0],
            [-18.0, -7.0, 40.0],
            0.8,
            n(96),
            rows(30),
            0.0,
        ),
        slab([-18.0, 7.0, 40.0], [1.0, 2.5, 3.0], z, 20.0, n(20)),
        slab([-18.0, -7.0, 40.0], [1.0, 2.5, 3.0], z, -20.0, n(20)),
        cylinder(
            [-18.0, 4.0, 37.0],
            [-18.0, 4.0, 43.0],
            0.5,
            n(64),
            rows(10),
            0.0,
        ),
        cylinder(
            [-18.0, -4.0, 37.0],
            [-18.0, -4.0, 43.0],
            0.5,
            n(64),
            rows(10),
            0.0,
        ),
        // Sight bar on the jacket, and the elevation arc on the column head.
        slab([8.0, 0.0, 48.2], [6.0, 0.3, 0.6], z, 0.0, n(30)),
        slab([0.0, -6.5, 41.0], [4.0, 0.5, 4.0], y, 15.0, n(30)),
    ];
    parts.shrink_to_fit();
    parts
}

fn joined(parts: &[Mesh]) -> Mesh {
    let refs: Vec<&Mesh> = parts.iter().collect();
    concat(&refs).unwrap()
}

/// A repair_run request as the window sends it: run `actions` on `mesh`, nothing selected.
fn run_request(job: u64, label: &str, actions: Value, mesh: &Mesh) -> Bundle {
    let mut meta = Map::new();
    meta.insert("job".into(), json!(job));
    meta.insert("label".into(), json!(label));
    meta.insert("actions".into(), actions);
    meta.insert("withStatus".into(), json!(true));
    let out = [OutMesh {
        info: Map::new(),
        positions: &mesh.positions,
        indices: &mesh.indices,
        edges: None,
    }];
    decode(&encode_with_extra(
        &meta,
        &out,
        &[vec![("selection", BlobRef::U32(&[]))]],
    ))
    .unwrap()
}

/// Runs Extended repair on `mesh` through repair_run, as the window sends it: the result mesh,
/// each action's report, and whether the run changed anything.
fn extended_repair(mesh: &Mesh) -> (Mesh, Vec<Value>, bool) {
    let script = builtin_script(EXTENDED_ID).unwrap();
    let request = run_request(1, &script.name, json!(script.actions), mesh);
    let bytes = jobs::repair_run(request, &RepairJobs::default(), &mut |_| {}).unwrap();
    let mut answer = decode(&bytes).unwrap();
    let changed = answer.meta["changed"] == true;
    let reports = answer.meta["reports"].as_array().unwrap().clone();
    let result = answer.meshes.remove(0).mesh;
    (
        if changed { result } else { mesh.clone() },
        reports,
        changed,
    )
}

fn report<'a>(reports: &'a [Value], action: &str) -> &'a Value {
    reports.iter().find(|r| r["action"] == action).unwrap()
}

fn assert_parts_are_closed_and_outward(parts: &[Mesh]) {
    for (i, part) in parts.iter().enumerate() {
        let s = stats(part);
        assert_eq!(
            (s.shells, s.open_edges, s.non_manifold_edges),
            (1, 0, 0),
            "part {i}"
        );
        assert!(s.volume > 0.0, "part {i} faces in: {}", s.volume);
    }
}

fn assert_wrapped_into_one_shell(parts: &[Mesh]) -> (Mesh, Vec<Value>) {
    assert_parts_are_closed_and_outward(parts);
    let input = joined(parts);
    assert_eq!(stats(&input).shells as usize, parts.len());
    let (out, reports, changed) = extended_repair(&input);
    let wrap = report(&reports, "wrap_part_surface");
    assert_eq!(
        wrap["warnings"],
        json!([]),
        "Wrap part surface: {}",
        wrap["message"]
    );
    assert!(changed);
    let s = stats(&out);
    assert_eq!(
        (s.shells, s.open_edges, s.non_manifold_edges),
        (1, 0, 0),
        "{reports:#?}"
    );
    let largest = parts.iter().map(|p| stats(p).volume).fold(0.0, f64::max);
    let sum: f64 = parts.iter().map(|p| stats(p).volume).sum();
    assert!(
        s.volume > largest && s.volume < sum,
        "volume {} not in {largest}..{sum}",
        s.volume
    );
    (out, reports)
}

#[test]
fn overlapping_closed_shells_in_one_part_wrap_into_one_shell() {
    let parts = synthetic_gun(0.1);
    assert_eq!(parts.len(), 25);
    assert_wrapped_into_one_shell(&parts);
}

/// Cancel in the middle of Wrap part surface, at several of its steps: the run fails with exactly
/// "cancelled" (the window then keeps its working copy as it was), stops within a second in a
/// release build, and leaves nothing registered. Each step reports its name, so the progress
/// dialog can show it.
#[test]
fn cancelling_in_the_middle_of_wrap_stops_within_a_second() {
    let input = joined(&synthetic_gun(0.1));
    let wrap = json!([{ "type": "wrap_part_surface", "enabled": true, "params": {} }]);
    // Debug builds run the same checks some tens of times slower.
    let limit = if cfg!(debug_assertions) {
        Duration::from_secs(20)
    } else {
        Duration::from_secs(1)
    };
    let mut seen: Vec<String> = Vec::new();
    for (job, stage) in [
        "Splitting the triangles where they cross",
        "Telling inside from outside",
        "Building the outer surface",
        "Rounding to the grid",
        "Wrapping the crossings left again",
    ]
    .into_iter()
    .enumerate()
    {
        let jobs = RepairJobs::default();
        let job = job as u64 + 1;
        let mut pressed: Option<Instant> = None;
        let result = jobs::repair_run(
            run_request(job, "Wrap part surface", wrap.clone(), &input),
            &jobs,
            &mut |p| {
                if let Some(d) = &p.detail
                    && !seen.contains(d)
                {
                    seen.push(d.clone());
                }
                if pressed.is_none() && p.detail.as_deref() == Some(stage) {
                    pressed = Some(Instant::now());
                    jobs.cancel(job);
                }
            },
        );
        let stopped = pressed.expect(stage).elapsed();
        assert_eq!(result.err().as_deref(), Some("cancelled"), "{stage}");
        assert!(stopped < limit, "{stage}: stopped after {stopped:?}");
        assert!(jobs.is_idle());
    }
    assert!(
        seen.iter().any(|d| d == "Closing and orienting the shells"),
        "{seen:?}"
    );
    // Not cancelled, the same run finishes and changes the part.
    let jobs = RepairJobs::default();
    let bytes = jobs::repair_run(
        run_request(9, "Wrap part surface", wrap, &input),
        &jobs,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(decode(&bytes).unwrap().meta["changed"], true);
}

/// The full-size model (25 shells, about 560k triangles), with timings:
/// `cargo test --release --test wrap_overlap -- --ignored --nocapture`. Set
/// TRICKY3D_SYNTHETIC_OUT to a .stl path to also write the model there, for trying it in the app.
#[test]
#[ignore]
fn overlapping_closed_shells_full_size() {
    let parts = synthetic_gun(1.0);
    let input = joined(&parts);
    let triangles = input.triangle_count();
    eprintln!("{} shells, {triangles} triangles", parts.len());
    assert!(triangles >= 500_000);
    if let Ok(path) = std::env::var("TRICKY3D_SYNTHETIC_OUT") {
        let named = [tricky3d_lib::formats::NamedMesh {
            name: "synthetic gun",
            mesh: &input,
        }];
        tricky3d_lib::formats::export_file(
            std::path::Path::new(&path),
            tricky3d_lib::formats::ExportFormat::StlBinary,
            &named,
        )
        .unwrap();
        eprintln!("wrote {path}");
    }
    let started = Instant::now();
    let (out, reports) = assert_wrapped_into_one_shell(&parts);
    eprintln!(
        "took {:.2?}, {} triangles",
        started.elapsed(),
        out.triangle_count()
    );
    for r in &reports {
        eprintln!(
            "  {} ({} ms): {} {}",
            r["label"], r["millis"], r["message"], r["warnings"]
        );
    }
}

/// The smallest part of the model that failed: a tube whose top line lies 2.4e-16 mm from the
/// jacket's bottom line, a sleeve lying exactly on the jacket, and a tilted plate through all of
/// them. The plate is cut into slivers too thin for an exact probe point between the jacket and
/// the sleeve; their winding numbers came from a guessed point and left the surface open.
#[test]
fn slivers_between_coincident_layers_get_exact_winding_numbers() {
    let y = [0.0, 1.0, 0.0];
    let parts = [
        cylinder([-12.0, 0.0, 41.0], [12.0, 0.0, 41.0], 3.0, 51, 5, 0.0),
        cylinder([-8.0, 0.0, 46.0], [25.0, 0.0, 46.0], 2.0, 26, 8, 0.0),
        cylinder([-8.0, 0.0, 46.0], [10.0, 0.0, 46.0], 2.0, 26, 4, 0.0),
        slab([-6.0, 0.0, 52.0], [0.4, 20.0, 14.0], y, 8.0, 6),
    ];
    assert_wrapped_into_one_shell(&parts);
}

/// Extended repair on the model files listed in TRICKY3D_MODELS (paths separated by '|'), each part
/// baked into one mesh as Merge parts would, with how long each action and each step of Wrap part
/// surface took (the steps the progress dialog shows):
/// `TRICKY3D_MODELS="a.stl|b.3mf" cargo test --release --test wrap_overlap -- --ignored
/// --nocapture extended_repair_on_model_files`. The files are only read.
#[test]
#[ignore]
fn extended_repair_on_model_files() {
    let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
        return;
    };
    for path in list.split('|').filter(|p| !p.is_empty()) {
        let imported = tricky3d_lib::formats::import_file(
            std::path::Path::new(path),
            true,
            &mut tricky3d_lib::progress::Progress::silent(),
        )
        .unwrap_or_else(|e| panic!("{path}: {e}"));
        let baked: Vec<Mesh> = imported
            .parts
            .iter()
            .map(|p| {
                tricky3d_lib::mesh::bake(&p.mesh, &p.matrix.unwrap_or(tricky3d_lib::mesh::IDENTITY))
            })
            .collect();
        let input = joined(&baked);
        let script = builtin_script(EXTENDED_ID).unwrap();
        let request = run_request(1, &script.name, json!(script.actions), &input);
        let mut steps: Vec<(String, Instant)> = Vec::new();
        let started = Instant::now();
        let bytes = jobs::repair_run(request, &RepairJobs::default(), &mut |p| {
            let name = match &p.detail {
                Some(d) => format!("{}: {d}", p.label),
                None => p.label.clone(),
            };
            if steps.last().is_none_or(|(n, _)| *n != name) {
                steps.push((name, Instant::now()));
            }
        })
        .unwrap();
        let took = started.elapsed();
        let mut answer = decode(&bytes).unwrap();
        let out = answer.meshes.remove(0).mesh;
        let s = stats(&out);
        eprintln!(
            "{path}\n  {} -> {} triangles in {took:.2?}: {} shells, {} open, {} non-manifold, volume {:.3}",
            input.triangle_count(),
            out.triangle_count(),
            s.shells,
            s.open_edges,
            s.non_manifold_edges,
            s.volume
        );
        for (i, (name, at)) in steps.iter().enumerate() {
            let end = steps.get(i + 1).map_or(started + took, |next| next.1);
            if name.starts_with("Wrap part surface") {
                eprintln!("    {name}: {:.2?}", end - *at);
            }
        }
        for r in answer.meta["reports"].as_array().unwrap() {
            eprintln!(
                "  {} ({} ms): {} {}",
                r["label"], r["millis"], r["message"], r["warnings"]
            );
        }
    }
}

/// The mesh the Repair window works on after the user imports `path`, answers "Place on platform"
/// with `place` and runs Merge parts on all of its parts, each step as the app takes it:
/// - import as the app first reads a file (`import_file` without joining an OBJ's objects);
/// - placement as src/io/files.ts `placedMatrices` does it: one translation for all parts, from
///   their world box, centring it on the default 200 mm platform in x and y, resting on z = 0;
/// - Merge parts through `jobs::merge_meshes` with each part's matrix in the bundle, as the window
///   sends it (a single part is refused, as the app refuses it, and stays as it is);
/// - entering repair (src/repair/session.ts): the part's matrix baked into world space and the
///   vertices the bake moved onto one position joined (`weldMesh`, the same as `join_coincident`).
fn as_the_app_sees_it(path: &str, place: bool) -> Mesh {
    use tricky3d_lib::mesh::{IDENTITY, Matrix, bake, join_coincident};
    let imported = tricky3d_lib::formats::import_file(
        std::path::Path::new(path),
        false,
        &mut tricky3d_lib::progress::Progress::silent(),
    )
    .unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut parts: Vec<(Mesh, Matrix)> = imported
        .parts
        .into_iter()
        .map(|p| (p.mesh, p.matrix.unwrap_or(IDENTITY)))
        .collect();
    if place {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for (mesh, m) in &parts {
            for q in mesh.positions.as_chunks::<3>().0 {
                let w = tricky3d_lib::mesh::transform_point(m, q.map(f64::from));
                for a in 0..3 {
                    lo[a] = lo[a].min(w[a]);
                    hi[a] = hi[a].max(w[a]);
                }
            }
        }
        let offset = [
            200.0 / 2.0 - (lo[0] + hi[0]) / 2.0,
            200.0 / 2.0 - (lo[1] + hi[1]) / 2.0,
            -lo[2],
        ];
        for (_, m) in &mut parts {
            for c in 0..4 {
                for a in 0..3 {
                    m[c * 4 + a] += offset[a] * m[c * 4 + 3];
                }
            }
        }
    }
    let mut meta = Map::new();
    meta.insert("job".into(), json!(1));
    let infos: Vec<Map<String, Value>> = parts
        .iter()
        .enumerate()
        .map(|(n, (_, m))| {
            let mut info = Map::new();
            info.insert("name".into(), json!(format!("Part {n}")));
            info.insert("matrix".into(), json!(m.to_vec()));
            info
        })
        .collect();
    let out: Vec<OutMesh> = parts
        .iter()
        .zip(infos)
        .map(|((mesh, _), info)| OutMesh {
            info,
            positions: &mesh.positions,
            indices: &mesh.indices,
            edges: None,
        })
        .collect();
    let request = decode(&encode_with_extra(
        &meta,
        &out,
        &vec![Vec::new(); out.len()],
    ))
    .unwrap();
    let (mesh, matrix) = match jobs::merge_meshes(request) {
        Ok(bytes) => (decode(&bytes).unwrap().meshes.remove(0).mesh, IDENTITY),
        Err(e) => {
            assert_eq!(parts.len(), 1, "{e}");
            parts.remove(0)
        }
    };
    let baked = bake(&mesh, &matrix);
    if matrix == IDENTITY {
        return baked;
    }
    join_coincident(&baked).unwrap_or(baked)
}

/// Extended repair through the app's path (`as_the_app_sees_it`) on the files in TRICKY3D_MODELS
/// (paths separated by '|'), left where the file put them and placed on the platform. Both must
/// come out closed and oriented, with Wrap part surface warning about nothing, no triangles
/// crossing, no shell of a single triangle, in the same number of shells, and no warning may
/// still say holes are open: where the part sits must not change what repair makes of it. A path
/// may end in `=n` to also require n shells. This caught two failures on placed models: Wrap lost
/// track of an edge it had left open, and the actions before it tore holes where the bake had put
/// two vertices on one position; and a model with noisy surfaces did not wrap at all, because
/// rounding to the grid stacked their layers so that splitting the edges cut it open.
/// `TRICKY3D_MODELS="a.stl=1|b.3mf" cargo test --release --test wrap_overlap -- --ignored
/// --nocapture extended_repair_through_the_app_path`. The files are only read.
#[test]
#[ignore]
fn extended_repair_through_the_app_path() {
    use tricky3d_lib::repair::topology::{Edges, Link, components};
    let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
        return;
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for entry in list.split('|').filter(|p| !p.is_empty()) {
        let (path, expected) = match entry.rsplit_once('=').map(|(p, n)| (p, n.parse::<u64>())) {
            Some((path, Ok(n))) => (path, Some(n)),
            _ => (entry, None),
        };
        let mut shells = Vec::new();
        for place in [false, true] {
            let input = as_the_app_sees_it(path, place);
            let started = Instant::now();
            let (out, reports, changed) = extended_repair(&input);
            let (a, b) = (stats(&input), stats(&out));
            eprintln!(
                "{path}, placed {place}: {} triangles, {} shells, {} open, {} non-manifold -> {} triangles, {} shells, {} open, {} non-manifold, volume {:.3}, in {:.2?}",
                input.triangle_count(),
                a.shells,
                a.open_edges,
                a.non_manifold_edges,
                out.triangle_count(),
                b.shells,
                b.open_edges,
                b.non_manifold_edges,
                b.volume,
                started.elapsed()
            );
            for r in &reports {
                eprintln!(
                    "  {} ({} ms): {} {} resolved {}",
                    r["label"], r["millis"], r["message"], r["warnings"], r["resolved"]
                );
            }
            let wrap = report(&reports, "wrap_part_surface");
            assert_eq!(wrap["warnings"], json!([]), "{path}: {}", wrap["message"]);
            assert!(changed);
            assert_eq!((b.open_edges, b.non_manifold_edges), (0, 0), "{path}");
            let mut control = tricky3d_lib::repair::Control::new(&cancel, |_| {});
            let status = tricky3d_lib::repair::status::analyze(&out, &mut control)
                .unwrap()
                .status;
            assert!(status.closed && status.oriented, "{path}: {status:?}");
            let crossing = tricky3d_lib::repair::tritri::intersecting_pairs(&out, &mut control)
                .unwrap()
                .len();
            assert_eq!(crossing, 0, "{path}: crossing pairs");
            let (shell_of, count) = components(&out, &Edges::new(&out), Link::Shell);
            let mut size = vec![0usize; count];
            for &s in &shell_of {
                size[s as usize] += 1;
            }
            assert!(!size.contains(&1), "{path}: a shell of one triangle");
            // The result is closed, so no warning may still say holes are open.
            for r in &reports {
                for w in r["warnings"].as_array().unwrap() {
                    let w = w.as_str().unwrap();
                    assert!(
                        !w.contains("could not be closed"),
                        "{path}: {}: {w}",
                        r["label"]
                    );
                }
            }
            shells.push(b.shells);
        }
        assert_eq!(shells[0], shells[1], "{path}");
        if let Some(expected) = expected {
            assert_eq!(shells[0], expected, "{path}");
        }
    }
}

/// What the corpus run checks of a result: closed, oriented, crossing pairs, shells of a single
/// triangle, shells.
fn result_check(mesh: &Mesh) -> Value {
    use tricky3d_lib::repair::topology::{Edges, Link, components};
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut control = tricky3d_lib::repair::Control::new(&cancel, |_| {});
    let status = tricky3d_lib::repair::status::analyze(mesh, &mut control)
        .unwrap()
        .status;
    let crossings = tricky3d_lib::repair::tritri::intersecting_pairs(mesh, &mut control)
        .unwrap()
        .len();
    let (shell_of, count) = components(mesh, &Edges::new(mesh), Link::Shell);
    let mut size = vec![0usize; count];
    for &s in &shell_of {
        size[s as usize] += 1;
    }
    let ok = status.closed && status.oriented && crossings == 0 && !size.contains(&1);
    json!({
        "ok": ok,
        "triangles": status.triangles,
        "closed": status.closed,
        "oriented": status.oriented,
        "crossings": crossings,
        "oneTriangleShells": size.iter().filter(|&&n| n == 1).count(),
        "shells": status.shells,
        "openEdges": status.border_edges,
        "nonManifoldEdges": status.non_manifold_edges,
        "volume": status.volume,
        "area": status.area,
    })
}

/// One file of the corpus run (scripts/corpus/repair-corpus.ps1): Extended repair through the app's
/// path on TRICKY3D_MODEL, placed on the platform, writing JSON lines to TRICKY3D_REPORT as it
/// goes (each step the progress dialog would show, so a run killed on its time limit still says
/// where it was), then one "result" line. When the result is not closed, oriented, free of
/// crossings and of single-triangle shells, the actions run again one by one and a "failedStep"
/// line names the action that left a clean mesh no longer clean (Wrap part surface when it never
/// was). TRICKY3D_OUT, when set, is a .stl path to write the result to; TRICKY3D_UNPLACED, when
/// set, leaves the model where the file has it. Never fails: the runner reads the lines.
#[test]
#[ignore]
fn extended_repair_report() {
    use std::io::Write;
    let (Ok(path), Ok(out)) = (
        std::env::var("TRICKY3D_MODEL"),
        std::env::var("TRICKY3D_REPORT"),
    ) else {
        return;
    };
    let mut log = std::fs::File::create(&out).unwrap();
    let mut line = |v: Value| {
        writeln!(log, "{v}").unwrap();
        log.flush().unwrap();
    };
    let started = Instant::now();
    let input = as_the_app_sees_it(&path, std::env::var("TRICKY3D_UNPLACED").is_err());
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut control = tricky3d_lib::repair::Control::new(&cancel, |_| {});
    let before = tricky3d_lib::repair::status::analyze(&input, &mut control)
        .unwrap()
        .status;
    line(
        json!({ "stage": "imported", "triangles": input.triangle_count(), "status": before, "at": started.elapsed().as_secs_f64() }),
    );
    let script = builtin_script(EXTENDED_ID).unwrap();
    let request = run_request(1, &script.name, json!(script.actions), &input);
    let mut last = String::new();
    let clock = Instant::now();
    let run = jobs::repair_run(request, &RepairJobs::default(), &mut |p| {
        let name = match &p.detail {
            Some(d) => format!("{}: {d}", p.label),
            None => p.label.clone(),
        };
        if name != last {
            line(json!({ "stage": name, "at": started.elapsed().as_secs_f64() }));
            last = name;
        }
    });
    let seconds = clock.elapsed().as_secs_f64();
    let bytes = match run {
        Ok(bytes) => bytes,
        Err(e) => {
            line(
                json!({ "result": { "ok": false, "error": e, "seconds": seconds, "failedStep": last } }),
            );
            return;
        }
    };
    let mut answer = decode(&bytes).unwrap();
    let changed = answer.meta["changed"] == true;
    let reports = answer.meta["reports"].as_array().unwrap().clone();
    let result = answer.meshes.remove(0).mesh;
    let result = if changed { result } else { input.clone() };
    let mut check = result_check(&result);
    check["seconds"] = json!(seconds);
    check["trianglesIn"] = json!(input.triangle_count());
    check["reports"] = json!(
        reports
            .iter()
            .map(|r| json!({ "action": r["action"], "changed": r["changed"], "ms": r["millis"], "message": r["message"], "warnings": r["warnings"] }))
            .collect::<Vec<_>>()
    );
    let ok = check["ok"] == true;
    line(json!({ "result": check }));
    if let Ok(path) = std::env::var("TRICKY3D_OUT") {
        let named = [tricky3d_lib::formats::NamedMesh {
            name: "repaired",
            mesh: &result,
        }];
        tricky3d_lib::formats::export_file(
            std::path::Path::new(&path),
            tricky3d_lib::formats::ExportFormat::StlBinary,
            &named,
        )
        .unwrap();
    }
    if !ok {
        // The same actions one by one (a script runs them with nothing selected, so each gets the
        // mesh the app's run gave it), checked after each.
        let mut current = input.clone();
        // The step that left a clean mesh no longer clean, or Wrap part surface (whose job is
        // to make it clean) when it never was.
        let mut failed = Value::Null;
        let mut clean = false;
        for step in &script.actions {
            let action = tricky3d_lib::repair::scripts::parse_action(step).unwrap();
            let label = tricky3d_lib::repair::defaults::label_of(step.action);
            let outcome = tricky3d_lib::repair::silent(|c| {
                tricky3d_lib::repair::run_action(&action, &current, &[], c)
            })
            .unwrap();
            if outcome.changed {
                current = outcome.mesh;
            }
            let after = result_check(&current);
            let now = after["ok"] == true;
            if (clean && !now) || (step.action == ActionId::WrapPartSurface && !now) {
                failed = json!({ "step": label, "after": after });
            }
            clean = now;
        }
        line(json!({ "failedStep": failed }));
    }
}
