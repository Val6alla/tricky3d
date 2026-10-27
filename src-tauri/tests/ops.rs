//! Split shells, Merge parts and Cut, through the same jobs the
//! commands run, on the committed fixtures. Regenerate the fixtures with
//! `cargo test --manifest-path src-tauri/Cargo.toml --test ops -- --ignored write_fixtures`.

mod common;

use std::collections::HashMap;
use std::io::Cursor;
use std::time::Instant;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{OutMesh, decode, encode};
use tricky3d_lib::formats::{ExportFormat, NamedMesh, import_file, write_export};
use tricky3d_lib::jobs;
use tricky3d_lib::mesh::{IDENTITY, Matrix, Mesh, Welder, analyze, bake, bounds, make_box, stats};
use tricky3d_lib::ops::cut::{CutPieces, Plane, cut};
use tricky3d_lib::ops::shells::split_shells;
use tricky3d_lib::ops::{concat, translate};
use tricky3d_lib::progress::Progress;

// ---------------------------------------------------------------------------------------------
// Fixtures

/// A sphere of `radius` around the origin: an icosahedron whose triangles are split in four
/// `levels` times, every new point pushed out onto the sphere (20 × 4^levels triangles).
fn icosphere(levels: u32, radius: f64) -> Mesh {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut points: Vec<[f64; 3]> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .map(unit)
    .to_vec();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..levels {
        let mut middle: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, points: &mut Vec<[f64; 3]>| {
            *middle.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let (p, q) = (points[a as usize], points[b as usize]);
                points.push(unit([p[0] + q[0], p[1] + q[1], p[2] + q[2]]));
                points.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let (ab, bc, ca) = (
                mid(a, b, &mut points),
                mid(b, c, &mut points),
                mid(c, a, &mut points),
            );
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    Mesh {
        positions: points
            .iter()
            .flat_map(|p| p.map(|v| (v * radius) as f32))
            .collect(),
        indices: faces.into_flattened(),
    }
}

fn unit(p: [f64; 3]) -> [f64; 3] {
    let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
    p.map(|v| v / l)
}

fn open_box() -> Mesh {
    // make_box lists the faces -z, +z, -y, +y, +x, -x: drop the two +z triangles.
    let mut mesh = make_box([0.0; 3], [20.0; 3]);
    mesh.indices.drain(6..12);
    mesh
}

fn file_bytes(format: ExportFormat, title: &str, parts: &[(&str, &Mesh)]) -> Vec<u8> {
    let named: Vec<NamedMesh> = parts
        .iter()
        .map(|&(name, mesh)| NamedMesh { name, mesh })
        .collect();
    write_export(Cursor::new(Vec::new()), format, title, &named)
        .unwrap()
        .into_inner()
}

/// Every fixture file and its bytes, built from the shapes the tests name.
fn fixture_files() -> Vec<(&'static str, Vec<u8>)> {
    let cube = make_box([0.0; 3], [20.0; 3]);
    let overlapping = make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0]);
    let disjoint = make_box([40.0, 0.0, 0.0], [60.0, 20.0, 20.0]);
    let sphere = icosphere(5, 10.0);
    use ExportFormat::{Obj, StlBinary};
    vec![
        (
            "cube_20mm.stl",
            file_bytes(StlBinary, "cube_20mm", &[("cube", &cube)]),
        ),
        (
            "two_cubes_overlapping.obj",
            file_bytes(Obj, "", &[("cube_a", &cube), ("cube_b", &overlapping)]),
        ),
        (
            "two_cubes_disjoint.obj",
            file_bytes(Obj, "", &[("cube_a", &cube), ("cube_b", &disjoint)]),
        ),
        (
            "open_box.stl",
            file_bytes(StlBinary, "open_box", &[("open box", &open_box())]),
        ),
        (
            "icosphere_20k.stl",
            file_bytes(StlBinary, "icosphere_20k", &[("sphere", &sphere)]),
        ),
    ]
}

#[test]
#[ignore]
fn write_fixtures() {
    for (name, bytes) in fixture_files() {
        std::fs::write(fixture(name), bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn fixtures_match_their_generator() {
    for (name, want) in fixture_files() {
        let mut got = std::fs::read(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        if name.ends_with(".obj") {
            // A Windows checkout may turn the text fixtures' line ends into CRLF.
            got = String::from_utf8(got)
                .unwrap()
                .replace("\r\n", "\n")
                .into_bytes();
        }
        assert!(got == want, "{name} differs from what write_fixtures makes");
    }
}

#[test]
fn icosphere_fixture_is_a_closed_sphere() {
    let parts = import("icosphere_20k.stl", false);
    let s = stats(&parts[0].1);
    assert_eq!(
        (
            s.triangles,
            s.vertices,
            s.shells,
            s.open_edges,
            s.non_manifold_edges
        ),
        (20_480, 10_242, 1, 0, 0)
    );
    let ball = 4.0 / 3.0 * std::f64::consts::PI * 1000.0;
    assert!(
        s.volume < ball && s.volume > 0.99 * ball,
        "volume {}",
        s.volume
    );
}

// ---------------------------------------------------------------------------------------------
// Helpers

/// (part name, mesh) per part, as the app imports the file.
fn import(name: &str, merge: bool) -> Vec<(String, Mesh)> {
    import_file(&fixture(name), merge, &mut Progress::silent())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .parts
        .into_iter()
        .map(|p| (p.name, p.mesh))
        .collect()
}

/// The bundle the window sends: one mesh per part with its name and matrix.
fn request(meta: Map<String, Value>, parts: &[(&str, &Mesh, Matrix)]) -> Vec<u8> {
    let out: Vec<OutMesh> = parts
        .iter()
        .map(|&(name, mesh, matrix)| {
            let mut info = Map::new();
            info.insert("name".into(), json!(name));
            if matrix != IDENTITY {
                info.insert("matrix".into(), json!(matrix.to_vec()));
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

fn plain(parts: &[(String, Mesh)]) -> Vec<(&str, &Mesh, Matrix)> {
    parts
        .iter()
        .map(|(name, mesh)| (name.as_str(), mesh, IDENTITY))
        .collect()
}

/// One result mesh as the window reads it.
struct Out {
    info: Map<String, Value>,
    mesh: Mesh,
    /// Floats in the problem-edge blob (6 per edge), None when the result has none.
    edge_floats: Option<u64>,
}

impl Out {
    fn stat(&self, key: &str) -> f64 {
        self.info["stats"][key]
            .as_f64()
            .unwrap_or_else(|| panic!("no stats.{key} in {:?}", self.info))
    }
}

fn read(bytes: &[u8]) -> (Map<String, Value>, Vec<Out>) {
    let h = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let header: Value = serde_json::from_slice(&bytes[4..4 + h]).unwrap();
    let bundle = decode(bytes).unwrap();
    let outs = bundle
        .meshes
        .into_iter()
        .enumerate()
        .map(|(k, m)| Out {
            info: m.info,
            mesh: m.mesh,
            edge_floats: header["meshes"][k]["edges"][1].as_u64(),
        })
        .collect();
    (bundle.meta, outs)
}

/// Checks a result's stats against the mesh it came with, and the problem edges against them.
fn assert_measured(out: &Out) {
    let s = stats(&out.mesh);
    assert_eq!(out.stat("triangles") as u64, s.triangles);
    assert_eq!(out.stat("shells") as u64, s.shells);
    assert_eq!(out.stat("openEdges") as u64, s.open_edges);
    assert_eq!(out.stat("nonManifoldEdges") as u64, s.non_manifold_edges);
    assert_eq!(out.stat("volume"), s.volume);
    assert_eq!(
        out.edge_floats,
        Some(6 * (s.open_edges + s.non_manifold_edges)),
        "problem edges"
    );
}

fn cut_meta(normal: [f64; 3], offset: f64, cap: bool, gap: f64) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("normal".into(), json!(normal));
    meta.insert("offset".into(), json!(offset));
    meta.insert("cap".into(), json!(cap));
    meta.insert("gap".into(), json!(gap));
    meta
}

fn run_cut(
    parts: &[(&str, &Mesh, Matrix)],
    meta: Map<String, Value>,
) -> (Map<String, Value>, Vec<Out>) {
    let bytes =
        jobs::cut_meshes(decode(&request(meta, parts)).unwrap()).unwrap_or_else(|e| panic!("{e}"));
    let (meta, outs) = read(&bytes);
    outs.iter().for_each(assert_measured);
    (meta, outs)
}

fn plane(normal: [f64; 3], offset: f64) -> Plane {
    Plane {
        normal: unit(normal),
        offset,
    }
}

fn side(out: &Out) -> &str {
    out.info["side"].as_str().unwrap()
}

// ---------------------------------------------------------------------------------------------
// Split shells and Merge parts

#[test]
fn split_two_disjoint_cubes_in_one_part() {
    let parts = import("two_cubes_disjoint.obj", true);
    assert_eq!(parts.len(), 1);
    let bytes = jobs::split_shells(decode(&request(Map::new(), &plain(&parts))).unwrap()).unwrap();
    let (meta, outs) = read(&bytes);
    assert_eq!(meta["counts"], json!([2]));
    assert_eq!(outs.len(), 2);
    for out in &outs {
        assert_measured(out);
        assert_eq!(out.info["source"], 0);
        assert_eq!(out.stat("volume"), 8000.0);
        assert_eq!((out.stat("shells"), out.stat("openEdges")), (1.0, 0.0));
    }
    // cube_a comes first: equal volumes keep file order.
    assert_eq!(bounds(&outs[0].mesh).unwrap()[0][0], 0.0);
    assert_eq!(bounds(&outs[1].mesh).unwrap()[0][0], 40.0);
}

#[test]
fn split_skips_parts_with_one_shell() {
    let cube = import("cube_20mm.stl", false);
    let pair = import("two_cubes_disjoint.obj", true);
    let parts = [plain(&cube), plain(&pair)].concat();
    let bytes = jobs::split_shells(decode(&request(Map::new(), &parts)).unwrap()).unwrap();
    let (meta, outs) = read(&bytes);
    assert_eq!(meta["counts"], json!([1, 2]));
    assert_eq!(outs.len(), 2);
    assert!(outs.iter().all(|o| o.info["source"] == 1));
}

#[test]
fn merge_two_overlapping_cubes_keeps_both_shells() {
    let parts = import("two_cubes_overlapping.obj", false);
    assert_eq!(parts.len(), 2);
    let bytes = jobs::merge_meshes(decode(&request(Map::new(), &plain(&parts))).unwrap()).unwrap();
    let (_, outs) = read(&bytes);
    assert_eq!(outs.len(), 1);
    assert_measured(&outs[0]);
    assert_eq!(outs[0].stat("shells"), 2.0);
    assert_eq!(outs[0].stat("volume"), 16_000.0);
    assert_eq!(outs[0].stat("triangles"), 24.0);
    // The overlap is not fused: every edge is still shared by exactly two triangles.
    assert_eq!(
        (outs[0].stat("openEdges"), outs[0].stat("nonManifoldEdges")),
        (0.0, 0.0)
    );
}

#[test]
fn merge_then_split_gives_back_the_parts() {
    let cubes = import("two_cubes_overlapping.obj", false);
    let sphere = import("icosphere_20k.stl", false);
    let moved = trs([5.0, 50.0, 0.0], 30.0, 0.0, [1.0, -1.0, 1.0]);
    let parts = vec![
        (cubes[0].0.as_str(), &cubes[0].1, IDENTITY),
        (sphere[0].0.as_str(), &sphere[0].1, moved),
        (cubes[1].0.as_str(), &cubes[1].1, IDENTITY),
    ];
    let merged = jobs::merge_meshes(decode(&request(Map::new(), &parts)).unwrap()).unwrap();
    let (_, merged) = read(&merged);
    let merged = &merged[0].mesh;
    let split = jobs::split_shells(
        decode(&request(
            Map::new(),
            &[("Merge of 3 parts", merged, IDENTITY)],
        ))
        .unwrap(),
    )
    .unwrap();
    let (meta, outs) = read(&split);
    assert_eq!(meta["counts"], json!([3]));
    // World space: the sphere comes back baked (and still facing out after the mirror).
    let want = [
        cubes[0].1.clone(),
        cubes[1].1.clone(),
        bake(&sphere[0].1, &moved),
    ];
    assert!(stats(&want[2]).volume > 0.0);
    let got: Vec<&Mesh> = outs.iter().map(|o| &o.mesh).collect();
    // Largest first (the two equal cubes in their order, then the smaller sphere): exactly the
    // same arrays that went in.
    assert_eq!(got, want.iter().collect::<Vec<_>>());
}

#[test]
fn merge_needs_two_parts() {
    let parts = import("cube_20mm.stl", false);
    let err =
        jobs::merge_meshes(decode(&request(Map::new(), &plain(&parts))).unwrap()).unwrap_err();
    assert_eq!(
        err,
        "Could not merge the parts: at least two parts are needed."
    );
}

// ---------------------------------------------------------------------------------------------
// Cut

#[test]
fn cut_cube_at_half_height_with_cap() {
    let parts = import("cube_20mm.stl", false);
    let (meta, outs) = run_cut(&plain(&parts), cut_meta([0.0, 0.0, 1.0], 10.0, true, 0.0));
    assert_eq!(meta["openOutline"], json!([false]));
    assert_eq!(
        outs.iter().map(side).collect::<Vec<_>>(),
        ["below", "above"]
    );
    for out in &outs {
        assert_eq!(out.info["source"], 0);
        assert!(
            (out.stat("volume") - 4000.0).abs() < 1e-9,
            "{}",
            out.stat("volume")
        );
        assert_eq!(
            (out.stat("openEdges"), out.stat("nonManifoldEdges")),
            (0.0, 0.0)
        );
        assert_eq!(out.stat("shells"), 1.0);
    }
    assert_eq!(bounds(&outs[0].mesh).unwrap()[1][2], 10.0);
    assert_eq!(bounds(&outs[1].mesh).unwrap()[0][2], 10.0);
}

#[test]
fn cut_cube_without_cap_leaves_both_pieces_open() {
    let parts = import("cube_20mm.stl", false);
    let (meta, outs) = run_cut(&plain(&parts), cut_meta([0.0, 0.0, 1.0], 10.0, false, 0.0));
    assert_eq!(meta["openOutline"], json!([false]));
    assert_eq!(outs.len(), 2);
    for out in &outs {
        assert!(out.stat("openEdges") > 0.0, "{}", side(out));
    }
}

#[test]
fn cut_open_box_leaves_the_upper_piece_open() {
    let parts = import("open_box.stl", false);
    let (meta, outs) = run_cut(&plain(&parts), cut_meta([0.0, 0.0, 1.0], 10.0, true, 0.0));
    // The outline is the four walls, a closed loop, so it is capped on both pieces; the upper
    // piece keeps the box's missing top: its 4 rim edges stay open.
    assert_eq!(meta["openOutline"], json!([false]));
    assert_eq!(outs.len(), 2);
    let (below, above) = (&outs[0], &outs[1]);
    assert_eq!((side(below), below.stat("openEdges")), ("below", 0.0));
    assert_eq!((side(above), above.stat("openEdges")), ("above", 4.0));
    // Even open, the caps cancel: the signed volumes add up to the box's.
    let total = below.stat("volume") + above.stat("volume");
    assert!((total - stats(&parts[0].1).volume).abs() < 1e-9);
    // Cut across the missing top, the outline itself is open.
    let (meta, outs) = run_cut(&plain(&parts), cut_meta([1.0, 0.0, 0.0], 10.0, true, 0.0));
    assert_eq!(meta["openOutline"], json!([true]));
    assert!(outs.iter().all(|o| o.stat("openEdges") > 0.0));
}

#[test]
fn cut_sphere_through_its_centre() {
    let parts = import("icosphere_20k.stl", false);
    let whole = stats(&parts[0].1).volume;
    let (meta, outs) = run_cut(&plain(&parts), cut_meta([0.0, 0.0, 1.0], 0.0, true, 0.0));
    assert_eq!(meta["openOutline"], json!([false]));
    assert_eq!(outs.len(), 2);
    for out in &outs {
        assert_eq!(
            (out.stat("openEdges"), out.stat("nonManifoldEdges")),
            (0.0, 0.0)
        );
        assert_eq!(out.stat("shells"), 1.0);
        assert!((out.stat("volume") / whole - 0.5).abs() < 1e-3);
    }
    let sum = outs[0].stat("volume") + outs[1].stat("volume");
    assert!((sum - whole).abs() <= 1e-3 * whole, "0.1% of the volume");
    assert!((sum - whole).abs() <= 1e-7 * whole, "{sum} vs {whole}");
}

#[test]
fn cut_planes_through_faces_and_beside_the_part() {
    let parts = import("cube_20mm.stl", false);
    let cube = &parts[0].1;
    for (offset, kept) in [
        (0.0, "above"),
        (20.0, "below"),
        (-1.0, "above"),
        (35.0, "below"),
    ] {
        // The cut itself keeps the whole part on one side...
        let pieces = cut(cube, &plane([0.0, 0.0, 1.0], offset), true);
        let whole = if kept == "below" {
            &pieces.below
        } else {
            &pieces.above
        };
        let other = if kept == "below" {
            &pieces.above
        } else {
            &pieces.below
        };
        assert_eq!(
            whole.as_ref(),
            Some(cube),
            "z = {offset}: the part is unchanged"
        );
        assert!(other.is_none(), "z = {offset}");
        // ...and the job, which only reports parts the plane crosses, returns nothing for it.
        let (_, outs) = run_cut(&plain(&parts), cut_meta([0.0, 0.0, 1.0], offset, true, 0.0));
        assert!(outs.is_empty(), "z = {offset}: the part is left alone");
    }
}

#[test]
fn cut_torus_through_its_middle_fills_an_annulus() {
    // The plane holds the outer and inner equator rings: no triangle crosses it, and each side
    // has two outline loops, one inside the other.
    let torus = torus(48, 24, 12.0, 4.0);
    let whole = stats(&torus).volume;
    let pieces = cut(&torus, &plane([0.0, 0.0, 1.0], 4.0), true);
    let [below, above] = closed_pieces(&pieces, "torus at z = 4");
    // Nothing is split, so the halves add up exactly; they differ from each other only by the
    // f32 rounding of the vertex rows above and below.
    assert!((below + above - whole).abs() < 1e-12 * whole);
    assert!((below - whole / 2.0).abs() < 1e-6 * whole);
    // Off the vertex rows too.
    let pieces = cut(&torus, &plane([0.0, 0.0, 1.0], 5.3), true);
    let [below, above] = closed_pieces(&pieces, "torus at z = 5.3");
    assert!((below + above - whole).abs() < 1e-7 * whole);
}

#[test]
fn cut_tangent_to_a_saddle_along_an_edge() {
    // Found by cut_sweep_wide: the plane holds two neighbouring vertices of the inner equator,
    // where it is nearly tangent to the saddle. Their edge runs across the outline with both its
    // triangles above the plane, and the cap must not use it as a diagonal.
    let coarse = torus(7, 5, 3.0, 1.0);
    let y = coarse.positions[27 * 3 + 1];
    assert_eq!(y, coarse.positions[28 * 3 + 1]);
    let whole = stats(&coarse).volume;
    let pieces = cut(&coarse, &plane([0.0, 1.0, 0.0], y as f64), true);
    let [below, above] = closed_pieces(&pieces, "saddle");
    assert!((below + above - whole).abs() < 1e-7 * whole);
}

#[test]
fn cut_where_crossing_points_crowd_together() {
    // Found by cut_sweep_wide while the straight runs were being fixed: planes that pass close to
    // torus vertices leave crossing points a few f32 steps apart, each of which looks as if it lay
    // on a nearby cap edge.
    let cases = [
        (
            torus(40, 20, 12.0, 4.0),
            [
                0.17085398316709072,
                -0.7763844534737749,
                -0.6066597867340207,
            ],
            -5.554983431272144,
        ),
        (
            torus(301, 97, 40.0, 0.5),
            [0.0, 1.0, 0.0],
            0.41226404905319214,
        ),
        (
            torus(301, 97, 40.0, 0.5),
            [
                0.025177929299580078,
                0.48929395458000713,
                0.8717554117340729,
            ],
            19.190118917060275,
        ),
    ];
    for (mesh, normal, offset) in cases {
        let whole = stats(&mesh).volume;
        let pieces = cut(&mesh, &Plane { normal, offset }, true);
        let [below, above] = closed_pieces(&pieces, &format!("{normal:?} {offset}"));
        assert!((below + above - whole).abs() < 1e-7 * whole);
    }
}

/// Volumes of both pieces after asserting each exists, is closed and is manifold.
fn closed_pieces(pieces: &CutPieces, what: &str) -> [f64; 2] {
    assert!(!pieces.open_outline, "{what}: open outline");
    [&pieces.below, &pieces.above].map(|m| {
        let s = stats(
            m.as_ref()
                .unwrap_or_else(|| panic!("{what}: a piece is missing")),
        );
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0), "{what}");
        s.volume
    })
}

/// Deterministic numbers in 0..1 for the sweep below.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> [f64; 3] {
        loop {
            let p = [0, 1, 2].map(|_| self.next() * 2.0 - 1.0);
            let l2 = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
            if (0.01..=1.0).contains(&l2) {
                return unit(p);
            }
        }
    }
}

/// Cuts `mesh` with every plane and checks the pieces are closed and manifold and their volumes
/// add up to the whole. Returns the largest relative volume error seen.
fn sweep(mesh: &Mesh, planes: &[Plane], what: &str) -> f64 {
    let whole = stats(mesh).volume;
    let mut worst = 0.0f64;
    for (k, p) in planes.iter().enumerate() {
        let pieces = cut(mesh, p, true);
        let what = format!("{what}, plane {k} {p:?}");
        assert!(!pieces.open_outline, "{what}: open outline");
        let mut sum = 0.0;
        for piece in [&pieces.below, &pieces.above].into_iter().flatten() {
            let s = stats(piece);
            assert_eq!(
                (s.open_edges, s.non_manifold_edges),
                (0, 0),
                "{what}: open / non-manifold edges"
            );
            sum += s.volume;
        }
        worst = worst.max((sum - whole).abs() / whole);
    }
    worst
}

/// Planes of every orientation across the mesh, and through its vertices.
fn planes_for(mesh: &Mesh, normals: &[[f64; 3]], offsets: usize, through: usize) -> Vec<Plane> {
    let count = mesh.vertex_count();
    let vertex = |v: usize| {
        let p = &mesh.positions[v * 3..v * 3 + 3];
        [p[0] as f64, p[1] as f64, p[2] as f64]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut out = Vec::new();
    for &n in normals {
        let (lo, hi) = (0..count)
            .map(|v| dot(n, vertex(v)))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| {
                (lo.min(d), hi.max(d))
            });
        for k in 0..offsets {
            // Strictly inside, from just past one end to just before the other.
            let f = (k as f64 + 0.5) / offsets as f64;
            out.push(Plane {
                normal: n,
                offset: lo + (hi - lo) * f,
            });
        }
        // Through vertices, spread over the vertex list.
        for k in 0..through {
            out.push(Plane {
                normal: n,
                offset: dot(n, vertex(k * count / through)),
            });
        }
        // Grazing both ends.
        out.push(Plane {
            normal: n,
            offset: lo,
        });
        out.push(Plane {
            normal: n,
            offset: hi,
        });
    }
    out
}

fn normals(random: usize, seed: u64) -> Vec<[f64; 3]> {
    let mut rng = Lcg(seed);
    let mut out: Vec<[f64; 3]> = vec![
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        unit([1.0, 1.0, 0.0]),
        unit([1.0, 1.0, 1.0]),
        unit([-1.0, 2.0, 0.5]),
    ];
    out.extend((0..random).map(|_| rng.normal()));
    out
}

#[test]
fn cuts_are_watertight_and_keep_the_volume_at_any_plane() {
    let torus = torus(40, 20, 12.0, 4.0);
    let sphere = import("icosphere_20k.stl", false).remove(0).1;
    let mut worst = 0.0f64;
    worst = worst.max(sweep(
        &torus,
        &planes_for(&torus, &normals(30, 7), 15, 8),
        "torus",
    ));
    // Through every vertex row of the torus (each one a ring of vertices in the plane).
    let rows: Vec<Plane> = (0..20)
        .map(|j| plane([0.0, 0.0, 1.0], torus.positions[j * 3 + 2] as f64))
        .collect();
    worst = worst.max(sweep(&torus, &rows, "torus rows"));
    worst = worst.max(sweep(
        &sphere,
        &planes_for(&sphere, &normals(8, 11), 7, 4),
        "sphere",
    ));
    // The only loss: each intersection vertex rounds to f32, a little off its triangles' planes.
    eprintln!("largest relative volume error: {worst:e}");
    assert!(worst < 1e-7, "{worst}");
}

/// The same sweep over tens of thousands of planes, for a release run:
/// `cargo test --release --manifest-path src-tauri/Cargo.toml --test ops -- --ignored cut_sweep_wide`
#[test]
#[ignore]
fn cut_sweep_wide() {
    let sphere = import("icosphere_20k.stl", false).remove(0).1;
    let mut worst = 0.0f64;
    for (n, mesh) in [
        torus(40, 20, 12.0, 4.0),
        torus(7, 5, 3.0, 1.0),
        torus(301, 97, 40.0, 0.5),
        sphere,
    ]
    .iter()
    .enumerate()
    {
        let planes = planes_for(mesh, &normals(200, 100 + n as u64), 40, 60);
        worst = worst.max(sweep(mesh, &planes, &format!("mesh {n}")));
        eprintln!("mesh {n}: {} planes, worst so far {worst:e}", planes.len());
    }
    assert!(worst < 1e-7, "{worst}");
}

/// Edges two triangles run the same way: the mesh is not consistently wound there.
fn misoriented(mesh: &Mesh) -> usize {
    let mut runs: HashMap<(u32, u32), u32> = HashMap::new();
    for t in mesh.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            *runs.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    runs.values().filter(|&&n| n > 1).count()
}

#[test]
fn cut_crossing_and_nested_shells_of_a_merged_part() {
    // Merge parts keeps each shell as it was. With one triangle missing somewhere the part is not
    // closed, so the window sends it to this cutter rather than to manifold-3d.
    let cube = make_box([0.0; 3], [20.0; 3]);
    let mut crossing = make_box([10.0, 10.0, 0.0], [30.0, 30.0, 20.0]);
    crossing.indices.drain(6..9); // one of its top triangles
    let inner = make_box([5.0; 3], [15.0; 3]);
    for (parts, below_volume, above_open) in [
        (vec![("Cube", &cube), ("Crossing", &crossing)], 8000.0, 3.0),
        (vec![("Cube", &cube), ("Inner", &inner)], 4500.0, 0.0),
    ] {
        let parts: Vec<_> = parts.into_iter().map(|(n, m)| (n, m, IDENTITY)).collect();
        let merged = jobs::merge_meshes(decode(&request(Map::new(), &parts)).unwrap()).unwrap();
        let (_, merged) = read(&merged);
        let merged = [("Merged", &merged[0].mesh, IDENTITY)];
        let (meta, outs) = run_cut(&merged, cut_meta([0.0, 0.0, 1.0], 10.0, true, 0.0));
        assert_eq!(meta["openOutline"], json!([false]));
        let (below, above) = (&outs[0], &outs[1]);
        assert_eq!((side(below), side(above)), ("below", "above"));
        assert!((below.stat("volume") - below_volume).abs() < 1e-9);
        assert_eq!(
            (below.stat("openEdges"), below.stat("nonManifoldEdges")),
            (0.0, 0.0)
        );
        assert_eq!(
            (above.stat("openEdges"), above.stat("nonManifoldEdges")),
            (above_open, 0.0)
        );
        // One cap per shell, each turned to close its own shell.
        assert_eq!((misoriented(&below.mesh), misoriented(&above.mesh)), (0, 0));
    }
}

/// A plate from an outline (counter-clockwise in x, y) and its holes, from z0 to z1: one quad per
/// outline edge for the walls, earcut for the faces, so the walls have no vertices but corners.
fn prism(outer: &[[f32; 2]], holes: &[&[[f32; 2]]], z0: f32, z1: f32) -> Mesh {
    let mut coords = Vec::new();
    let mut starts = Vec::new();
    let mut all: Vec<[f32; 2]> = Vec::new();
    for (k, ring) in std::iter::once(outer)
        .chain(holes.iter().copied())
        .enumerate()
    {
        if k > 0 {
            starts.push(all.len());
        }
        for p in ring {
            coords.extend([p[0] as f64, p[1] as f64]);
            all.push(*p);
        }
    }
    let at = |p: [f32; 2], z: f32| [p[0], p[1], z];
    let mut w = Welder::new(0);
    for t in earcutr::earcut(&coords, &starts, 2).unwrap().chunks(3) {
        let (a, b, c) = (all[t[0]], all[t[1]], all[t[2]]);
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        let (b, c) = if area < 0.0 { (c, b) } else { (b, c) };
        w.triangle(at(a, z1), at(b, z1), at(c, z1));
        w.triangle(at(a, z0), at(c, z0), at(b, z0));
    }
    for ring in std::iter::once(outer).chain(holes.iter().copied()) {
        for i in 0..ring.len() {
            let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
            w.triangle(at(p, z0), at(q, z0), at(q, z1));
            w.triangle(at(p, z0), at(q, z1), at(p, z1));
        }
    }
    w.finish().unwrap()
}

#[test]
fn cut_notched_plate_whose_hole_lines_up_with_the_notch() {
    // A 40 × 40 mm plate, 5 mm thick, with a 10 mm notch at the top and a 10 mm square hole whose
    // left side is in line with the notch's right side (x = 20).
    let outer = [
        [0.0, 0.0],
        [40.0, 0.0],
        [40.0, 30.0],
        [30.0, 30.0],
        [30.0, 40.0],
        [20.0, 40.0],
        [20.0, 30.0],
        [10.0, 30.0],
        [10.0, 20.0],
        [0.0, 20.0],
    ];
    let hole: &[[f32; 2]] = &[[20.0, 10.0], [20.0, 20.0], [30.0, 20.0], [30.0, 10.0]];
    let plate = prism(&outer, &[hole], 0.0, 5.0);
    let s = stats(&plate);
    assert_eq!(
        (s.volume, s.open_edges, s.non_manifold_edges),
        (5500.0, 0, 0)
    );
    for z in [2.5, 1.0, 4.0] {
        let pieces = cut(&plate, &plane([0.0, 0.0, 1.0], z), true);
        let [below, above] = closed_pieces(&pieces, &format!("plate at z = {z}"));
        assert!((below - 1100.0 * z).abs() < 1e-9 && (above - 1100.0 * (5.0 - z)).abs() < 1e-9);
    }
    // With one top triangle missing the cut still closes, and only that triangle stays open.
    let mut open = plate.clone();
    open.indices.drain(0..3);
    let pieces = cut(&open, &plane([0.0, 0.0, 1.0], 2.5), true);
    assert!(!pieces.open_outline);
    let [below, above] = [&pieces.below, &pieces.above].map(|m| stats(m.as_ref().unwrap()));
    assert_eq!((below.open_edges, below.non_manifold_edges), (0, 0));
    assert_eq!((above.open_edges, above.non_manifold_edges), (3, 0));
}

/// A closed cylinder around z with `segments` side quads and caps fanned from the centre: a cut
/// along its length crosses every cap triangle, so its outline has long runs of points on a line.
fn fan_cylinder(segments: u32, radius: f64, height: f32) -> Mesh {
    let mut positions = Vec::new();
    for i in 0..segments {
        let a = std::f64::consts::TAU * i as f64 / segments as f64;
        let (x, y) = ((radius * a.cos()) as f32, (radius * a.sin()) as f32);
        positions.extend([x, y, 0.0, x, y, height]);
    }
    let centre = segments * 2;
    positions.extend([0.0, 0.0, 0.0, 0.0, 0.0, height]);
    let mut indices = Vec::new();
    for i in 0..segments {
        let j = (i + 1) % segments;
        let (b0, t0, b1, t1) = (2 * i, 2 * i + 1, 2 * j, 2 * j + 1);
        indices.extend([b0, b1, t1, b0, t1, t0, centre, b1, b0, centre + 1, t0, t1]);
    }
    Mesh { positions, indices }
}

/// Closed mesh of the outer faces of the filled cells of a grid (cells `size` mm, `n` per axis),
/// facing out and welded, as in the cutter's own tests.
fn voxels(n: [i32; 3], size: f32, filled: impl Fn(i32, i32, i32) -> bool) -> Mesh {
    let is = |c: [i32; 3]| (0..3).all(|k| (0..n[k]).contains(&c[k])) && filled(c[0], c[1], c[2]);
    let mut w = Welder::new(0);
    for x in 0..n[0] {
        for y in 0..n[1] {
            for z in 0..n[2] {
                let c = [x, y, z];
                if !is(c) {
                    continue;
                }
                for k in 0..3 {
                    for s in [-1, 1] {
                        let mut nb = c;
                        nb[k] += s;
                        if is(nb) {
                            continue;
                        }
                        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
                        let corner = |di: i32, dj: i32| {
                            let mut p = c;
                            p[k] += (s > 0) as i32;
                            p[i] += di;
                            p[j] += dj;
                            p.map(|v| v as f32 * size)
                        };
                        let mut quad = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
                        if s < 0 {
                            quad.reverse();
                        }
                        w.triangle(quad[0], quad[1], quad[2]);
                        w.triangle(quad[0], quad[2], quad[3]);
                    }
                }
            }
        }
    }
    w.finish().unwrap()
}

#[test]
fn cut_fan_capped_cylinder_lengthwise() {
    // The f32-rounded crossing points along the caps are left out of the triangulation and fanned
    // back in; the pieces stay closed either way.
    let cylinder = fan_cylinder(20_000, 50.0, 30.0);
    let whole = stats(&cylinder).volume;
    for p in [plane([1.0, 0.0, 0.0], 10.0), plane([0.0, 0.0, 1.0], 15.0)] {
        let pieces = cut(&cylinder, &p, true);
        let [below, above] = closed_pieces(&pieces, &format!("{p:?}"));
        assert!((below + above - whole).abs() < 1e-7 * whole, "{p:?}");
    }
}

/// `cargo test --release --manifest-path src-tauri/Cargo.toml --test ops -- --ignored --nocapture
/// cut_timings`: the reviewer's slow cases, each well under a second in release.
#[test]
#[ignore]
fn cut_timings() {
    let lattice = voxels([401, 401, 2], 1.0, |x, y, z| {
        (x % 2 == 0 && y % 2 == 0) || z == 0
    });
    let plate = voxels([301, 301, 1], 1.0, |x, y, _| x % 2 == 0 || y % 2 == 0);
    let cylinder = fan_cylinder(250_000, 50.0, 30.0);
    let column = voxels([3, 36_000, 1], 1.0, |x, y, _| !(x == 1 && y % 3 == 1));
    let mut far = fan_cylinder(250_000, 10.0, 6.0);
    translate(&mut far, [1000.0, 1000.0, 0.0]);
    let cases = [
        (
            "cylinder 1M triangles, lengthwise",
            &cylinder,
            [1.0, 0.0, 0.0],
            10.0,
        ),
        (
            "cylinder 1M triangles, across",
            &cylinder,
            [0.0, 0.0, 1.0],
            15.0,
        ),
        ("lattice of 40,401 pillars", &lattice, [0.0, 0.0, 1.0], 1.5),
        ("plate with 22,500 holes", &plate, [0.0, 0.0, 1.0], 0.5),
        // A 20 mm cylinder 1 m from the origin: its crossing points along the caps round to the
        // same spot in places. Every point was kept there (90 s), or the cap was left open.
        (
            "cylinder 1M, 1 m off the origin, along",
            &far,
            [1.0, 0.0, 0.0],
            1002.0,
        ),
        // One row of holes in the plane basis: the right-hand links once all repeated the left
        // ones, and the fill took 3 s and was left mostly open.
        ("column of 12,000 holes", &column, [0.0, 0.0, 1.0], 0.5),
    ];
    for (what, mesh, normal, offset) in cases {
        // The best of three, so another process taking the CPU for a moment does not count.
        let mut seconds = f64::INFINITY;
        let mut pieces = CutPieces::default();
        for _ in 0..3 {
            let start = Instant::now();
            pieces = cut(mesh, &plane(normal, offset), true);
            seconds = seconds.min(start.elapsed().as_secs_f64());
        }
        eprintln!(
            "{what:<36} {:>9} triangles {:>8.1} ms",
            mesh.triangle_count(),
            seconds * 1000.0
        );
        let [below, above] = closed_pieces(&pieces, what);
        let whole = stats(mesh).volume;
        assert!((below + above - whole).abs() < 1e-7 * whole, "{what}");
        if !cfg!(debug_assertions) {
            assert!(seconds < 1.0, "{what}: {seconds} s");
        }
    }
}

/// `cargo test --release --manifest-path src-tauri/Cargo.toml --test ops -- --ignored --nocapture
/// cut_finely_divided_cube_far_from_the_origin`: the verifier's 20 mm cube of 200 × 200 cells a
/// face (480,000 triangles), turned and moved about 500 mm off the origin and cut parallel to two
/// of its faces, through its middle. Its outline is four runs of 400 points, all rounded off their
/// lines; the fast fill once left 1 open edge and 82 edges of four triangles on each piece.
#[test]
#[ignore]
fn cut_finely_divided_cube_far_from_the_origin() {
    let cube = voxels([200, 200, 200], 0.1, |_, _, _| true);
    let [w, x, y, z] = [
        0.21896555203092033,
        0.63873285277986,
        0.6477883531387743,
        -0.3527671744100496,
    ];
    let r = [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
        ],
    ];
    let shift = [
        -482.36694223458034,
        -380.27270615346407,
        -403.98217041855344,
    ];
    let turn = |p: [f64; 3]| [0, 1, 2].map(|k| r[k][0] * p[0] + r[k][1] * p[1] + r[k][2] * p[2]);
    let mut mesh = cube.clone();
    for v in mesh.positions.as_chunks_mut::<3>().0 {
        let p = turn(v.map(|c| c as f64));
        *v = [0, 1, 2].map(|k| (p[k] + shift[k]) as f32);
    }
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let normal = unit(turn([0.0, 0.0, 1.0]));
    let offset = dot(normal, turn([10.424881448844392, 10.0, 10.0])) + dot(normal, shift);
    assert!((offset - 635.4022572406391).abs() < 1e-9, "{offset}");
    let pieces = cut(&mesh, &Plane { normal, offset }, true);
    let [below, above] = closed_pieces(&pieces, "far cube");
    for m in [&pieces.below, &pieces.above] {
        assert_eq!(misoriented(m.as_ref().unwrap()), 0);
    }
    let whole = stats(&mesh).volume;
    assert!(
        (below + above - whole).abs() < 1e-6 * whole,
        "{below} + {above} vs {whole}"
    );
}

/// `cargo test --release --manifest-path src-tauri/Cargo.toml --test ops -- --ignored --nocapture
/// cut_sweep_voxels`: random 4 × 4 × 4 voxel solids that are closed manifolds, cut on every half
/// cell along each axis and by tilted planes through their vertices, where outlines run straight,
/// touch themselves and pass through vertices.
#[test]
#[ignore]
fn cut_sweep_voxels() {
    let mut rng = Lcg(0x0bad_cafe);
    let (mut shapes, mut tried, mut failed) = (0, 0, Vec::new());
    while shapes < 150 {
        let bits: Vec<bool> = (0..64).map(|_| rng.next() < 0.55).collect();
        let solid = voxels([4, 4, 4], 1.0, |x, y, z| {
            bits[(x * 16 + y * 4 + z) as usize]
        });
        let s = stats(&solid);
        if solid.indices.is_empty() || s.open_edges != 0 || s.non_manifold_edges != 0 {
            continue;
        }
        shapes += 1;
        let mut planes = Vec::new();
        for k in 0..3 {
            for i in 0..=8 {
                let mut normal = [0.0; 3];
                normal[k] = 1.0;
                planes.push(plane(normal, i as f64 * 0.5));
            }
        }
        for _ in 0..20 {
            let normal = [0, 1, 2].map(|_| (rng.next() * 5.0).floor() - 2.0);
            if normal == [0.0; 3] {
                continue;
            }
            let p = plane(normal, 0.0);
            let v = (rng.next() * solid.vertex_count() as f64) as usize;
            let at = [0, 1, 2].map(|k| solid.positions[v * 3 + k] as f64);
            let offset = p.normal[0] * at[0] + p.normal[1] * at[1] + p.normal[2] * at[2];
            planes.push(Plane { offset, ..p });
        }
        for p in &planes {
            tried += 1;
            let pieces = cut(&solid, p, true);
            let mut sum = 0.0;
            let mut good = !pieces.open_outline;
            for m in [&pieces.below, &pieces.above].into_iter().flatten() {
                let t = stats(m);
                good &= t.open_edges == 0 && t.non_manifold_edges == 0 && misoriented(m) == 0;
                sum += t.volume;
            }
            if !good || (sum - s.volume).abs() > 1e-6 * s.volume {
                failed.push(format!("shape {shapes} {p:?}"));
            }
        }
    }
    eprintln!("{} of {tried} cuts failed", failed.len());
    for f in &failed {
        eprintln!("  {f}");
    }
    assert!(failed.is_empty());
}

#[test]
fn cut_shifted_and_rotated_parts_in_world_space() {
    // A part moved up 100 mm and turned: the cut plane is in world space, the pieces come back
    // in world space with identity matrices, and the gap moves the upper piece along the normal.
    let parts = import("cube_20mm.stl", false);
    let matrix = trs([0.0, 0.0, 100.0], 0.0, 0.0, [1.0, 1.0, -1.0]);
    let request_parts = [(parts[0].0.as_str(), &parts[0].1, matrix)];
    let (_, outs) = run_cut(&request_parts, cut_meta([0.0, 0.0, 2.0], 180.0, true, 5.0));
    assert_eq!(outs.len(), 2);
    let below = bounds(&outs[0].mesh).unwrap();
    let above = bounds(&outs[1].mesh).unwrap();
    assert_eq!((below[0][2], below[1][2]), (80.0, 90.0));
    assert_eq!((above[0][2], above[1][2]), (95.0, 105.0));
    for out in &outs {
        assert!(
            (out.stat("volume") - 4000.0).abs() < 1e-9,
            "mirror baked facing out"
        );
        assert!(!out.info.contains_key("matrix"));
    }
}

#[test]
fn cut_settings_are_checked() {
    let parts = import("cube_20mm.stl", false);
    let bad = |meta: Map<String, Value>| {
        jobs::cut_meshes(decode(&request(meta, &plain(&parts))).unwrap()).unwrap_err()
    };
    let want = "Could not cut the part: the cut settings are not valid.";
    assert_eq!(bad(cut_meta([0.0, 0.0, 0.0], 1.0, true, 0.0)), want);
    assert_eq!(bad(cut_meta([0.0, 0.0, 1.0], 1.0, true, -1.0)), want);
    let mut meta = cut_meta([0.0, 0.0, 1.0], 1.0, true, 0.0);
    meta.remove("cap");
    assert_eq!(bad(meta), want);
    let mut meta = cut_meta([0.0, 0.0, 1.0], 1.0, true, 0.0);
    meta.insert("normal".into(), json!([0, 1]));
    assert_eq!(bad(meta), want);
    let mut meta = cut_meta([0.0, 0.0, 1.0], 1.0, true, 0.0);
    meta.insert("offset".into(), json!("10"));
    assert_eq!(bad(meta), want);
    // The gap may be left out.
    let mut meta = cut_meta([0.0, 0.0, 1.0], 10.0, true, 0.0);
    meta.remove("gap");
    assert!(jobs::cut_meshes(decode(&request(meta, &plain(&parts))).unwrap()).is_ok());
}

// ---------------------------------------------------------------------------------------------
// Jobs: bundles, matrices and errors

#[test]
fn split_bakes_matrices_and_keeps_shells_facing_out() {
    let pair = import("two_cubes_disjoint.obj", true);
    let mirrored = trs([0.0, 0.0, 50.0], 90.0, 0.0, [-1.0, 1.0, 1.0]);
    let parts = [(pair[0].0.as_str(), &pair[0].1, mirrored)];
    let bytes = jobs::split_shells(decode(&request(Map::new(), &parts)).unwrap()).unwrap();
    let (_, outs) = read(&bytes);
    assert_eq!(outs.len(), 2);
    for out in &outs {
        assert_measured(out);
        // cos 90° is not exactly 0, so the corners move by f32 noise.
        assert!(
            (out.stat("volume") - 8000.0).abs() < 1e-6,
            "{}",
            out.stat("volume")
        );
        assert_eq!(bounds(&out.mesh).unwrap()[0][2], 50.0);
    }
}

#[test]
fn analyze_job_measures_without_sending_meshes_back() {
    let parts = import("open_box.stl", false);
    let cube = make_box([0.0; 3], [20.0; 3]);
    // The matrix is ignored: these meshes are world space already.
    let moved = trs([5.0, 0.0, 0.0], 0.0, 0.0, [2.0; 3]);
    let request_parts = [("Open", &parts[0].1, moved), ("Cube", &cube, IDENTITY)];
    let bytes =
        jobs::analyze_meshes(decode(&request(Map::new(), &request_parts)).unwrap()).unwrap();
    let (_, outs) = read(&bytes);
    assert_eq!(outs.len(), 2);
    assert!(outs.iter().all(|o| o.mesh == Mesh::default()));
    assert_eq!(outs[0].stat("openEdges"), 4.0);
    assert_eq!(outs[0].edge_floats, Some(24));
    assert_eq!(outs[1].stat("volume"), 8000.0);
    assert_eq!(outs[1].edge_floats, Some(0));
}

#[test]
fn damaged_parts_are_refused_with_their_name() {
    let broken = Mesh {
        positions: vec![0.0; 9],
        indices: vec![0, 1, 7],
    };
    let cube = make_box([0.0; 3], [20.0; 3]);
    let parts = [("Cube", &cube, IDENTITY), ("Broken", &broken, IDENTITY)];
    let body = || decode(&request(cut_meta([0.0, 0.0, 1.0], 1.0, true, 0.0), &parts)).unwrap();
    let reason =
        "the data for \"Broken\" is damaged (a triangle refers to a vertex that does not exist)";
    assert_eq!(
        jobs::split_shells(body()).unwrap_err(),
        format!("Could not split the part: {reason}.")
    );
    assert_eq!(
        jobs::merge_meshes(body()).unwrap_err(),
        format!("Could not merge the parts: {reason}.")
    );
    assert_eq!(
        jobs::cut_meshes(body()).unwrap_err(),
        format!("Could not cut the part: {reason}.")
    );
    assert_eq!(
        jobs::analyze_meshes(body()).unwrap_err(),
        format!("Could not measure the part: {reason}.")
    );
    // A matrix that is not 16 numbers.
    let mut out = OutMesh {
        info: Map::new(),
        positions: &cube.positions,
        indices: &cube.indices,
        edges: None,
    };
    out.info.insert("name".into(), json!("Cube"));
    out.info.insert("matrix".into(), json!([1, 0, 0]));
    let err = jobs::split_shells(decode(&encode(&Map::new(), &[out])).unwrap()).unwrap_err();
    assert_eq!(
        err,
        "Could not split the part: the data for \"Cube\" is damaged (its matrix is not 16 numbers)."
    );
}

/// Release builds abort on a panic, which would close the app with the user's work in it. Feeds
/// every operation random triangle soups: repeated, degenerate and non-manifold triangles, points
/// on a coarse grid so many sit exactly on the planes, and inside-out pieces.
#[test]
fn garbage_meshes_never_panic() {
    let mut rng = Lcg(42);
    for round in 0..300 {
        let vertices = 3 + (rng.next() * 40.0) as usize;
        let positions: Vec<f32> = (0..vertices * 3)
            .map(|_| (rng.next() * 6.0).floor() as f32 - 3.0)
            .collect();
        let triangles = 1 + (rng.next() * 60.0) as usize;
        let indices: Vec<u32> = (0..triangles * 3)
            .map(|_| (rng.next() * vertices as f64) as u32)
            .collect();
        let mesh = Mesh { positions, indices };
        mesh.check().unwrap();
        for _ in 0..4 {
            let normal = if rng.next() < 0.5 {
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]][(rng.next() * 3.0) as usize]
            } else {
                rng.normal()
            };
            let offset = (rng.next() * 6.0).floor() - 3.0;
            for cap in [true, false] {
                let pieces = cut(&mesh, &Plane { normal, offset }, cap);
                for piece in [&pieces.below, &pieces.above].into_iter().flatten() {
                    piece
                        .check()
                        .unwrap_or_else(|e| panic!("round {round}: {e}"));
                }
            }
        }
        for shell in split_shells(&mesh) {
            shell.check().unwrap();
        }
        concat(&[&mesh, &mesh]).unwrap().check().unwrap();
    }
}

// ---------------------------------------------------------------------------------------------
// Size

/// Debug builds are slow, so this checks the operations at 200k triangles finish and agree;
/// `big_mesh_timings` times them at 1M+ in release.
#[test]
fn operations_handle_a_large_mesh() {
    let big = torus(400, 250, 40.0, 10.0);
    assert_eq!(big.triangle_count(), 200_000);
    let mut other = big.clone();
    translate(&mut other, [200.0, 0.0, 0.0]);
    let both = concat(&[&big, &other]).unwrap();
    // The copy's volume differs from the original's by rounding, so either may come first.
    let shells = split_shells(&both);
    assert_eq!(shells.len(), 2);
    assert!(shells.contains(&big) && shells.contains(&other));
    let pieces = cut(&big, &plane([0.3, 0.1, 1.0], 12.0), true);
    let [b, a] = closed_pieces(&pieces, "big torus");
    let whole = stats(&big).volume;
    assert!((a + b - whole).abs() < 1e-9 * whole);
}

/// `cargo test --release --manifest-path src-tauri/Cargo.toml --test ops -- --ignored
/// --nocapture big_mesh_timings`
#[test]
#[ignore]
fn big_mesh_timings() {
    let big = torus(1000, 500, 60.0, 20.0);
    assert_eq!(big.triangle_count(), 1_000_000);
    let time = |what: &str, f: &mut dyn FnMut()| {
        let start = Instant::now();
        f();
        eprintln!(
            "{what:<44} {:>8.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    };
    let mut other = big.clone();
    translate(&mut other, [200.0, 0.0, 0.0]);
    let mut both = Mesh::default();
    time("concat 2 x 1M triangles", &mut || {
        both = concat(&[&big, &other]).unwrap();
    });
    let mut shells = Vec::new();
    time("split_shells 2M triangles (2 shells)", &mut || {
        shells = split_shells(&both);
    });
    assert_eq!(shells.len(), 2);
    let mut pieces = CutPieces::default();
    time("cut 1M, cap, oblique through the hole", &mut || {
        pieces = cut(&big, &plane([0.3, 0.1, 1.0], 20.0), true);
    });
    closed_pieces(&pieces, "oblique");
    time("cut 1M, cap, vertical (two outlines)", &mut || {
        pieces = cut(&big, &plane([1.0, 0.0, 0.0], 3.0), true);
    });
    closed_pieces(&pieces, "vertical");
    time("cut 1M, cap, through a vertex row", &mut || {
        pieces = cut(&big, &plane([0.0, 0.0, 1.0], 20.0), true);
    });
    closed_pieces(&pieces, "vertex row");
    time("analyze 1M", &mut || {
        analyze(&big, &mut |_| {});
    });
    // The jobs as the commands run them, bundle decode and encode included.
    let request_parts = [("Big", &big, IDENTITY)];
    let two = [("A", &big, IDENTITY), ("B", &other, IDENTITY)];
    time("job cut_meshes 1M (with analyze + encode)", &mut || {
        let meta = cut_meta([0.3, 0.1, 1.0], 20.0, true, 0.0);
        jobs::cut_meshes(decode(&request(meta, &request_parts)).unwrap()).unwrap();
    });
    time("job merge_meshes 2 x 1M", &mut || {
        jobs::merge_meshes(decode(&request(Map::new(), &two)).unwrap()).unwrap();
    });
    let merged = concat(&[&big, &other]).unwrap();
    let merged_parts = [("Merged", &merged, IDENTITY)];
    time("job split_shells 2M", &mut || {
        jobs::split_shells(decode(&request(Map::new(), &merged_parts)).unwrap()).unwrap();
    });
}
