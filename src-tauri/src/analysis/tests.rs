//! Known answers for the part analyses: boxes, prisms and spheres whose numbers are exact.

use std::f64::consts::PI;

use serde_json::json;

use super::gravity::centre_of_gravity;
use super::shadow::{union_area, upward};
use super::skin::{DOWNSKIN, PART, SkinSettings, UPSKIN, skins};
use super::standard::{self, triangle_quality};
use super::support::{SupportSettings, support_volume};
use crate::arrange::{V3, World, rot_z, rotated_about};
use crate::mesh::{IDENTITY, Matrix, Mesh, Welder, make_box, stats};
use crate::ops::concat;
use crate::repair::silent;

fn close(what: &str, got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: got {got}, want {want} (± {tol})"
    );
}

fn close3(what: &str, got: V3, want: V3, tol: f64) {
    for k in 0..3 {
        close(&format!("{what}[{k}]"), got[k], want[k], tol);
    }
}

/// The Part library's sphere.
fn sphere(r: f64, segments: u32, rings: u32) -> Mesh {
    let params = json!({"radius": r, "segments": segments, "rings": rings});
    crate::library::generate("sphere", params.as_object().unwrap()).unwrap()
}

fn area2(ring: &[[f64; 2]]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum()
}

/// A solid over a polygon with holes in a plane (u, v), from w = 0 to `depth`, placed by `at`
/// and facing out.
fn extrude(
    outline: &[[f64; 2]],
    holes: &[&[[f64; 2]]],
    depth: f64,
    at: impl Fn([f64; 2], f64) -> [f32; 3],
) -> Mesh {
    // Outline anticlockwise, holes clockwise: the solid is left of every loop.
    let mut loops: Vec<Vec<[f64; 2]>> = vec![outline.to_vec()];
    loops.extend(holes.iter().map(|h| h.to_vec()));
    for (i, ring) in loops.iter_mut().enumerate() {
        if (area2(ring) > 0.0) != (i == 0) {
            ring.reverse();
        }
    }
    let all: Vec<[f64; 2]> = loops.iter().flatten().copied().collect();
    let flat: Vec<f64> = all.iter().flatten().copied().collect();
    let mut starts = Vec::new();
    let mut n = loops[0].len();
    for ring in &loops[1..] {
        starts.push(n);
        n += ring.len();
    }
    let caps = earcutr::earcut(&flat, &starts, 2).unwrap();
    let mut w = Welder::new(64);
    for c in caps.chunks(3) {
        let [mut a, mut b, d] = [all[c[0]], all[c[1]], all[c[2]]];
        if area2(&[a, b, d]) < 0.0 {
            std::mem::swap(&mut a, &mut b);
        }
        w.triangle(at(a, depth), at(b, depth), at(d, depth));
        w.triangle(at(a, 0.0), at(d, 0.0), at(b, 0.0));
    }
    for ring in &loops {
        for i in 0..ring.len() {
            let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
            w.triangle(at(p, 0.0), at(q, 0.0), at(q, depth));
            w.triangle(at(p, 0.0), at(q, depth), at(p, depth));
        }
    }
    let mut m = w.finish().unwrap();
    if stats(&m).volume < 0.0 {
        for t in m.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    m
}

/// A prism from y = 0 to `depth` over a simple polygon in (x, z), facing out.
fn prism(outline: &[[f64; 2]], depth: f64) -> Mesh {
    extrude(outline, &[], depth, |p, y| {
        [p[0] as f32, y as f32, p[1] as f32]
    })
}

/// The X axis turned by `degrees` (about the origin), as a column-major matrix.
fn tilt_x(degrees: f64) -> Matrix {
    let (s, c) = degrees.to_radians().sin_cos();
    let mut m = IDENTITY;
    m[5] = c;
    m[6] = s;
    m[9] = -s;
    m[10] = c;
    m
}

fn tilt_y(degrees: f64) -> Matrix {
    let (s, c) = degrees.to_radians().sin_cos();
    let mut m = IDENTITY;
    m[0] = c;
    m[2] = -s;
    m[8] = s;
    m[10] = c;
    m
}

fn shadow_of(mesh: &Mesh, m: &Matrix) -> f64 {
    let (flats, _) = upward(&World::new(mesh, m));
    silent(|c| union_area(&flats, c)).unwrap()
}

// ---------------------------------------------------------------------------------------------
// Standard analysis

/// The Information panel's numbers for one part (src/info.ts infoRows): world volume and area
/// from the local stats, triangles, shells and open edges.
fn panel(mesh: &Mesh, m: &Matrix) -> (f64, f64, u64, u64, u64) {
    let s = stats(mesh);
    let (volume, area) = standard::panel_volume_and_area(mesh, m);
    (volume, area, s.triangles, s.shells, s.open_edges)
}

#[test]
fn standard_analysis_of_a_cube_matches_the_information_panel() {
    let cube = make_box([-10.0; 3], [10.0; 3]);
    let mut m = IDENTITY;
    m[12] = 100.0;
    m[13] = 100.0;
    m[14] = 10.0;
    let a = silent(|c| standard::analyze(&cube, &m, c)).unwrap();
    let (volume, area, triangles, shells, open) = panel(&cube, &m);
    assert_eq!(a.volume.to_bits(), volume.to_bits());
    assert_eq!(a.area.to_bits(), area.to_bits());
    assert_eq!(
        (a.triangles, a.shells, a.boundary_edges),
        (triangles, shells, open)
    );
    assert_eq!(a.volume, 8000.0);
    assert_eq!(a.area, 2400.0);
    assert!(a.volume_valid && a.closed && a.orientable);
    assert_eq!(
        (a.min, a.max, a.size),
        ([90.0, 90.0, 0.0], [110.0, 110.0, 20.0], [20.0; 3])
    );
    assert_eq!(
        (a.points, a.edges, a.holes, a.flipped_triangles, a.bad_edges),
        (8, 18, 0, 0, 0)
    );
    assert_eq!(a.boundary_length, 0.0);
    // Every edge has two triangles; the faces are split into right isosceles triangles.
    assert_eq!(
        (a.triangles_per_edge.min, a.triangles_per_edge.max),
        (2.0, 2.0)
    );
    assert_eq!(a.triangles_per_edge.deviation, 0.0);
    close("quality", a.triangle_quality.min, 3f64.sqrt() / 2.0, 1e-12);
    close("quality", a.triangle_quality.max, 3f64.sqrt() / 2.0, 1e-12);
    close("edge min", a.edge_length.min, 20.0, 1e-12);
    close("edge max", a.edge_length.max, 20.0 * 2f64.sqrt(), 1e-12);
    // 12 sides of 20 mm and 6 diagonals.
    close(
        "edge avg",
        a.edge_length.average,
        (12.0 * 20.0 + 6.0 * 20.0 * 2f64.sqrt()) / 18.0,
        1e-12,
    );
    // 18 edges round 8 points: 36 ends.
    close(
        "edges per point",
        a.edges_per_point.average,
        36.0 / 8.0,
        1e-12,
    );
}

#[test]
fn standard_analysis_of_a_sphere_matches_the_information_panel_and_counts_its_damage() {
    let ball = sphere(10.0, 64, 32);
    // Scaled unevenly, so the area takes the panel's exact pass.
    let mut m = IDENTITY;
    m[0] = 1.5;
    m[14] = 10.0;
    let a = silent(|c| standard::analyze(&ball, &m, c)).unwrap();
    let (volume, area, triangles, shells, open) = panel(&ball, &m);
    assert_eq!(a.volume.to_bits(), volume.to_bits());
    assert_eq!(a.area.to_bits(), area.to_bits());
    assert_eq!(
        (a.triangles, a.shells, a.boundary_edges),
        (triangles, shells, open)
    );
    assert!(a.closed && a.orientable && a.volume_valid);
    // Euler: a closed sphere has E = 3T/2 and V = T/2 + 2.
    assert_eq!(a.edges * 2, a.triangles * 3);
    assert_eq!(a.points, a.triangles / 2 + 2);

    // Without its first two triangles: holes, boundary edges and their length.
    let mut open_ball = ball.clone();
    open_ball.indices.drain(0..6);
    let b = silent(|c| standard::analyze(&open_ball, &IDENTITY, c)).unwrap();
    let s = stats(&open_ball);
    assert_eq!(b.boundary_edges, s.open_edges);
    assert!(!b.closed && !b.volume_valid);
    assert!(b.holes >= 1 && b.boundary_length > 0.0);
    // One triangle turned: flipped, still orientable.
    let mut turned = ball.clone();
    turned.indices.swap(1, 2);
    let t = silent(|c| standard::analyze(&turned, &IDENTITY, c)).unwrap();
    assert_eq!(t.flipped_triangles, 1);
    assert!(t.orientable);
}

#[test]
fn triangle_quality_is_one_for_equilateral_and_zero_for_flat() {
    let h = 3f64.sqrt() / 2.0;
    close(
        "equilateral",
        triangle_quality([0.0; 3], [1.0, 0.0, 0.0], [0.5, h, 0.0]),
        1.0,
        1e-12,
    );
    assert_eq!(
        triangle_quality([0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]),
        0.0
    );
    assert_eq!(triangle_quality([0.0; 3], [0.0; 3], [0.0; 3]), 0.0);
}

// ---------------------------------------------------------------------------------------------
// Centre of gravity

fn gravity(mesh: &Mesh, m: &Matrix) -> V3 {
    silent(|c| centre_of_gravity(&World::new(mesh, m), c))
        .unwrap()
        .unwrap()
        .centre
}

#[test]
fn centre_of_gravity_of_a_box_an_l_shape_and_a_hollow_part() {
    let mut m = IDENTITY;
    m[12] = 50.0;
    let g =
        silent(|c| centre_of_gravity(&World::new(&make_box([0.0; 3], [20.0, 10.0, 6.0]), &m), c))
            .unwrap()
            .unwrap();
    close3("box", g.centre, [60.0, 5.0, 3.0], 1e-9);
    close("box volume", g.volume, 1200.0, 1e-9);

    // L in (x, z): a 30 x 10 foot and a 10 x 30 upright, 10 deep. Foot 300 mm² at (15, 5),
    // upright above it 200 mm² at (5, 20).
    let l = prism(
        &[
            [0.0, 0.0],
            [30.0, 0.0],
            [30.0, 10.0],
            [10.0, 10.0],
            [10.0, 30.0],
            [0.0, 30.0],
        ],
        10.0,
    );
    let want = [
        (300.0 * 15.0 + 200.0 * 5.0) / 500.0,
        5.0,
        (300.0 * 5.0 + 200.0 * 20.0) / 500.0,
    ];
    close3("L", gravity(&l, &IDENTITY), want, 1e-9);
    // Turned and moved with the part.
    let r = rotated_about(&IDENTITY, &rot_z(0.7), [0.0; 3]);
    let turned = crate::arrange::apply(&rot_z(0.7), want);
    close3("L turned", gravity(&l, &r), turned, 1e-9);

    // A 20 mm cube with a 6 mm cavity off centre (facing in): the solid's centre moves away from it.
    let outer = make_box([0.0; 3], [20.0; 3]);
    let mut cavity = make_box([2.0, 2.0, 2.0], [8.0, 8.0, 8.0]);
    for t in cavity.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    let hollow = concat(&[&outer, &cavity]).unwrap();
    let (vo, vc) = (8000.0, 216.0);
    let want = (vo * 10.0 - vc * 5.0) / (vo - vc);
    close3("hollow", gravity(&hollow, &IDENTITY), [want; 3], 1e-9);
}

#[test]
fn centre_of_gravity_of_a_part_hollowed_by_generate_shell() {
    use crate::ops::shell::{ShellKind, ShellSettings, generate};
    // A 20 x 10 x 6 box hollowed 1 mm: the cavity is a 18 x 8 x 4 box in the middle, so the
    // centre stays at the box's centre while the volume drops to 1200 - 576.
    let block = make_box([0.0; 3], [20.0, 10.0, 6.0]);
    let s = ShellSettings {
        kind: ShellKind::HollowPart,
        thickness: 1.0,
        accuracy: 0.25,
        smooth: false,
    };
    let shell = silent(|c| generate(&block, &s, c)).unwrap().mesh.unwrap();
    let g = silent(|c| centre_of_gravity(&World::new(&shell, &IDENTITY), c))
        .unwrap()
        .unwrap();
    close3("hollow box", g.centre, [10.0, 5.0, 3.0], 0.02);
    close("hollow box volume", g.volume, 1200.0 - 576.0, 6.0);

    // An L hollowed the same way: its centre is the L's within the shell's grid accuracy, and
    // differs from the solid L's by less than the cavity can move it.
    let l = prism(
        &[
            [0.0, 0.0],
            [30.0, 0.0],
            [30.0, 10.0],
            [10.0, 10.0],
            [10.0, 30.0],
            [0.0, 30.0],
        ],
        10.0,
    );
    let hollow_l = silent(|c| generate(&l, &s, c)).unwrap().mesh.unwrap();
    let solid = gravity(&l, &IDENTITY);
    let g = gravity(&hollow_l, &IDENTITY);
    // The y centre stays in the middle by symmetry.
    close("hollow L y", g[1], 5.0, 0.02);
    assert!(
        (g[0] - solid[0]).abs() < 2.0 && (g[2] - solid[2]).abs() < 2.0,
        "{g:?} vs {solid:?}"
    );
}

#[test]
fn a_flat_part_has_no_centre_of_gravity() {
    // A square sheet with both faces: it encloses nothing.
    let mut sheet = make_box([0.0; 3], [10.0; 3]);
    sheet.indices.truncate(6);
    let back: Vec<u32> = sheet
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|t| [t[0], t[2], t[1]])
        .collect();
    sheet.indices.extend(back);
    assert!(
        silent(|c| centre_of_gravity(&World::new(&sheet, &IDENTITY), c))
            .unwrap()
            .is_none()
    );
}

// ---------------------------------------------------------------------------------------------
// Shadow area

#[test]
fn shadow_area_of_boxes_upright_and_tilted() {
    let block = make_box([0.0; 3], [20.0, 10.0, 5.0]);
    close("box", shadow_of(&block, &IDENTITY), 200.0, 1e-9);
    // Tilted 45° about X: 20 wide, (10 + 5) / √2 deep.
    close(
        "tilted about X",
        shadow_of(&block, &tilt_x(45.0)),
        20.0 * 15.0 / 2f64.sqrt(),
        1e-9,
    );
    // Tilted 45° about Y, then turned about Z: 10 deep, (20 + 5) / √2 wide.
    let m = rotated_about(&tilt_y(45.0), &rot_z(0.5), [3.0, 4.0, 0.0]);
    close(
        "tilted about Y",
        shadow_of(&block, &m),
        10.0 * 25.0 / 2f64.sqrt(),
        1e-9,
    );
}

#[test]
fn shadow_area_of_a_sphere_is_its_equator_polygon() {
    let r = 10.0;
    let n = 256.0;
    let ball = sphere(r, 256, 128);
    let area = shadow_of(&ball, &IDENTITY);
    // The equator is a 256-gon: exactly the area of its (f32) corners, and within f32 rounding of
    // the regular polygon's n/2 r² sin(2π/n), 0.03 mm² short of π r².
    let b = crate::mesh::bounds(&ball).unwrap();
    let (cx, cy, cz) = (
        (b[0][0] + b[1][0]) as f64 / 2.0,
        (b[0][1] + b[1][1]) as f64 / 2.0,
        (b[0][2] + b[1][2]) as f64 / 2.0,
    );
    let mut equator: Vec<[f64; 2]> = ball
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| (p[2] as f64 - cz).abs() < 1e-4)
        .map(|p| [p[0] as f64, p[1] as f64])
        .collect();
    assert_eq!(equator.len(), 256);
    equator.sort_by(|a, b| {
        (a[1] - cy)
            .atan2(a[0] - cx)
            .total_cmp(&(b[1] - cy).atan2(b[0] - cx))
    });
    close("sphere equator", area, area2(&equator) / 2.0, 1e-9);
    close(
        "sphere polygon",
        area,
        n / 2.0 * r * r * (2.0 * PI / n).sin(),
        1e-4,
    );
    close("sphere circle", area, PI * r * r, 0.05);
}

#[test]
fn overlapping_parts_of_one_mesh_count_once() {
    // Two crossing bars in one mesh, one on top of the other: 2 x 200 - 100.
    let a = make_box([0.0, 5.0, 0.0], [20.0, 15.0, 5.0]);
    let b = make_box([5.0, 0.0, 5.0], [15.0, 20.0, 10.0]);
    let both = concat(&[&a, &b]).unwrap();
    close("cross", shadow_of(&both, &IDENTITY), 300.0, 1e-9);
    // A square ring standing up leaves its hole out; lying on its side it does not.
    let ring = extrude(
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        &[&[[3.0, 3.0], [7.0, 3.0], [7.0, 7.0], [3.0, 7.0]]],
        10.0,
        |p, z| [p[0] as f32, p[1] as f32, z as f32],
    );
    close("ring", shadow_of(&ring, &IDENTITY), 100.0 - 16.0, 1e-9);
    close(
        "ring on its side",
        shadow_of(&ring, &tilt_x(90.0)),
        100.0,
        1e-9,
    );
}

#[test]
fn a_cancel_stops_the_shadow_area() {
    let (flats, _) = upward(&World::new(&sphere(10.0, 64, 32), &IDENTITY));
    let stop = std::sync::atomic::AtomicBool::new(true);
    let mut control = crate::repair::Control::new(&stop, |_| {});
    assert!(union_area(&flats, &mut control).is_err());
}

// ---------------------------------------------------------------------------------------------
// Support volume

/// A T in (x, z): a 10 wide stem 20 tall under a 30 wide bar 5 thick, 10 deep. Its two
/// overhangs are 10 x 10 each, flat, 20 mm up.
fn tee() -> Mesh {
    prism(
        &[
            [10.0, 0.0],
            [20.0, 0.0],
            [20.0, 20.0],
            [30.0, 20.0],
            [30.0, 25.0],
            [0.0, 25.0],
            [0.0, 20.0],
            [10.0, 20.0],
        ],
        10.0,
    )
}

fn support(mesh: &Mesh, m: &Matrix, angle: f64) -> super::support::Support {
    silent(|c| support_volume(mesh, &World::new(mesh, m), &SupportSettings { angle }, c))
        .unwrap()
        .support
}

#[test]
fn support_volume_of_a_tee() {
    for angle in [0.0, 45.0, 90.0] {
        let s = support(&tee(), &IDENTITY, angle);
        close("tee volume", s.volume, 2.0 * 10.0 * 10.0 * 20.0, 1e-9);
        close("tee area", s.area, 200.0, 1e-9);
    }
    // Lifted 5 mm: the stem's foot needs 10 x 10 x 5 too.
    let mut up = IDENTITY;
    up[14] = 5.0;
    close(
        "lifted tee",
        support(&tee(), &up, 0.0).volume,
        200.0 * 25.0 + 100.0 * 5.0,
        1e-9,
    );
    // Half under the platform: only what is above counts.
    let mut down = IDENTITY;
    down[14] = -10.0;
    close(
        "sunk tee",
        support(&tee(), &down, 0.0).volume,
        2.0 * 100.0 * 10.0,
        1e-9,
    );
}

#[test]
fn support_volume_counts_a_45_degree_overhang_only_from_45_degrees() {
    // A stem with a wing whose underside rises at 45° from (20, 10) to (30, 20).
    let wing = prism(
        &[
            [10.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [30.0, 20.0],
            [30.0, 25.0],
            [10.0, 25.0],
        ],
        10.0,
    );
    // The underside seen from below is 10 x 10, from 10 to 20 mm up: 15 on average.
    close(
        "at 45",
        support(&wing, &IDENTITY, 45.0).volume,
        1500.0,
        1e-6,
    );
    close("at 44", support(&wing, &IDENTITY, 44.0).volume, 0.0, 1e-12);
}

// ---------------------------------------------------------------------------------------------
// Upskins and downskins

fn skin_settings(angle: f64) -> SkinSettings {
    SkinSettings {
        upskins: true,
        up_angle: angle,
        downskins: true,
        down_angle: angle,
        min_component: 10.0,
        filter_small: false,
    }
}

#[test]
fn upskins_and_downskins_of_a_box_at_zero_degrees() {
    let block = make_box([0.0; 3], [20.0, 20.0, 10.0]);
    let world = World::new(&block, &IDENTITY);
    let s = silent(|c| skins(&block, &world, &skin_settings(0.0), c)).unwrap();
    close("upskin", s.up.area, 400.0, 1e-9);
    close("downskin", s.down.area, 400.0, 1e-9);
    assert_eq!((s.up.components, s.down.components), (1, 1));
    assert_eq!(s.classes.iter().filter(|&&c| c == UPSKIN).count(), 2);
    assert_eq!(s.classes.iter().filter(|&&c| c == DOWNSKIN).count(), 2);
    // Too small to count.
    let mut big = skin_settings(0.0);
    big.min_component = 400.0;
    let s = silent(|c| skins(&block, &world, &big, c)).unwrap();
    assert_eq!((s.up.area, s.up.components), (0.0, 0));
    assert!(s.classes.iter().all(|&c| c == PART));
    // Downskins off.
    let mut up_only = skin_settings(0.0);
    up_only.downskins = false;
    let s = silent(|c| skins(&block, &world, &up_only, c)).unwrap();
    assert!(!s.down.analyzed && s.down.area == 0.0 && s.up.area == 400.0);
}

#[test]
fn a_tilted_face_counts_from_its_tilt_and_small_islands_are_filtered() {
    // The 45° wing's underside is a downskin from 45° on; the T's top is one upskin.
    let wing = prism(
        &[
            [10.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [30.0, 20.0],
            [30.0, 25.0],
            [10.0, 25.0],
        ],
        10.0,
    );
    let world = World::new(&wing, &IDENTITY);
    let at = |angle: f64| silent(|c| skins(&wing, &world, &skin_settings(angle), c)).unwrap();
    close("under 45", at(44.0).down.area, 100.0, 1e-9); // the foot only
    close(
        "at 45",
        at(45.0).down.area,
        100.0 + 100.0 * 2f64.sqrt(),
        1e-9,
    );
    assert_eq!(at(45.0).down.components, 2);

    // A tiny steep dimple in a flat top: its triangles stay part unless filtered.
    let mut w = Welder::new(64);
    let p = |x: f32, y: f32, z: f32| [x, y, z];
    // A 10 x 10 top at z = 0 round a 1 x 1 square whose middle rises to a sharp 0.9 mm point.
    let (a, b, c, d) = (
        p(4.5, 4.5, 0.0),
        p(5.5, 4.5, 0.0),
        p(5.5, 5.5, 0.0),
        p(4.5, 5.5, 0.0),
    );
    let tip = p(5.0, 5.0, 0.9);
    for (q, r) in [(a, b), (b, c), (c, d), (d, a)] {
        w.triangle(q, r, tip);
    }
    let corners = [
        p(0.0, 0.0, 0.0),
        p(10.0, 0.0, 0.0),
        p(10.0, 10.0, 0.0),
        p(0.0, 10.0, 0.0),
    ];
    let inner = [a, b, c, d];
    for i in 0..4 {
        let (o0, o1) = (corners[i], corners[(i + 1) % 4]);
        let (i0, i1) = (inner[i], inner[(i + 1) % 4]);
        w.triangle(o0, o1, i1);
        w.triangle(o0, i1, i0);
    }
    let top = w.finish().unwrap();
    let world = World::new(&top, &IDENTITY);
    let mut s = skin_settings(10.0);
    s.min_component = 5.0;
    let plain = silent(|c| skins(&top, &world, &s, c)).unwrap();
    close("dimple left out", plain.up.area, 99.0, 1e-6);
    s.filter_small = true;
    let filtered = silent(|c| skins(&top, &world, &s, c)).unwrap();
    assert!(filtered.up.area > 99.5, "{}", filtered.up.area);
    assert_eq!(filtered.up.components, 1);
}

// ---------------------------------------------------------------------------------------------
// Wall thickness

fn wall(mesh: &Mesh, threshold: f64, acceptance: f64, stop_early: bool) -> super::wall::WallDetail {
    let s = super::wall::WallSettings {
        threshold,
        acceptance,
        stop_early,
    };
    silent(|c| super::wall::wall_thickness(&World::new(mesh, &IDENTITY), &s, c)).unwrap()
}

#[test]
fn a_3_mm_plate_reads_3_mm_and_fails_at_4() {
    let plate = make_box([0.0; 3], [50.0, 50.0, 3.0]);
    let ok = wall(&plate, 2.5, 5.0, false);
    close("plate", ok.result.min_thickness.unwrap(), 3.0, 1e-9);
    assert!(ok.result.passed && ok.result.complete);
    assert_eq!(ok.result.thin_area, 0.0);
    let from = ok.result.min_from.unwrap();
    let to = ok.result.min_to.unwrap();
    close("from-to", (from[2] - to[2]).abs(), 3.0, 1e-9);
    // At 4 mm the top and bottom are too thin: 5000 of 5600 mm².
    let thin = wall(&plate, 4.0, 5.0, false);
    assert!(!thin.result.passed);
    close(
        "thin share",
        thin.result.thin_percent,
        100.0 * 5000.0 / 5600.0,
        1e-9,
    );
    // The sides read across the plate.
    let world = World::new(&plate, &IDENTITY);
    let side = thin
        .thickness
        .iter()
        .zip(world.triangles.iter())
        .filter(|(_, t)| world.normal_area(**t).0[2] == 0.0)
        .map(|(d, _)| *d)
        .fold(f32::INFINITY, f32::min);
    close("side", side as f64, 50.0, 1e-4);
}

#[test]
fn a_box_with_one_1_mm_wall_marks_only_that_wall() {
    // A 20 mm cube hollowed to leave one wall (X = 0) 1 mm thick and the others 3 mm.
    let outer = make_box([0.0; 3], [20.0; 3]);
    let mut cavity = make_box([1.0, 3.0, 3.0], [17.0, 17.0, 17.0]);
    for t in cavity.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    let hollow = concat(&[&outer, &cavity]).unwrap();
    let d = wall(&hollow, 2.0, 5.0, false);
    let world = World::new(&hollow, &IDENTITY);
    for (i, &t) in world.triangles.iter().enumerate() {
        let corners = world.corners(t);
        let on_thin_wall =
            corners.iter().all(|p| p[0] == 0.0) || corners.iter().all(|p| p[0] == 1.0);
        let thin = d.thickness[i] < 2.0;
        assert_eq!(thin, on_thin_wall, "triangle {i}: {}", d.thickness[i]);
    }
    close("thinnest", d.result.min_thickness.unwrap(), 1.0, 1e-9);
    // 400 mm² outside and 14 x 14 inside.
    close("thin area", d.result.thin_area, 400.0 + 196.0, 1e-9);
}

#[test]
fn the_fast_mode_stops_once_the_test_has_failed() {
    let plate = make_box([0.0; 3], [50.0, 50.0, 1.0]);
    let (points, triangles) = super::compare::refine(&World::new(&plate, &IDENTITY), 300_000);
    let fine = Mesh {
        positions: points,
        indices: triangles,
    };
    let full = wall(&fine, 2.0, 5.0, false);
    assert!(full.result.complete && !full.result.passed);
    let fast = wall(&fine, 2.0, 5.0, true);
    assert!(!fast.result.complete && !fast.result.passed);
    assert!(fast.result.thin_percent > 5.0);
    assert!(
        fast.thickness.iter().any(|d| d.is_nan()),
        "some triangles left unmeasured"
    );
    close(
        "fast thinnest",
        fast.result.min_thickness.unwrap(),
        1.0,
        1e-6,
    );
}

// ---------------------------------------------------------------------------------------------
// Mesh compare

fn compare(
    reference: &Mesh,
    at: &Matrix,
    comparison: &Mesh,
    to: &Matrix,
    direction: super::compare::Direction,
    align: bool,
) -> super::compare::Comparison {
    let s = super::compare::CompareSettings { direction, align };
    silent(|c| {
        super::compare::compare(
            &World::new(reference, at),
            &World::new(comparison, to),
            &s,
            c,
        )
    })
    .unwrap()
}

#[test]
fn a_sphere_compared_with_itself_is_zero_everywhere() {
    let ball = sphere(10.0, 48, 24);
    let c = compare(
        &ball,
        &IDENTITY,
        &ball,
        &IDENTITY,
        super::compare::Direction::Both,
        false,
    );
    for (r, surface) in [c.forward.unwrap(), c.backward.unwrap()] {
        assert!(r.max_unsigned < 1e-5, "{r:?}");
        assert!(surface.distances.iter().all(|d| d.abs() < 1e-5));
        assert!(
            surface.triangles.len() / 3 > ball.triangle_count(),
            "refined"
        );
    }
}

#[test]
fn a_box_against_itself_scaled_by_1_percent_reads_by_face_edge_and_corner() {
    let block = make_box([-10.0; 3], [10.0; 3]);
    let mut bigger = IDENTITY;
    for k in [0, 5, 10] {
        bigger[k] = 1.01;
    }
    let c = compare(
        &block,
        &IDENTITY,
        &block,
        &bigger,
        super::compare::Direction::Both,
        false,
    );
    // The reference lies inside the bigger box, 0.1 mm under each of its faces: every point of it
    // is 0.1 mm from the nearest face.
    let (forward, surface) = c.forward.unwrap();
    let tol = 1e-5;
    close("deepest", forward.max_negative, -0.1, tol);
    close("average", forward.average_signed, -0.1, tol);
    assert!(forward.max_positive.abs() < 1e-6);
    assert!(
        surface
            .distances
            .iter()
            .all(|d| (*d as f64 + 0.1).abs() < tol)
    );
    // The other way the bigger box lies outside the reference: 0.1 mm off a face, 0.1√2 off an
    // edge, 0.1√3 off a corner.
    let (backward, surface) = c.backward.unwrap();
    close("furthest", backward.max_positive, 0.1 * 3f64.sqrt(), tol);
    assert!(backward.max_negative.abs() < 1e-6);
    let mut faces = 0;
    for (i, &d) in surface.distances.iter().enumerate() {
        let p = &surface.points[i * 3..i * 3 + 3];
        let on = p.iter().filter(|v| (v.abs() - 10.1).abs() < 1e-4).count();
        close(
            &format!("point {p:?}"),
            d as f64,
            0.1 * (on as f64).sqrt(),
            tol,
        );
        faces += usize::from(on == 1);
    }
    // Most points lie inside a face.
    assert!(faces * 2 > surface.distances.len());
}

#[test]
fn align_recovers_a_known_rotation_and_offset() {
    let part = tee();
    // Turned 30° about (1, 2, 3) and moved by (5, -3, 2).
    let axis = {
        let l = (1.0f64 + 4.0 + 9.0).sqrt();
        [1.0 / l, 2.0 / l, 3.0 / l]
    };
    let (s, c) = 30f64.to_radians().sin_cos();
    let k = |i: usize, j: usize| axis[i] * axis[j] * (1.0 - c);
    let r = [
        [c + k(0, 0), k(0, 1) - axis[2] * s, k(0, 2) + axis[1] * s],
        [k(1, 0) + axis[2] * s, c + k(1, 1), k(1, 2) - axis[0] * s],
        [k(2, 0) - axis[1] * s, k(2, 1) + axis[0] * s, c + k(2, 2)],
    ];
    let mut m = IDENTITY;
    for (row, rr) in r.iter().enumerate() {
        for (col, v) in rr.iter().enumerate() {
            m[col * 4 + row] = *v;
        }
    }
    m[12] = 5.0;
    m[13] = -3.0;
    m[14] = 2.0;
    let one_way = super::compare::Direction::ReferenceToComparison;
    let unaligned = compare(&part, &IDENTITY, &part, &m, one_way, false);
    assert!(unaligned.forward.unwrap().0.max_unsigned > 3.0);
    let aligned = compare(&part, &IDENTITY, &part, &m, one_way, true);
    let fit = aligned.fit.unwrap();
    close("angle", fit.angle, 30.0, 0.01);
    // The fit undoes the move: rotation r⁻¹ = rᵀ.
    for (i, row) in fit.rotation.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            close("rotation", *v, r[j][i], 1e-4);
        }
    }
    let (forward, _) = aligned.forward.unwrap();
    assert!(forward.max_unsigned < 1e-3, "{forward:?}");
    assert!(fit.rms < 1e-3);
}

#[test]
fn closest_points_on_a_triangle() {
    use super::compare::closest_on_triangle;
    let (a, b, c) = ([0.0; 3], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]);
    assert_eq!(
        closest_on_triangle([2.0, 2.0, 5.0], a, b, c),
        [2.0, 2.0, 0.0]
    );
    assert_eq!(closest_on_triangle([-3.0, -3.0, 1.0], a, b, c), a);
    assert_eq!(
        closest_on_triangle([5.0, -4.0, 0.0], a, b, c),
        [5.0, 0.0, 0.0]
    );
    assert_eq!(
        closest_on_triangle([10.0, 10.0, 0.0], a, b, c),
        [5.0, 5.0, 0.0]
    );
}

// ---------------------------------------------------------------------------------------------
// Lifting out: interlocking and Z-removability

/// A closed torus round the Z axis through the origin: `n` × `m` quads, facing out.
fn torus(n: usize, m: usize, major: f64, minor: f64) -> Mesh {
    let mut positions = Vec::with_capacity(n * m * 3);
    for i in 0..n {
        let u = std::f64::consts::TAU * i as f64 / n as f64;
        for j in 0..m {
            let v = std::f64::consts::TAU * j as f64 / m as f64;
            let r = major + minor * v.cos();
            positions.extend_from_slice(&[
                (r * u.cos()) as f32,
                (r * u.sin()) as f32,
                (minor * v.sin()) as f32,
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
    let mut mesh = Mesh { positions, indices };
    if stats(&mesh).volume < 0.0 {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    mesh
}

/// `mesh` stood up: turned 90° about the X axis, then moved by `at`.
fn stood_up(mesh: &Mesh, at: [f64; 3]) -> Mesh {
    let mut m = tilt_x(90.0);
    m[12] = at[0];
    m[13] = at[1];
    m[14] = at[2];
    crate::mesh::bake(mesh, &m)
}

fn solid(mesh: &Mesh) -> super::lift::Solid {
    super::lift::Solid::new(World::new(mesh, &IDENTITY))
}

#[test]
fn a_box_on_a_box_lifts_out_top_first() {
    let bottom = make_box([0.0; 3], [20.0, 20.0, 10.0]);
    let top = make_box([5.0, 5.0, 10.0], [15.0, 15.0, 20.0]);
    let beside = make_box([30.0, 0.0, 0.0], [40.0, 10.0, 10.0]);
    let solids = [solid(&bottom), solid(&top), solid(&beside)];
    assert!(super::lift::blocks(&solids[0], &solids[1]));
    assert!(!super::lift::blocks(&solids[1], &solids[0]));
    assert!(!super::lift::blocks(&solids[0], &solids[2]));
    let r = silent(|c| super::lift::interlock(&solids, c)).unwrap();
    assert_eq!(r.rounds, vec![vec![1, 2], vec![0]]);
    assert!(r.trapped.is_empty());
}

#[test]
fn a_ring_round_a_post_is_interlocked() {
    // A ring standing up (in the X/Z plane) round a bar lying along Y through it.
    let ring = stood_up(&torus(48, 16, 10.0, 2.0), [0.0, 0.0, 12.0]);
    let post = make_box([-3.0, -30.0, 9.0], [3.0, 30.0, 15.0]);
    let solids = [solid(&ring), solid(&post)];
    let r = silent(|c| super::lift::interlock(&solids, c)).unwrap();
    assert!(r.rounds.is_empty());
    assert_eq!(r.trapped, vec![0, 1]);
    // With the bar beside the ring both lift out.
    let apart = make_box([20.0, -30.0, 9.0], [26.0, 30.0, 15.0]);
    let r = silent(|c| super::lift::interlock(&[solid(&ring), solid(&apart)], c)).unwrap();
    assert!(r.trapped.is_empty());
}

#[test]
fn z_removability_of_a_cup_a_chain_and_a_trapped_ball() {
    let removable = |mesh: &Mesh| {
        silent(|c| super::lift::removability(mesh, &World::new(mesh, &IDENTITY), c)).unwrap()
    };
    // A cup upside down: walls and a lid that touch it (two shells, one body).
    let walls = extrude(
        &[[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
        &[&[[2.0, 2.0], [18.0, 2.0], [18.0, 18.0], [2.0, 18.0]]],
        18.0,
        |p, z| [p[0] as f32, p[1] as f32, z as f32],
    );
    let lid = make_box([0.0, 0.0, 18.0], [20.0, 20.0, 20.0]);
    let cup = concat(&[&walls, &lid]).unwrap();
    let (r, classes) = removable(&cup);
    assert!(r.removable, "{r:?}");
    assert_eq!((r.bodies, r.trapped_bodies), (1, 0));
    assert!(classes.iter().all(|&c| c == 1));
    // A ball under the lid, inside the cup: the ball is trapped, the cup is not.
    let ball = sphere(5.0, 24, 12);
    let ball_at = crate::mesh::bake(&ball, &{
        // Centred at (10, 10, 8): 3 to 13 mm up, clear of the walls and under the lid.
        let b = crate::mesh::bounds(&ball).unwrap();
        let mut m = IDENTITY;
        for k in 0..3 {
            m[12 + k] = [10.0, 10.0, 8.0][k] - (b[0][k] + b[1][k]) as f64 / 2.0;
        }
        m
    });
    let rattle = concat(&[&cup, &ball_at]).unwrap();
    let (r, classes) = removable(&rattle);
    assert!(!r.removable);
    assert_eq!((r.bodies, r.trapped_bodies), (2, 1));
    let cup_triangles = cup.triangle_count();
    assert!(classes[..cup_triangles].iter().all(|&c| c == 1));
    assert!(classes[cup_triangles..].iter().all(|&c| c == 2));
    // Two chain links through each other in one part: both trapped.
    let a = torus(48, 16, 10.0, 2.0);
    let b = stood_up(&a, [10.0, 0.0, 0.0]);
    let chain = concat(&[&a, &b]).unwrap();
    let (r, _) = removable(&chain);
    assert_eq!((r.removable, r.bodies, r.trapped_bodies), (false, 2, 2));
    // A hollow box (a cavity kept with its body) is one body and lifts out.
    let outer = make_box([0.0; 3], [20.0; 3]);
    let mut cavity = make_box([5.0; 3], [15.0; 3]);
    for t in cavity.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    let (r, _) = removable(&concat(&[&outer, &cavity]).unwrap());
    assert_eq!((r.removable, r.bodies), (true, 1));
}
