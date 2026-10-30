//! Known answers for Align, Duplicate, Lay flat, Rotate to smallest box and Orient.

use super::align::{Align, align};
use super::duplicate::{Copies, Duplicate, GridAxis, Spacing, duplicate};
use super::orient::{BoxAxes, Goal, Turn, lay_flat_at, lay_flat_auto, min_box, orient};
use super::*;
use crate::mesh::{Matrix, make_box};
use crate::repair::silent;

/// The 4x4 matrix of rotation `r` and move `t`.
fn matrix(r: &Rot, t: V3) -> Matrix {
    let mut m = [0.0; 16];
    for row in 0..3 {
        for col in 0..3 {
            m[col * 4 + row] = r[row][col];
        }
        m[12 + row] = t[row];
    }
    m[15] = 1.0;
    m
}

fn rot_x(a: f64) -> Rot {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(a: f64) -> Rot {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// A 20 × 10 × 5 box turned some way that lines up with nothing, and moved off the origin.
fn tilted_box() -> (crate::mesh::Mesh, Matrix) {
    let r = compose(&rot_z(0.83), &compose(&rot_y(0.41), &rot_x(1.17)));
    (
        make_box([0.0; 3], [20.0, 10.0, 5.0]),
        matrix(&r, [60.0, 70.0, 30.0]),
    )
}

fn world_box(mesh: &crate::mesh::Mesh, m: &Matrix) -> Aabb {
    World::new(mesh, m).bounds().unwrap()
}

fn assert_near(got: V3, want: V3, tol: f64, what: &str) {
    for k in 0..3 {
        assert!(
            (got[k] - want[k]).abs() <= tol,
            "{what}: got {got:?}, want {want:?}"
        );
    }
}

#[test]
fn rotate_to_smallest_box_brings_a_turned_box_back_to_20_by_10_by_5() {
    let (mesh, m) = tilted_box();
    let part = World::new(&mesh, &m);
    let before = part.bounds().unwrap();
    let turn = min_box(&part, BoxAxes::All).unwrap();
    let after = world_box(&mesh, &turn.apply(&m));
    assert_near(size(&after), [20.0, 10.0, 5.0], 1e-6, "box size");
    // It keeps its height above the platform and its place.
    assert!((after[0][2] - before[0][2]).abs() < 1e-9);
    assert_near(
        [centre(&after)[0], centre(&after)[1], 0.0],
        [centre(&before)[0], centre(&before)[1], 0.0],
        1e-9,
        "centre",
    );
}

#[test]
fn rotate_to_smallest_box_about_z_only_keeps_the_part_standing() {
    let mesh = make_box([0.0; 3], [20.0, 10.0, 5.0]);
    for angle in [0.4, -1.1, 2.9, 0.0] {
        let m = matrix(&rot_z(angle), [100.0, 100.0, 0.0]);
        let turn = min_box(&World::new(&mesh, &m), BoxAxes::Z).unwrap();
        let after = world_box(&mesh, &turn.apply(&m));
        assert_near(size(&after), [20.0, 10.0, 5.0], 1e-9, "box size");
        assert!(after[0][2].abs() < 1e-12, "stays on the platform");
        // A box lined up already does not turn.
        if angle == 0.0 {
            assert_eq!(turn.rotation, rot_z(0.0));
        }
    }
    // A box standing on its end stays on its end.
    let m = matrix(
        &compose(&rot_z(0.3), &rot_x(std::f64::consts::FRAC_PI_2)),
        [0.0; 3],
    );
    let after = world_box(
        &mesh,
        &min_box(&World::new(&mesh, &m), BoxAxes::Z)
            .unwrap()
            .apply(&m),
    );
    assert_near(size(&after), [20.0, 5.0, 10.0], 1e-9, "on its end");
}

/// The box's six faces in its own frame: outward normal and one corner on it.
fn faces() -> [(V3, V3); 6] {
    [
        ([0.0, 0.0, -1.0], [0.0; 3]),
        ([0.0, 0.0, 1.0], [0.0, 0.0, 5.0]),
        ([0.0, -1.0, 0.0], [0.0; 3]),
        ([0.0, 1.0, 0.0], [0.0, 10.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0; 3]),
        ([1.0, 0.0, 0.0], [20.0, 0.0, 0.0]),
    ]
}

#[test]
fn lay_flat_puts_the_clicked_face_exactly_on_the_platform_facing_down() {
    let (mesh, m) = tilted_box();
    let part = World::new(&mesh, &m);
    let r = compose(&rot_z(0.83), &compose(&rot_y(0.41), &rot_x(1.17)));
    for (n, p) in faces() {
        let normal = apply(&r, n);
        let point = add(apply(&r, p), [60.0, 70.0, 30.0]);
        let turn = lay_flat_at(&part, point, normal).unwrap();
        let after = World::new(&mesh, &turn.apply(&m));
        // The face's normal now points straight down...
        let turned = apply(&compose(&turn.rotation, &r), n);
        assert_near(turned, [0.0, 0.0, -1.0], 1e-12, "normal");
        // ...and all four of its corners are at Z = 0, the lowest of the part.
        let on_face: Vec<V3> = after
            .points
            .iter()
            .zip(&part.points)
            .filter(|(_, before)| dot(normal, sub(**before, point)).abs() < 1e-9)
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(on_face.len(), 4, "{n:?}");
        for p in &on_face {
            assert!(p[2].abs() < 1e-9, "{n:?}: corner at z = {}", p[2]);
        }
        assert!(after.bounds().unwrap()[0][2].abs() < 1e-9);
    }
}

#[test]
fn lay_flat_automatically_picks_the_largest_flat_face() {
    let (mesh, m) = tilted_box();
    let turn = lay_flat_auto(&World::new(&mesh, &m)).unwrap();
    let after = world_box(&mesh, &turn.apply(&m));
    assert!((size(&after)[2] - 5.0).abs() < 1e-9, "{:?}", size(&after));
    assert!(after[0][2].abs() < 1e-9);

    // A step: a 60 × 10 × 4 slab on a 40 × 10 × 4 one. The slab's 600 mm² underside is the
    // largest flat face found first, but the narrow box is below it, so the part cannot rest on
    // it; it rests on the slab's top (also 600 mm²) instead, upside down.
    let narrow = make_box([0.0; 3], [40.0, 10.0, 4.0]);
    let wide = make_box([0.0, 0.0, 4.0], [60.0, 10.0, 8.0]);
    let step = crate::ops::concat(&[&narrow, &wide]).unwrap();
    let m = matrix(&rot_x(2.2), [0.0, 0.0, 50.0]);
    let turn = lay_flat_auto(&World::new(&step, &m)).unwrap();
    let after = World::new(&step, &turn.apply(&m));
    let b = after.bounds().unwrap();
    assert!(b[0][2].abs() < 1e-9);
    assert!((size(&b)[2] - 8.0).abs() < 1e-9, "{:?}", size(&b));
    // The narrow box's underside (vertex 0, at z = 0 before) is now on top.
    assert!(
        (after.points[0][2] - 8.0).abs() < 1e-9,
        "{:?}",
        after.points[0]
    );

    // A torus has no flat face: it lies flat on its hull's disc, not on its rim, from any start.
    let ring = torus_mesh(48, 24, 15.0, 5.0);
    for r in [rot_x(1.3), compose(&rot_y(0.7), &rot_x(2.9))] {
        let m = matrix(&r, [0.0, 0.0, 40.0]);
        let turn = lay_flat_auto(&World::new(&ring, &m)).unwrap();
        let b = world_box(&ring, &turn.apply(&m));
        assert!((size(&b)[2] - 10.0).abs() < 0.05, "{:?}", size(&b));
    }
}

/// A closed torus around Z on z = 0: `n` × `m` quads.
fn torus_mesh(n: usize, m: usize, major: f64, minor: f64) -> crate::mesh::Mesh {
    let mut positions = Vec::new();
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
    let mut indices = Vec::new();
    for i in 0..n {
        for j in 0..m {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    crate::mesh::Mesh { positions, indices }
}

#[test]
fn orient_ranks_the_ways_to_stand_by_the_chosen_goal() {
    let (mesh, m) = tilted_box();
    let part = World::new(&mesh, &m);
    let rank = |goal| silent(|c| orient(&part, goal, c)).unwrap();

    let (now, best) = rank(Goal::LowestHeight);
    assert!(now.height > 5.0 + 1.0);
    assert!((best[0].scores.height - 5.0).abs() < 1e-9);
    assert!(best[0].scores.support_area.abs() < 1e-9);
    // The three ways a box stands, each once.
    assert_eq!(best.len(), 3);
    let heights: Vec<f64> = best.iter().map(|c| c.scores.height).collect();
    for (h, want) in heights.iter().zip([5.0, 10.0, 20.0]) {
        assert!((h - want).abs() < 1e-9, "{heights:?}");
    }

    let (_, best) = rank(Goal::SmallestFootprint);
    assert!((best[0].scores.footprint - 50.0).abs() < 1e-6);
    assert!((best[0].scores.height - 20.0).abs() < 1e-9);

    // Standing as it is, the tilted box needs support under its lower faces.
    assert!(now.support_area > 0.0);
    let (_, best) = rank(Goal::LeastSupport);
    assert!(best[0].scores.support_area.abs() < 1e-9);

    // A candidate's turn stands the part that way, on the platform, squared to the axes: the
    // box's sides are the part's own, longest along X.
    for c in &best {
        let b = world_box(&mesh, &c.turn.apply(&m));
        assert!(b[0][2].abs() < 1e-9);
        assert!((size(&b)[2] - c.scores.height).abs() < 1e-9);
        let s = size(&b);
        let volume = s[0] * s[1] * s[2];
        assert!((volume - 1000.0).abs() < 1e-6, "not squared: {s:?}");
        assert!(s[0] >= s[1] - 1e-9);
    }
}

#[test]
fn orient_counts_overhangs_as_support_area() {
    // A T: a 30 × 10 × 4 bar on a 10 × 10 × 20 post. Upright, the bar's underside either side of
    // the post (2 × 10 × 10 mm²) needs support; upside down, nothing does.
    let post = make_box([10.0, 0.0, 0.0], [20.0, 10.0, 20.0]);
    let bar = make_box([0.0, 0.0, 20.0], [30.0, 10.0, 24.0]);
    let t = crate::ops::concat(&[&post, &bar]).unwrap();
    let part = World::new(&t, &crate::mesh::IDENTITY);
    let (now, best) = silent(|c| orient(&part, Goal::LeastSupport, c)).unwrap();
    // The mesh is two boxes, so the bar's whole underside counts, also where it sits on the post:
    // 300 mm². Lying on its side (all faces upright or on the platform) it needs none.
    assert!((now.support_area - 300.0).abs() < 1e-9, "{now:?}");
    assert!(best[0].scores.support_area.abs() < 1e-9, "{:?}", best[0]);
}

#[test]
fn align_lines_parts_up_exactly() {
    let mesh = make_box([0.0; 3], [10.0, 20.0, 5.0]);
    let small = make_box([-2.0; 3], [2.0; 3]);
    let parts = [
        (mesh.clone(), matrix(&rot_z(0.0), [13.25, 40.5, 0.0])),
        (small.clone(), matrix(&rot_z(0.7), [100.125, -3.75, 22.5])),
        (mesh.clone(), matrix(&rot_x(0.5), [-7.0, 61.0, 9.0])),
    ];
    let boxes: Vec<Aabb> = parts.iter().map(|(m, x)| world_box(m, x)).collect();
    let all = union_all(&boxes).unwrap();
    let platform = [200.0, 180.0, 150.0];
    for how in [
        Align::Left,
        Align::Right,
        Align::Front,
        Align::Back,
        Align::Bottom,
        Align::Top,
        Align::CentreX,
        Align::CentreY,
        Align::CentreZ,
        Align::PlatformCentre,
        Align::Origin,
        Align::Drop,
    ] {
        let moves = align(&boxes, how, platform);
        let after: Vec<Aabb> = parts
            .iter()
            .zip(&moves)
            .map(|((m, x), d)| world_box(m, &translated(x, *d)))
            .collect();
        let group = union_all(&after).unwrap();
        for (b, before) in after.iter().zip(&boxes) {
            let c = centre(b);
            let want = match how {
                Align::Left => b[0][0] - all[0][0],
                Align::Right => b[1][0] - all[1][0],
                Align::Front => b[0][1] - all[0][1],
                Align::Back => b[1][1] - all[1][1],
                Align::Bottom => b[0][2] - all[0][2],
                Align::Top => b[1][2] - all[1][2],
                Align::CentreX => c[0] - centre(&all)[0],
                Align::CentreY => c[1] - centre(&all)[1],
                Align::CentreZ => c[2] - centre(&all)[2],
                Align::PlatformCentre => {
                    (centre(&group)[0] - 100.0).abs()
                        + (centre(&group)[1] - 90.0).abs()
                        + (b[0][2] - before[0][2]).abs()
                }
                Align::Origin => group[0].iter().map(|v| v.abs()).sum(),
                Align::Drop => b[0][2],
            };
            assert!(want.abs() < 1e-3, "{how:?}: off by {want}");
            // Only the axis aligned moves (the platform moves aside).
            if how.relative() {
                let axis = match how {
                    Align::Left | Align::Right | Align::CentreX => 0,
                    Align::Front | Align::Back | Align::CentreY => 1,
                    _ => 2,
                };
                for k in (0..3).filter(|&k| k != axis) {
                    assert!(
                        (b[0][k] - before[0][k]).abs() < 1e-9,
                        "{how:?} moved axis {k}"
                    );
                }
            }
        }
    }
}

fn axis(count: u32, gap: f64) -> GridAxis {
    GridAxis {
        count,
        gap,
        negative: false,
    }
}

#[test]
fn duplicate_in_a_grid_spaces_copies_exactly() {
    let group = [[10.0, 20.0, 0.0], [30.0, 30.0, 5.0]];
    let how = Duplicate::Grid {
        axes: [axis(3, 2.5), axis(2, 1.0), axis(1, 0.0)],
        spacing: Spacing::Boxes,
    };
    let Copies {
        group: moved,
        offsets,
    } = duplicate(&group, &[], [200.0; 3], &how).unwrap();
    assert_eq!(moved, [0.0; 3]);
    // 3 × 2 places, the originals in the first: 5 copies, 22.5 mm apart in X, 11 mm in Y.
    assert_eq!(
        offsets,
        vec![
            [22.5, 0.0, 0.0],
            [45.0, 0.0, 0.0],
            [0.0, 11.0, 0.0],
            [22.5, 11.0, 0.0],
            [45.0, 11.0, 0.0]
        ]
    );
    // Towards negative X, spaced by origins.
    let how = Duplicate::Grid {
        axes: [
            GridAxis {
                count: 2,
                gap: 7.0,
                negative: true,
            },
            axis(1, 0.0),
            axis(3, 6.0),
        ],
        spacing: Spacing::Origins,
    };
    let c = duplicate(&group, &[], [200.0; 3], &how).unwrap();
    assert_eq!(
        c.offsets,
        vec![
            [-7.0, 0.0, 0.0],
            [0.0, 0.0, 6.0],
            [-7.0, 0.0, 6.0],
            [0.0, 0.0, 12.0],
            [-7.0, 0.0, 12.0]
        ]
    );
    assert!(
        duplicate(
            &group,
            &[],
            [200.0; 3],
            &Duplicate::Grid {
                axes: [axis(1, 0.0), axis(1, 0.0), axis(1, 0.0)],
                spacing: Spacing::Boxes
            }
        )
        .is_err()
    );
}

#[test]
fn duplicate_in_place_stacks_copies_on_the_originals() {
    let c = duplicate(
        &[[0.0; 3], [1.0; 3]],
        &[],
        [200.0; 3],
        &Duplicate::InPlace { copies: 4 },
    )
    .unwrap();
    assert_eq!(c.offsets, vec![[0.0; 3]; 4]);
    assert!(
        duplicate(
            &[[0.0; 3], [1.0; 3]],
            &[],
            [200.0; 3],
            &Duplicate::InPlace { copies: 0 }
        )
        .is_err()
    );
}

#[test]
fn fill_the_platform_makes_as_many_as_the_arithmetic_says() {
    // A 20 × 10 × 5 box, 5 mm apart, 2 mm from the sides of a 200 × 200 × 200 platform:
    // floor((196 + 5) / 25) = 8 along X, floor((196 + 5) / 15) = 13 along Y.
    let group = [[50.0, 60.0, 0.0], [70.0, 70.0, 5.0]];
    let fill = |stack, limit, others: &[Aabb]| {
        duplicate(
            &group,
            others,
            [200.0; 3],
            &Duplicate::Fill {
                spacing: 5.0,
                wall: 2.0,
                limit,
                stack,
            },
        )
        .unwrap()
    };
    let flat = fill(false, 10_000, &[]);
    assert_eq!(flat.offsets.len() + 1, 8 * 13);
    // The originals move to the front-left corner, 2 mm from the sides.
    assert_eq!(flat.group, [2.0 - 50.0, 2.0 - 60.0, 0.0]);
    // Every place is 25 mm or 15 mm on from the last, and none leaves the platform.
    for o in &flat.offsets {
        let x = 50.0 + o[0] - 2.0;
        let y = 60.0 + o[1] - 2.0;
        assert_eq!(x % 25.0, 0.0);
        assert_eq!(y % 15.0, 0.0);
        assert!(50.0 + o[0] + 20.0 <= 198.0 && 60.0 + o[1] + 10.0 <= 198.0);
    }
    // In layers: floor((200 + 5) / 10) = 20 of them.
    assert_eq!(fill(true, 10_000, &[]).offsets.len() + 1, 8 * 13 * 20);
    // A limit, and a part in the way.
    assert_eq!(fill(false, 7, &[]).offsets.len(), 7);
    let blocker = [[0.0, 0.0, 0.0], [60.0, 40.0, 30.0]];
    let around = fill(false, 10_000, &[blocker]);
    // A place is free when it starts 5 mm or more past the blocker in X or in Y.
    let free = |x0: f64, y0: f64| !(x0 - 5.0 < 60.0 && y0 - 5.0 < 40.0);
    let expected = (0..13)
        .flat_map(|j| (0..8).map(move |i| (2.0 + 25.0 * i as f64, 2.0 + 15.0 * j as f64)))
        .filter(|&(x, y)| free(x, y))
        .count();
    assert_eq!(around.offsets.len() + 1, expected);
}

#[test]
fn turns_are_proper_rotations() {
    let (mesh, m) = tilted_box();
    let part = World::new(&mesh, &m);
    let turns: Vec<Turn> = [
        min_box(&part, BoxAxes::All),
        min_box(&part, BoxAxes::Z),
        lay_flat_auto(&part),
    ]
    .into_iter()
    .flatten()
    .collect();
    for t in turns {
        let r = t.rotation;
        let det = dot(r[0], cross(r[1], r[2]));
        assert!((det - 1.0).abs() < 1e-12, "{r:?}");
        for row in r {
            assert!((len(row) - 1.0).abs() < 1e-12);
        }
        // The part keeps its shape: a turned box is still a box of the same volume.
        let after = World::new(&mesh, &t.apply(&m));
        let (v, _) = crate::mesh::volume_and_area(&crate::mesh::bake(&mesh, &t.apply(&m)));
        assert!((v - 1000.0).abs() < 1e-2, "{v}");
        assert_eq!(after.points.len(), 8);
    }
}

// ---------------------------------------------------------------------------------------------
// Packing

use super::collide::{Body, collide};
use super::pack::{Item, Method, Outcome, Rotation, pack};

fn item(mesh: &crate::mesh::Mesh, m: Matrix, priority: i32) -> Item {
    Item {
        world: World::new(mesh, &m),
        priority,
        rotation: Rotation::Default,
    }
}

fn run_pack(items: &[Item], obstacles: &[World], platform: V3, method: &Method) -> Outcome {
    silent(|c| pack(items, obstacles, platform, method, c))
        .unwrap()
        .unwrap()
}

fn placed(items: &[Item], out: &Outcome) -> Vec<World> {
    items
        .iter()
        .zip(&out.turns)
        .map(|(it, t)| super::orient::moved(&it.world, t))
        .collect()
}

fn bounding_box(distance: f64, stack: bool) -> Method {
    Method::BoundingBox {
        distance,
        voxel: 5.0,
        wall: 0.0,
        floor: 0.0,
        ceiling: 0.0,
        stack,
    }
}

#[test]
fn bounding_box_packs_eight_50_mm_cubes_in_one_layer_exactly() {
    let cube = make_box([0.0; 3], [50.0; 3]);
    // Scattered, some floating.
    let items: Vec<Item> = (0..8)
        .map(|i| {
            item(
                &cube,
                matrix(
                    &IDENTITY_ROT,
                    [300.0 + 60.0 * i as f64, -80.0, 7.0 * i as f64],
                ),
                5,
            )
        })
        .collect();
    for stack in [false, true] {
        let out = run_pack(
            &items,
            &[],
            [200.0, 200.0, 200.0],
            &bounding_box(1.0, stack),
        );
        assert!(out.fits.iter().all(|&f| f));
        let mut corners: Vec<[f64; 3]> = placed(&items, &out)
            .iter()
            .map(|w| w.bounds().unwrap()[0])
            .collect();
        corners.sort_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])));
        // Three to a row (4 × 50 + 3 × 1 > 200), each exactly 1 mm from the next, one layer.
        let want = [
            [0.0, 0.0],
            [51.0, 0.0],
            [102.0, 0.0],
            [0.0, 51.0],
            [51.0, 51.0],
            [102.0, 51.0],
            [0.0, 102.0],
            [51.0, 102.0],
        ];
        for (c, w) in corners.iter().zip(want) {
            assert!(
                (c[0] - w[0]).abs() < 1e-9 && (c[1] - w[1]).abs() < 1e-9,
                "{corners:?}"
            );
            assert!(c[2].abs() < 1e-9, "one layer: {corners:?}");
        }
        assert!((out.height - 50.0).abs() < 1e-9);
    }
    // A distance of 0 still keeps the 0.05 mm clearance, since parts touching face to face would
    // collide: three to a row on 200 mm, four on 201 mm.
    for (width, row) in [(200.0, 3), (201.0, 4)] {
        let out = run_pack(
            &items,
            &[],
            [width, 200.0, 200.0],
            &bounding_box(0.0, false),
        );
        let worlds = placed(&items, &out);
        let first_row = worlds
            .iter()
            .filter(|w| w.bounds().unwrap()[0][1] == 0.0)
            .count();
        assert_eq!(first_row, row, "{width} mm");
        assert_no_collisions(&items, &out, &[], [width, 200.0, 200.0], "distance 0");
    }
}

#[test]
fn bounding_box_in_2d_puts_what_does_not_fit_beside_the_platform() {
    let cube = make_box([0.0; 3], [50.0; 3]);
    let items: Vec<Item> = (0..20)
        .map(|_| item(&cube, crate::mesh::IDENTITY, 5))
        .collect();
    let out = run_pack(
        &items,
        &[],
        [200.0, 200.0, 200.0],
        &bounding_box(1.0, false),
    );
    // 3 × 3 fit in one layer; the rest go past the platform's +X side, on Z = 0, apart.
    assert_eq!(out.fits.iter().filter(|&&f| f).count(), 9);
    let worlds = placed(&items, &out);
    for (w, fits) in worlds.iter().zip(&out.fits) {
        let b = w.bounds().unwrap();
        if *fits {
            assert!(b[1][0] <= 200.0 && b[1][1] <= 200.0);
        } else {
            assert!(b[0][0] > 200.0 && b[0][2].abs() < 1e-9);
        }
    }
    // Stacked, all 20 fit: 9 a layer, three layers.
    let out = run_pack(&items, &[], [200.0, 200.0, 200.0], &bounding_box(1.0, true));
    assert!(out.fits.iter().all(|&f| f));
    assert!((out.height - 152.0).abs() < 1e-9);
}

#[test]
fn higher_priority_packs_first_and_lowest() {
    let small = make_box([0.0; 3], [20.0; 3]);
    let big = make_box([0.0; 3], [60.0; 3]);
    let items = vec![
        item(&big, matrix(&IDENTITY_ROT, [0.0, 0.0, 50.0]), 2),
        item(&small, matrix(&IDENTITY_ROT, [0.0, 0.0, 50.0]), 9),
        item(&big, matrix(&IDENTITY_ROT, [0.0, 0.0, 50.0]), 2),
    ];
    let out = run_pack(&items, &[], [100.0, 100.0, 300.0], &bounding_box(1.0, true));
    let worlds = placed(&items, &out);
    // The small one, priority 9, takes the first corner although it is smallest.
    assert_eq!(worlds[1].bounds().unwrap()[0], [0.0; 3]);
    for method in [mc(5, 1), scan3d(5)] {
        let out = run_pack(&items, &[], [100.0, 100.0, 300.0], &method);
        let worlds = placed(&items, &out);
        let low = |w: &World| w.bounds().unwrap()[0][2];
        assert!(low(&worlds[1]) < 1e-9, "{method:?}");
        assert!(low(&worlds[1]) <= low(&worlds[0]) && low(&worlds[1]) <= low(&worlds[2]));
    }
}

fn mc(performance: u32, seed: u64) -> Method {
    Method::MonteCarlo {
        performance,
        distance: 1.0,
        rotation: Rotation::Z,
        wall: 0.0,
        floor: 0.0,
        ceiling: 0.0,
        seed,
    }
}

fn scan3d(performance: u32) -> Method {
    Method::Scanline3d {
        performance,
        voxel: 2.0,
        z_steps: 4,
        flip: true,
        wall: 0.0,
        floor: 0.0,
        ceiling: 0.0,
    }
}

fn scan2d() -> Method {
    Method::Scanline2d {
        performance: 3,
        align: true,
        z_steps: 12,
        wall: 4.0,
        spacing: 1,
        voxel: 1.0,
    }
}

/// An L, a T and a block, turned and scattered, four of each.
fn mixed() -> Vec<Item> {
    let foot = make_box([0.0; 3], [40.0, 10.0, 4.0]);
    let post = make_box([0.0, 0.0, 4.0], [4.0, 10.0, 30.0]);
    let l = crate::ops::concat(&[&foot, &post]).unwrap();
    let bar = make_box([0.0, 0.0, 20.0], [30.0, 10.0, 24.0]);
    let stem = make_box([10.0, 0.0, 0.0], [20.0, 10.0, 20.0]);
    let t = crate::ops::concat(&[&bar, &stem]).unwrap();
    let block = make_box([0.0; 3], [25.0, 15.0, 8.0]);
    let mut out = Vec::new();
    for k in 0..4 {
        let r = compose(&rot_z(0.4 * k as f64), &rot_x(0.3 * k as f64));
        let at = [20.0 * k as f64, 300.0, 10.0];
        out.push(item(&l, matrix(&r, at), 5));
        out.push(item(&t, matrix(&r, at), 5));
        out.push(item(&block, matrix(&r, at), 5));
    }
    out
}

fn assert_no_collisions(
    items: &[Item],
    out: &Outcome,
    obstacles: &[World],
    platform: V3,
    what: &str,
) {
    let worlds = placed(items, out);
    let mut bodies: Vec<Body> = worlds.iter().cloned().map(Body::new).collect();
    bodies.extend(obstacles.iter().cloned().map(Body::new));
    for i in 0..bodies.len() {
        for j in i + 1..bodies.len() {
            assert!(
                !collide(&bodies[i], &bodies[j]),
                "{what}: {i} and {j} collide"
            );
        }
    }
    for (w, f) in worlds.iter().zip(&out.fits) {
        let b = w.bounds().unwrap();
        if *f {
            for k in 0..3 {
                assert!(
                    b[0][k] >= -1e-9 && b[1][k] <= platform[k] + 1e-9,
                    "{what}: outside {b:?}"
                );
            }
        } else {
            assert!(
                b[0][0] > platform[0],
                "{what}: a part that does not fit stays on the platform"
            );
        }
    }
}

#[test]
fn every_method_packs_without_collisions_round_a_locked_part() {
    let items = mixed();
    let platform = [120.0, 120.0, 120.0];
    let locked = World::new(
        &make_box([0.0; 3], [30.0, 30.0, 40.0]),
        &matrix(&IDENTITY_ROT, [45.0, 45.0, 0.0]),
    );
    let obstacles = vec![locked];
    for method in [
        bounding_box(1.0, true),
        bounding_box(1.0, false),
        scan2d(),
        scan3d(4),
        mc(6, 7),
    ] {
        let out = run_pack(&items, &obstacles, platform, &method);
        assert_no_collisions(&items, &out, &obstacles, platform, &format!("{method:?}"));
        assert!(
            out.fits.iter().filter(|&&f| f).count() >= 4,
            "{method:?}: {:?}",
            out.fits
        );
        if let Method::Scanline2d { .. } = method {
            // One layer: every part on the platform.
            for (w, f) in placed(&items, &out).iter().zip(&out.fits) {
                if *f {
                    assert!(w.bounds().unwrap()[0][2].abs() < 1e-9);
                }
            }
        }
    }
}

#[test]
fn monte_carlo_gives_the_same_result_for_the_same_seed() {
    let items = mixed();
    let platform = [150.0, 150.0, 150.0];
    let a = run_pack(&items, &[], platform, &mc(7, 42));
    let b = run_pack(&items, &[], platform, &mc(7, 42));
    assert_eq!(a, b);
    assert_no_collisions(&items, &a, &[], platform, "Monte Carlo");
    // Rotation per part is observed: a part that may not turn keeps its orientation.
    let mut fixed = mixed();
    fixed[0].rotation = Rotation::None;
    let c = run_pack(&fixed, &[], platform, &mc(7, 42));
    assert_eq!(c.turns[0].rotation, IDENTITY_ROT);
}

#[test]
fn packing_settings_out_of_range_are_refused() {
    let items = mixed();
    let bad = Method::Scanline2d {
        performance: 3,
        align: false,
        z_steps: 40,
        wall: 0.0,
        spacing: 1,
        voxel: 1.0,
    };
    let answer = silent(|c| pack(&items, &[], [100.0; 3], &bad, c)).unwrap();
    assert!(answer.is_err());
}

#[test]
fn a_cancelled_pack_stops_with_cancelled() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let items = mixed();
    for at in [0.0, 0.25, 0.6] {
        let cancel = AtomicBool::new(false);
        let mut control = crate::repair::Control::new(&cancel, |f| {
            if f >= at {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        let answer = pack(&items, &[], [150.0; 3], &mc(5, 1), &mut control);
        assert_eq!(
            answer.err(),
            Some(crate::repair::RepairError::Cancelled),
            "at {at}"
        );
    }
}
