//! Measure mode through the jobs its commands run: a click as the window sends it (world point, the
//! clicked part's matrix, the owning part's matrix), the mesh kept between clicks, clip planes taken
//! into the part's frame, and the result worked out under the part's matrix.

mod common;

use common::*;
use serde_json::{Map, Value, json};
use tricky3d_lib::bundle::{OutMesh, decode, encode};
use tricky3d_lib::jobs::{self, MESH_NEEDED};
use tricky3d_lib::measure::{Feature, MeasureCache, Op, evaluate};
use tricky3d_lib::mesh::{IDENTITY, Matrix, Mesh};

fn moved(x: f64, y: f64, z: f64) -> Matrix {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

/// The measure_pick request src/measure/ipc.ts sends: `frames` is the clicked part's matrix and the owning part's;
/// `mesh` None sends none (the kept one is used).
fn click(
    key: u64,
    kind: &str,
    triangle: u32,
    hit: [f64; 3],
    (matrix, owner): (Matrix, Matrix),
    mesh: Option<&Mesh>,
    extra: &[(&str, Value)],
) -> Vec<u8> {
    let mut meta = Map::new();
    meta.insert("meshKey".into(), json!(key));
    meta.insert("kind".into(), json!(kind));
    meta.insert("triangle".into(), json!(triangle));
    meta.insert("hit".into(), json!(hit));
    meta.insert("matrix".into(), json!(matrix.to_vec()));
    meta.insert("owner".into(), json!(owner.to_vec()));
    meta.insert("snap".into(), json!("auto"));
    meta.insert("tolerance".into(), json!(0.5));
    meta.insert("clip".into(), json!([]));
    for (k, v) in extra {
        meta.insert((*k).into(), v.clone());
    }
    let empty = Mesh::default();
    let m = mesh.unwrap_or(&empty);
    encode(
        &meta,
        &[OutMesh {
            info: Map::new(),
            positions: &m.positions,
            indices: &m.indices,
            edges: None,
        }],
    )
}

/// The top face's first triangle of `make_box`: triangles 2 and 3 are +Z.
const TOP: u32 = 2;

#[test]
fn a_click_becomes_a_feature_in_the_owners_frame_and_the_mesh_is_kept() {
    let cache = MeasureCache::default();
    let cube = cube(20.0);
    let at = moved(100.0, 0.0, 0.0);
    // The first click sends the mesh; a point near the top corner (100, 0, 20) snaps to it.
    let reply = jobs::measure_pick(
        decode(&click(
            7,
            "point",
            TOP,
            [100.3, 0.2, 20.0],
            (at, at),
            Some(&cube),
            &[],
        ))
        .unwrap(),
        &cache,
    )
    .unwrap();
    assert_eq!(
        reply.feature,
        Feature::Point {
            p: [0.0, 0.0, 20.0]
        }
    );
    assert_eq!(
        reply.world,
        Feature::Point {
            p: [100.0, 0.0, 20.0]
        }
    );

    // Later clicks send no mesh. The owner is another part 50 mm along Y: the feature comes in its frame.
    let owner = moved(0.0, 50.0, 0.0);
    let reply = jobs::measure_pick(
        decode(&click(
            7,
            "edge",
            TOP,
            [110.0, 0.4, 20.0],
            (at, owner),
            None,
            &[],
        ))
        .unwrap(),
        &cache,
    )
    .unwrap();
    assert_eq!(
        reply.world,
        Feature::Edge {
            a: [100.0, 0.0, 20.0],
            b: [120.0, 0.0, 20.0]
        }
    );
    assert_eq!(
        reply.feature,
        Feature::Edge {
            a: [100.0, -50.0, 20.0],
            b: [120.0, -50.0, 20.0]
        }
    );

    // A surface brings its outline in world space.
    let reply = jobs::measure_pick(
        decode(&click(
            7,
            "surface",
            TOP,
            [105.0, 5.0, 20.0],
            (at, at),
            None,
            &[],
        ))
        .unwrap(),
        &cache,
    )
    .unwrap();
    assert_eq!(reply.outline.len(), 4 * 6);
    assert!(
        reply
            .outline
            .chunks(3)
            .all(|p| p[2] == 20.0 && (100.0..=120.0).contains(&p[0]))
    );

    // A mesh never sent: the window is asked for it.
    let missing = jobs::measure_pick(
        decode(&click(
            8,
            "point",
            TOP,
            [0.0; 3],
            (IDENTITY, IDENTITY),
            None,
            &[],
        ))
        .unwrap(),
        &cache,
    );
    assert_eq!(missing.unwrap_err(), MESH_NEEDED);
    cache.clear();
    let gone = jobs::measure_pick(
        decode(&click(
            7,
            "point",
            TOP,
            [0.0; 3],
            (IDENTITY, IDENTITY),
            None,
            &[],
        ))
        .unwrap(),
        &cache,
    );
    assert_eq!(gone.unwrap_err(), MESH_NEEDED);
}

#[test]
fn clip_planes_snap_in_the_parts_frame_and_results_follow_the_matrix() {
    let cache = MeasureCache::default();
    let cube = cube(20.0);
    // Part moved 100 mm along X and scaled 2: the plane x <= 107 world is x <= 3.5 in its frame.
    let mut m = moved(100.0, 0.0, 0.0);
    m[0] = 2.0;
    m[5] = 2.0;
    m[10] = 2.0;
    let plane = json!([{ "normal": [-1.0, 0.0, 0.0], "constant": 107.0 }]);
    let reply = jobs::measure_pick(
        decode(&click(
            1,
            "point",
            TOP,
            [106.6, 10.0, 40.0],
            (m, m),
            Some(&cube),
            &[("clip", plane), ("snap", json!("clip-line"))],
        ))
        .unwrap(),
        &cache,
    )
    .unwrap();
    let Feature::Point { p } = reply.world else {
        panic!()
    };
    assert!(
        (p[0] - 107.0).abs() < 1e-9 && (p[1] - 10.0).abs() < 1e-9 && (p[2] - 40.0).abs() < 1e-9,
        "{p:?}"
    );

    // The plate is 3 mm thick in its frame and 6 mm under the matrix.
    let plate = tricky3d_lib::mesh::make_box([0.0; 3], [30.0, 30.0, 3.0]);
    let wall = jobs::measure_pick(
        decode(&click(
            2,
            "wall",
            TOP,
            [110.0, 10.0, 6.0],
            (m, m),
            Some(&plate),
            &[],
        ))
        .unwrap(),
        &cache,
    )
    .unwrap();
    assert_eq!(
        evaluate(Op::Thickness, std::slice::from_ref(&wall.feature), &m)
            .unwrap()
            .value,
        6.0
    );
    assert_eq!(
        evaluate(Op::Thickness, &[wall.feature], &IDENTITY)
            .unwrap()
            .value,
        3.0
    );

    // A 3-point circle from three point features.
    let points =
        [[10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [-10.0, 0.0, 0.0]].map(|p| Feature::Point { p });
    let Feature::Circle { radius, .. } = jobs::measure_fit("circle3", &points).unwrap() else {
        panic!()
    };
    assert!((radius - 10.0).abs() < 1e-12);
    assert!(jobs::measure_fit("sphere4", &points).is_err());
}

#[test]
fn measurements_travel_with_their_part_through_save_and_open() {
    let dir = temp_dir("measure-project");
    let file = dir.join("measured.3mf");
    let cube = cube(20.0);
    let json = r#"[{"id":"m1","tool":"point-point","features":[{"kind":"point","p":[0,0,20]},{"kind":"point","p":[20,0,20]}],"axes":[true,false,false],"diameter":false}]"#;
    let mut meta = Map::new();
    meta.insert("path".into(), json!(file.to_string_lossy()));
    meta.insert("platform".into(), json!([200.0, 200.0, 200.0]));
    let parts = [("Measured", Some(json)), ("Plain", None)].map(|(name, measurements)| {
        let mut info = Map::new();
        info.insert("name".into(), json!(name));
        if let Some(m) = measurements {
            info.insert("measurements".into(), json!(m));
        }
        info
    });
    let out: Vec<OutMesh> = parts
        .iter()
        .map(|info| OutMesh {
            info: info.clone(),
            positions: &cube.positions,
            indices: &cube.indices,
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
            .get("measurements")
            .and_then(Value::as_str),
        Some(json)
    );
    assert_eq!(opened.meshes[1].info.get("measurements"), None);
}
