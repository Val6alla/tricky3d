//! What you see is what you export. A model is imported (and centred on the platform, as the window
//! does), worked on, and exported; every part must land where the platform showed it, in every
//! format and in any other program. These tests take the jobs the window runs (import, cut, merge,
//! generate shell, export, save) and compare every part's box after the round trip with the box the
//! platform showed (src/mesh.ts boundsOf: the part's own vertices through its matrix), to 0.001 mm,
//! and every triangle with the triangle the platform showed. Export writes all parts to one file or
//! one file per part (jobs::export_each), which never replaces a file without asking.
//!
//! The parts: a barrel and a stand modelled 5 m from the origin in their file and centred on the
//! platform on import; the stand moved, turned and scaled; the barrel cut in two; two bolts merged;
//! a box hollowed by Generate shell; a Boolean result (a world-space mesh with the identity matrix,
//! as src/boolean/session.ts adds it); and a part imported without centring, still 5 m out.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{Bundle, OutMesh, decode, encode};
use tricky3d_lib::formats::threemf::{self, ModelOut, ObjectOut};
use tricky3d_lib::formats::{ExportFormat, NamedMesh, export_file, import_file};
use tricky3d_lib::jobs::{self, RepairJobs};
use tricky3d_lib::mesh::{IDENTITY, Matrix, Mesh, bake, bounds, multiply};
use tricky3d_lib::ops::shells::split_shells;
use tricky3d_lib::progress::Progress;
use tricky3d_lib::project::{self, PartOut};

const TOLERANCE: f64 = 0.001;

const PLATFORM: [f64; 3] = [200.0, 200.0, 200.0];

/// Every export format with its extension.
const FORMATS: [(&str, &str); 7] = [
    ("stl-binary", "stl"),
    ("stl-ascii", "stl"),
    ("obj", "obj"),
    ("ply", "ply"),
    ("3mf", "3mf"),
    ("amf", "amf"),
    ("off", "off"),
];

/// Formats that keep one object per part; the others merge every part into one mesh.
const PER_PART: [&str; 3] = ["obj", "3mf", "amf"];

/// A part as the window holds it: its own mesh and the matrix that places it.
#[derive(Clone, Debug)]
struct Part {
    name: String,
    mesh: Mesh,
    matrix: Matrix,
}

type Box3 = [[f64; 3]; 2];

/// The box the platform shows for a part: each of its vertices through its matrix, in f64, as
/// boundsOf in src/mesh.ts.
fn shown(mesh: &Mesh, m: &Matrix) -> Box3 {
    let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for p in mesh.positions.as_chunks::<3>().0 {
        let [x, y, z] = p.map(f64::from);
        let w = [
            m[0] * x + m[4] * y + m[8] * z + m[12],
            m[1] * x + m[5] * y + m[9] * z + m[13],
            m[2] * x + m[6] * y + m[10] * z + m[14],
        ];
        for k in 0..3 {
            b[0][k] = b[0][k].min(w[k]);
            b[1][k] = b[1][k].max(w[k]);
        }
    }
    b
}

fn box_of(mesh: &Mesh) -> Box3 {
    let b = bounds(mesh).unwrap();
    [b[0].map(f64::from), b[1].map(f64::from)]
}

/// The boxes of a world-space mesh's shells, in a fixed order.
fn shell_boxes(world: &Mesh) -> Vec<Box3> {
    let mut out: Vec<Box3> = split_shells(world).iter().map(box_of).collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

fn assert_box(got: Box3, want: Box3, what: &str) {
    for side in 0..2 {
        for k in 0..3 {
            assert!(
                (got[side][k] - want[side][k]).abs() <= TOLERANCE,
                "{what}: box {got:?}, the platform showed {want:?}"
            );
        }
    }
}

fn assert_boxes(got: &[Box3], want: &[Box3], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: shells");
    for (g, w) in got.iter().zip(want) {
        assert_box(*g, *w, what);
    }
}

/// A bundle the window sends: meta plus each part's name, matrix and mesh.
fn bundle(meta: Map<String, Value>, parts: &[&Part]) -> Bundle {
    let out: Vec<OutMesh> = parts
        .iter()
        .map(|p| {
            let mut info = Map::new();
            info.insert("name".into(), json!(p.name));
            info.insert("matrix".into(), json!(p.matrix.to_vec()));
            OutMesh {
                info,
                positions: &p.mesh.positions,
                indices: &p.mesh.indices,
                edges: None,
            }
        })
        .collect();
    decode(&encode(&meta, &out)).unwrap()
}

/// The world-space results of an operation's answer, each with the identity matrix as the window
/// adds them.
fn results(bytes: &[u8], name: &str) -> Vec<Part> {
    decode(bytes)
        .unwrap()
        .meshes
        .into_iter()
        .enumerate()
        .map(|(k, m)| Part {
            name: format!("{name} {}", k + 1),
            mesh: m.mesh,
            matrix: IDENTITY,
        })
        .collect()
}

fn translation(t: [f64; 3]) -> Matrix {
    let mut m = IDENTITY;
    m[12..15].copy_from_slice(&t);
    m
}

/// Import as the window does it: read and weld in Rust, then (when placing) one translation for the
/// whole file that rests it on the platform with its box centred (src/io/files.ts placedMatrices).
fn import(path: &Path, place: bool) -> Vec<Part> {
    let imported = import_file(path, false, &mut Progress::silent()).unwrap();
    let mut parts: Vec<Part> = imported
        .parts
        .into_iter()
        .map(|p| Part {
            name: p.name,
            mesh: p.mesh,
            matrix: p.matrix.unwrap_or(IDENTITY),
        })
        .collect();
    if place {
        let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for p in &parts {
            let s = shown(&p.mesh, &p.matrix);
            for k in 0..3 {
                b[0][k] = b[0][k].min(s[0][k]);
                b[1][k] = b[1][k].max(s[1][k]);
            }
        }
        let offset = [
            PLATFORM[0] / 2.0 - (b[0][0] + b[1][0]) / 2.0,
            PLATFORM[1] / 2.0 - (b[0][1] + b[1][1]) / 2.0,
            -b[0][2],
        ];
        for p in &mut parts {
            p.matrix = multiply(&translation(offset), &p.matrix);
        }
    }
    parts
}

/// A cylinder of `radius` and `length` along X with its axis at `centre`, closed.
fn cylinder_x(radius: f64, length: f64, centre: [f64; 3]) -> Mesh {
    let mut params = Map::new();
    params.insert("radius".into(), json!(radius));
    params.insert("height".into(), json!(length));
    params.insert("segments".into(), json!(48));
    let along_z = tricky3d_lib::library::generate("cylinder", &params).unwrap();
    // z up the axis → x; the cylinder stands on z = 0, so its middle is at length / 2.
    let turn: Matrix = [
        0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let place = translation([centre[0] - length / 2.0, centre[1], centre[2]]);
    bake(&along_z, &multiply(&place, &turn))
}

/// The model file: a barrel resting on a stand, both 5 m from the origin.
fn gun_file(dir: &Path) -> PathBuf {
    let far = [5000.0, -3000.0, 250.0];
    let stand = cube(30.0);
    let stand = bake(&stand, &translation(far));
    let barrel = cylinder_x(6.0, 80.0, [far[0] + 15.0, far[1] + 15.0, far[2] + 36.5]);
    let path = dir.join("gun.obj");
    export_file(
        &path,
        ExportFormat::Obj,
        &[
            NamedMesh {
                name: "barrel",
                mesh: &barrel,
            },
            NamedMesh {
                name: "stand",
                mesh: &stand,
            },
        ],
    )
    .unwrap();
    path
}

/// The parts on the platform after the session the module comment describes.
fn worked_on(dir: &Path) -> Vec<Part> {
    let file = gun_file(dir);
    let mut parts = import(&file, true);
    assert_eq!(parts.len(), 2);
    let barrel = parts.remove(0);
    let mut stand = parts.remove(0);
    // Gizmo moves, turns and scales act in world space on top of the placement.
    stand.matrix = multiply(
        &trs([-40.0, 25.0, 0.0], 30.0, 0.0, [1.2, 0.8, 1.0]),
        &stand.matrix,
    );

    // Cut the barrel across (capped, 2 mm gap), as the Cut dialog does through Rust.
    let x = (shown(&barrel.mesh, &barrel.matrix)[0][0] + shown(&barrel.mesh, &barrel.matrix)[1][0])
        / 2.0;
    let mut meta = Map::new();
    meta.insert("normal".into(), json!([1.0, 0.0, 0.0]));
    meta.insert("offset".into(), json!(x));
    meta.insert("cap".into(), json!(true));
    meta.insert("gap".into(), json!(2.0));
    let cut = results(
        &jobs::cut_meshes(bundle(meta, &[&barrel])).unwrap(),
        "barrel (cut)",
    );
    assert_eq!(cut.len(), 2);

    // Two bolts merged into one part.
    let bolt = |t: [f64; 3]| Part {
        name: "bolt".into(),
        mesh: cube(4.0),
        matrix: trs(t, 15.0, 10.0, [1.0, 1.0, 2.0]),
    };
    let merged = results(
        &jobs::merge_meshes(bundle(
            Map::new(),
            &[&bolt([20.0, 20.0, 0.0]), &bolt([40.0, 30.0, 5.0])],
        ))
        .unwrap(),
        "Merge of 2 parts",
    );

    // A box hollowed by Generate shell.
    let mut meta = Map::new();
    meta.insert("job".into(), json!(1));
    meta.insert(
        "settings".into(),
        json!({"kind": "hollow-part", "thickness": 2.0, "accuracy": 0.5, "smooth": false}),
    );
    let box_part = Part {
        name: "Box".into(),
        mesh: cube(20.0),
        matrix: trs([150.0, 150.0, 4.0], 20.0, 0.0, [1.0; 3]),
    };
    let hollow = results(
        &jobs::shell_generate(
            bundle(meta, &[&box_part]),
            &RepairJobs::default(),
            &mut |_| {},
        )
        .unwrap(),
        "Box (hollow)",
    );
    assert_eq!(hollow.len(), 1);

    // A Boolean result: world space, identity matrix.
    let boolean = Part {
        name: "Union".into(),
        mesh: bake(
            &torus(24, 12, 10.0, 3.0),
            &trs([160.0, 40.0, 0.0], 0.0, 0.0, [1.0; 3]),
        ),
        matrix: IDENTITY,
    };

    // Imported without centring: still 5 m out.
    let far = import(&file, false).remove(1);

    let mut all = vec![stand];
    all.extend(cut);
    all.extend(merged);
    all.extend(hollow);
    all.push(boolean);
    all.push(Part {
        name: "stand as in the file".into(),
        ..far
    });
    all
}

fn export(path: &Path, format: &str, parts: &[&Part]) {
    let mut meta = Map::new();
    meta.insert("path".into(), json!(path.to_string_lossy()));
    meta.insert("format".into(), json!(format));
    jobs::export_mesh(bundle(meta, parts)).unwrap_or_else(|e| panic!("{format}: {e}"));
}

/// Re-imported into a fresh project with centring off: each part's world mesh.
fn reimported(path: &Path) -> Vec<Mesh> {
    import(path, false)
        .into_iter()
        .map(|p| bake(&p.mesh, &p.matrix))
        .collect()
}

fn platform_shells(parts: &[&Part]) -> Vec<Box3> {
    let mut out: Vec<Box3> = parts
        .iter()
        .flat_map(|p| shell_boxes(&bake(&p.mesh, &p.matrix)))
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

#[test]
fn every_format_puts_every_part_where_the_platform_showed_it() {
    let dir = temp_dir("export-positions");
    let parts = worked_on(&dir);
    let all: Vec<&Part> = parts.iter().collect();
    for (format, ext) in FORMATS {
        // Several parts in one file: each where it was, so they keep their places relative to each other.
        let path = dir.join(format!("all-{format}.{ext}"));
        export(&path, format, &all);
        let back = reimported(&path);
        if PER_PART.contains(&format) {
            assert_eq!(back.len(), parts.len(), "{format}: parts");
            for (got, part) in back.iter().zip(&parts) {
                assert_box(
                    box_of(got),
                    shown(&part.mesh, &part.matrix),
                    &format!("{format}, {} in one file", part.name),
                );
            }
        }
        let mut got: Vec<Box3> = back.iter().flat_map(shell_boxes).collect();
        got.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_boxes(&got, &platform_shells(&all), &format!("{format}, one file"));

        // Each part in a file of its own.
        for (k, part) in parts.iter().enumerate() {
            let path = dir.join(format!("part{k}-{format}.{ext}"));
            export(&path, format, &[part]);
            let back = reimported(&path);
            let mut got: Vec<Box3> = back.iter().flat_map(shell_boxes).collect();
            got.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_boxes(
                &got,
                &platform_shells(&[part]),
                &format!("{format}, {} on its own", part.name),
            );
        }
    }
}

#[test]
fn exported_3mf_and_amf_state_millimetres_and_3mf_items_carry_no_transform() {
    let dir = temp_dir("export-units");
    let parts = worked_on(&dir);
    let all: Vec<&Part> = parts.iter().collect();
    let threemf_path = dir.join("all.3mf");
    export(&threemf_path, "3mf", &all);
    let package = threemf::read(
        &std::fs::read(&threemf_path).unwrap(),
        &mut Progress::silent(),
    )
    .unwrap();
    assert_eq!(package.items.len(), parts.len());
    for (item, part) in package.items.iter().zip(&parts) {
        assert_eq!(
            item.matrix, IDENTITY,
            "{}: one object, no transform",
            part.name
        );
        assert_box(
            box_of(&item.mesh),
            shown(&part.mesh, &part.matrix),
            &part.name,
        );
    }
    let model = unzip_model(&threemf_path);
    assert!(model.contains("unit=\"millimeter\""));
    assert!(!model.contains("<components") && !model.contains("transform="));

    let amf_path = dir.join("all.amf");
    export(&amf_path, "amf", &all);
    let text = std::fs::read_to_string(&amf_path).unwrap();
    assert!(text.contains("<amf unit=\"millimeter\""));
    assert!(!text.contains("<constellation"));
}

fn unzip_model(path: &Path) -> String {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    zip.by_name("3D/3dmodel.model")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// The boxes a program that does not apply 3MF build transforms sees: each object's vertices as
/// they are in the file.
fn without_transforms(path: &Path) -> Vec<Box3> {
    threemf::read(&std::fs::read(path).unwrap(), &mut Progress::silent())
        .unwrap()
        .items
        .iter()
        .map(|item| box_of(&item.mesh))
        .collect()
}

/// How Save project wrote a project before this change: each object in part-local coordinates,
/// with the part's matrix as its build item's transform.
fn save_project_as_before(path: &Path, parts: &[Part]) {
    let objects = parts
        .iter()
        .map(|p| ObjectOut {
            name: &p.name,
            mesh: &p.mesh,
            transform: Some(p.matrix),
            metadata: Vec::new(),
        })
        .collect();
    let model = ModelOut {
        namespaces: &[],
        metadata: Vec::new(),
        objects,
    };
    let file = std::fs::File::create(path).unwrap();
    threemf::write(file, &model).unwrap();
}

fn save_project(path: &Path, parts: &[Part]) {
    let out: Vec<PartOut> = parts
        .iter()
        .map(|p| PartOut {
            name: &p.name,
            matrix: p.matrix,
            color: None,
            visible: true,
            transparent: false,
            locked: false,
            priority: None,
            rotation: None,
            measurements: None,
            source: Some("C:\\models\\gun.obj"),
            mesh: &p.mesh,
        })
        .collect();
    project::save_file(path, PLATFORM, &out).unwrap();
}

/// The bug as it was: the 3MF that Save wrote placed each part by its build item's transform. A
/// part fresh from an import is placed by its centring translation, a moved part by its matrix, and
/// a result of Cut, Merge, Generate shell or a Boolean by none at all (it is already in world
/// space), so a program that leaves the transforms out puts the imported parts back 5 m out, the
/// moved ones elsewhere again, and the results where they were: the barrel in one place, its stand
/// in another.
#[test]
fn a_saved_project_no_longer_relies_on_build_transforms() {
    let dir = temp_dir("project-positions");
    let parts = worked_on(&dir);
    let platform: Vec<Box3> = parts.iter().map(|p| shown(&p.mesh, &p.matrix)).collect();

    let before = dir.join("before.3mf");
    save_project_as_before(&before, &parts);
    let scattered = without_transforms(&before);
    let off = |a: Box3, b: Box3| {
        (0..3)
            .map(|k| (a[0][k] - b[0][k]).abs())
            .fold(0.0, f64::max)
    };
    let stand = off(scattered[0], platform[0]);
    let barrel = off(scattered[1], platform[1]);
    assert!(
        stand > 1000.0,
        "the stand landed {stand} mm from where the platform showed it"
    );
    assert!(barrel < TOLERANCE, "the cut barrel stayed put");

    let after = dir.join("after.3mf");
    save_project(&after, &parts);
    for (got, (want, part)) in without_transforms(&after)
        .iter()
        .zip(platform.iter().zip(&parts))
    {
        assert_box(*got, *want, &format!("{} without transforms", part.name));
    }
    let package = threemf::read(&std::fs::read(&after).unwrap(), &mut Progress::silent()).unwrap();
    assert!(package.items.iter().all(|item| item.matrix == IDENTITY));

    // Opened again, every part gets its own matrix back exactly and sits where it was.
    let opened = decode(&jobs::open_project(&after, &mut Progress::silent()).unwrap()).unwrap();
    assert_eq!(opened.meshes.len(), parts.len());
    for (m, part) in opened.meshes.iter().zip(&parts) {
        assert_eq!(
            m.info["matrix"],
            json!(part.matrix.to_vec()),
            "{}",
            part.name
        );
        assert_eq!(m.info["source"], json!("C:\\models\\gun.obj"));
        let matrix: Matrix = serde_json::from_value::<Vec<f64>>(m.info["matrix"].clone())
            .unwrap()
            .try_into()
            .unwrap();
        assert_box(
            shown(&m.mesh, &matrix),
            shown(&part.mesh, &part.matrix),
            &part.name,
        );
        assert_eq!(
            m.mesh.triangle_count(),
            part.mesh.triangle_count(),
            "{}",
            part.name
        );
    }

    // A project saved before this change still opens with every part where it was.
    let opened = decode(&jobs::open_project(&before, &mut Progress::silent()).unwrap()).unwrap();
    for (m, part) in opened.meshes.iter().zip(&parts) {
        let matrix: Matrix = serde_json::from_value::<Vec<f64>>(m.info["matrix"].clone())
            .unwrap()
            .try_into()
            .unwrap();
        assert_box(
            shown(&m.mesh, &matrix),
            shown(&part.mesh, &part.matrix),
            &part.name,
        );
    }
}

/// Saving a reopened project again writes the same vertices: taking the matrix out and baking it
/// back in settles after the first save instead of drifting.
#[test]
fn saving_a_reopened_project_again_does_not_drift() {
    let dir = temp_dir("project-drift");
    let parts = worked_on(&dir);
    let mut current = parts.clone();
    let mut previous: Option<Vec<Mesh>> = None;
    for round in 0..4 {
        let path = dir.join(format!("round{round}.3mf"));
        save_project(&path, &current);
        let opened = decode(&jobs::open_project(&path, &mut Progress::silent()).unwrap()).unwrap();
        current = opened
            .meshes
            .into_iter()
            .zip(&parts)
            .map(|(m, p)| Part {
                name: p.name.clone(),
                mesh: m.mesh,
                matrix: p.matrix,
            })
            .collect();
        let written: Vec<Mesh> =
            threemf::read(&std::fs::read(&path).unwrap(), &mut Progress::silent())
                .unwrap()
                .items
                .into_iter()
                .map(|item| item.mesh)
                .collect();
        if let Some(previous) = &previous
            && round >= 2
        {
            assert_eq!(&written, previous, "round {round} wrote other vertices");
        }
        previous = Some(written);
    }
}

type Triangle = [[f32; 3]; 3];

/// A mesh's triangles as corner coordinates, each starting at its smallest corner so the winding
/// is kept, sorted: two meshes with the same triangles where they are give the same list, however
/// a file numbers or orders its vertices and triangles.
fn triangles<'m>(meshes: impl IntoIterator<Item = &'m Mesh>) -> Vec<Triangle> {
    let mut out: Vec<Triangle> = meshes
        .into_iter()
        .flat_map(|mesh| {
            mesh.indices.as_chunks::<3>().0.iter().map(|t| {
                let v = t.map(|i| {
                    let i = i as usize * 3;
                    [
                        mesh.positions[i],
                        mesh.positions[i + 1],
                        mesh.positions[i + 2],
                    ]
                });
                let first = (0..3)
                    .min_by(|&a, &b| v[a].partial_cmp(&v[b]).unwrap())
                    .unwrap();
                [v[first], v[(first + 1) % 3], v[(first + 2) % 3]]
            })
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

/// The same triangles, facing the same way, at the same places to 0.001 mm.
fn assert_same_triangles(got: &[Mesh], want: &[Mesh], what: &str) {
    let (got, want) = (triangles(got), triangles(want));
    assert_eq!(got.len(), want.len(), "{what}: triangle count");
    for (g, w) in got.iter().zip(&want) {
        let off = (0..3)
            .flat_map(|c| (0..3).map(move |k| (g[c][k] - w[c][k]).abs()))
            .fold(0.0f32, f32::max);
        assert!(
            f64::from(off) <= TOLERANCE,
            "{what}: triangle {g:?}, the platform showed {w:?}"
        );
    }
}

/// Each part's mesh as the platform shows it.
fn on_platform(parts: &[&Part]) -> Vec<Mesh> {
    parts.iter().map(|p| bake(&p.mesh, &p.matrix)).collect()
}

/// Export → import into a fresh project with centring off gives back every triangle where the
/// platform showed it, in every format, binary and ASCII STL both: all parts in one file, and each
/// part in a file of its own.
#[test]
fn every_format_gives_back_the_same_triangles_where_they_were() {
    let dir = temp_dir("export-triangles");
    let parts = worked_on(&dir);
    let all: Vec<&Part> = parts.iter().collect();
    for (format, ext) in FORMATS {
        let path = dir.join(format!("all-{format}.{ext}"));
        export(&path, format, &all);
        assert_same_triangles(
            &reimported(&path),
            &on_platform(&all),
            &format!("{format}, one file"),
        );
        if PER_PART.contains(&format) {
            // One object per part, each part's own triangles.
            for (got, part) in reimported(&path).iter().zip(&parts) {
                assert_same_triangles(
                    std::slice::from_ref(got),
                    &on_platform(&[part]),
                    &format!("{format}, {} in one file", part.name),
                );
            }
        }
    }
}

/// Export with one file per part, as the window sends it to Rust.
fn export_each(path: &Path, format: &str, parts: &[&Part], replace: bool) -> jobs::EachResult {
    let mut meta = Map::new();
    meta.insert("path".into(), json!(path.to_string_lossy()));
    meta.insert("format".into(), json!(format));
    meta.insert("replace".into(), json!(replace));
    jobs::export_each(bundle(meta, parts)).unwrap_or_else(|e| panic!("{format}: {e}"))
}

/// One file per part: each named after its part in the folder picked (the name picked is not
/// written), parts with the same name do not overwrite each other, and every part comes back where
/// it was, in every format.
#[test]
fn one_file_per_part_names_each_file_after_its_part_and_keeps_its_place() {
    let dir = temp_dir("export-each");
    let mut parts = worked_on(&dir);
    // A copy keeps its original's name.
    let copy = Part {
        matrix: multiply(&translation([0.0, 60.0, 0.0]), &parts[0].matrix),
        ..parts[0].clone()
    };
    parts.push(copy);
    parts.push(Part {
        name: "plate: top/left?".into(),
        ..parts[1].clone()
    });
    let all: Vec<&Part> = parts.iter().collect();
    for (format, ext) in FORMATS {
        let folder = dir.join(format!("each-{format}"));
        std::fs::create_dir_all(&folder).unwrap();
        let picked = folder.join(format!("picked name.{ext}"));
        let done = export_each(&picked, format, &all, false);
        assert!(done.existing.is_empty(), "{format}");
        let want: Vec<String> = [
            "gun - stand",
            "barrel (cut) 1",
            "barrel (cut) 2",
            "Merge of 2 parts 1",
            "Box (hollow) 1",
            "Union",
            "stand as in the file",
            "gun - stand (2)",
            "plate_ top_left_",
        ]
        .iter()
        .map(|name| format!("{name}.{ext}"))
        .collect();
        assert_eq!(done.written, want, "{format}");
        assert!(!picked.exists(), "{format}: the name picked is not written");
        let mut files: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let mut sorted = want.clone();
        sorted.sort();
        assert_eq!(files, sorted, "{format}: no other file");
        for (name, part) in want.iter().zip(&parts) {
            assert_same_triangles(
                &reimported(&folder.join(name)),
                &on_platform(&[part]),
                &format!("{format}, {name}"),
            );
        }
    }
}

/// One file per part never replaces a file without asking: while any of its files is there it
/// writes none and names them, and only the answer "Replace" (`replace`) writes over them.
#[test]
fn one_file_per_part_asks_before_replacing_and_writes_nothing_until_then() {
    let dir = temp_dir("export-each-replace");
    let parts = worked_on(&dir);
    let all: Vec<&Part> = parts[..3].iter().collect();
    let picked = dir.join("parts.stl");
    let kept = b"a file that was there before";
    std::fs::write(dir.join("barrel (cut) 2.stl"), kept).unwrap();
    std::fs::write(dir.join("GUN - STAND.stl"), kept).unwrap();
    let asked = export_each(&picked, "stl-binary", &all, false);
    assert!(asked.written.is_empty());
    // A file system that ignores letter case (Windows, and macOS by default) sees
    // "GUN - STAND.stl" as "gun - stand.stl"; it is the folder that decides, not the system.
    let mut existing = asked.existing.clone();
    existing.sort();
    let want = if dir.join("gun - stand.stl").exists() {
        vec!["barrel (cut) 2.stl", "gun - stand.stl"]
    } else {
        vec!["barrel (cut) 2.stl"]
    };
    assert_eq!(existing, want);
    assert_eq!(std::fs::read(dir.join("barrel (cut) 2.stl")).unwrap(), kept);
    assert!(
        !dir.join("barrel (cut) 1.stl").exists(),
        "nothing written before the answer"
    );

    let done = export_each(&picked, "stl-ascii", &all, true);
    assert!(done.existing.is_empty());
    assert_eq!(
        done.written,
        [
            "gun - stand.stl",
            "barrel (cut) 1.stl",
            "barrel (cut) 2.stl"
        ]
    );
    for (name, part) in done.written.iter().zip(&parts) {
        let path = dir.join(name);
        assert!(
            std::fs::read(&path).unwrap().starts_with(b"solid "),
            "{name}: ASCII STL"
        );
        assert_same_triangles(&reimported(&path), &on_platform(&[part]), name);
    }
}

/// A file's short 8.3 name, which Windows also answers to; the long one when the volume keeps none.
#[cfg(windows)]
fn short_name(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `buf` holds 1024 units; both outlive the call.
    let n = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            wide.as_ptr(),
            buf.as_mut_ptr(),
            1024,
        )
    } as usize;
    assert!(n > 0 && n < buf.len(), "no short name for {path:?}");
    PathBuf::from(OsString::from_wide(&buf[..n]))
}

/// One file per part never writes one part over another through a short 8.3 name. Windows also
/// answers to "GUN-ST~1.STL" for "gun - stand.stl", so a part with that name would otherwise
/// replace the file written for the stand a moment before.
#[cfg(windows)]
#[test]
fn one_file_per_part_never_writes_over_its_own_file_through_a_short_name() {
    let dir = temp_dir("export-each-short-names");
    let parts = worked_on(&dir);
    let stand = &parts[0];
    // The short name the stand's file gets in an empty folder.
    let probe = dir.join("probe");
    std::fs::create_dir_all(&probe).unwrap();
    export_each(&probe.join("x.stl"), "stl-binary", &[stand], false);
    let long = probe.join(format!("{}.stl", stand.name));
    let alias = short_name(&long);
    if alias == long {
        eprintln!("this volume keeps no short names; nothing to test");
        return;
    }
    let alias_stem = alias.file_stem().unwrap().to_string_lossy().into_owned();
    let twin = Part {
        name: alias_stem.clone(),
        ..parts[1].clone()
    };

    let folder = dir.join("each");
    std::fs::create_dir_all(&folder).unwrap();
    let done = export_each(&folder.join("x.stl"), "stl-binary", &[stand, &twin], false);
    assert_eq!(
        done.written,
        [
            format!("{}.stl", stand.name),
            format!("{alias_stem} (2).stl")
        ]
    );
    for (name, part) in done.written.iter().zip([stand, &twin]) {
        assert_same_triangles(&reimported(&folder.join(name)), &on_platform(&[part]), name);
    }

    // An old "gun - stand.stl" is there, so both names are, and the answer is Replace: the stand
    // replaces the old file, and the twin still does not land on the stand's new one.
    let again = dir.join("again");
    std::fs::create_dir_all(&again).unwrap();
    std::fs::write(again.join(format!("{}.stl", stand.name)), b"old").unwrap();
    let asked = export_each(&again.join("x.stl"), "stl-binary", &[stand, &twin], false);
    assert_eq!(asked.existing.len(), 2, "{:?}", asked.existing);
    let done = export_each(&again.join("x.stl"), "stl-binary", &[stand, &twin], true);
    assert_eq!(
        done.written,
        [
            format!("{}.stl", stand.name),
            format!("{alias_stem} (2).stl")
        ]
    );
    for (name, part) in done.written.iter().zip([stand, &twin]) {
        assert_same_triangles(&reimported(&again.join(name)), &on_platform(&[part]), name);
    }
}
