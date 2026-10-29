//! Part library › Mathematical: classic surfaces and solids made printable (thickened and closed),
//! fractals, the convex hull of the selected parts, and z = f(x, y) from a typed formula.

use std::f64::consts::{PI, TAU};

use super::build::{Builder, Cap, P, heightfield, steps};
use super::design::tube;
use super::expr;
use super::hull::convex_hull;
use super::solid::{field, voxels};
use super::{Category, Item, Param, Sources, Values, choice, count, item, mm, num, text};
use crate::mesh::Mesh;
use crate::ops::shell::field::Surface;
use crate::repair::{Control, RepairError, RepairResult};

fn math(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    params: Vec<Param>,
) -> Item {
    item(id, label, Category::Mathematical, description, params)
}

pub(super) fn catalog() -> Vec<Item> {
    vec![
        math(
            "mobius",
            "Möbius strip",
            "A band with a half twist, thickened into a solid: it has one side and one edge.",
            vec![
                mm("radius", "Radius", 25.0),
                mm("width", "Width", 15.0),
                mm("thickness", "Thickness", 2.0),
                count("half_twists", "Half twists", 1.0, 1.0, 15.0).help("Odd counts make a Möbius band; even ones a twisted ring."),
                count("segments", "Segments", 160.0, 24.0, 4000.0),
            ],
        ),
        math(
            "klein",
            "Klein bottle",
            "A bottle whose neck passes through its side into its own bottom, as a solid shell.",
            vec![
                mm("size", "Height", 60.0),
                mm("thickness", "Wall thickness", 2.0),
                num("accuracy", "Accuracy", "mm", 0.6, 0.05, 10.0).help("The grid step; smaller is finer and slower."),
            ],
        ),
        math(
            "menger",
            "Menger sponge",
            "A cube with its centre and face centres taken out, again and again.",
            vec![mm("size", "Size", 30.0), count("level", "Level", 3.0, 0.0, 4.0)],
        ),
        math(
            "sierpinski",
            "Sierpinski pyramid",
            "A square pyramid made of five half-size pyramids, each made the same way.",
            vec![
                mm("base", "Base length", 40.0),
                mm("height", "Height (Z)", 28.0),
                count("level", "Level", 3.0, 0.0, 7.0),
            ],
        ),
        math(
            "convex_hull",
            "Convex Hull",
            "The smallest convex solid round the selected parts.",
            Vec::new(),
        )
        .made_from_parts(),
        math(
            "fractal",
            "Fractal",
            "The power-8 bulb fractal: points whose repeated power map stays bounded.",
            vec![
                mm("size", "Size", 40.0),
                num("power", "Power", "", 8.0, 2.0, 16.0),
                count("iterations", "Iterations", 8.0, 2.0, 40.0),
                count("resolution", "Resolution", 120.0, 16.0, 500.0).help("Grid points along the size."),
            ],
        ),
        math(
            "gyroid",
            "Gyroid",
            "A gyroid lattice wall filling a box.",
            vec![
                mm("length", "Length (X)", 30.0),
                mm("width", "Width (Y)", 30.0),
                mm("height", "Height (Z)", 30.0),
                mm("cell", "Cell size", 10.0),
                mm("thickness", "Wall thickness", 1.5),
                num("accuracy", "Accuracy", "mm", 0.3, 0.02, 10.0).help("The grid step; smaller is finer and slower."),
            ],
        ),
        math(
            "function_surface",
            "Function Surface",
            "z = f(x, y) from your formula, as a slab of even thickness.",
            vec![
                text("formula", "Formula for z", "3 * sin(x / 5) * cos(y / 5)", 500).help(
                    "Use x and y (mm from the middle), r, pi, e, + - * / ^ and functions such as sin, cos, sqrt, abs, exp.",
                ),
                mm("length", "Length (X)", 60.0),
                mm("width", "Width (Y)", 60.0),
                mm("thickness", "Thickness", 2.0),
                count("resolution", "Resolution", 120.0, 4.0, 1000.0).help("Grid points along the longer side."),
            ],
        ),
        math(
            "mesh_pipe",
            "Mesh pipe",
            "A round tube along a curve.",
            vec![
                choice(
                    "curve",
                    "Curve",
                    "trefoil",
                    &[
                        ("trefoil", "Trefoil knot"),
                        ("torus_knot", "Torus knot (2, 5)"),
                        ("wave", "Wavy ring"),
                        ("spring", "Spring"),
                    ],
                ),
                mm("size", "Size", 50.0),
                mm("radius", "Tube radius", 3.0),
                count("segments", "Segments along", 300.0, 24.0, 10000.0),
                count("around", "Segments around", 16.0, 3.0, 256.0),
            ],
        ),
        math(
            "superellipsoid",
            "Superellipsoid",
            "A ball made boxy or star-like by two exponents.",
            vec![
                mm("radius_x", "Radius X", 20.0),
                mm("radius_y", "Radius Y", 20.0),
                mm("radius_z", "Radius Z", 15.0),
                num("east_west", "East–west exponent", "", 0.5, 0.1, 4.0).help("Round the Z axis: below 1 boxy, 1 round, above 1 pinched."),
                num("north_south", "North–south exponent", "", 0.5, 0.1, 4.0).help("From pole to pole, the same way."),
                count("segments", "Segments", 96.0, 8.0, 2048.0),
                count("rings", "Rings", 64.0, 4.0, 1024.0),
            ],
        ),
    ]
}

pub(super) fn make(
    id: &str,
    v: &Values,
    sources: &Sources,
    control: &mut Control,
) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    match id {
        "mobius" => mobius(v).map_err(failed),
        "klein" => klein(v, control),
        "menger" => menger(v, control),
        "sierpinski" => sierpinski(v).map_err(failed),
        "convex_hull" => hull_of(sources, control),
        "fractal" => fractal(v, control),
        "gyroid" => gyroid(v, control),
        "function_surface" => function_surface(v).map_err(failed),
        "mesh_pipe" => mesh_pipe(v).map_err(failed),
        "superellipsoid" => superellipsoid(v).map_err(failed),
        _ => unreachable!("every mathematical item has a generator"),
    }
}

fn stand(mesh: Mesh) -> RepairResult<Mesh> {
    let mut b = Builder::default();
    b.append(&mesh);
    b.finish().map_err(RepairError::Failed)
}

fn mobius(v: &Values) -> Result<Mesh, String> {
    let (r, w, t) = (v.get("radius")?, v.get("width")?, v.get("thickness")?);
    let (k, n) = (v.count("half_twists")?, v.count("segments")?);
    if w / 2.0 >= r || t / 2.0 >= r {
        return Err("The band is wider than its radius allows.".into());
    }
    // A w × t rectangle (corners anticlockwise) turned by half a turn per half twist round the ring.
    let corners = [
        [w / 2.0, t / 2.0],
        [-w / 2.0, t / 2.0],
        [-w / 2.0, -t / 2.0],
        [w / 2.0, -t / 2.0],
    ];
    let rings: Vec<Vec<P>> = (0..n)
        .map(|i| {
            let a = TAU * i as f64 / n as f64;
            let turn = k as f64 * a / 2.0;
            corners
                .iter()
                .map(|c| {
                    let (x, z) = (
                        c[0] * turn.cos() - c[1] * turn.sin(),
                        c[0] * turn.sin() + c[1] * turn.cos(),
                    );
                    [(r + x) * a.cos(), (r + x) * a.sin(), z]
                })
                .collect()
        })
        .collect();
    let mut b = Builder::default();
    // After an odd number of half turns each corner lands on the opposite one.
    b.closed_stack(&rings, if k % 2 == 1 { 2 } else { 0 });
    b.finish()
}

/// A point of the classic bottle-shaped Klein bottle (u in 0..π, v in 0..2π), about 3 units tall.
fn klein_point(u: f64, v: f64) -> P {
    let (cu, su) = (u.cos(), u.sin());
    let (cv, sv) = (v.cos(), v.sin());
    let x = -2.0 / 15.0
        * cu
        * (3.0 * cv - 30.0 * su + 90.0 * cu.powi(4) * su - 60.0 * cu.powi(6) * su
            + 5.0 * cu * cv * su);
    let y = -1.0 / 15.0
        * su
        * (3.0 * cv - 3.0 * cu.powi(2) * cv - 48.0 * cu.powi(4) * cv + 48.0 * cu.powi(6) * cv
            - 60.0 * su
            + 5.0 * cu * cv * su
            - 5.0 * cu.powi(3) * cv * su
            - 80.0 * cu.powi(5) * cv * su
            + 80.0 * cu.powi(7) * cv * su);
    let z = 2.0 / 15.0 * (3.0 + 5.0 * cu * su) * sv;
    [x, y, z]
}

/// The Klein bottle's surface (which passes through itself where the neck enters the side) taken
/// as a wall: every point within half the thickness of it. Its boundary is found on a grid, so
/// the solid is closed and the bottle's inside stays hollow.
fn klein(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let (size, t) = (
        v.get("size").map_err(failed)?,
        v.get("thickness").map_err(failed)?,
    );
    let mut cell = v.get("accuracy").map_err(failed)?;
    if v.preview {
        cell = cell.max(size / 60.0);
    }
    if cell > t / 2.0 {
        return Err(failed(format!(
            "Use an accuracy of at most half the wall thickness ({} mm).",
            t / 2.0
        )));
    }
    // The surface as triangles. u from 0 to π closes on itself (K(π, v) = K(0, π − v)); the
    // distance to the surface does not care whether the seam is joined, so the last row is simply
    // sampled too.
    let (nu, nv) = (240, 96);
    let mut raw: Vec<P> = Vec::with_capacity((nu + 1) * nv);
    for i in 0..=nu {
        for j in 0..nv {
            raw.push(klein_point(
                PI * i as f64 / nu as f64,
                TAU * j as f64 / nv as f64,
            ));
        }
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &raw {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    // Stand the bottle up along its longest axis.
    let up = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap();
    let (x, y) = ((up + 1) % 3, (up + 2) % 3);
    let s = (size - t) / (hi[up] - lo[up]);
    let mut surface = Builder::default();
    for p in &raw {
        surface.point([p[x] * s, p[y] * s, (p[up] - lo[up]) * s]);
    }
    let id = |i: usize, j: usize| (i * nv + j % nv) as u32;
    for i in 0..nu {
        for j in 0..nv {
            surface.quad(id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
        }
    }
    let surface = surface.raw().map_err(failed)?;
    let near = Surface::new(&surface);
    let box_lo = [lo[x] * s - t, lo[y] * s - t, -t];
    let box_hi = [hi[x] * s + t, hi[y] * s + t, (hi[up] - lo[up]) * s + t];
    let half = t / 2.0;
    let (lo, hi) = (box_lo, box_hi);
    let mesh = field(lo, hi, cell, |p| near.distance(p, t) - half, control)?
        .ok_or_else(|| failed("The bottle came out empty; make the wall thicker.".into()))?;
    stand(mesh)
}

/// A Menger sponge of level n: 20ⁿ cubes of size/3ⁿ, so its volume is (20/27)ⁿ of the cube's.
fn menger(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let (size, level) = (
        v.get("size").map_err(failed)?,
        v.count("level").map_err(failed)?,
    );
    let n = 3usize.pow(level as u32);
    let solid = |mut i: usize, mut j: usize, mut k: usize| {
        for _ in 0..level {
            let ones = usize::from(i % 3 == 1) + usize::from(j % 3 == 1) + usize::from(k % 3 == 1);
            if ones >= 2 {
                return false;
            }
            (i, j, k) = (i / 3, j / 3, k / 3);
        }
        true
    };
    let mesh = voxels([n; 3], size / n as f64, solid, control)?;
    stand(mesh)
}

/// The Sierpinski square pyramid of level n: 5ⁿ pyramids of half the size per level, so its volume
/// is (5/8)ⁿ of the whole pyramid's. Each little pyramid is its own closed shell, shrunk about its
/// middle by one part in ten thousand so that neighbours, which would touch along an edge or at a
/// tip, stay apart (and nothing joins them into edges shared by four faces when the part is saved
/// and opened again); the volume is then (5/8)ⁿ × (1 − 10⁻⁴)³ of the pyramid's.
pub(crate) const SIERPINSKI_SHRINK: f64 = 1e-4;

fn sierpinski(v: &Values) -> Result<Mesh, String> {
    let (base, height, level) = (v.get("base")?, v.get("height")?, v.count("level")?);
    let mut pyramids: Vec<(P, f64)> = vec![([0.0, 0.0, 0.0], 1.0)];
    for _ in 0..level {
        let mut next = Vec::with_capacity(pyramids.len() * 5);
        for (c, s) in pyramids {
            let h = s / 2.0;
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                next.push((
                    [c[0] + dx * s * base / 4.0, c[1] + dy * s * base / 4.0, c[2]],
                    h,
                ));
            }
            next.push(([c[0], c[1], c[2] + h * height], h));
        }
        pyramids = next;
    }
    let mut b = Builder::default();
    let keep = 1.0 - SIERPINSKI_SHRINK;
    for (c, s) in pyramids {
        let (half, top) = (base * s / 2.0, height * s);
        // The centroid of a pyramid is a quarter of the way up.
        let mid = [c[0], c[1], c[2] + top / 4.0];
        let shrink = |p: P| {
            [
                mid[0] + (p[0] - mid[0]) * keep,
                mid[1] + (p[1] - mid[1]) * keep,
                mid[2] + (p[2] - mid[2]) * keep,
            ]
        };
        let corners: Vec<u32> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .map(|&(dx, dy)| b.point(shrink([c[0] + dx * half, c[1] + dy * half, c[2]])))
            .collect();
        let apex = b.point(shrink([c[0], c[1], c[2] + top]));
        b.quad(corners[0], corners[3], corners[2], corners[1]);
        for k in 0..4 {
            b.triangle(corners[k], corners[(k + 1) % 4], apex);
        }
    }
    b.finish()
}

fn hull_of(sources: &Sources, control: &mut Control) -> RepairResult<Mesh> {
    if sources.parts.is_empty() {
        // The tile picture: the hull of a sample, a twisted ring of points.
        let pts: Vec<[f32; 3]> = (0..60)
            .map(|k| {
                let a = TAU * k as f64 / 60.0;
                let r = 12.0 + 5.0 * (3.0 * a).cos();
                [
                    (r * a.cos()) as f32,
                    (r * a.sin()) as f32,
                    (8.0 + 8.0 * (2.0 * a).sin()) as f32,
                ]
            })
            .collect();
        return stand(convex_hull(&pts, control)?);
    }
    let pts: Vec<[f32; 3]> = sources
        .parts
        .iter()
        .flat_map(|m| m.positions.as_chunks::<3>().0.iter().copied())
        .collect();
    convex_hull(&pts, control)
}

/// The bulb fractal's escape test, as a signed estimate of the distance to its surface: negative
/// for points whose orbit stays bounded.
fn bulb(p: P, power: f64, iterations: usize) -> f64 {
    let mut z = p;
    let mut dr = 1.0;
    let mut r = 0.0;
    for _ in 0..iterations {
        r = (z[0] * z[0] + z[1] * z[1] + z[2] * z[2]).sqrt();
        if r > 2.0 {
            return 0.5 * r.ln() * r / dr;
        }
        let theta = (z[2] / r.max(1e-300)).acos() * power;
        let phi = z[1].atan2(z[0]) * power;
        dr = r.powf(power - 1.0) * power * dr + 1.0;
        let zr = r.powf(power);
        z = [
            zr * theta.sin() * phi.cos() + p[0],
            zr * theta.sin() * phi.sin() + p[1],
            zr * theta.cos() + p[2],
        ];
    }
    let _ = r;
    -1e-3
}

fn fractal(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let (size, power) = (
        v.get("size").map_err(failed)?,
        v.get("power").map_err(failed)?,
    );
    let (iterations, mut resolution) = (
        v.count("iterations").map_err(failed)?,
        v.count("resolution").map_err(failed)?,
    );
    if v.preview {
        resolution = resolution.min(72);
    }
    // The bulb lies within a radius of about 1.2; scaled so its box is `size` across.
    let extent = 1.2;
    let s = size / (2.0 * extent);
    let cell = size / resolution as f64;
    let mesh = field(
        [-size / 2.0; 3],
        [size / 2.0; 3],
        cell,
        |p| {
            let q = [p[0] / s, p[1] / s, p[2] / s];
            if q[0] * q[0] + q[1] * q[1] + q[2] * q[2] > extent * extent {
                return 1.0;
            }
            bulb(q, power, iterations) * s
        },
        control,
    )?
    .ok_or_else(|| failed("The fractal came out empty; use more iterations.".into()))?;
    stand(mesh)
}

fn gyroid(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let size = [
        v.get("length").map_err(failed)?,
        v.get("width").map_err(failed)?,
        v.get("height").map_err(failed)?,
    ];
    let (cell_size, t) = (
        v.get("cell").map_err(failed)?,
        v.get("thickness").map_err(failed)?,
    );
    let mut step = v.get("accuracy").map_err(failed)?;
    if v.preview {
        step = step.max(size.iter().fold(0.0f64, |a, &b| a.max(b)) / 60.0);
    }
    if step > t / 2.0 {
        return Err(failed(format!(
            "Use an accuracy of at most half the wall thickness ({} mm).",
            t / 2.0
        )));
    }
    let k = TAU / cell_size;
    // |∇g| averages about 1.2·k over the surface; dividing by it turns g into roughly millimetres.
    let scale = 1.0 / (1.2 * k);
    let half = size.map(|s| s / 2.0);
    let mesh = field(
        [-half[0], -half[1], -half[2]],
        half,
        step,
        |p| {
            let (x, y, z) = (k * p[0], k * p[1], k * p[2]);
            let g = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            let wall = g.abs() * scale - t / 2.0;
            let outside_box = (0..3)
                .map(|a| p[a].abs() - half[a])
                .fold(f64::NEG_INFINITY, f64::max);
            wall.max(outside_box)
        },
        control,
    )?
    .ok_or_else(|| failed("The gyroid came out empty; make the wall thicker.".into()))?;
    stand(mesh)
}

fn function_surface(v: &Values) -> Result<Mesh, String> {
    let formula = expr::parse(&v.text("formula")?)?;
    let (l, w, t) = (v.get("length")?, v.get("width")?, v.get("thickness")?);
    let n = v.count("resolution")?;
    let step = l.max(w) / n as f64;
    let (nx, ny) = (
        ((l / step).round() as usize).max(1),
        ((w / step).round() as usize).max(1),
    );
    let (xs, ys) = (steps(-l / 2.0, l / 2.0, nx), steps(-w / 2.0, w / 2.0, ny));
    let mut z = Vec::with_capacity(xs.len() * ys.len());
    for &y in &ys {
        for &x in &xs {
            let value = formula.eval(x, y);
            if !value.is_finite() {
                return Err(format!(
                    "The formula has no value at x = {x:.2}, y = {y:.2}; change it or the size."
                ));
            }
            if value.abs() > super::MAX_MM {
                return Err(format!(
                    "The formula gives {value:.0} mm at x = {x:.2}, y = {y:.2}, more than 10 m."
                ));
            }
            z.push(value);
        }
    }
    let nx1 = xs.len();
    heightfield(&xs, &ys, |i, j| z[j * nx1 + i], |i, j| z[j * nx1 + i] - t).finish()
}

/// The curve of a mesh pipe, `n` points, about `size` across.
fn pipe_curve(kind: &str, size: f64, n: usize) -> (Vec<P>, bool) {
    let points = |f: &dyn Fn(f64) -> P, closed: bool| -> Vec<P> {
        (0..n)
            .map(|k| {
                let t = if closed {
                    TAU * k as f64 / n as f64
                } else {
                    k as f64 / (n - 1) as f64
                };
                f(t)
            })
            .collect()
    };
    match kind {
        "torus_knot" => {
            let s = size / 6.0;
            (
                points(
                    &|t| {
                        let (p, q) = (2.0, 5.0);
                        let r = 2.0 + (q * t).cos();
                        [
                            s * r * (p * t).cos(),
                            s * r * (p * t).sin(),
                            -s * (q * t).sin(),
                        ]
                    },
                    true,
                ),
                true,
            )
        }
        "wave" => {
            let s = size / 2.0;
            (
                points(
                    &|t| [s * t.cos(), s * t.sin(), s * 0.2 * (6.0 * t).sin()],
                    true,
                ),
                true,
            )
        }
        "spring" => {
            let (r, turns, lead) = (size / 3.0, 4.0, size / 4.0);
            (
                points(
                    &|t| {
                        let a = TAU * turns * t;
                        [r * a.cos(), r * a.sin(), lead * turns * t]
                    },
                    false,
                ),
                false,
            )
        }
        _ => {
            let s = size / 6.0;
            (
                points(
                    &|t| {
                        [
                            s * (t.sin() + 2.0 * (2.0 * t).sin()),
                            s * (t.cos() - 2.0 * (2.0 * t).cos()),
                            -s * (3.0 * t).sin(),
                        ]
                    },
                    true,
                ),
                true,
            )
        }
    }
}

fn mesh_pipe(v: &Values) -> Result<Mesh, String> {
    let kind = v.choice("curve")?;
    let (size, radius) = (v.get("size")?, v.get("radius")?);
    let (n, around) = (v.count("segments")?, v.count("around")?);
    let (path, closed) = pipe_curve(kind, size, n);
    // The tube must not reach another stretch of the curve: the nearest approach of points more
    // than a few tube widths apart along it.
    let len: f64 = (1..n).map(|k| dist(path[k], path[k - 1])).sum();
    let spacing = len / n as f64;
    let skip = ((4.0 * radius) / spacing).ceil() as usize + 2;
    let coarse = (n / 400).max(1);
    let mut nearest = f64::INFINITY;
    for i in (0..n).step_by(coarse) {
        for j in (i + skip..n).step_by(coarse) {
            let along = if closed {
                (j - i).min(n - (j - i))
            } else {
                j - i
            };
            if along >= skip {
                nearest = nearest.min(dist(path[i], path[j]));
            }
        }
    }
    if 2.2 * radius >= nearest {
        return Err(format!(
            "The tube would run into itself; make it thinner than {:.2} mm or the curve bigger.",
            nearest / 2.2
        ));
    }
    let mut curvature_ok = true;
    for k in 1..n - 1 {
        // The bend radius from three neighbouring points must exceed the tube's.
        let (a, b, c) = (path[k - 1], path[k], path[k + 1]);
        let (ab, bc, ca) = (dist(a, b), dist(b, c), dist(c, a));
        let area = {
            let s = (ab + bc + ca) / 2.0;
            (s * (s - ab) * (s - bc) * (s - ca)).max(0.0).sqrt()
        };
        if area > 0.0 && ab * bc * ca / (4.0 * area) <= radius * 1.05 {
            curvature_ok = false;
        }
    }
    if !curvature_ok {
        return Err("The curve bends too tightly for a tube that thick; make it thinner or the curve bigger.".into());
    }
    let mesh = tube(&path, radius, around, closed)?;
    let mut b = Builder::default();
    b.append(&mesh);
    b.finish()
}

fn dist(a: P, b: P) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// sign(c) · |c|^e.
fn signed_pow(c: f64, e: f64) -> f64 {
    c.signum() * c.abs().powf(e)
}

fn superellipsoid(v: &Values) -> Result<Mesh, String> {
    let r = [v.get("radius_x")?, v.get("radius_y")?, v.get("radius_z")?];
    let (ew, ns) = (v.get("east_west")?, v.get("north_south")?);
    let (n, m) = (v.count("segments")?, v.count("rings")?);
    let rings: Vec<Vec<P>> = (1..m)
        .map(|i| {
            let eta = -PI / 2.0 + PI * i as f64 / m as f64;
            let (ce, se) = (signed_pow(eta.cos(), ns), signed_pow(eta.sin(), ns));
            (0..n)
                .map(|k| {
                    let w = -PI + TAU * k as f64 / n as f64;
                    [
                        r[0] * ce * signed_pow(w.cos(), ew),
                        r[1] * ce * signed_pow(w.sin(), ew),
                        r[2] * se,
                    ]
                })
                .collect()
        })
        .collect();
    let mut b = Builder::default();
    b.stack(
        &rings,
        Cap::Point([0.0, 0.0, -r[2]]),
        Cap::Point([0.0, 0.0, r[2]]),
    );
    b.finish()
}
