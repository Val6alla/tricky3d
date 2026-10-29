//! Part library › Design: sample models of tricky3D's own, each built from simple solids that
//! overlap a little and are joined into one closed body by Wrap part surface. Sizes are in mm; a
//! Scale parameter sizes the whole model.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::build::{Builder, P, P2, circle_2d, extrude, revolve};
use super::primitive::{ellipsoid, frustum_of_cone};
use super::solid::union;
use super::{Category, Item, Polygon, Values, item, num};
use crate::mesh::{Matrix, Mesh, bake, make_box, multiply};
use crate::repair::{Control, RepairError, RepairResult};

const IDS: [&str; 8] = [
    "toy_car",
    "small_house",
    "toy_plane",
    "boat",
    "rocket",
    "chess_pawn",
    "bottle_opener",
    "robot",
];

pub(super) fn has(id: &str) -> bool {
    IDS.contains(&id)
}

pub(super) fn catalog() -> Vec<Item> {
    let sample = |id, label, description| {
        item(
            id,
            label,
            Category::Design,
            description,
            vec![num("scale", "Scale", "%", 100.0, 10.0, 1000.0)],
        )
    };
    vec![
        sample("toy_car", "Toy car", "A small car with four wheels."),
        sample(
            "small_house",
            "Small house",
            "A house with a pitched roof and a chimney.",
        ),
        sample(
            "toy_plane",
            "Toy plane",
            "A propeller plane with wings and a tail.",
        ),
        sample("boat", "Boat", "A little boat with a cabin and a mast."),
        sample("rocket", "Rocket", "A rocket on three fins."),
        sample("chess_pawn", "Chess pawn", "A pawn turned on a lathe."),
        sample(
            "bottle_opener",
            "Bottle opener",
            "A flat opener with a hook to catch the cap.",
        ),
        sample("robot", "Robot", "A boxy robot with an antenna."),
    ]
}

fn translate(t: P) -> Matrix {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0,
    ]
}

/// Rotation by `a` radians about the X axis.
fn about_x(a: f64) -> Matrix {
    let (s, c) = a.sin_cos();
    [
        1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// Rotation by `a` radians about the Y axis.
fn about_y(a: f64) -> Matrix {
    let (s, c) = a.sin_cos();
    [
        c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// Rotation by `a` radians about the Z axis.
fn about_z(a: f64) -> Matrix {
    let (s, c) = a.sin_cos();
    [
        c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn cuboid(min: P, max: P) -> Mesh {
    make_box(min.map(|v| v as f32), max.map(|v| v as f32))
}

/// A cylinder of `r` along X from x0 to x1 through (y, z).
fn rod_x(r: f64, x0: f64, x1: f64, y: f64, z: f64) -> Result<Mesh, String> {
    let c = frustum_of_cone(r, Some(r), x1 - x0, 32)?;
    Ok(bake(
        &c,
        &multiply(&translate([x0, y, z]), &about_y(FRAC_PI_2)),
    ))
}

/// A cylinder of `r` along Y from y0 to y1 through (x, z).
fn rod_y(r: f64, y0: f64, y1: f64, x: f64, z: f64) -> Result<Mesh, String> {
    let c = frustum_of_cone(r, Some(r), y1 - y0, 32)?;
    Ok(bake(
        &c,
        &multiply(&translate([x, y0, z]), &about_x(-FRAC_PI_2)),
    ))
}

/// A cylinder of `r` standing from z0 to z1 at (x, y).
fn rod_z(r: f64, z0: f64, z1: f64, x: f64, y: f64) -> Result<Mesh, String> {
    let c = frustum_of_cone(r, Some(r), z1 - z0, 32)?;
    Ok(bake(&c, &translate([x, y, z0])))
}

fn ball(r: [f64; 3], centre: P) -> Result<Mesh, String> {
    let e = ellipsoid(r, 48, 24)?;
    // `ellipsoid` stands on z = 0; move its middle to the centre.
    Ok(bake(
        &e,
        &translate([centre[0], centre[1], centre[2] - r[2]]),
    ))
}

/// A 2D outline (in X and Z) extruded along Y from y0 to y1.
fn profile_y(outline: Vec<P2>, y0: f64, y1: f64) -> Result<Mesh, String> {
    let mut b = Builder::default();
    extrude(&mut b, &[Polygon::new(outline, Vec::new())], y0, y1)?;
    let m = b.finish_in_place()?;
    // (x, z, y) → (x, y, z): a mirror, so the triangles turn.
    let mut out = m;
    for p in out.positions.as_chunks_mut::<3>().0 {
        *p = [p[0], p[2], p[1]];
    }
    for t in out.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    Ok(out)
}

pub(super) fn make(id: &str, v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let parts = match id {
        "toy_car" => car(),
        "small_house" => house(),
        "toy_plane" => plane(),
        "boat" => boat(),
        "rocket" => rocket(),
        "chess_pawn" => return pawn(v.get("scale").map_err(failed)? / 100.0).map_err(failed),
        "bottle_opener" => return opener(v.get("scale").map_err(failed)? / 100.0).map_err(failed),
        "robot" => robot(),
        _ => unreachable!("every sample has a builder"),
    }
    .map_err(failed)?;
    let joined = union(&parts, control)?;
    let s = v.get("scale").map_err(failed)? / 100.0;
    let mut b = Builder::default();
    b.append(&joined);
    for p in &mut b.points {
        *p = p.map(|c| c * s);
    }
    b.finish().map_err(failed)
}

fn car() -> Result<Vec<Mesh>, String> {
    let mut parts = vec![
        // The body, rounded at the front and back by its profile.
        profile_y(
            vec![
                [-21.0, 4.0],
                [21.0, 4.0],
                [22.0, 8.0],
                [20.0, 12.0],
                [-20.0, 12.0],
                [-22.0, 8.0],
            ],
            -9.0,
            9.0,
        )?,
        // The cabin, narrower and sloped.
        profile_y(
            vec![[-11.0, 11.5], [9.0, 11.5], [5.0, 19.0], [-8.0, 19.0]],
            -7.5,
            7.5,
        )?,
    ];
    for x in [-13.0, 13.0] {
        parts.push(rod_y(5.0, -10.5, -6.0, x, 5.0)?);
        parts.push(rod_y(5.0, 6.0, 10.5, x, 5.0)?);
        parts.push(rod_y(1.5, -8.0, 8.0, x, 5.0)?);
    }
    Ok(parts)
}

fn house() -> Result<Vec<Mesh>, String> {
    Ok(vec![
        cuboid([-15.0, -12.0, 0.0], [15.0, 12.0, 18.0]),
        // A pitched roof overhanging the walls.
        profile_y(
            vec![
                [-17.0, 17.0],
                [17.0, 17.0],
                [17.0, 18.5],
                [0.0, 30.0],
                [-17.0, 18.5],
            ],
            -14.0,
            14.0,
        )?,
        cuboid([6.0, -4.0, 20.0], [10.0, 0.0, 32.0]),
        // A doorstep and a door frame standing proud of the wall.
        cuboid([-4.0, -13.5, 0.0], [4.0, -11.5, 1.0]),
        cuboid([-3.5, -12.6, 1.0], [3.5, -11.5, 12.0]),
    ])
}

fn plane() -> Result<Vec<Mesh>, String> {
    let fuselage = ball([24.0, 4.5, 4.5], [0.0, 0.0, 8.0])?;
    Ok(vec![
        fuselage,
        // Wings, tail plane and fin.
        cuboid([-5.0, -24.0, 6.5], [5.0, 24.0, 8.0]),
        cuboid([-23.0, -9.0, 8.5], [-17.0, 9.0, 9.5]),
        profile_y(
            vec![[-24.0, 8.0], [-17.0, 8.0], [-21.0, 17.0], [-24.0, 17.0]],
            -0.6,
            0.6,
        )?,
        // The propeller hub and two blades.
        rod_x(1.6, 23.0, 26.0, 0.0, 8.0)?,
        cuboid([25.0, -0.8, 0.5], [26.0, 0.8, 15.5]),
        // Wheels under the wings.
        rod_y(2.2, -7.0, -5.0, 2.0, 2.2)?,
        rod_y(2.2, 5.0, 7.0, 2.0, 2.2)?,
        cuboid([1.2, -6.5, 2.0], [2.8, -5.5, 7.0]),
        cuboid([1.2, 5.5, 2.0], [2.8, 6.5, 7.0]),
    ])
}

fn boat() -> Result<Vec<Mesh>, String> {
    // The hull: cross-sections from stern to bow, narrowing to a point at the bow.
    let n = 24;
    let mut b = Builder::default();
    let mut rings: Vec<Vec<P>> = Vec::new();
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let x = -20.0 + 40.0 * t;
        let half = 9.0 * (1.0 - t.powi(3)).sqrt().max(0.0);
        let half = half.max(0.4);
        let depth = 5.0 + 1.5 * t;
        // A U-shaped section from deck edge round the bottom to the other deck edge.
        let section: Vec<P> = (0..12)
            .map(|i| {
                let a = PI * i as f64 / 11.0;
                [x, -half * a.cos(), 8.0 - depth * a.sin()]
            })
            .collect();
        rings.push(section);
    }
    b.stack(&rings, super::build::Cap::Flat, super::build::Cap::Flat);
    let hull = b.finish_in_place()?;
    Ok(vec![
        hull,
        cuboid([-12.0, -5.0, 7.5], [2.0, 5.0, 14.0]),
        cuboid([-13.0, -6.0, 13.5], [3.0, 6.0, 15.0]),
        rod_z(0.9, 7.0, 32.0, 8.0, 0.0)?,
        cuboid([8.0, -0.3, 14.0], [16.0, 0.3, 28.0]),
    ])
}

fn rocket() -> Result<Vec<Mesh>, String> {
    let body = revolve(
        &[
            (0.0, 0.0),
            (5.0, 0.0),
            (6.0, 2.0),
            (6.0, 30.0),
            (5.5, 36.0),
            (3.5, 42.0),
            (0.0, 46.0),
        ],
        48,
    )?;
    let mut parts = vec![body];
    for k in 0..3 {
        let fin = profile_y(
            vec![
                [4.0, 0.0],
                [14.0, 0.0],
                [12.0, 4.0],
                [5.0, 16.0],
                [4.0, 16.0],
            ],
            -0.8,
            0.8,
        )?;
        parts.push(bake(&fin, &about_z(TAU * k as f64 / 3.0)));
    }
    Ok(parts)
}

fn pawn(scale: f64) -> Result<Mesh, String> {
    let s = |v: f64| v * scale;
    let profile: Vec<(f64, f64)> = [
        (0.0, 0.0),
        (11.0, 0.0),
        (11.0, 2.5),
        (9.5, 4.0),
        (7.0, 5.0),
        (5.0, 9.0),
        (4.0, 15.0),
        (7.0, 16.0),
        (7.0, 17.5),
        (4.0, 18.5),
        (3.5, 20.0),
    ]
    .iter()
    .map(|&(r, z)| (s(r), s(z)))
    .chain((1..=16).map(|k| {
        // The head: a ball resting on the neck.
        let a = -PI / 2.0 + PI * (k as f64 / 16.0) + 0.43 * (1.0 - k as f64 / 16.0);
        let (r, z) = (6.5 * a.cos(), 26.0 + 6.5 * a.sin());
        (s(r.max(0.0)), s(z))
    }))
    .collect();
    let mut cleaned = profile;
    let last = cleaned.len() - 1;
    cleaned[last].0 = 0.0;
    revolve(&cleaned, 64)
}

fn opener(scale: f64) -> Result<Mesh, String> {
    let s = |p: P2| [p[0] * scale, p[1] * scale];
    // A handle with rounded ends: a stadium 90 × 26.
    let (len, w) = (90.0, 26.0);
    let mut outline: Vec<P2> = Vec::new();
    for k in 0..=24 {
        let a = -PI / 2.0 + PI * k as f64 / 24.0;
        outline.push(s([
            len / 2.0 - w / 2.0 + w / 2.0 * a.cos(),
            w / 2.0 * a.sin(),
        ]));
    }
    for k in 0..=24 {
        let a = PI / 2.0 + PI * k as f64 / 24.0;
        outline.push(s([
            -len / 2.0 + w / 2.0 + w / 2.0 * a.cos(),
            w / 2.0 * a.sin(),
        ]));
    }
    // The opening near one end, its far edge bent into a hook that catches under the cap.
    let mut hole: Vec<P2> = Vec::new();
    for k in 0..=20 {
        let a = PI / 2.0 + PI * k as f64 / 20.0;
        hole.push(s([22.0 + 8.0 * a.cos(), 7.0 * a.sin()]));
    }
    hole.extend(
        [
            [22.0, -7.0],
            [33.0, -7.0],
            [33.0, -2.0],
            [29.0, -1.0],
            [33.0, 0.5],
            [33.0, 7.0],
            [22.0, 7.0],
        ]
        .map(s),
    );
    // A hanging hole at the other end.
    let hang = circle_2d(s([-37.0, 0.0]), 4.0 * scale, 32, 0.0);
    let mut b = Builder::default();
    extrude(
        &mut b,
        &[Polygon::new(outline, vec![hole, hang])],
        0.0,
        4.0 * scale,
    )?;
    b.finish()
}

fn robot() -> Result<Vec<Mesh>, String> {
    Ok(vec![
        // Feet, legs, body, arms, neck, head.
        cuboid([-9.0, -5.0, 0.0], [-2.0, 6.0, 3.0]),
        cuboid([2.0, -5.0, 0.0], [9.0, 6.0, 3.0]),
        cuboid([-8.0, -3.0, 2.5], [-3.0, 3.0, 14.0]),
        cuboid([3.0, -3.0, 2.5], [8.0, 3.0, 14.0]),
        cuboid([-11.0, -5.0, 13.5], [11.0, 5.0, 30.0]),
        cuboid([-15.5, -2.5, 15.0], [-10.5, 2.5, 28.0]),
        cuboid([10.5, -2.5, 15.0], [15.5, 2.5, 28.0]),
        cuboid([-3.0, -3.0, 29.5], [3.0, 3.0, 32.5]),
        cuboid([-8.0, -6.0, 32.0], [8.0, 6.0, 43.0]),
        // Eyes standing proud of the face, an antenna with a ball on top.
        rod_y(1.8, -7.0, -5.5, -3.5, 38.5)?,
        rod_y(1.8, -7.0, -5.5, 3.5, 38.5)?,
        rod_z(0.8, 42.5, 50.0, 0.0, 0.0)?,
        ball([2.0, 2.0, 2.0], [0.0, 0.0, 51.0])?,
    ])
}
