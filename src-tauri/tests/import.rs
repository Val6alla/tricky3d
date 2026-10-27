//! Every import format, from hand-written text fixtures and binary files generated here.

mod common;

use common::*;
use tricky3d_lib::formats::{ExportFormat, ImportFormat, NamedMesh, import_file, write_export};
use tricky3d_lib::mesh::{IDENTITY, bounds, stats};
use tricky3d_lib::progress::Progress;

const INCH: f32 = 25.4;

fn import(path: &std::path::Path, merge: bool) -> tricky3d_lib::formats::Imported {
    import_file(path, merge, &mut Progress::silent()).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn ascii_stl_with_two_solids_is_one_part() {
    let got = import(&fixture("two_solids.stl"), false);
    assert_eq!(got.format, ImportFormat::Stl);
    assert_eq!(got.parts.len(), 1);
    let part = &got.parts[0];
    assert_eq!(part.name, "two_solids");
    assert_eq!(part.matrix, None);
    let s = stats(&part.mesh);
    assert_eq!(
        (s.triangles, s.vertices, s.shells, s.open_edges),
        (8, 8, 2, 0)
    );
    assert_close(s.volume, 2.0 * 1000.0 / 6.0, 1e-9, "volume");
    assert_eq!(
        bounds(&part.mesh),
        Some([[0.0, 0.0, 0.0], [30.0, 10.0, 10.0]])
    );
}

#[test]
fn binary_stl_whose_header_starts_with_solid() {
    let dir = temp_dir("binary-solid");
    let mesh = cube(20.0);
    let mut bytes = write_export(
        std::io::Cursor::new(Vec::new()),
        ExportFormat::StlBinary,
        "x",
        &[NamedMesh {
            name: "c",
            mesh: &mesh,
        }],
    )
    .unwrap()
    .into_inner();
    bytes[..22].copy_from_slice(b"solid binary not text ");
    let path = dir.join("tricky.stl");
    std::fs::write(&path, &bytes).unwrap();
    let got = import(&path, false);
    let s = stats(&got.parts[0].mesh);
    assert_eq!((s.triangles, s.vertices, s.open_edges), (12, 8, 0));
    assert_close(s.volume, 8000.0, 1e-9, "volume");

    // Cut short: the message says how much is there.
    std::fs::write(&path, &bytes[..84 + 50 * 7 + 20]).unwrap();
    let err = import_file(&path, false, &mut Progress::silent()).unwrap_err();
    assert_eq!(
        err,
        "Could not read \"tricky.stl\": the file ends early (expected 12 triangles, found 7)."
    );

    // One whole triangle short of what the header declares (a writer's off-by-one): read what
    // is there and say so.
    std::fs::write(&path, &bytes[..84 + 50 * 11]).unwrap();
    let got = import(&path, false);
    assert_eq!(got.parts[0].mesh.triangle_count(), 11);
    assert_eq!(
        got.warnings,
        ["The file declares 12 triangles but holds 11; those were read."]
    );
}

#[test]
fn obj_with_two_objects_quads_and_negative_indices() {
    let got = import(&fixture("two_objects.obj"), false);
    assert_eq!(got.format, ImportFormat::Obj);
    assert_eq!((got.object_count, got.splittable), (2, true));
    let names: Vec<&str> = got.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["two_objects - Box", "two_objects - Tri"]);
    let s = stats(&got.parts[0].mesh);
    assert_eq!(
        (s.triangles, s.vertices, s.open_edges, s.shells),
        (12, 8, 0, 1)
    );
    assert_close(s.volume, 1000.0, 1e-9, "box volume");
    assert_eq!(
        bounds(&got.parts[1].mesh),
        Some([[20.0, 0.0, 0.0], [30.0, 10.0, 0.0]])
    );

    let merged = import(&fixture("two_objects.obj"), true);
    assert_eq!(
        (merged.object_count, merged.splittable, merged.parts.len()),
        (2, false, 1)
    );
    assert_eq!(merged.parts[0].name, "two_objects");
    assert_eq!(merged.parts[0].mesh.triangle_count(), 13);
}

#[test]
fn ascii_ply_with_quads() {
    let got = import(&fixture("quad_cube.ply"), false);
    assert_eq!(got.format, ImportFormat::Ply);
    let s = stats(&got.parts[0].mesh);
    assert_eq!(
        (s.triangles, s.vertices, s.open_edges, s.shells),
        (12, 8, 0, 1)
    );
    assert_close(s.volume, 8000.0, 1e-9, "volume");
    assert_close(s.area, 2400.0, 1e-9, "area");
}

#[test]
fn binary_ply_both_byte_orders() {
    let dir = temp_dir("ply");
    let mesh = torus(24, 12, 30.0, 8.0);
    let want = stats(&mesh);
    let be = dir.join("torus-be.ply");
    std::fs::write(&be, ply_big_endian(&mesh)).unwrap();
    let le = dir.join("torus-le.ply");
    let bytes = write_export(
        std::io::Cursor::new(Vec::new()),
        ExportFormat::Ply,
        "x",
        &[NamedMesh {
            name: "t",
            mesh: &mesh,
        }],
    )
    .unwrap();
    std::fs::write(&le, bytes.into_inner()).unwrap();
    for path in [be, le] {
        let got = import(&path, false);
        assert_eq!(stats(&got.parts[0].mesh), want, "{}", path.display());
        assert_same_shape(&got.parts[0].mesh, &mesh, "binary PLY");
    }
}

#[test]
fn three_mf_in_inches_with_components_and_two_items() {
    let dir = temp_dir("3mf-inch");
    let path = dir.join("inches.3mf");
    std::fs::write(
        &path,
        package_3mf(&std::fs::read_to_string(fixture("inch_components.model")).unwrap()),
    )
    .unwrap();
    let got = import(&path, false);
    assert_eq!(got.format, ImportFormat::ThreeMf);
    assert_eq!(got.object_count, 2);
    assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    let names: Vec<&str> = got.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["inches - Unit cube", "inches - Two cubes"]);

    // Item 1: a one-inch cube lifted one inch.
    let cube = &got.parts[0];
    assert_eq!(bounds(&cube.mesh), Some([[0.0; 3], [INCH; 3]]));
    let mut lifted = IDENTITY;
    lifted[14] = 25.4;
    assert_eq!(cube.matrix, Some(lifted));
    assert_close(
        stats(&cube.mesh).volume,
        (INCH as f64).powi(3),
        1e-6,
        "cube volume (f32 vertices)",
    );

    // Item 2: the component copy two inches along x is baked in; the item turns 90° about z and
    // moves (5, 5, 0) inches, which stays in the part matrix (translation in mm).
    let pair = &got.parts[1];
    let s = stats(&pair.mesh);
    assert_eq!((s.triangles, s.shells, s.open_edges), (24, 2, 0));
    assert_eq!(
        bounds(&pair.mesh),
        Some([[0.0; 3], [3.0 * INCH, INCH, INCH]])
    );
    let m = pair.matrix.unwrap();
    assert_eq!(
        &m[..12],
        &[0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]
    );
    assert_close(m[12], 127.0, 1e-9, "x");
    assert_close(m[13], 127.0, 1e-9, "y");
    assert_eq!((m[14], m[15]), (0.0, 1.0));
}

#[test]
fn three_mf_with_prefixed_core_namespace_extensions_and_a_production_path() {
    let dir = temp_dir("3mf-prod");
    // The root model uses a "c:" prefix for core elements and points at a second model file
    // through p:path, as Bambu Studio writes. It also requires the slice extension.
    let root = r#"<?xml version="1.0" encoding="UTF-8"?>
<c:model unit="millimeter" xmlns:c="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" xmlns:s="http://schemas.microsoft.com/3dmanufacturing/slice/2015/07" requiredextensions="p s">
 <c:resources>
  <c:object id="5" name="Holder">
   <c:components>
    <c:component p:path="/3D/Objects/object_1.model" objectid="1" transform="2 0 0 0 1 0 0 0 1 0 0 0"/>
   </c:components>
  </c:object>
 </c:resources>
 <c:build><c:item objectid="5" transform="1 0 0 0 1 0 0 0 1 10 0 0"/></c:build>
</c:model>"#;
    let sub = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model"><mesh><vertices>{}</vertices><triangles>{}</triangles></mesh></object></resources><build/></model>"#,
        cube(10.0)
            .positions
            .chunks(3)
            .map(|p| format!(r#"<vertex x="{}" y="{}" z="{}"/>"#, p[0], p[1], p[2]))
            .collect::<String>(),
        cube(10.0)
            .indices
            .chunks(3)
            .map(|t| format!(r#"<triangle v1="{}" v2="{}" v3="{}"/>"#, t[0], t[1], t[2]))
            .collect::<String>()
    );
    let rels = rels("/3D/3dmodel.model");
    let bytes = zip_bytes(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("3D/3dmodel.model", root.as_bytes()),
        ("3D/Objects/object_1.model", sub.as_bytes()),
    ]);
    let path = dir.join("bambu-like.3mf");
    std::fs::write(&path, bytes).unwrap();
    let got = import(&path, false);
    assert_eq!(got.parts.len(), 1);
    assert_eq!(got.parts[0].name, "bambu-like");
    assert_eq!(
        bounds(&got.parts[0].mesh),
        Some([[0.0; 3], [20.0, 10.0, 10.0]])
    );
    assert_eq!(got.parts[0].matrix.unwrap()[12], 10.0);
    assert_eq!(got.warnings.len(), 1, "{:?}", got.warnings);
    assert!(got.warnings[0].contains("slice"));
}

#[test]
fn three_mf_component_loops_are_rejected() {
    let dir = temp_dir("3mf-loop");
    let model = r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>
<object id="1"><components><component objectid="2"/></components></object>
<object id="2"><components><component objectid="1"/></components></object>
</resources><build><item objectid="1"/></build></model>"#;
    let path = dir.join("loop.3mf");
    std::fs::write(&path, package_3mf(model)).unwrap();
    let err = import_file(&path, false, &mut Progress::silent()).unwrap_err();
    assert_eq!(
        err,
        "Could not read \"loop.3mf\": its components refer to each other in a loop."
    );
}

#[test]
fn three_mf_components_of_missing_objects_are_rejected() {
    let dir = temp_dir("3mf-missing");
    let model = r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>
<object id="1"><components><component objectid="7"/></components></object>
</resources><build><item objectid="1"/></build></model>"#;
    let path = dir.join("missing.3mf");
    std::fs::write(&path, package_3mf(model)).unwrap();
    let err = import_file(&path, false, &mut Progress::silent()).unwrap_err();
    assert_eq!(
        err,
        "Could not read \"missing.3mf\": a component refers to object 7, which does not exist."
    );
}

#[test]
fn three_mf_nesting_counts_a_shared_object_at_its_deepest_use() {
    // Object 1 is a triangle and objects 2 to `top` each use the one below. The build places
    // object 100, which uses object 10 and then `top`, whose chain reaches object 10 again deeper.
    let model = |top: u32| {
        let mut model = String::from(
            r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#,
        );
        for id in 2..=top {
            model += &format!(
                r#"<object id="{id}"><components><component objectid="{}"/></components></object>"#,
                id - 1
            );
        }
        model += &format!(
            r#"<object id="100"><components><component objectid="10"/><component objectid="{top}"/></components></object></resources><build><item objectid="100"/></build></model>"#
        );
        package_3mf(&model)
    };
    let dir = temp_dir("3mf-deep");
    let path = dir.join("deep.3mf");
    // Through object 32, the triangle sits 32 levels below the build item.
    std::fs::write(&path, model(32)).unwrap();
    assert_eq!(import(&path, false).parts[0].mesh.triangle_count(), 2);
    std::fs::write(&path, model(33)).unwrap();
    let err = import_file(&path, false, &mut Progress::silent()).unwrap_err();
    assert_eq!(
        err,
        "Could not read \"deep.3mf\": its components are nested more than 32 levels deep."
    );
}

/// A 3MF whose build item expands to fan_out^levels copies of one small mesh: object 1 is the
/// mesh, and every object above it uses the one below `fan_out` times.
fn component_tree_3mf(vertices: usize, triangles: usize, fan_out: usize, levels: u32) -> Vec<u8> {
    let mut model = String::from(
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><mesh><vertices>"#,
    );
    for i in 0..vertices {
        model += &format!(r#"<vertex x="{i}" y="{}" z="0"/>"#, i % 2);
    }
    model += "</vertices><triangles>";
    for _ in 0..triangles {
        model += r#"<triangle v1="0" v2="1" v3="2"/>"#;
    }
    model += "</triangles></mesh></object>";
    for id in 2..=levels + 1 {
        model += &format!(r#"<object id="{id}"><components>"#);
        for _ in 0..fan_out {
            model += &format!(r#"<component objectid="{}"/>"#, id - 1);
        }
        model += "</components></object>";
    }
    model += &format!(
        r#"</resources><build><item objectid="{}"/></build></model>"#,
        levels + 1
    );
    package_3mf(&model)
}

#[test]
fn three_mf_components_that_multiply_are_rejected_before_memory_runs_out() {
    let dir = temp_dir("3mf-bomb");
    let path = dir.join("bomb.3mf");
    let cases = [
        // A ~1 KB file whose 31 doubling levels would be 2,147,483,648 triangles.
        (
            component_tree_3mf(3, 1, 2, 21),
            "it repeats its components too many times",
        ),
        (
            component_tree_3mf(3, 1, 2, 31),
            "it repeats its components too many times",
        ),
        // 10^30 copies, more than a u64 can count.
        (
            component_tree_3mf(3, 1, 10, 30),
            "it repeats its components too many times",
        ),
        (
            component_tree_3mf(3, 1, 1, 40),
            "its components are nested more than 32 levels deep",
        ),
        (
            component_tree_3mf(3, 10_000, 10, 4),
            "it would have more than 50,000,000 triangles, which is more than tricky3D can handle",
        ),
        (
            component_tree_3mf(20_000, 1, 10, 4),
            "it would have more than 100,000,000 vertices, which is more than tricky3D can handle",
        ),
    ];
    for (bytes, reason) in cases {
        std::fs::write(&path, bytes).unwrap();
        let started = std::time::Instant::now();
        let err = match import_file(&path, false, &mut Progress::silent()) {
            Ok(got) => panic!(
                "loaded {} triangles instead of failing with: {reason}",
                got.parts[0].mesh.triangle_count()
            ),
            Err(err) => err,
        };
        assert_eq!(err, format!("Could not read \"bomb.3mf\": {reason}."));
        assert!(started.elapsed().as_secs() < 2, "{reason}");
    }

    // Repeating components within the limits still works: 2^12 copies of one triangle.
    std::fs::write(&path, component_tree_3mf(3, 1, 2, 12)).unwrap();
    let got = import(&path, false);
    assert_eq!(got.parts[0].mesh.triangle_count(), 4096);
}

/// A 3MF whose model repeats one vertex line `copies` times: tens of megabytes of model text in a
/// few hundred kilobytes, like a zip bomb.
fn repeated_vertices_3mf(copies: usize) -> Vec<u8> {
    let vertex = r#"<vertex x="0" y="0" z="0"/>"#;
    let model = format!(
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><mesh><vertices><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/>{}</vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#,
        vertex.repeat(copies)
    );
    package_3mf(&model)
}

#[test]
fn three_mf_that_packs_very_tightly_still_loads() {
    // The unpack ratio limit only starts above 512 MB (see xml::unpack_limit); a small file that
    // packs far tighter than any mesh, such as a project of many copies of one part, still loads.
    let dir = temp_dir("3mf-tight");
    let path = dir.join("tight.3mf");
    std::fs::write(&path, repeated_vertices_3mf(400_000)).unwrap();
    assert_eq!(import(&path, false).parts[0].mesh.vertex_count(), 3);
}

#[test]
fn amf_in_inches_plain_and_zipped() {
    let dir = temp_dir("amf");
    let text = std::fs::read(fixture("two_objects_inch.amf")).unwrap();
    let zipped = dir.join("zipped.amf");
    std::fs::write(&zipped, zip_bytes(&[("model.amf", &text)])).unwrap();
    for path in [fixture("two_objects_inch.amf"), zipped] {
        let got = import(&path, false);
        assert_eq!(got.format, ImportFormat::Amf);
        assert_eq!(got.object_count, 2);
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let names: Vec<String> = got.parts.iter().map(|p| p.name.clone()).collect();
        assert_eq!(
            names,
            [format!("{stem} - Wedge"), format!("{stem} - Second & last")]
        );
        assert_eq!(bounds(&got.parts[0].mesh), Some([[0.0; 3], [INCH; 3]]));
        assert_eq!(
            bounds(&got.parts[1].mesh),
            Some([[2.0 * INCH, 0.0, 0.0], [3.0 * INCH, INCH, INCH]])
        );
        let s = stats(&got.parts[1].mesh);
        assert_eq!(
            (s.triangles, s.open_edges),
            (4, 0),
            "both volumes are in one mesh"
        );
        // A right-angled tetrahedron: a sixth of its box, measured on the f32 vertices.
        let [lo, hi] = bounds(&got.parts[1].mesh).unwrap();
        let box_volume: f64 = (0..3).map(|k| hi[k] as f64 - lo[k] as f64).product();
        assert_close(s.volume, box_volume / 6.0, 1e-9, "volume");
        assert_eq!(
            got.warnings.len(),
            1,
            "constellation warning: {:?}",
            got.warnings
        );
    }
}

#[test]
fn errors_are_sentences_that_name_the_file() {
    let dir = temp_dir("errors");
    let cases: [(&str, &[u8], &str); 6] = [
        (
            "empty.stl",
            b"",
            "Could not read \"empty.stl\": the file is empty.",
        ),
        (
            "junk.stl",
            b"hello there",
            "Could not read \"junk.stl\": it is not an STL file.",
        ),
        (
            "junk.3mf",
            b"not a zip",
            "Could not read \"junk.3mf\": it is not a valid zip package",
        ),
        (
            "junk.ply",
            b"ply\nformat ascii 1.0\n",
            "Could not read \"junk.ply\": its PLY header is incomplete.",
        ),
        (
            "broken.amf",
            b"<amf><object id=\"0\"><mesh>",
            "Could not read \"broken.amf\": ",
        ),
        (
            "lines.obj",
            b"v 0 0 0\nv 1 0 0\nl 1 2\n",
            "Could not read \"lines.obj\": it has no triangles.",
        ),
    ];
    for (name, bytes, want) in cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let err = import_file(&path, false, &mut Progress::silent()).unwrap_err();
        assert!(err.starts_with(want), "{name}: {err}");
        assert!(err.ends_with('.'), "{name}: {err}");
    }
    let missing = import_file(&dir.join("gone.obj"), false, &mut Progress::silent()).unwrap_err();
    assert_eq!(
        missing,
        "Could not read \"gone.obj\": the file does not exist any more."
    );
}

#[test]
fn off_cube_with_quads() {
    let dir = temp_dir("off");
    let path = dir.join("cube.OFF");
    let text = "OFF\n8 6 12\n0 0 0\n10 0 0\n10 10 0\n0 10 0\n0 0 10\n10 0 10\n10 10 10\n0 10 10\n\
                4 0 3 2 1\n4 4 5 6 7\n4 0 1 5 4\n4 1 2 6 5\n4 2 3 7 6\n4 3 0 4 7\n";
    std::fs::write(&path, text).unwrap();
    let got = import(&path, false);
    assert_eq!(got.format, ImportFormat::Off);
    let s = stats(&got.parts[0].mesh);
    assert_eq!((s.triangles, s.vertices, s.open_edges), (12, 8, 0));
    assert_close(s.volume, 1000.0, 1e-9, "volume");
}
