//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use tricky3d_lib::mesh::{Matrix, Mesh, bounds, make_box, multiply, stats};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// A fresh, empty directory for one test, removed again when the test is done with it, also when
/// it fails.
pub fn temp_dir(name: &str) -> tricky3d_lib::test_dir::TempDir {
    tricky3d_lib::test_dir::temp_dir(name)
}

pub fn cube(size: f32) -> Mesh {
    make_box([0.0; 3], [size; 3])
}

/// A closed torus around the z axis resting on z = 0: `n` × `m` quads, 2nm triangles, facing out.
pub fn torus(n: usize, m: usize, major: f64, minor: f64) -> Mesh {
    let mut positions = Vec::with_capacity(n * m * 3);
    for i in 0..n {
        let u = std::f64::consts::TAU * i as f64 / n as f64;
        for j in 0..m {
            let v = std::f64::consts::TAU * j as f64 / m as f64;
            let r = major + minor * v.cos();
            positions.extend_from_slice(&[
                (r * u.cos()) as f32,
                (r * u.sin()) as f32,
                (minor + minor * v.sin()) as f32,
            ]);
        }
    }
    let id = |i: usize, j: usize| ((i % n) * m + (j % m)) as u32;
    let mut indices = Vec::with_capacity(n * m * 6);
    for i in 0..n {
        for j in 0..m {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    Mesh { positions, indices }
}

/// Column-major translation × rotation (about z, then x, in degrees) × scale.
pub fn trs(t: [f64; 3], rz_deg: f64, rx_deg: f64, s: [f64; 3]) -> Matrix {
    let (sz, cz) = rz_deg.to_radians().sin_cos();
    let (sx, cx) = rx_deg.to_radians().sin_cos();
    let rz = [
        cz, sz, 0.0, 0.0, -sz, cz, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let rx = [
        1.0, 0.0, 0.0, 0.0, 0.0, cx, sx, 0.0, 0.0, -sx, cx, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let scale = [
        s[0], 0.0, 0.0, 0.0, 0.0, s[1], 0.0, 0.0, 0.0, 0.0, s[2], 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let mut m = multiply(&rx, &multiply(&rz, &scale));
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

/// Triangle count, volume and bounds of a mesh, for comparisons.
pub fn summary(mesh: &Mesh) -> (u64, f64, [[f32; 3]; 2]) {
    let s = stats(mesh);
    (s.triangles, s.volume, bounds(mesh).unwrap())
}

pub fn assert_same_shape(got: &Mesh, want: &Mesh, what: &str) {
    let (gt, gv, gb) = summary(got);
    let (wt, wv, wb) = summary(want);
    assert_eq!(gt, wt, "{what}: triangle count");
    assert!((gv - wv).abs() <= 1e-5, "{what}: volume {gv} vs {wv}");
    for k in 0..3 {
        for side in 0..2 {
            assert!(
                (gb[side][k] - wb[side][k]).abs() <= 1e-5,
                "{what}: bounds {gb:?} vs {wb:?}"
            );
        }
    }
}

pub fn assert_close(got: f64, want: f64, tolerance: f64, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: got {got}, want {want}"
    );
}

/// A zip with the given entries, deflated.
pub fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

pub const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#;

pub fn rels(target: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="{target}" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#
    )
}

/// A 3MF package around one model XML at 3D/3dmodel.model.
pub fn package_3mf(model: &str) -> Vec<u8> {
    let rels = rels("/3D/3dmodel.model");
    zip_bytes(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("3D/3dmodel.model", model.as_bytes()),
    ])
}

/// Binary big-endian PLY of a mesh (float x y z; list uchar uint vertex_indices).
pub fn ply_big_endian(mesh: &Mesh) -> Vec<u8> {
    let mut out = format!(
        "ply\nformat binary_big_endian 1.0\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nelement face {}\nproperty list uchar uint vertex_indices\nend_header\n",
        mesh.vertex_count(),
        mesh.triangle_count()
    )
    .into_bytes();
    for v in &mesh.positions {
        out.extend_from_slice(&v.to_be_bytes());
    }
    for t in mesh.indices.as_chunks::<3>().0 {
        out.push(3);
        for i in t {
            out.extend_from_slice(&i.to_be_bytes());
        }
    }
    out
}
