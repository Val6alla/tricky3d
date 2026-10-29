//! Every library item: closed, oriented, free of crossings and of degenerate triangles, the right
//! way out, and where a value is known exactly, that value.

use serde_json::{Map, Value, json};

use super::build::Builder;
use super::*;
use crate::mesh::{bake, bounds, make_box, stats};
use crate::repair::{silent, status, tritri};

/// Asserts what Repair's status tab must show for a made part: closed, oriented, no degenerate
/// triangles, outward (positive volume), and no two triangles crossing.
pub(crate) fn assert_clean(mesh: &Mesh, what: &str) {
    let s = silent(|c| status::analyze(mesh, c)).unwrap().status;
    assert!(s.closed && s.oriented, "{what}: {s:?}");
    assert_eq!(s.degenerate_triangles, 0, "{what}: degenerate triangles");
    assert!(s.volume > 0.0, "{what}: faces inward");
    let crossing = silent(|c| tritri::intersecting_pairs(mesh, c)).unwrap();
    assert!(
        crossing.is_empty(),
        "{what}: {} crossing pairs",
        crossing.len()
    );
}

fn params(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn made(id: &str, v: Value) -> Mesh {
    generate(id, &params(v)).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn with_parts(id: &str, v: Value, parts: Vec<Mesh>) -> Made {
    let sources = Sources {
        parts,
        picture: None,
    };
    silent(|c| make(id, &params(v), &sources, false, c)).unwrap_or_else(|e| panic!("{id}: {e:?}"))
}

fn size_of(mesh: &Mesh) -> [f64; 3] {
    let b = bounds(mesh).unwrap();
    [0, 1, 2].map(|k| (b[1][k] - b[0][k]) as f64)
}

fn close(what: &str, got: f64, want: f64, tolerance: f64) {
    let error = (got - want).abs() / want.abs().max(1e-12);
    assert!(
        error <= tolerance,
        "{what}: {got} vs {want}, off by {error:e}, allowed {tolerance:e}"
    );
}

/// Values that keep the field and Wrap-based items quick in a debug build; everything else at its
/// defaults.
fn quick(id: &str) -> Value {
    match id {
        "klein" => json!({"accuracy": 1.0}),
        "gyroid" => json!({"accuracy": 0.6, "length": 20, "width": 20, "height": 20}),
        "fractal" => json!({"resolution": 48}),
        "menger" => json!({"level": 2}),
        "sierpinski" => json!({"level": 3}),
        "bitmap" => json!({"detail": 80}),
        "heightmap" => json!({"detail": 60}),
        _ => json!({}),
    }
}

#[test]
fn every_item_at_its_defaults_is_clean_and_stands_on_the_platform() {
    let sample_part = bake(
        &make_box([0.0; 3], [20.0, 10.0, 8.0]),
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 50.0, 60.0, 5.0, 1.0,
        ],
    );
    let mut seen = std::collections::HashSet::new();
    for item in catalog() {
        assert!(seen.insert(item.id), "{} twice", item.id);
        for p in &item.params {
            if let Kind::Number {
                default, min, max, ..
            } = p.kind
            {
                assert!(min <= default && default <= max, "{} {}", item.id, p.key);
            }
        }
        if item.soon.is_some() {
            let err = generate(item.id, &Map::new()).unwrap_err();
            assert!(err.contains("not available yet"), "{}: {err}", item.id);
            continue;
        }
        let mesh = if item.input == Input::Parts {
            let m = with_parts(item.id, quick(item.id), vec![sample_part.clone()]);
            assert!(m.world, "{} is made where the part is", item.id);
            m.mesh
        } else {
            made(item.id, quick(item.id))
        };
        assert_clean(&mesh, item.id);
        if item.input == Input::None {
            let b = bounds(&mesh).unwrap();
            assert!(
                b[0][2].abs() < 1e-3,
                "{} does not stand on z = 0: {b:?}",
                item.id
            );
            assert!(
                (b[0][0] + b[1][0]).abs() < 1e-3 && (b[0][1] + b[1][1]).abs() < 1e-3,
                "{} is not centred: {b:?}",
                item.id
            );
        }
    }
}

#[test]
fn the_categories_hold_what_the_brief_lists() {
    let ids = |c: Category| -> Vec<&'static str> {
        catalog()
            .into_iter()
            .filter(|i| i.category == c)
            .map(|i| i.id)
            .collect()
    };
    assert_eq!(ids(Category::Primitive).len(), 13);
    for id in [
        "rect_grid",
        "hex_grid",
        "hex_cylinder",
        "square_grid",
        "bitmap",
        "heightmap",
        "qr_code",
        "screw",
        "nut",
        "gear",
        "light_gear",
        "vase",
        "picture_sweep",
        "bracelet",
        "brick",
        "base_plate",
        "text",
        "mold",
        "whistle",
        "mesh_on_spline",
    ] {
        assert!(ids(Category::Design).contains(&id), "{id}");
    }
    assert_eq!(
        ids(Category::Mathematical),
        [
            "mobius",
            "klein",
            "menger",
            "sierpinski",
            "convex_hull",
            "fractal",
            "gyroid",
            "function_surface",
            "mesh_pipe",
            "superellipsoid"
        ]
    );
    assert_eq!(
        ids(Category::Quality),
        [
            "tensile",
            "flexure",
            "compression",
            "nist",
            "microscale",
            "multi_feature",
            "test_rod"
        ]
    );
    let soon: Vec<&str> = catalog()
        .into_iter()
        .filter(|i| i.soon.is_some())
        .map(|i| i.id)
        .collect();
    assert_eq!(soon, ["mesh_on_spline", "nist"]);
}

#[test]
fn a_menger_sponge_keeps_twenty_of_every_twenty_seven_cubes() {
    for level in 0..=3 {
        let mesh = made("menger", json!({"size": 27, "level": level}));
        assert_clean(&mesh, "menger");
        let want = 27f64.powi(3) * (20.0f64 / 27.0).powi(level);
        close("menger volume", stats(&mesh).volume, want, 1e-9);
        assert_eq!(size_of(&mesh), [27.0; 3]);
    }
}

#[test]
fn a_sierpinski_pyramid_keeps_five_eighths_per_level() {
    let (base, height) = (40.0, 28.0);
    let whole = base * base * height / 3.0;
    let shrink = (1.0 - math::SIERPINSKI_SHRINK).powi(3);
    for level in 0..=4 {
        let mesh = made(
            "sierpinski",
            json!({"base": base, "height": height, "level": level}),
        );
        assert_clean(&mesh, "sierpinski");
        let want = whole * (5.0f64 / 8.0).powi(level) * shrink;
        close("sierpinski volume", stats(&mesh).volume, want, 1e-6);
        // And within the shrink of the ideal (5/8)^n.
        close(
            "sierpinski ideal",
            stats(&mesh).volume,
            whole * (5.0f64 / 8.0).powi(level),
            4e-4,
        );
        let s = size_of(&mesh);
        close("sierpinski base", s[0], base, 1e-3);
        close("sierpinski height", s[2], height, 1e-3);
    }
}

#[test]
fn tensile_bars_follow_d638_and_the_flexure_bar_d790() {
    for (kind, lo, wo) in [
        ("I", 165.0, 19.0),
        ("II", 183.0, 19.0),
        ("III", 246.0, 29.0),
        ("IV", 115.0, 19.0),
        ("V", 63.5, 9.53),
    ] {
        let t = 3.2;
        let mesh = made("tensile", json!({"spec": kind, "thickness": t}));
        assert_clean(&mesh, kind);
        let s = size_of(&mesh);
        close("overall length", s[0], lo, 1e-6);
        close("overall width", s[1], wo, 1e-6);
        close("thickness", s[2], t, 1e-6);
        let bone = quality::d638(kind);
        // The fillets are 48-sided arcs: their chords cut a sliver off each.
        close("tensile volume", stats(&mesh).volume, bone.area() * t, 2e-4);
        // The narrow part is W wide all along its length L.
        let narrowest = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|p| (p[0].abs() as f64) <= bone.l / 2.0 + 1e-4)
            .map(|p| p[1].abs() as f64)
            .fold(0.0, f64::max);
        close("narrow width", 2.0 * narrowest, bone.w, 1e-6);
    }
    let bar = made("flexure", json!({}));
    assert_eq!(size_of(&bar), [127.0, 12.7f32 as f64, 3.2f32 as f64]);
    close(
        "flexure volume",
        stats(&bar).volume,
        127.0 * 12.7 * 3.2,
        1e-6,
    );
    let prism = made("compression", json!({"spec": "prism"}));
    close(
        "compression prism",
        stats(&prism).volume,
        12.7 * 12.7 * 25.4,
        1e-6,
    );
    let cylinder = made("compression", json!({"spec": "cylinder", "segments": 512}));
    close(
        "compression cylinder",
        stats(&cylinder).volume,
        std::f64::consts::PI * 6.35 * 6.35 * 25.4,
        1e-4,
    );
    let rod = made("test_rod", json!({"diameter": 5, "length": 50}));
    close("rod length", size_of(&rod)[2], 50.0, 1e-6);
}

#[test]
fn a_gears_outside_diameter_is_module_times_teeth_plus_two() {
    for (teeth, module) in [(20, 2.0), (31, 1.5), (8, 3.0), (120, 0.5)] {
        let mesh = made("gear", json!({"teeth": teeth, "module": module, "bore": 0}));
        assert_clean(&mesh, "gear");
        let reach = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| (p[0] as f64).hypot(p[1] as f64))
            .fold(0.0, f64::max);
        // Centred on its box; for an odd count the box's middle is off the axis by a little.
        let centre_error = if teeth % 2 == 1 { module * 0.6 } else { 1e-4 };
        assert!(
            (2.0 * reach - module * (teeth as f64 + 2.0)).abs() <= 2.0 * centre_error,
            "{teeth} teeth: {}",
            2.0 * reach
        );
        if teeth % 2 == 0 {
            close(
                "outside diameter",
                size_of(&mesh)[0],
                module * (teeth as f64 + 2.0),
                1e-6,
            );
        }
    }
    let light = made("light_gear", json!({}));
    let solid = made(
        "gear",
        json!({"teeth": 40, "module": 2, "thickness": 6, "bore": 8}),
    );
    assert!(stats(&light).volume < 0.9 * stats(&solid).volume);
}

/// Winding number of the closed mesh round `p` (1 inside, 0 outside), by solid angles.
fn winding(mesh: &Mesh, p: [f64; 3]) -> f64 {
    let mut total = 0.0;
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|v| {
            let q = mesh.point(v);
            [q[0] - p[0], q[1] - p[1], q[2] - p[2]]
        });
        let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
        let lc = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let dot = |u: [f64; 3], w: [f64; 3]| u[0] * w[0] + u[1] * w[1] + u[2] * w[2];
        let div = la * lb * lc + dot(a, b) * lc + dot(b, c) * la + dot(c, a) * lb;
        total += 2.0 * det.atan2(div);
    }
    total / (4.0 * std::f64::consts::PI)
}

#[test]
fn a_nut_fits_its_screw_at_the_clearance_given() {
    for (size, head) in [("M6", "hexagon"), ("M3", "socket"), ("M10", "slotted")] {
        let screw = made("screw", json!({"thread": size, "length": 12, "head": head}));
        assert_clean(&screw, size);
        let nut = made("nut", json!({"thread": size, "clearance": 0.2}));
        assert_clean(&nut, size);
        let m = threads::metric(size);
        // Both stand centred on the Z axis; the screw's tip and the nut's bottom at z = 0. Placed
        // whole pitches up the thread, the nut's thread lines up with the screw's.
        let lift = (2.0 * m.pitch).max(1.0);
        let lift = (lift / m.pitch).round() * m.pitch;
        let placed = bake(
            &nut,
            &[
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, lift, 1.0,
            ],
        );
        let mut both = Builder::default();
        both.append(&screw);
        both.append(&placed);
        let both = both.raw().unwrap();
        let crossing = silent(|c| tritri::intersecting_pairs(&both, c)).unwrap();
        let n = screw.triangle_count() as u32;
        let between = crossing
            .iter()
            .filter(|(a, b)| (*a < n) != (*b < n))
            .count();
        assert_eq!(between, 0, "{size}: screw and nut cross");
        // Nor is one inside the other: points just inside the screw's thread where the nut sits are
        // not in the nut, and points just inside the nut's thread are not in the screw.
        let near = |mesh: &Mesh, inward: f64| -> Vec<[f64; 3]> {
            mesh.positions
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| p.map(f64::from))
                .filter(|p| p[2] > lift + 0.3 && p[2] < lift + m.nut_height - 0.3)
                .filter(|p| p[0].hypot(p[1]) < m.d)
                .step_by(97)
                .map(|p| {
                    let r = p[0].hypot(p[1]);
                    let k = (r + inward) / r;
                    [p[0] * k, p[1] * k, p[2]]
                })
                .collect()
        };
        let screw_points = near(&screw, -0.02);
        assert!(screw_points.len() > 10);
        for p in screw_points {
            assert!(
                winding(&placed, p).abs() < 0.5,
                "{size}: the screw at {p:?} is in the nut"
            );
        }
        for p in near(&placed, 0.02) {
            assert!(
                winding(&screw, p).abs() < 0.5,
                "{size}: the nut at {p:?} is in the screw"
            );
        }
        // So the Boolean intersection is empty. Half a pitch off, the threads would collide.
        let off = bake(
            &nut,
            &[
                1.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                0.0,
                lift + m.pitch / 2.0,
                1.0,
            ],
        );
        let mut clash = Builder::default();
        clash.append(&screw);
        clash.append(&off);
        let clash = clash.raw().unwrap();
        let crossing = silent(|c| tritri::intersecting_pairs(&clash, c)).unwrap();
        assert!(
            crossing.iter().any(|(a, b)| (*a < n) != (*b < n)),
            "{size}: a misplaced nut must collide, or the test proves nothing"
        );
    }
}

#[test]
fn a_qr_code_raises_its_dark_modules() {
    let mesh = made(
        "qr_code",
        json!({"data": "tricky3D", "size": 25, "quiet": 2}),
    );
    assert_clean(&mesh, "qr");
    let code = encode_qr("tricky3D", Ecc::Medium).unwrap();
    let dark = (0..code.size)
        .flat_map(|y| (0..code.size).map(move |x| (x, y)))
        .filter(|&(x, y)| code.dark(x, y))
        .count() as f64;
    let module = 25.0 / (code.size + 4) as f64;
    // The plate (a sample cell smaller than the size) plus the raised modules.
    let plate = (25.0 - module / 4.0).powi(2) * 2.0;
    close(
        "qr volume",
        stats(&mesh).volume,
        plate + dark * module * module * 1.0,
        0.02,
    );
    assert!(
        generate("qr_code", &params(json!({"code": "aztec"})))
            .unwrap_err()
            .contains("not available")
    );
}

#[test]
fn reliefs_and_height_fields_take_their_size_from_the_picture() {
    let mesh = made("bitmap", json!({"width": 60, "detail": 60, "levels": 4}));
    let s = size_of(&mesh);
    close("bitmap width", s[0], 60.0, 1e-6);
    close("bitmap depth", s[1], 45.0, 1e-6);
    close("bitmap height", s[2], 4.0, 1e-6);
    let cut = made(
        "bitmap",
        json!({"width": 60, "detail": 60, "relief": "cut", "keep_aspect": false, "depth": 30}),
    );
    assert_clean(&cut, "cut bitmap");
    close("cut depth", size_of(&cut)[1], 30.0, 1e-6);
    // Every grey level its own band: sharp edges must not leave slivers.
    let fine = made("bitmap", json!({"levels": 250, "detail": 150}));
    assert_clean(&fine, "bitmap of 250 levels");
    let hm = made(
        "heightmap",
        json!({"width": 40, "detail": 40, "base": 1, "height": 5}),
    );
    let s = size_of(&hm);
    close("heightmap width", s[0], 40.0, 1e-6);
    assert!(s[2] <= 6.0 + 1e-6 && s[2] > 5.0, "{s:?}");
}

#[test]
fn a_function_surface_is_a_slab_of_even_thickness() {
    let mesh = made(
        "function_surface",
        json!({"formula": "5 * sin(x / 8) * cos(y / 6) + 0.01 * x * y", "length": 50, "width": 40, "thickness": 2, "resolution": 80}),
    );
    assert_clean(&mesh, "function surface");
    close("slab volume", stats(&mesh).volume, 50.0 * 40.0 * 2.0, 1e-6);
    let err = generate("function_surface", &params(json!({"formula": "ln(x)"}))).unwrap_err();
    assert!(err.starts_with("The formula has no value"), "{err}");
    let err = generate("function_surface", &params(json!({"formula": "sin(x"}))).unwrap_err();
    assert_eq!(err, "The formula ends too early.");
}

#[test]
fn grids_have_their_holes() {
    let mesh = made(
        "rect_grid",
        json!({"length": 60, "width": 40, "height": 3, "holes_x": 6, "holes_y": 4, "wall": 2}),
    );
    let (hx, hy) = ((60.0 - 7.0 * 2.0) / 6.0, (40.0 - 5.0 * 2.0) / 4.0);
    close(
        "rect grid volume",
        stats(&mesh).volume,
        3.0 * (60.0 * 40.0 - 24.0 * hx * hy),
        1e-9,
    );
    let mesh = made(
        "square_grid",
        json!({"cell": 5, "wall": 1, "holes_x": 8, "holes_y": 3, "height": 2}),
    );
    close(
        "square grid volume",
        stats(&mesh).volume,
        2.0 * (49.0 * 19.0 - 24.0 * 25.0),
        1e-9,
    );
    // A honeycomb's cells hold a hexagonal hole each.
    let mesh = made(
        "hex_grid",
        json!({"hole": 8, "wall": 1.5, "holes_x": 3, "holes_y": 2, "height": 3}),
    );
    let hole = 3.0 * 3f64.sqrt() / 2.0 * 16.0;
    let cell_apothem = 4.0 * 3f64.sqrt() / 2.0 + 0.75;
    let cell = 2.0 * 3f64.sqrt() * cell_apothem * cell_apothem;
    close(
        "hex grid volume",
        stats(&mesh).volume,
        3.0 * 6.0 * (cell - hole),
        1e-6,
    );
    let tube = made("hex_cylinder", json!({}));
    let s = size_of(&tube);
    close("tube height", s[2], 40.0, 1e-6);
}

#[test]
fn the_convex_hull_holds_the_selection_and_the_base_plate_sits_under_it() {
    let ring = bake(
        &primitive::torus(15.0, 4.0, 4.0, 48, 16).unwrap(),
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 100.0, 80.0, 3.0, 1.0,
        ],
    );
    let hull = with_parts("convex_hull", json!({}), vec![ring.clone()]);
    assert!(hull.world);
    assert_clean(&hull.mesh, "hull");
    let hb = bounds(&hull.mesh).unwrap();
    assert_eq!(hb, bounds(&ring).unwrap(), "the hull's box is the ring's");
    // Every point of the ring is in the hull.
    for p in ring.positions.as_chunks::<3>().0.iter().step_by(37) {
        assert!(winding(&hull.mesh, p.map(f64::from)) > -0.01);
    }

    let plate = with_parts(
        "base_plate",
        json!({"offset": 2, "thickness": 2}),
        vec![ring.clone()],
    );
    assert_clean(&plate.mesh, "base plate");
    let b = bounds(&plate.mesh).unwrap();
    let rb = bounds(&ring).unwrap();
    assert!(
        (b[1][2] - rb[0][2]).abs() < 1e-4,
        "the plate's top meets the part's bottom"
    );
    assert!((b[1][2] - b[0][2] - 2.0).abs() < 1e-4);
    // The shadow of the ring (an annulus) plus the 2 mm offset, its hole kept.
    let reach = 15.0 + 4.0 + 2.0;
    assert!(
        ((b[1][0] - b[0][0]) as f64 - 2.0 * reach).abs() < 0.5,
        "{b:?}"
    );
    let inner = 15.0 - 4.0 - 2.0;
    let area = std::f64::consts::PI * (reach * reach - inner * inner);
    close("plate volume", stats(&plate.mesh).volume, area * 2.0, 0.03);
    let filled = with_parts(
        "base_plate",
        json!({"offset": 2, "outer_only": true}),
        vec![ring.clone()],
    );
    close(
        "filled plate volume",
        stats(&filled.mesh).volume,
        std::f64::consts::PI * reach * reach * 2.0,
        0.03,
    );
    for kind in ["hexagon", "square"] {
        let structured = with_parts("base_plate", json!({"type": kind}), vec![ring.clone()]);
        assert_clean(&structured.mesh, kind);
        assert!(
            stats(&structured.mesh).volume < 0.8 * stats(&plate.mesh).volume,
            "{kind}"
        );
    }
    let inverted = with_parts("base_plate", json!({"invert": true}), vec![ring.clone()]);
    assert_clean(&inverted.mesh, "inverted");
    // Without a selection there is nothing to make.
    let err =
        silent(|c| make("base_plate", &Map::new(), &Sources::default(), false, c)).unwrap_err();
    assert!(matches!(err, RepairError::Failed(s) if s.starts_with("Select the part")));
}

#[test]
fn the_klein_bottle_stays_hollow() {
    let mesh = made("klein", json!({"accuracy": 1.0}));
    let s = silent(|c| status::analyze(&mesh, c)).unwrap().status;
    // The outer surface, and the inside of the bottle as a cavity.
    assert!(s.shells >= 2, "{s:?}");
    close("klein height", size_of(&mesh)[2], 60.0, 0.05);
}

#[test]
fn shapes_with_exact_sizes() {
    // Flat across at one side of the ring, on edge at the other after half a turn.
    let mob = made("mobius", json!({"radius": 25, "width": 15, "thickness": 2}));
    let s = size_of(&mob);
    assert!((s[0] - (25.0 + 7.5 + 25.0 + 1.0)).abs() < 0.5, "{s:?}");
    let ball = made(
        "superellipsoid",
        json!({"east_west": 1, "north_south": 1, "radius_x": 10, "radius_y": 10, "radius_z": 10, "segments": 256, "rings": 128}),
    );
    close(
        "round superellipsoid",
        stats(&ball).volume,
        4.0 / 3.0 * std::f64::consts::PI * 1000.0,
        2e-3,
    );
    let boxy = made(
        "superellipsoid",
        json!({"east_west": 0.1, "north_south": 0.1, "radius_x": 10, "radius_y": 10, "radius_z": 10}),
    );
    assert!(stats(&boxy).volume > 0.9 * 8000.0);
    let brick = made("brick", json!({}));
    let s = size_of(&brick);
    close("brick length", s[0], 31.8, 1e-3);
    close("brick width", s[1], 15.8, 1e-3);
    close("brick height", s[2], 9.6 + 1.7, 1e-3);
    let bracelet = made("bracelet", json!({"wrist": 170, "fit": "close"}));
    let inner = bracelet
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (p[0] as f64).hypot(p[1] as f64))
        .fold(f64::INFINITY, f64::min);
    close(
        "bracelet circumference",
        std::f64::consts::TAU * inner,
        170.0,
        0.01,
    );
    let sweep = made("picture_sweep", json!({"spin": 0, "length": 30}));
    let pipe = made("mesh_pipe", json!({}));
    assert!(stats(&sweep).volume > 0.0 && stats(&pipe).volume > 0.0);
    let err = generate("mesh_pipe", &params(json!({"radius": 20}))).unwrap_err();
    assert!(err.starts_with("The tube would run into itself"), "{err}");
}

/// The tile pictures are preview builds at the defaults: every item that is not Soon makes one,
/// clean, without a selection or a picture (the samples stand in).
#[test]
fn every_tile_picture_builds_at_the_defaults() {
    for item in catalog().into_iter().filter(|i| i.soon.is_none()) {
        let made = silent(|c| make(item.id, &Map::new(), &Sources::default(), true, c))
            .unwrap_or_else(|e| panic!("{}: {e:?}", item.id));
        assert!(
            !made.world,
            "{}: a tile picture stands at the origin",
            item.id
        );
        assert_clean(&made.mesh, item.id);
    }
}
