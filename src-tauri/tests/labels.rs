//! Labels end to end, through the job the `label_apply` command runs: the part arrives with its
//! matrix and comes back labelled in world space with its stats; a picture arrives as grey levels;
//! a list makes named copies; a run reports progress and can be cancelled; a label that cannot go
//! where it is says why; and the labelled part is saved in a project and opened again unchanged.

mod common;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{BlobRef, OutMesh, decode, encode_with_extra};
use tricky3d_lib::jobs::{self, CANCELLED, Fonts, RepairJobs};
use tricky3d_lib::mesh::{IDENTITY, Mesh, stats};
use tricky3d_lib::progress::Progress;
use tricky3d_lib::project::{self, PartOut};

/// A label as src/label/types.ts toSpec sends it.
fn label(kind: &str, build: &str, point: [f64; 3], normal: [f64; 3]) -> Value {
    json!({
        "kind": kind, "text": "No 7", "entries": [], "font": "bundled", "fontSize": 48,
        "height": 6, "width": 0, "keepAspect": true, "depth": 1, "build": build, "thickness": 2,
        "point": point, "normal": normal, "angle": 0, "tagDirection": null, "threshold": 0.5,
    })
}

/// The request the window sends: meta {labels, job}, the part with its matrix, then pictures.
fn request(
    labels: Value,
    job: u64,
    part: &Mesh,
    matrix: [f64; 16],
    picture: Option<(usize, usize, Vec<f32>, usize)>,
) -> tricky3d_lib::bundle::Bundle {
    let mut meta = Map::new();
    meta.insert("labels".into(), labels);
    meta.insert("job".into(), json!(job));
    let mut info = Map::new();
    info.insert("name".into(), json!("Block"));
    info.insert("matrix".into(), json!(matrix.to_vec()));
    let mut out = vec![OutMesh {
        info,
        positions: &part.positions,
        indices: &part.indices,
        edges: None,
    }];
    let mut extra = vec![Vec::new()];
    let gray;
    if let Some((w, h, g, index)) = picture {
        gray = g;
        let mut info = Map::new();
        info.insert("picture".into(), json!(true));
        info.insert("label".into(), json!(index));
        info.insert("width".into(), json!(w));
        info.insert("height".into(), json!(h));
        out.push(OutMesh {
            info,
            positions: &[],
            indices: &[],
            edges: None,
        });
        extra.push(vec![("gray", BlobRef::F32(&gray))]);
    }
    decode(&encode_with_extra(&meta, &out, &extra)).unwrap()
}

/// Each copy's list entries and mesh, and the note the run sent back.
type Applied = (Vec<(String, Mesh)>, Option<String>);

fn apply(
    labels: Value,
    part: &Mesh,
    matrix: [f64; 16],
    picture: Option<(usize, usize, Vec<f32>, usize)>,
) -> Result<Applied, String> {
    let bytes = jobs::label_apply(
        request(labels, 1, part, matrix, picture),
        &Fonts::default(),
        &RepairJobs::default(),
        &mut |_| {},
    )?;
    let bundle = decode(&bytes).unwrap();
    let note = bundle
        .meta
        .get("note")
        .and_then(Value::as_str)
        .map(str::to_string);
    let copies = bundle
        .meshes
        .into_iter()
        .map(|m| {
            assert!(m.info.get("stats").is_some(), "stats sent back");
            (
                m.info
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                m.mesh,
            )
        })
        .collect();
    Ok((copies, note))
}

fn translation(x: f64, y: f64, z: f64) -> [f64; 16] {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

#[test]
fn a_moved_part_is_labelled_where_it_stands_and_reopens_unchanged() {
    // The block stands at (100, 50, 0) on the platform: the label's point is in world space.
    let block = cube(20.0);
    let moved = translation(100.0, 50.0, 0.0);
    let (copies, note) = apply(
        json!([label("text", "add", [110.0, 60.0, 20.0], [0.0, 0.0, 1.0])]),
        &block,
        moved,
        None,
    )
    .unwrap();
    assert!(note.is_none());
    let (_, labelled) = &copies[0];
    let s = stats(labelled);
    assert_eq!((s.open_edges, s.non_manifold_edges, s.shells), (0, 0, 1));
    assert!(
        s.volume > 8000.0 + 10.0,
        "the letters add volume: {}",
        s.volume
    );
    let lo = labelled
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| p[0] as f64)
        .fold(f64::INFINITY, f64::min);
    assert_close(lo, 100.0, 1e-6, "world space");
    // Saved in a project and opened again: the same mesh.
    let dir = temp_dir("labels-project");
    let file = dir.join("labelled.3mf");
    let part = PartOut {
        name: "Block labelled",
        matrix: IDENTITY,
        color: None,
        visible: true,
        transparent: false,
        locked: false,
        priority: None,
        rotation: None,
        measurements: None,
        analyses: None,
        colors: None,
        source: None,
        mesh: labelled,
    };
    project::save_file(&file, [200.0; 3], &[part]).unwrap();
    let reopened = project::open_file(&file, &mut Progress::silent()).unwrap();
    assert_eq!(reopened.parts.len(), 1);
    assert_same_shape(&reopened.parts[0].mesh, labelled, "reopened");
    let r = stats(&reopened.parts[0].mesh);
    assert_eq!((r.open_edges, r.non_manifold_edges), (0, 0));
    assert_close(r.volume, s.volume, 1e-6, "reopened volume");
}

#[test]
fn image_labels_take_their_picture_and_lists_make_named_copies() {
    let block = cube(30.0);
    let (w, h) = (40, 20);
    let gray: Vec<f32> = (0..w * h)
        .map(|i| if (i % w) < w / 2 { 0.0 } else { 1.0 })
        .collect();
    let (copies, _) = apply(
        json!([label(
            "image",
            "subtract",
            [15.0, 15.0, 30.0],
            [0.0, 0.0, 1.0]
        )]),
        &block,
        IDENTITY,
        Some((w, h, gray, 0)),
    )
    .unwrap();
    assert!(stats(&copies[0].1).volume < 27_000.0);
    // No picture: refused with a sentence.
    let missing = apply(
        json!([label("image", "add", [15.0, 15.0, 30.0], [0.0, 0.0, 1.0])]),
        &block,
        IDENTITY,
        None,
    )
    .unwrap_err();
    assert!(missing.contains("Pick a picture"), "{missing}");
    let mut list = label("list", "subtract", [15.0, 15.0, 30.0], [0.0, 0.0, 1.0]);
    list["entries"] = json!(["A1", "A2", "A3", "A4", "A5"]);
    let (copies, note) = apply(json!([list]), &block, IDENTITY, None).unwrap();
    assert!(note.is_none());
    let names: Vec<&str> = copies.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, ["A1", "A2", "A3", "A4", "A5"]);
    for (name, mesh) in &copies {
        let s = stats(mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0), "{name}");
    }
}

#[test]
fn a_label_that_cannot_go_there_says_why_and_a_run_can_be_cancelled() {
    let block = cube(20.0);
    let past = apply(
        json!([label("text", "add", [19.0, 10.0, 20.0], [0.0, 0.0, 1.0])]),
        &block,
        IDENTITY,
        None,
    )
    .unwrap_err();
    assert!(past.contains("edge"), "{past}");
    let two = json!([
        label("text", "add", [10.0, 10.0, 20.0], [0.0, 0.0, 1.0]),
        label("text", "add", [10.0, 0.0, 10.0], [0.0, 0.0, 1.0])
    ]);
    let second = apply(two, &block, IDENTITY, None).unwrap_err();
    assert!(second.starts_with("Label 2: "), "{second}");
    // Cancelled before it starts: nothing comes back.
    let jobs = RepairJobs::default();
    jobs.cancel(9);
    let cancelled = jobs::label_apply(
        request(
            json!([label("text", "add", [10.0, 10.0, 20.0], [0.0, 0.0, 1.0])]),
            9,
            &block,
            IDENTITY,
            None,
        ),
        &Fonts::default(),
        &jobs,
        &mut |_| {},
    );
    assert_eq!(cancelled.unwrap_err(), CANCELLED);
    // Progress goes to the job's number.
    let mut seen = Vec::new();
    jobs::label_apply(
        request(
            json!([label("text", "add", [10.0, 10.0, 20.0], [0.0, 0.0, 1.0])]),
            10,
            &block,
            IDENTITY,
            None,
        ),
        &Fonts::default(),
        &RepairJobs::default(),
        &mut |p| seen.push(p.job),
    )
    .unwrap();
    assert!(seen.iter().all(|&j| j == 10));
}

#[test]
fn fonts_are_the_bundled_one_and_those_found_and_no_other_file() {
    let fonts = Fonts::default();
    let dir = temp_dir("labels-fonts");
    let listed = fonts.list(|| vec![dir.to_path_buf()]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "bundled");
    assert!(fonts.data("bundled").is_ok());
    // A path the window makes up is not a font it may read.
    let made_up = fonts
        .data(&format!("{}#0", fixture("cube_20mm.stl").display()))
        .unwrap_err();
    assert!(made_up.contains("not available"), "{made_up}");
    // The shape request draws the bundled font.
    let mut meta = Map::new();
    meta.insert("kind".into(), json!("text"));
    meta.insert("text".into(), json!("Hi"));
    let shape = jobs::label_shape(
        tricky3d_lib::bundle::Bundle {
            meta,
            meshes: Vec::new(),
        },
        &fonts,
    )
    .unwrap();
    assert!(shape.aspect > 0.5 && !shape.fill.is_empty() && !shape.outline.is_empty());
}

/// What `label_fit` answers for a label placed at `point` facing `normal`.
fn fit_reply(meta: Value, part: &Mesh) -> tricky3d_lib::label::fit::Fit {
    let mut info = Map::new();
    info.insert("name".into(), json!("Block"));
    info.insert("matrix".into(), json!(IDENTITY.to_vec()));
    let out = vec![OutMesh {
        info,
        positions: &part.positions,
        indices: &part.indices,
        edges: None,
    }];
    let bundle = decode(&encode_with_extra(
        meta.as_object().unwrap(),
        &out,
        &[Vec::new()],
    ))
    .unwrap();
    jobs::label_fit(bundle, &Fonts::default()).unwrap()
}

#[test]
fn a_new_label_is_sized_to_the_face_it_is_placed_on() {
    let block = cube(20.0);
    let at = |kind: &str, extra: Value| {
        let mut meta = json!({"kind": kind, "text": "No 7", "font": "bundled", "fontSize": 48,
            "point": [10.0, 10.0, 20.0], "normal": [0.0, 0.0, 1.0], "angle": 0});
        for (k, v) in extra.as_object().unwrap() {
            meta[k] = v.clone();
        }
        fit_reply(meta, &block)
    };
    let text = at("text", json!({}));
    assert!(
        text.fits && text.height < 10.0 && text.height > 1.0,
        "{text:?}"
    );
    assert_eq!(text.thickness, Some(20.0));
    // An image label without its picture yet is sized as a square: 80 % of 20 mm, capped at 10.
    assert_eq!(at("image", json!({})).height, 10.0);
    // A list is sized by its widest entry.
    let list = at("list", json!({"entries": ["1", "No 7"]}));
    assert_eq!(list.height, text.height);
}
