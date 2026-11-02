//! Known answers for Hollow: a 40 mm cube with 2 mm walls, its cavity, holes and plugs.

use std::f64::consts::PI;

use super::*;
use crate::mesh::{make_box, stats};
use crate::repair::{silent, status, tritri};

const T: f64 = 2.0;
/// The grid step: the cube's faces fall on grid planes or between them; either way the cavity's
/// flat faces sit on the offset to a twentieth of a step (Generate shell's tidying keeps them there).
const H: f64 = 0.4;

fn settings() -> HollowSettings {
    HollowSettings {
        thickness: T,
        accuracy: H,
        smoothness: 0,
    }
}

fn cube() -> Mesh {
    make_box([0.0; 3], [40.0; 3])
}

fn cavity_of(part: &Mesh) -> Mesh {
    silent(|c| cavity(part, &settings(), c))
        .unwrap()
        .mesh
        .expect("a cavity")
}

fn hole(at: P3, normal: P3, diameter: f64) -> Hole {
    Hole {
        at,
        normal,
        diameter,
        taper: 0.0,
        flare: 0.0,
    }
}

/// Euler characteristic V − E + F of a closed mesh: 2 per sphere-like shell, 2 less per handle.
fn euler(mesh: &Mesh) -> i64 {
    let edges = crate::repair::topology::Edges::new(mesh);
    mesh.vertex_count() as i64 - edges.len() as i64 + mesh.triangle_count() as i64
}

fn assert_sound(mesh: &Mesh) {
    let s = silent(|c| status::analyze(mesh, c)).unwrap().status;
    assert!(
        s.closed && s.oriented,
        "closed {} oriented {}",
        s.closed,
        s.oriented
    );
    assert!(
        silent(|c| tritri::intersecting_pairs(mesh, c))
            .unwrap()
            .is_empty(),
        "crossings"
    );
}

#[test]
fn a_40_mm_cube_with_2_mm_walls_has_a_36_mm_cavity() {
    let part = cube();
    let made = silent(|c| cavity(&part, &settings(), c)).unwrap();
    assert!(made.thin.is_none(), "{:?}", made.thin.map(|t| t.1));
    let cavity = made.mesh.unwrap();
    let v = stats(&cavity).volume;
    let want = 36f64.powi(3);
    // The cavity's faces lie on the offset to 0.05 of a step and its 12 edges and 8 corners are cut
    // off by up to a step: a few mm³ in 46,656.
    assert!(
        (v - want).abs() < 6.0 * 36.0 * 36.0 * 0.05 * H + 12.0 * 36.0 * H * H,
        "{v}"
    );
    let b = crate::mesh::bounds(&cavity).unwrap();
    for k in 0..3 {
        assert!((b[0][k] as f64 - T).abs() < 0.05 * H + 1e-4, "{b:?}");
        assert!((b[1][k] as f64 - 40.0 + T).abs() < 0.05 * H + 1e-4, "{b:?}");
    }
    // With no holes, Apply gives the part and its cavity: two shells.
    let applied = silent(|c| apply(&part, &cavity, &settings(), &[], None, c)).unwrap();
    assert_sound(&applied.mesh);
    assert_eq!(euler(&applied.mesh), 4);
    assert!((stats(&applied.mesh).volume - (64_000.0 - v)).abs() < 1e-3);
}

#[test]
fn one_3_mm_hole_opens_the_cavity_once_and_takes_its_own_volume() {
    let part = cube();
    let cavity = cavity_of(&part);
    let hollow_volume = 64_000.0 - stats(&cavity).volume;
    let holes = [hole([20.0, 20.0, 0.0], [0.0, 0.0, -1.0], 3.0)];
    let scene = Scene::new(&part, Some(&cavity), settings());
    let check = check_hole(&scene, &holes, 0);
    assert!(check.ok, "{check:?}");
    let depth = check.depth.unwrap();
    assert!((depth - T).abs() < 0.05 * H + 1e-3, "{depth}");
    let applied = silent(|c| apply(&part, &cavity, &settings(), &holes, None, c)).unwrap();
    assert!(applied.skipped.is_empty());
    let mesh = &applied.mesh;
    assert_sound(mesh);
    // One shell now (the outside and the cavity joined through the hole), still a sphere-like
    // surface: exactly one opening.
    assert_eq!(stats(mesh).shells, 1);
    assert_eq!(euler(mesh), 2);
    // The hole took a 48-sided prism 3 mm across through the 2 mm floor.
    let polygon = 0.5 * 48.0 * (2.0 * PI / 48.0).sin() * 1.5 * 1.5;
    let taken = hollow_volume - stats(mesh).volume;
    let want = polygon * depth;
    assert!(
        (taken - want).abs() < 0.02 * want,
        "took {taken} mm³, want {want}"
    );
}

#[test]
fn a_tapered_flared_hole_is_wider_outside() {
    let part = cube();
    let cavity = cavity_of(&part);
    let mut h = hole([10.0, 0.0, 20.0], [0.0, -1.0, 0.0], 2.0);
    h.taper = 15.0;
    h.flare = 0.5;
    let applied = silent(|c| apply(&part, &cavity, &settings(), &[h], None, c)).unwrap();
    assert_sound(&applied.mesh);
    assert_eq!(euler(&applied.mesh), 2);
    // At the outer face the hole is the diameter, widened by the taper over the wall, and by the
    // flare: the rim's points on y = 0 lie that far from the axis.
    let want = 1.0 + T * 15f64.to_radians().tan() + 0.5;
    let rim = applied
        .mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[1].abs() < 1e-4)
        .map(|p| ((p[0] as f64 - 10.0).powi(2) + (p[2] as f64 - 20.0).powi(2)).sqrt())
        .filter(|&r| r < 5.0)
        .fold(0.0f64, f64::max);
    // The rounding is drawn with eight steps, closer near the rim: a few hundredths short there.
    assert!((rim - want).abs() < 0.08, "rim {rim}, want {want}");
}

#[test]
fn auto_holes_land_at_the_bottom_corners_of_the_cavity() {
    let part = cube();
    let cavity = cavity_of(&part);
    let scene = Scene::new(&part, Some(&cavity), settings());
    let template = hole([0.0; 3], [0.0, 0.0, -1.0], 3.0);
    let holes = auto_holes(&scene, 2, &template);
    assert_eq!(holes.len(), 2, "{holes:?}");
    let corners = [[2.0, 2.0], [2.0, 38.0], [38.0, 2.0], [38.0, 38.0]];
    let mut used = Vec::new();
    for h in &holes {
        // On the bottom face, pointing down, under a corner of the cavity's floor, the whole hole
        // inside the floor.
        assert!(h.at[2].abs() < 1e-4, "{h:?}");
        assert!(dot(h.normal, [0.0, 0.0, -1.0]) > 0.999, "{h:?}");
        let (k, d) = corners
            .iter()
            .enumerate()
            .map(|(k, c)| {
                (
                    k,
                    ((h.at[0] - c[0]).powi(2) + (h.at[1] - c[1]).powi(2)).sqrt(),
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(d < 3.5, "{h:?} is {d} mm from the nearest corner");
        for axis in 0..2 {
            assert!(
                h.at[axis] >= 2.0 + 1.5 - 0.05 && h.at[axis] <= 38.0 - 1.5 + 0.05,
                "{h:?}"
            );
        }
        used.push(k);
    }
    // Spread apart: opposite corners.
    assert_ne!(used[0], used[1]);
    assert!(exact::length(sub(holes[0].at, holes[1].at)) > 40.0);
    let applied = silent(|c| apply(&part, &cavity, &settings(), &holes, None, c)).unwrap();
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    assert_sound(&applied.mesh);
    // Two openings: one handle.
    assert_eq!(euler(&applied.mesh), 0);
}

#[test]
fn a_hole_on_a_solid_area_or_running_into_another_is_refused() {
    let part = cube();
    let cavity = cavity_of(&part);
    let scene = Scene::new(&part, Some(&cavity), settings());
    // Under the cube's edge, x and y within the 2 mm wall: only solid wall above.
    let solid = [hole([1.0, 1.0, 0.0], [0.0, 0.0, -1.0], 1.0)];
    let check = check_hole(&scene, &solid, 0);
    assert!(!check.ok);
    assert!(
        check
            .reason
            .unwrap()
            .starts_with("No cavity lies behind this point")
    );
    // A part with no cavity at all.
    let plate = make_box([0.0; 3], [40.0, 40.0, 3.0]);
    let none = Scene::new(&plate, None, settings());
    assert!(
        check_hole(&none, &[hole([20.0, 20.0, 0.0], [0.0, 0.0, -1.0], 2.0)], 0)
            .reason
            .unwrap()
            .starts_with("There is no cavity yet")
    );
    // Two holes nearly on top of each other: the second is refused, the first kept.
    let pair = [
        hole([20.0, 20.0, 0.0], [0.0, 0.0, -1.0], 3.0),
        hole([22.0, 20.0, 0.0], [0.0, 0.0, -1.0], 3.0),
    ];
    assert!(check_hole(&scene, &pair, 0).ok);
    assert!(
        check_hole(&scene, &pair, 1)
            .reason
            .unwrap()
            .contains("hole 1")
    );
    let applied = silent(|c| apply(&part, &cavity, &settings(), &pair, None, c)).unwrap();
    assert_eq!(applied.skipped.len(), 1);
    assert_eq!(applied.skipped[0].0, 1);
    assert_eq!(euler(&applied.mesh), 2);
    // A bad diameter is a sentence too.
    assert!(
        check_hole(&scene, &[hole([20.0, 20.0, 0.0], [0.0, 0.0, -1.0], 0.1)], 0)
            .reason
            .unwrap()
            .starts_with("Enter a hole diameter")
    );
}

#[test]
fn a_plug_fits_its_hole_with_the_clearance() {
    let part = cube();
    let cavity = cavity_of(&part);
    let holes = [hole([20.0, 20.0, 0.0], [0.0, 0.0, -1.0], 3.0)];
    let clearance = 0.2;
    let applied =
        silent(|c| apply(&part, &cavity, &settings(), &holes, Some(clearance), c)).unwrap();
    assert_eq!(applied.plugs.len(), 1, "{:?}", applied.warnings);
    let plug = &applied.plugs[0];
    assert_sound(plug);
    assert_eq!(stats(plug).shells, 1);
    // A cylinder 2.6 mm across through the 2 mm floor.
    let polygon = 0.5 * 48.0 * (2.0 * PI / 48.0).sin() * 1.3 * 1.3;
    assert!(
        (stats(plug).volume - polygon * T).abs() < 0.03 * polygon * T,
        "{}",
        stats(plug).volume
    );
    // Nothing shared: no triangle of the plug crosses the part, and the plug lies outside the
    // part's solid (its middle is in the hole), the clearance all round.
    let both = crate::ops::concat(&[&applied.mesh, plug]).unwrap();
    let first_plug = applied.mesh.triangle_count() as u32;
    let pairs = silent(|c| tritri::intersecting_pairs(&both, c)).unwrap();
    assert!(
        pairs
            .iter()
            .all(|&(a, b)| (a < first_plug) == (b < first_plug)),
        "the plug crosses the part"
    );
    let holed = Solid::new(&applied.mesh);
    assert!(!holed.inside([20.0, 20.0, 1.0]));
    let gap = plug
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((p[0] as f64 - 20.0).powi(2) + (p[1] as f64 - 20.0).powi(2)).sqrt())
        .fold(0.0f64, f64::max);
    assert!((1.5 - gap - clearance).abs() < 1e-3, "plug radius {gap}");
}

#[test]
fn settings_out_of_range_are_sentences() {
    let bad = HollowSettings {
        thickness: 0.0,
        ..settings()
    };
    assert!(bad.problem().unwrap().contains("wall thickness"));
    let rough = HollowSettings {
        smoothness: 4,
        ..settings()
    };
    assert!(rough.problem().is_some());
    assert!(settings().problem().is_none());
}

/// Holes of every size, taper and flare, anywhere on a hollow sphere (a curved outside and a
/// curved cavity, faceted differently): each one checked as fine cuts cleanly.
#[test]
fn holes_anywhere_on_a_sphere_cut_cleanly() {
    let params = serde_json::json!({"radius": 20.0, "segments": 64, "rings": 32});
    let part = crate::library::generate("sphere", params.as_object().unwrap()).unwrap();
    let s = HollowSettings {
        thickness: 2.5,
        accuracy: 0.5,
        smoothness: 2,
    };
    let cavity = silent(|c| cavity(&part, &s, c)).unwrap().mesh.unwrap();
    let scene = Scene::new(&part, Some(&cavity), s);
    let b = crate::mesh::bounds(&part).unwrap();
    let mid: P3 = [0, 1, 2].map(|k| (b[0][k] as f64 + b[1][k] as f64) / 2.0);
    // A fixed sequence of pseudo-random numbers in 0..1.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut cut_count = 0;
    for _ in 0..10 {
        let (theta, phi) = (std::f64::consts::TAU * next(), (2.0 * next() - 1.0).acos());
        let n = [phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos()];
        // Onto the faceted surface: where the ray from the centre leaves it.
        let (f, t, _) = scene
            .part
            .hits(mid, add(mid, scale(n, 30.0)))
            .into_iter()
            .find(|h| h.2 > 0.0)
            .unwrap();
        let h = Hole {
            at: add(mid, scale(n, 30.0 * f)),
            normal: scene.part.unit_normal(t),
            diameter: 1.0 + 4.0 * next(),
            taper: 20.0 * next(),
            flare: 0.6 * next(),
        };
        if !check_hole(&scene, &[h], 0).ok {
            continue;
        }
        let applied = silent(|c| apply(&part, &cavity, &s, &[h], Some(0.15), c))
            .unwrap_or_else(|e| panic!("{h:?}: {e:?}"));
        assert_sound(&applied.mesh);
        assert_eq!(euler(&applied.mesh), 2, "{h:?}");
        assert_eq!(applied.plugs.len(), 1, "{h:?}: {:?}", applied.warnings);
        cut_count += 1;
    }
    assert!(
        cut_count >= 8,
        "only {cut_count} holes were checked as fine"
    );
}
