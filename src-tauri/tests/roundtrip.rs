//! Export every format through the same path as the `export_mesh` command (a bundle from the
//! window), read the file back, and compare with the expected baked meshes. Projects round-trip
//! their part fields and unbaked matrices.

mod common;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{OutMesh, decode, encode};
use tricky3d_lib::formats::{ExportFormat, ImportFormat, NamedMesh, export_file, import_file};
use tricky3d_lib::jobs;
use tricky3d_lib::mesh::{Matrix, Mesh, bake, make_box};
use tricky3d_lib::progress::Progress;

/// Two parts with rotation + translation + non-uniform scale (the second one mirrored).
fn parts() -> Vec<(&'static str, Mesh, Matrix)> {
    vec![
        (
            "Cube <one> & \"two\"",
            make_box([0.0; 3], [20.0; 3]),
            trs([10.0, -5.0, 3.25], 30.0, 45.0, [1.5, 0.75, 2.0]),
        ),
        (
            "Ring",
            torus(16, 8, 12.0, 4.0),
            trs([-40.0, 22.5, 0.5], -70.0, 15.0, [0.5, -1.25, 3.0]),
        ),
    ]
}

/// The bundle the window sends for export_mesh / save_project.
fn request(
    meta: Map<String, Value>,
    parts: &[(&str, Mesh, Matrix)],
    extra: impl Fn(usize) -> Map<String, Value>,
) -> Vec<u8> {
    let out: Vec<OutMesh> = parts
        .iter()
        .enumerate()
        .map(|(k, (name, mesh, matrix))| {
            let mut info = extra(k);
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

fn export(path: &std::path::Path, format: &str, parts: &[(&str, Mesh, Matrix)]) {
    let mut meta = Map::new();
    meta.insert("path".into(), json!(path.to_string_lossy()));
    meta.insert("format".into(), json!(format));
    let bytes = request(meta, parts, |_| Map::new());
    jobs::export_mesh(decode(&bytes).unwrap()).unwrap_or_else(|e| panic!("{e}"));
}

fn merged(meshes: &[Mesh]) -> Mesh {
    let mut out = Mesh::default();
    for m in meshes {
        let base = out.vertex_count() as u32;
        out.positions.extend_from_slice(&m.positions);
        out.indices.extend(m.indices.iter().map(|i| i + base));
    }
    out
}

#[test]
fn every_export_format_reads_back_the_baked_meshes() {
    let dir = temp_dir("roundtrip");
    let parts = parts();
    let baked: Vec<Mesh> = parts
        .iter()
        .map(|(_, mesh, matrix)| bake(mesh, matrix))
        .collect();
    let whole = merged(&baked);
    assert!(
        baked
            .iter()
            .all(|m| tricky3d_lib::mesh::stats(m).volume > 0.0),
        "baking keeps volumes positive"
    );

    for (format, file, one_object_per_part) in [
        ("stl-binary", "out-binary.stl", false),
        ("stl-ascii", "out-ascii.stl", false),
        ("obj", "out.obj", true),
        ("ply", "out.ply", false),
        ("3mf", "out.3mf", true),
        ("amf", "out.amf", true),
    ] {
        let path = dir.join(file);
        export(&path, format, &parts);
        // A second export over the first must replace it cleanly.
        export(&path, format, &parts);
        let got = import_file(&path, false, &mut Progress::silent())
            .unwrap_or_else(|e| panic!("{format}: {e}"));
        if one_object_per_part {
            assert_eq!(got.parts.len(), 2, "{format}: one object per part");
            for (k, part) in got.parts.iter().enumerate() {
                assert_same_shape(&part.mesh, &baked[k], format);
                assert_eq!(part.matrix, None, "{format}: export bakes the matrix");
            }
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            assert_eq!(
                got.parts[0].name,
                format!("{stem} - Cube <one> & \"two\""),
                "{format}: names survive escaping"
            );
            assert_eq!(got.parts[1].name, format!("{stem} - Ring"));
        } else {
            assert_eq!(got.parts.len(), 1, "{format}: parts merged into one mesh");
            assert_same_shape(&got.parts[0].mesh, &whole, format);
        }
        // Text formats use shortest round-trip numbers, so every coordinate comes back bit-exact.
        let mut want: Vec<u32> = whole
            .positions
            .iter()
            .map(|v| (v + 0.0).to_bits())
            .collect();
        let mut seen: Vec<u32> = got
            .parts
            .iter()
            .flat_map(|p| p.mesh.positions.iter().map(|v| v.to_bits()))
            .collect();
        want.sort_unstable();
        want.dedup();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, want, "{format}: coordinates are bit-exact");
    }
    let leftovers: Vec<String> = names_in(&dir)
        .into_iter()
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temporary files left behind: {leftovers:?}"
    );
}

fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn two_writes_to_one_file_at_once_each_leave_a_whole_file() {
    let dir = temp_dir("concurrent-writes");
    let path = dir.join("race.stl");
    let meshes = [torus(200, 100, 30.0, 10.0), torus(150, 90, 20.0, 5.0)];
    for round in 0..8 {
        let barrier = std::sync::Barrier::new(2);
        let results: Vec<Result<(), String>> = std::thread::scope(|s| {
            let writers: Vec<_> = meshes
                .iter()
                .map(|mesh| {
                    let (barrier, path) = (&barrier, &path);
                    s.spawn(move || {
                        barrier.wait();
                        export_file(
                            path,
                            ExportFormat::StlBinary,
                            &[NamedMesh { name: "p", mesh }],
                        )
                    })
                })
                .collect();
            writers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        for r in results {
            r.unwrap_or_else(|e| panic!("round {round}: {e}"));
        }
        // The last rename wins; the file is always one whole export, never a mix.
        let got = import_file(&path, false, &mut Progress::silent())
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        let mesh = &got.parts[0].mesh;
        let want = meshes
            .iter()
            .find(|m| m.triangle_count() == mesh.triangle_count())
            .unwrap_or_else(|| panic!("round {round}: {} triangles", mesh.triangle_count()));
        assert_same_shape(mesh, want, "concurrent export");
    }
    assert_eq!(names_in(&dir), ["race.stl"]);
}

#[test]
fn file_names_up_to_the_system_limit_can_be_written() {
    let dir = temp_dir("long-names");
    let mesh = cube(20.0);
    let mut written = Vec::new();
    for length in [249, 255] {
        let name = format!("{}.stl", "n".repeat(length - 4));
        let path = dir.join(&name);
        export_file(
            &path,
            ExportFormat::StlBinary,
            &[NamedMesh {
                name: "c",
                mesh: &mesh,
            }],
        )
        .unwrap_or_else(|e| panic!("{length} characters: {e}"));
        // Writing over an existing file works too.
        export_file(
            &path,
            ExportFormat::StlAscii,
            &[NamedMesh {
                name: "c",
                mesh: &mesh,
            }],
        )
        .unwrap_or_else(|e| panic!("{length} characters: {e}"));
        let got = import_file(&path, false, &mut Progress::silent()).unwrap();
        assert_same_shape(&got.parts[0].mesh, &mesh, "long name");
        written.push(name);
    }
    assert_eq!(names_in(&dir), written);
}

#[test]
fn export_errors_name_the_file() {
    let dir = temp_dir("export-errors");
    let parts = parts();
    let mut meta = Map::new();
    meta.insert(
        "path".into(),
        json!(dir.join("no-such-folder").join("x.stl").to_string_lossy()),
    );
    meta.insert("format".into(), json!("stl-binary"));
    let err =
        jobs::export_mesh(decode(&request(meta, &parts, |_| Map::new())).unwrap()).unwrap_err();
    assert_eq!(err, "Could not write \"x.stl\": the folder does not exist.");

    let mut meta = Map::new();
    meta.insert("path".into(), json!(dir.join("y.stl").to_string_lossy()));
    meta.insert("format".into(), json!("stl-binary"));
    let broken = vec![(
        "Broken",
        Mesh {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 7],
        },
        tricky3d_lib::mesh::IDENTITY,
    )];
    let err =
        jobs::export_mesh(decode(&request(meta, &broken, |_| Map::new())).unwrap()).unwrap_err();
    assert!(
        err.starts_with("Could not write \"y.stl\": the data for \"Broken\" is damaged"),
        "{err}"
    );

    // Paths come from the window: only files with the format's extension are ever written.
    let mut meta = Map::new();
    meta.insert(
        "path".into(),
        json!(dir.join("notes.txt").to_string_lossy()),
    );
    meta.insert("format".into(), json!("stl-ascii"));
    let err =
        jobs::export_mesh(decode(&request(meta, &parts, |_| Map::new())).unwrap()).unwrap_err();
    assert_eq!(
        err,
        "Could not write \"notes.txt\": the file name must end in .stl."
    );
    let mut meta = Map::new();
    meta.insert(
        "path".into(),
        json!(dir.join("project.json").to_string_lossy()),
    );
    meta.insert("platform".into(), json!([200, 200, 200]));
    let err =
        jobs::save_project(decode(&request(meta, &parts, |_| Map::new())).unwrap()).unwrap_err();
    assert_eq!(
        err,
        "Could not save \"project.json\": the file name must end in .3mf."
    );
    assert!(!dir.join("notes.txt").exists() && !dir.join("project.json").exists());
}

#[test]
fn project_round_trip_keeps_fields_and_unbaked_matrices() {
    let dir = temp_dir("project");
    let path = dir.join("my project.3mf");
    let mut parts = parts();
    // Saved meshes are welded already, so they come back vertex for vertex.
    for (_, mesh, _) in &mut parts {
        *mesh = tricky3d_lib::mesh::weld_indexed(&mesh.positions, &mesh.indices).unwrap();
    }
    let fields = [
        ("#e05a2b", true, false, true),
        ("#3f7fbf", false, true, false),
    ];
    let mut meta = Map::new();
    meta.insert("path".into(), json!(path.to_string_lossy()));
    meta.insert("platform".into(), json!([250.0, 210.0, 220.5]));
    let bytes = request(meta, &parts, |k| {
        let (color, visible, transparent, locked) = fields[k];
        let mut info = Map::new();
        info.insert("color".into(), json!(color));
        info.insert("visible".into(), json!(visible));
        info.insert("transparent".into(), json!(transparent));
        info.insert("locked".into(), json!(locked));
        info
    });
    jobs::save_project(decode(&bytes).unwrap()).unwrap_or_else(|e| panic!("{e}"));

    let opened = decode(&jobs::open_project(&path, &mut Progress::silent()).unwrap()).unwrap();
    assert_eq!(opened.meta["platform"], json!([250.0, 210.0, 220.5]));
    assert_eq!(opened.meta["warnings"], json!([]));
    assert_eq!(opened.meshes.len(), 2);
    for (k, m) in opened.meshes.iter().enumerate() {
        let (name, mesh, matrix) = &parts[k];
        let (color, visible, transparent, locked) = fields[k];
        assert_eq!(m.info["name"], json!(name));
        assert_eq!(
            m.info["matrix"],
            json!(matrix.to_vec()),
            "matrices are exact and not baked"
        );
        assert_eq!(m.info["color"], json!(color));
        assert_eq!(m.info["visible"], json!(visible));
        assert_eq!(m.info["transparent"], json!(transparent));
        assert_eq!(m.info["locked"], json!(locked));
        assert_eq!(&m.mesh, mesh, "mesh in part-local coordinates");
        assert_eq!(m.info["stats"]["triangles"], json!(mesh.triangle_count()));
    }

    // Any 3MF reader sees an ordinary package; ours reads it as an import too.
    let as_import = import_file(&path, false, &mut Progress::silent()).unwrap();
    assert_eq!(as_import.format, ImportFormat::ThreeMf);
    assert_eq!(as_import.parts[1].matrix, Some(parts[1].2));
}

#[test]
fn a_3mf_from_elsewhere_opens_as_a_project() {
    let dir = temp_dir("foreign-project");
    let path = dir.join("other.3mf");
    std::fs::write(
        &path,
        package_3mf(&std::fs::read_to_string(fixture("inch_components.model")).unwrap()),
    )
    .unwrap();
    let opened = decode(&jobs::open_project(&path, &mut Progress::silent()).unwrap()).unwrap();
    assert_eq!(opened.meta["platform"], Value::Null);
    assert_eq!(opened.meshes[0].info["name"], "Unit cube");
    assert!(opened.meshes[0].info.get("color").is_none());
    assert_eq!(
        opened.meshes[1].info["matrix"].as_array().unwrap().len(),
        16
    );
}

#[test]
fn a_project_saved_before_the_rename_keeps_its_fields() {
    let dir = temp_dir("meshbench-project");
    let path = dir.join("old.3mf");
    let model = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:meshbench="urn:meshbench:project:2026">
 <metadata name="Application">Meshbench 0.1.0</metadata>
 <metadata name="meshbench:platform">250 210 220.5</metadata>
 <resources>
  <object id="1" type="model" name="Wedge">
   <metadatagroup>
    <metadata name="meshbench:color">#e05a2b</metadata>
    <metadata name="meshbench:visible">false</metadata>
    <metadata name="meshbench:transparent">true</metadata>
    <metadata name="meshbench:locked">true</metadata>
   </metadatagroup>
   <mesh>
    <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/></vertices>
    <triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="1" v2="2" v3="3"/><triangle v1="0" v2="3" v3="2"/></triangles>
   </mesh>
  </object>
 </resources>
 <build><item objectid="1"/></build>
</model>"##;
    std::fs::write(&path, package_3mf(model)).unwrap();
    let opened = decode(&jobs::open_project(&path, &mut Progress::silent()).unwrap()).unwrap();
    assert_eq!(opened.meta["platform"], json!([250.0, 210.0, 220.5]));
    assert_eq!(opened.meta["warnings"], json!([]));
    let part = &opened.meshes[0].info;
    assert_eq!(part["name"], "Wedge");
    assert_eq!(part["color"], "#e05a2b");
    assert_eq!(
        (&part["visible"], &part["transparent"], &part["locked"]),
        (&json!(false), &json!(true), &json!(true))
    );
}

#[test]
fn an_empty_project_keeps_its_platform() {
    let dir = temp_dir("empty-project");
    let path = dir.join("empty.3mf");
    let mut meta = Map::new();
    meta.insert("path".into(), json!(path.to_string_lossy()));
    meta.insert("platform".into(), json!([180, 180, 200]));
    jobs::save_project(decode(&encode(&meta, &[])).unwrap()).unwrap();
    let opened = decode(&jobs::open_project(&path, &mut Progress::silent()).unwrap()).unwrap();
    assert_eq!(opened.meta["platform"], json!([180.0, 180.0, 200.0]));
    assert!(opened.meshes.is_empty());

    // As a mesh import the same file has nothing to offer.
    let err = import_file(&path, false, &mut Progress::silent()).unwrap_err();
    assert_eq!(err, "Could not read \"empty.3mf\": it has no triangles.");
}

#[test]
fn import_bundle_carries_stats_and_problem_edges() {
    let dir = temp_dir("import-bundle");
    let path = dir.join("open box.stl");
    let mut mesh = make_box([0.0; 3], [20.0; 3]);
    mesh.indices.drain(0..6); // no bottom
    let parts = vec![("Box", mesh, tricky3d_lib::mesh::IDENTITY)];
    export(&path, "stl-binary", &parts);

    let mut messages = Vec::new();
    let bytes = jobs::import_mesh(
        &path,
        false,
        &mut Progress::new(|stage, f| messages.push((stage.name(), f))),
    )
    .unwrap();
    let b = decode(&bytes).unwrap();
    assert_eq!(b.meta["format"], "stl");
    assert_eq!(b.meta["objectCount"], 1);
    assert_eq!(b.meta["splittable"], false);
    assert_eq!(b.meta["warnings"], json!([]));
    let m = &b.meshes[0];
    assert_eq!(m.info["name"], "open box");
    assert!(m.info.get("matrix").is_none(), "identity is omitted");
    let stats = &m.info["stats"];
    assert_eq!(stats["triangles"], 10);
    assert_eq!(stats["vertices"], 8);
    assert_eq!(stats["openEdges"], 4);
    assert_eq!(stats["nonManifoldEdges"], 0);
    assert_eq!(stats["shells"], 1);
    assert_eq!(m.mesh.triangle_count(), 10);

    // The edges blob: four open edges, two endpoints each.
    let raw: Value = {
        let h = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        serde_json::from_slice(&bytes[4..4 + h]).unwrap()
    };
    let edges = &raw["meshes"][0]["edges"];
    assert_eq!(edges[1], 4 * 6);
    assert_eq!(edges[0].as_u64().unwrap() % 8, 0);

    assert!(
        !messages.is_empty() && messages.len() < 60,
        "{} progress messages",
        messages.len()
    );
    assert_eq!(messages.last().unwrap(), &("analyze", 1.0));
}

#[test]
fn mesh_stats_command_measures_in_order() {
    let a = make_box([0.0; 3], [20.0; 3]);
    let b = make_box([0.0; 3], [10.0; 3]);
    let out: Vec<OutMesh> = [&a, &b]
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let mut info = Map::new();
            info.insert("name".into(), json!(k.to_string()));
            OutMesh {
                info,
                positions: &m.positions,
                indices: &m.indices,
                edges: None,
            }
        })
        .collect();
    let stats = jobs::mesh_stats(decode(&encode(&Map::new(), &out)).unwrap()).unwrap();
    let json = serde_json::to_value(&stats).unwrap();
    assert_eq!(json[0]["volume"], 8000.0);
    assert_eq!(json[0]["area"], 2400.0);
    assert_eq!(json[1]["volume"], 1000.0);
    assert_eq!(json[0]["openEdges"], 0);
    assert_eq!(json[0]["nonManifoldEdges"], 0);
}
