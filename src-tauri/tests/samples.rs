//! Opt-in tests (`cargo test --release -- --ignored`): sample files for trying the app, the
//! 5-million-triangle import timing, and a mutation run that feeds damaged files to every reader.

mod common;

use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use common::*;
use tricky3d_lib::bundle::decode;
use tricky3d_lib::formats::{ExportFormat, NamedMesh, import_file, ply, threemf, write_export};
use tricky3d_lib::jobs;
use tricky3d_lib::mesh::{Mesh, bake};
use tricky3d_lib::progress::Progress;

fn write(path: &Path, bytes: Vec<u8>) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn export_bytes(format: ExportFormat, title: &str, parts: &[NamedMesh]) -> Vec<u8> {
    write_export(Cursor::new(Vec::new()), format, title, parts)
        .unwrap()
        .into_inner()
}

/// 5,000,000 triangles: a closed torus of 2,000 × 1,250 quads.
fn big_torus() -> Mesh {
    torus(2000, 1250, 60.0, 20.0)
}

/// Writes one sample per import format into TRICKY3D_SAMPLES_DIR (does nothing without it).
#[test]
#[ignore]
fn write_samples() {
    let Some(dir) = std::env::var_os("TRICKY3D_SAMPLES_DIR") else {
        eprintln!("TRICKY3D_SAMPLES_DIR is not set; no samples written");
        return;
    };
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    let cube = cube(20.0);
    let ring = torus(64, 32, 30.0, 10.0);
    let lifted_ring = bake(&ring, &trs([45.0, 0.0, 0.0], 0.0, 0.0, [1.0; 3]));
    let one = |name, mesh| vec![NamedMesh { name, mesh }];

    write(
        &dir.join("torus-binary.stl"),
        export_bytes(ExportFormat::StlBinary, "torus", &one("Torus", &ring)),
    );
    write(
        &dir.join("cube-ascii.stl"),
        export_bytes(ExportFormat::StlAscii, "cube", &one("Cube", &cube)),
    );
    write(
        &dir.join("cube-and-torus.obj"),
        export_bytes(
            ExportFormat::Obj,
            "x",
            &[
                NamedMesh {
                    name: "Cube",
                    mesh: &cube,
                },
                NamedMesh {
                    name: "Torus",
                    mesh: &lifted_ring,
                },
            ],
        ),
    );
    write(
        &dir.join("torus-ascii.ply"),
        ply::write_text(Vec::new(), &one("Torus", &ring)).unwrap(),
    );
    write(
        &dir.join("torus-binary-le.ply"),
        export_bytes(ExportFormat::Ply, "x", &one("Torus", &ring)),
    );
    write(&dir.join("cube-binary-be.ply"), ply_big_endian(&cube));

    // Three build items with transforms (placements stay part matrices on import).
    let small = make_small_cube();
    let multi = threemf::ModelOut {
        namespaces: &[],
        metadata: vec![("Application", threemf::application())],
        objects: vec![
            threemf::ObjectOut {
                name: "Cube",
                mesh: &cube,
                transform: Some(trs([-40.0, 0.0, 0.0], 0.0, 0.0, [1.0; 3])),
                metadata: vec![],
            },
            threemf::ObjectOut {
                name: "Torus",
                mesh: &ring,
                transform: Some(trs([20.0, 0.0, 0.0], 0.0, 0.0, [1.0; 3])),
                metadata: vec![],
            },
            threemf::ObjectOut {
                name: "Turned block",
                mesh: &small,
                transform: Some(trs([0.0, 50.0, 0.0], 30.0, 0.0, [2.0, 1.0, 1.0])),
                metadata: vec![],
            },
        ],
    };
    write(
        &dir.join("three-items.3mf"),
        threemf::write(Cursor::new(Vec::new()), &multi)
            .unwrap()
            .into_inner(),
    );
    write(
        &dir.join("inches-components.3mf"),
        package_3mf(&std::fs::read_to_string(fixture("inch_components.model")).unwrap()),
    );

    let amf = export_bytes(
        ExportFormat::Amf,
        "x",
        &[
            NamedMesh {
                name: "Cube",
                mesh: &cube,
            },
            NamedMesh {
                name: "Torus",
                mesh: &lifted_ring,
            },
        ],
    );
    write(&dir.join("cube-and-torus.amf"), amf.clone());
    write(
        &dir.join("cube-and-torus-zipped.amf"),
        zip_bytes(&[("cube-and-torus.amf", &amf)]),
    );

    let big = big_torus();
    write(
        &dir.join("big-5m.stl"),
        export_bytes(ExportFormat::StlBinary, "big", &one("Big torus", &big)),
    );

    // Every sample must import.
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e != "md") {
            let got = import_file(&path, false, &mut Progress::silent())
                .unwrap_or_else(|e| panic!("{e}"));
            eprintln!("{}: {} part(s)", path.display(), got.parts.len());
        }
    }
}

fn make_small_cube() -> Mesh {
    tricky3d_lib::mesh::make_box([-5.0, -5.0, 0.0], [5.0, 5.0, 10.0])
}

/// Times a 5M-triangle binary STL import through `jobs::import_mesh`, the code the command runs
/// (read, parse, weld, measure, problem edges, bundle). Run in release mode.
#[test]
#[ignore]
fn big_import_timing() {
    let dir = temp_dir("big");
    let path = dir.join("big-5m.stl");
    let mesh = big_torus();
    write(
        &path,
        export_bytes(
            ExportFormat::StlBinary,
            "big",
            &[NamedMesh {
                name: "Big",
                mesh: &mesh,
            }],
        ),
    );
    drop(mesh);
    let size = std::fs::metadata(&path).unwrap().len();

    let mut messages = 0;
    let start = Instant::now();
    let bytes = jobs::import_mesh(&path, false, &mut Progress::new(|_, _| messages += 1)).unwrap();
    let elapsed = start.elapsed();
    eprintln!(
        "big-5m.stl ({:.0} MB): imported in {:.2} s with {messages} progress messages",
        size as f64 / 1e6,
        elapsed.as_secs_f64()
    );

    let b = decode(&bytes).unwrap();
    let stats = &b.meshes[0].info["stats"];
    assert_eq!(stats["triangles"], 5_000_000);
    assert_eq!(stats["vertices"], 2_500_000);
    assert_eq!(stats["openEdges"], 0);
    assert_eq!(stats["shells"], 1);
    assert!(messages < 60, "{messages} progress messages");
}

/// Feeds truncated and randomly damaged copies of valid files to every reader; none may panic
/// (the release build aborts on a panic).
#[test]
#[ignore]
fn damaged_files_never_panic() {
    let dir = temp_dir("fuzz");
    let cube = cube(20.0);
    let ring = torus(12, 6, 30.0, 10.0);
    let parts = [
        NamedMesh {
            name: "Cube & <ring>",
            mesh: &cube,
        },
        NamedMesh {
            name: "Ring",
            mesh: &ring,
        },
    ];
    let mut seeds: Vec<(&str, Vec<u8>)> = vec![
        ("stl", export_bytes(ExportFormat::StlBinary, "x", &parts)),
        ("stl", export_bytes(ExportFormat::StlAscii, "x", &parts)),
        ("obj", export_bytes(ExportFormat::Obj, "x", &parts)),
        ("obj", std::fs::read(fixture("two_objects.obj")).unwrap()),
        ("ply", export_bytes(ExportFormat::Ply, "x", &parts)),
        ("ply", ply::write_text(Vec::new(), &parts).unwrap()),
        ("ply", ply_big_endian(&ring)),
        ("ply", std::fs::read(fixture("quad_cube.ply")).unwrap()),
        ("3mf", export_bytes(ExportFormat::ThreeMf, "x", &parts)),
        ("amf", export_bytes(ExportFormat::Amf, "x", &parts)),
        (
            "amf",
            std::fs::read(fixture("two_objects_inch.amf")).unwrap(),
        ),
    ];
    // The 3MF model text itself, damaged before zipping, reaches the XML reader.
    let model = std::fs::read_to_string(fixture("inch_components.model")).unwrap();
    seeds.push(("model", model.into_bytes()));

    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut runs = 0;
    for (ext, seed) in &seeds {
        for round in 0..300 {
            let mut bytes = seed.clone();
            match round % 3 {
                0 => bytes.truncate((next() as usize) % (bytes.len() + 1)),
                1 => {
                    for _ in 0..1 + next() % 8 {
                        let at = (next() as usize) % bytes.len();
                        bytes[at] = next() as u8;
                    }
                }
                _ => {
                    // Digits and separators are where the parsers' number handling lives.
                    for _ in 0..1 + next() % 4 {
                        let at = (next() as usize) % bytes.len();
                        bytes[at] = b"0123456789-+.e /<>\"\n"[(next() % 20) as usize];
                    }
                }
            }
            let (file_ext, data) = if *ext == "model" {
                ("3mf", package_3mf(&String::from_utf8_lossy(&bytes)))
            } else {
                (*ext, bytes)
            };
            let path = dir.join(format!("case.{file_ext}"));
            std::fs::write(&path, &data).unwrap();
            let _ = import_file(&path, round % 2 == 0, &mut Progress::silent());
            let _ = tricky3d_lib::project::read(&data, &mut Progress::silent());
            runs += 1;
        }
    }
    eprintln!("{runs} damaged files read without a panic");
}
