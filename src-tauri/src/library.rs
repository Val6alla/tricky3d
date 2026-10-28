//! Part library › Primitive: closed, outward-facing meshes of simple solids made to the sizes the
//! user sets (mm, degrees and segment counts), and the catalog of their parameters, which the
//! window builds its form from (src/library). Every shape sits centred on the local Z axis with its
//! lowest point at z = 0, so a part made from it rests on the platform where it is placed.
//!
//! Curved shapes keep their points on the true surface (as CAD tessellation does), so a coarse
//! mesh is a little smaller than the solid it approximates; the tests bound that by the segment
//! counts.

use serde::Serialize;
use serde_json::{Map, Value};
use std::f64::consts::{PI, TAU};

use crate::mesh::{Mesh, cross, dot, make_box, sub};

/// A parameter of a primitive. Field names match `LibraryParam` in src/library/ipc.ts.
#[derive(Clone, Debug, Serialize)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    /// "mm", "°" or "" (a count).
    pub unit: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    /// Whole numbers only (segment counts).
    pub integer: bool,
}

/// A primitive of the catalog. Field names match `LibraryPrimitive` in src/library/ipc.ts.
#[derive(Clone, Debug, Serialize)]
pub struct Primitive {
    pub id: &'static str,
    pub label: &'static str,
    pub params: Vec<Param>,
}

/// More triangles than this would take long to draw and help nobody.
pub const MAX_TRIANGLES: usize = 4_000_000;

const MAX_MM: f64 = 10_000.0;

const fn mm(key: &'static str, label: &'static str, default: f64) -> Param {
    Param {
        key,
        label,
        unit: "mm",
        default,
        min: 0.001,
        max: MAX_MM,
        integer: false,
    }
}

const fn count(key: &'static str, label: &'static str, default: f64, min: f64, max: f64) -> Param {
    Param {
        key,
        label,
        unit: "",
        default,
        min,
        max,
        integer: true,
    }
}

const SEGMENTS: Param = count("segments", "Segments", 64.0, 3.0, 2048.0);
const RINGS: Param = count("rings", "Rings", 32.0, 2.0, 1024.0);
const TUBE_SEGMENTS: Param = count("tube_segments", "Tube segments", 32.0, 3.0, 1024.0);

/// Every primitive with its parameters, in the order the library shows them.
pub fn catalog() -> Vec<Primitive> {
    let p = |id, label, params: Vec<Param>| Primitive { id, label, params };
    vec![
        p(
            "box",
            "Box",
            vec![
                mm("length", "Length (X)", 20.0),
                mm("width", "Width (Y)", 20.0),
                mm("height", "Height (Z)", 20.0),
            ],
        ),
        p(
            "pyramid",
            "Pyramid",
            vec![
                mm("length", "Length (X)", 20.0),
                mm("width", "Width (Y)", 20.0),
                mm("height", "Height (Z)", 20.0),
                count("sides", "Sides", 4.0, 3.0, 256.0),
            ],
        ),
        p(
            "pyramid_frustum",
            "Frustum of pyramid",
            vec![
                mm("length", "Bottom length (X)", 20.0),
                mm("width", "Bottom width (Y)", 20.0),
                mm("top_length", "Top length (X)", 10.0),
                mm("top_width", "Top width (Y)", 10.0),
                mm("height", "Height (Z)", 20.0),
                count("sides", "Sides", 4.0, 3.0, 256.0),
            ],
        ),
        p(
            "cylinder",
            "Cylinder",
            vec![
                mm("radius", "Radius", 10.0),
                mm("height", "Height (Z)", 20.0),
                SEGMENTS,
            ],
        ),
        p(
            "cone",
            "Cone",
            vec![
                mm("radius", "Radius", 10.0),
                mm("height", "Height (Z)", 20.0),
                SEGMENTS,
            ],
        ),
        p(
            "cone_frustum",
            "Frustum of cone",
            vec![
                mm("radius", "Bottom radius", 10.0),
                mm("top_radius", "Top radius", 5.0),
                mm("height", "Height (Z)", 20.0),
                SEGMENTS,
            ],
        ),
        p(
            "sphere",
            "Sphere",
            vec![mm("radius", "Radius", 10.0), SEGMENTS, RINGS],
        ),
        p(
            "ellipsoid",
            "Ellipsoid",
            vec![
                mm("radius_x", "Radius X", 15.0),
                mm("radius_y", "Radius Y", 10.0),
                mm("radius_z", "Radius Z", 8.0),
                SEGMENTS,
                RINGS,
            ],
        ),
        p(
            "torus",
            "Torus",
            vec![
                mm("radius", "Ring radius", 15.0),
                mm("tube_radius", "Tube radius", 5.0),
                SEGMENTS,
                TUBE_SEGMENTS,
            ],
        ),
        p(
            "elliptical_torus",
            "Elliptical torus",
            vec![
                mm("radius", "Ring radius", 15.0),
                mm("tube_width", "Tube radius across", 5.0),
                mm("tube_height", "Tube radius up", 3.0),
                SEGMENTS,
                TUBE_SEGMENTS,
            ],
        ),
        p(
            "helix",
            "Helix",
            vec![
                mm("radius", "Coil radius", 10.0),
                mm("tube_radius", "Wire radius", 1.5),
                Param {
                    min: 0.05,
                    max: 200.0,
                    integer: false,
                    ..count("turns", "Turns", 3.0, 0.0, 0.0)
                },
                mm("lead", "Lead length", 8.0),
                count("segments", "Segments per turn", 64.0, 8.0, 2048.0),
                count("tube_segments", "Wire segments", 16.0, 3.0, 512.0),
            ],
        ),
        p(
            "rhombicuboctahedron",
            "Rhombicuboctahedron",
            vec![mm("edge", "Edge length", 10.0)],
        ),
        p(
            "hyperboloid",
            "Rotation hyperboloid",
            vec![
                mm("radius", "Radius", 10.0),
                mm("height", "Height (Z)", 20.0),
                Param {
                    unit: "°",
                    min: 0.0,
                    max: 170.0,
                    integer: false,
                    ..count("twist", "Rotation angle", 60.0, 0.0, 0.0)
                },
                SEGMENTS,
                count("rings", "Rings", 16.0, 1.0, 1024.0),
            ],
        ),
    ]
}

/// The parameter values of a request, each checked against the catalog; a missing one takes its
/// default. Errors are sentences for the user.
struct Values<'a> {
    spec: &'a Primitive,
    given: &'a Map<String, Value>,
}

impl Values<'_> {
    fn get(&self, key: &str) -> Result<f64, String> {
        let p = self
            .spec
            .params
            .iter()
            .find(|p| p.key == key)
            .expect("generators only ask for their own parameters");
        let v = match self.given.get(key) {
            None | Some(Value::Null) => p.default,
            Some(v) => v
                .as_f64()
                .ok_or_else(|| format!("{} must be a number.", p.label))?,
        };
        if !v.is_finite() || v < p.min || v > p.max || (p.integer && v.fract() != 0.0) {
            let unit = if p.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", p.unit)
            };
            let kind = if p.integer { "a whole number " } else { "" };
            return Err(format!(
                "{} must be {kind}from {}{unit} to {}{unit}.",
                p.label, p.min, p.max
            ));
        }
        Ok(v)
    }

    fn count(&self, key: &str) -> Result<usize, String> {
        self.get(key).map(|v| v as usize)
    }
}

/// The mesh of primitive `id` with `params` (key → number), or a sentence saying what is wrong.
pub fn generate(id: &str, params: &Map<String, Value>) -> Result<Mesh, String> {
    let all = catalog();
    let spec = all
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("The part library has no primitive \"{id}\"."))?;
    let v = Values {
        spec,
        given: params,
    };
    let mesh = match id {
        "box" => {
            let [x, y, z] = [v.get("length")?, v.get("width")?, v.get("height")?];
            Ok(make_box(
                [(-x / 2.0) as f32, (-y / 2.0) as f32, 0.0],
                [(x / 2.0) as f32, (y / 2.0) as f32, z as f32],
            ))
        }
        "pyramid" => frustum_of_pyramid(
            v.get("length")?,
            v.get("width")?,
            None,
            v.get("height")?,
            v.count("sides")?,
        ),
        "pyramid_frustum" => frustum_of_pyramid(
            v.get("length")?,
            v.get("width")?,
            Some((v.get("top_length")?, v.get("top_width")?)),
            v.get("height")?,
            v.count("sides")?,
        ),
        "cylinder" => {
            let r = v.get("radius")?;
            frustum_of_cone(r, Some(r), v.get("height")?, v.count("segments")?)
        }
        "cone" => frustum_of_cone(
            v.get("radius")?,
            None,
            v.get("height")?,
            v.count("segments")?,
        ),
        "cone_frustum" => frustum_of_cone(
            v.get("radius")?,
            Some(v.get("top_radius")?),
            v.get("height")?,
            v.count("segments")?,
        ),
        "sphere" => {
            let r = v.get("radius")?;
            ellipsoid([r, r, r], v.count("segments")?, v.count("rings")?)
        }
        "ellipsoid" => ellipsoid(
            [v.get("radius_x")?, v.get("radius_y")?, v.get("radius_z")?],
            v.count("segments")?,
            v.count("rings")?,
        ),
        "torus" => {
            let r = v.get("tube_radius")?;
            torus(
                v.get("radius")?,
                r,
                r,
                v.count("segments")?,
                v.count("tube_segments")?,
            )
        }
        "elliptical_torus" => torus(
            v.get("radius")?,
            v.get("tube_width")?,
            v.get("tube_height")?,
            v.count("segments")?,
            v.count("tube_segments")?,
        ),
        "helix" => helix(
            v.get("radius")?,
            v.get("tube_radius")?,
            v.get("turns")?,
            v.get("lead")?,
            v.count("segments")?,
            v.count("tube_segments")?,
        ),
        "rhombicuboctahedron" => rhombicuboctahedron(v.get("edge")?),
        "hyperboloid" => hyperboloid(
            v.get("radius")?,
            v.get("height")?,
            v.get("twist")?,
            v.count("segments")?,
            v.count("rings")?,
        ),
        _ => unreachable!("every catalog id has a generator"),
    };
    let mesh = mesh?;
    if mesh.triangle_count() > MAX_TRIANGLES {
        return Err(format!(
            "That would make {} triangles; lower the segment counts to stay under {}.",
            crate::mesh::fmt_count(mesh.triangle_count() as u64),
            crate::mesh::fmt_count(MAX_TRIANGLES as u64)
        ));
    }
    Ok(mesh)
}

// ---------------------------------------------------------------------------------------------
// Building meshes

/// Collects points and triangles, then makes the result face outward and rest on z = 0.
#[derive(Default)]
struct Builder {
    points: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
}

/// How a stack of rings is closed at one end.
enum Cap {
    /// A flat lid: a fan from the ring's centre.
    Flat,
    /// A fan to a single point (a cone's tip, a sphere's pole).
    Point([f64; 3]),
}

impl Builder {
    fn point(&mut self, p: [f64; 3]) -> u32 {
        self.points.push(p);
        (self.points.len() - 1) as u32
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.triangles.push([a, b, c]);
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }

    /// Closed loops of points (each the same length, running anticlockwise seen from the top of
    /// the stack) joined one to the next, and closed at both ends. The winding is consistent; the
    /// final sign is fixed in `finish`.
    fn stack(&mut self, rings: &[Vec<[f64; 3]>], bottom: Cap, top: Cap) {
        let n = rings[0].len();
        let ids: Vec<Vec<u32>> = rings
            .iter()
            .map(|ring| ring.iter().map(|&p| self.point(p)).collect())
            .collect();
        for pair in ids.windows(2) {
            let (lo, hi) = (&pair[0], &pair[1]);
            for k in 0..n {
                let k1 = (k + 1) % n;
                self.quad(lo[k], lo[k1], hi[k1], hi[k]);
            }
        }
        let centre = |ring: &[[f64; 3]]| {
            let mut c = [0.0; 3];
            for p in ring {
                for (a, v) in c.iter_mut().zip(p) {
                    *a += v / ring.len() as f64;
                }
            }
            c
        };
        let first = &ids[0];
        let c = match bottom {
            Cap::Flat => self.point(centre(&rings[0])),
            Cap::Point(p) => self.point(p),
        };
        for k in 0..n {
            self.triangle(c, first[(k + 1) % n], first[k]);
        }
        let last = &ids[ids.len() - 1];
        let c = match top {
            Cap::Flat => self.point(centre(&rings[rings.len() - 1])),
            Cap::Point(p) => self.point(p),
        };
        for k in 0..n {
            self.triangle(c, last[k], last[(k + 1) % n]);
        }
    }

    /// The mesh, turned inside out if its winding came out facing in, centred on the Z axis and
    /// lifted so its lowest point is at z = 0.
    fn finish(mut self) -> Result<Mesh, String> {
        let signed: f64 = self
            .triangles
            .iter()
            .map(|t| {
                dot(
                    self.points[t[0] as usize],
                    cross(self.points[t[1] as usize], self.points[t[2] as usize]),
                )
            })
            .sum();
        if signed < 0.0 {
            for t in &mut self.triangles {
                t.swap(1, 2);
            }
        }
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &self.points {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let shift = [-(lo[0] + hi[0]) / 2.0, -(lo[1] + hi[1]) / 2.0, -lo[2]];
        let positions = self
            .points
            .iter()
            .flat_map(|p| {
                [
                    (p[0] + shift[0]) as f32,
                    (p[1] + shift[1]) as f32,
                    (p[2] + shift[2]) as f32,
                ]
            })
            .collect();
        let indices = self.triangles.iter().flatten().copied().collect();
        let mesh = Mesh { positions, indices };
        mesh.check()
            .map_err(|reason| format!("The primitive could not be built: {reason}."))?;
        Ok(mesh)
    }
}

/// `n` points anticlockwise round the unit circle, the first at `start` radians.
fn circle(n: usize, start: f64) -> impl Iterator<Item = (f64, f64)> {
    (0..n).map(move |k| {
        let a = start + TAU * k as f64 / n as f64;
        (a.cos(), a.sin())
    })
}

/// The unit outline of a pyramid's base with `sides` sides and a flat side towards −Y, scaled so
/// its box is exactly 1 × 1 (so a 4-sided pyramid has a square base).
fn base_outline(sides: usize) -> Vec<(f64, f64)> {
    let start = -PI / 2.0 + PI / sides as f64;
    let raw: Vec<(f64, f64)> = circle(sides, start).collect();
    let (x0, x1) = raw
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.0), b.max(p.0))
        });
    let (y0, y1) = raw
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.1), b.max(p.1))
        });
    raw.iter()
        .map(|&(x, y)| {
            (
                (x - (x0 + x1) / 2.0) / (x1 - x0),
                (y - (y0 + y1) / 2.0) / (y1 - y0),
            )
        })
        .collect()
}

/// A pyramid (no top) or its frustum with a top of `top` (length, width), `sides` sides.
fn frustum_of_pyramid(
    length: f64,
    width: f64,
    top: Option<(f64, f64)>,
    height: f64,
    sides: usize,
) -> Result<Mesh, String> {
    let outline = base_outline(sides);
    let ring = |l: f64, w: f64, z: f64| {
        outline
            .iter()
            .map(|&(x, y)| [x * l, y * w, z])
            .collect::<Vec<_>>()
    };
    let mut b = Builder::default();
    match top {
        Some((tl, tw)) => b.stack(
            &[ring(length, width, 0.0), ring(tl, tw, height)],
            Cap::Flat,
            Cap::Flat,
        ),
        None => b.stack(
            &[ring(length, width, 0.0)],
            Cap::Flat,
            Cap::Point([0.0, 0.0, height]),
        ),
    }
    b.finish()
}

/// A cylinder or frustum of a cone (a top radius), or a cone (no top).
fn frustum_of_cone(
    radius: f64,
    top: Option<f64>,
    height: f64,
    segments: usize,
) -> Result<Mesh, String> {
    let ring = |r: f64, z: f64| {
        circle(segments, 0.0)
            .map(|(c, s)| [r * c, r * s, z])
            .collect::<Vec<_>>()
    };
    let mut b = Builder::default();
    match top {
        Some(t) => b.stack(&[ring(radius, 0.0), ring(t, height)], Cap::Flat, Cap::Flat),
        None => b.stack(
            &[ring(radius, 0.0)],
            Cap::Flat,
            Cap::Point([0.0, 0.0, height]),
        ),
    }
    b.finish()
}

/// An ellipsoid with semi-axes `r` (a sphere when they are equal): `segments` round the Z axis,
/// `rings` from pole to pole.
fn ellipsoid(r: [f64; 3], segments: usize, rings: usize) -> Result<Mesh, String> {
    let rings_of: Vec<Vec<[f64; 3]>> = (1..rings)
        .map(|i| {
            let polar = PI * i as f64 / rings as f64;
            let (s, c) = polar.sin_cos();
            circle(segments, 0.0)
                .map(|(x, y)| [r[0] * s * x, r[1] * s * y, -r[2] * c])
                .collect()
        })
        .collect();
    let mut b = Builder::default();
    b.stack(
        &rings_of,
        Cap::Point([0.0, 0.0, -r[2]]),
        Cap::Point([0.0, 0.0, r[2]]),
    );
    b.finish()
}

/// A torus round the Z axis whose tube is an ellipse `across` wide and `up` high (semi-axes), its
/// centre line `radius` from the axis.
fn torus(radius: f64, across: f64, up: f64, segments: usize, tube: usize) -> Result<Mesh, String> {
    if across >= radius {
        return Err("The tube radius must be smaller than the ring radius, or the tube would cross the middle.".into());
    }
    let mut b = Builder::default();
    let mut ids = Vec::with_capacity(segments * tube);
    for (cu, su) in circle(segments, 0.0) {
        for (cv, sv) in circle(tube, 0.0) {
            let r = radius + across * cv;
            ids.push(b.point([r * cu, r * su, up * sv]));
        }
    }
    let id = |i: usize, j: usize| ids[(i % segments) * tube + (j % tube)];
    for i in 0..segments {
        for j in 0..tube {
            b.quad(id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
        }
    }
    b.finish()
}

/// A wire of radius `wire` wound `turns` times round the Z axis at `radius`, rising `lead` per
/// turn, its ends cut square to the wire.
fn helix(
    radius: f64,
    wire: f64,
    turns: f64,
    lead: f64,
    per_turn: usize,
    tube: usize,
) -> Result<Mesh, String> {
    if wire >= radius {
        return Err("The wire radius must be smaller than the coil radius.".into());
    }
    let c = lead / TAU;
    let speed = (radius * radius + c * c).sqrt();
    // Next turns lie `lead` apart along Z; a wire cut square to its slope reaches wire / cos(slope)
    // up and down, so they stay apart with a little room to spare.
    if lead * radius / speed <= 2.05 * wire {
        return Err(format!(
            "The turns would touch: make the lead length more than {:.3} mm, or the wire thinner.",
            2.05 * wire * speed / radius
        ));
    }
    let steps = ((turns * per_turn as f64).ceil() as usize).max(2);
    let end = TAU * turns;
    let rings: Vec<Vec<[f64; 3]>> = (0..=steps)
        .map(|i| {
            let t = end * i as f64 / steps as f64;
            let (st, ct) = t.sin_cos();
            let centre = [radius * ct, radius * st, c * t];
            // Frenet frame of the helix: the normal points at the axis.
            let tangent = [-radius * st / speed, radius * ct / speed, c / speed];
            let normal = [-ct, -st, 0.0];
            let binormal = cross(tangent, normal);
            circle(tube, 0.0)
                .map(|(cv, sv)| {
                    let d: Vec<f64> = (0..3)
                        .map(|k| wire * (cv * normal[k] + sv * binormal[k]))
                        .collect();
                    [centre[0] + d[0], centre[1] + d[1], centre[2] + d[2]]
                })
                .collect()
        })
        .collect();
    let mut b = Builder::default();
    b.stack(&rings, Cap::Flat, Cap::Flat);
    b.finish()
}

/// The Archimedean solid with 8 triangles and 18 squares, every edge `edge` long.
fn rhombicuboctahedron(edge: f64) -> Result<Mesh, String> {
    let h = edge / 2.0;
    let far = h * (1.0 + 2f64.sqrt());
    let mut points = Vec::with_capacity(24);
    for (a, b) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        for z in [far, -far] {
            points.push([a * h, b * h, z]);
            points.push([a * h, z, b * h]);
            points.push([z, a * h, b * h]);
        }
    }
    hull(&points)
}

/// The convex hull of a few points (each one a corner of it), faces fanned from a corner. Brute
/// force: fine for the few dozen points a primitive has.
fn hull(points: &[[f64; 3]]) -> Result<Mesh, String> {
    let scale = points.iter().flatten().fold(0.0f64, |m, v| m.max(v.abs()));
    let eps = 1e-9 * scale.max(1.0);
    let mut faces: Vec<([f64; 3], f64)> = Vec::new();
    let n = points.len();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let normal = cross(sub(points[j], points[i]), sub(points[k], points[i]));
                let len = dot(normal, normal).sqrt();
                if len <= eps * scale {
                    continue;
                }
                let mut normal = normal.map(|v| v / len);
                let mut offset = dot(normal, points[i]);
                let side: Vec<f64> = points.iter().map(|p| dot(normal, *p) - offset).collect();
                if side.iter().all(|&d| d <= eps) {
                } else if side.iter().all(|&d| d >= -eps) {
                    normal = normal.map(|v| -v);
                    offset = -offset;
                } else {
                    continue;
                }
                if !faces
                    .iter()
                    .any(|(m, o)| dot(*m, normal) > 1.0 - 1e-9 && (o - offset).abs() <= eps)
                {
                    faces.push((normal, offset));
                }
            }
        }
    }
    let mut b = Builder::default();
    let ids: Vec<u32> = points.iter().map(|&p| b.point(p)).collect();
    for (normal, offset) in faces {
        let on: Vec<usize> = (0..n)
            .filter(|&i| (dot(normal, points[i]) - offset).abs() <= eps)
            .collect();
        let mut centre = [0.0; 3];
        for &i in &on {
            for k in 0..3 {
                centre[k] += points[i][k] / on.len() as f64;
            }
        }
        // Order the face's corners anticlockwise seen from outside.
        let u = sub(points[on[0]], centre);
        let w = cross(normal, u);
        let mut order: Vec<(f64, usize)> = on
            .iter()
            .map(|&i| {
                let d = sub(points[i], centre);
                (dot(d, w).atan2(dot(d, u)), i)
            })
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        for pair in order[1..].windows(2) {
            b.triangle(ids[order[0].1], ids[pair[0].1], ids[pair[1].1]);
        }
    }
    b.finish()
}

/// A one-sheet hyperboloid made of straight lines: each runs from the bottom circle to the top
/// circle, `twist` degrees further round. `rings` rows of points along the lines.
fn hyperboloid(
    radius: f64,
    height: f64,
    twist: f64,
    segments: usize,
    rings: usize,
) -> Result<Mesh, String> {
    let phi = twist.to_radians();
    let rows: Vec<Vec<[f64; 3]>> = (0..=rings)
        .map(|i| {
            let s = i as f64 / rings as f64;
            circle(segments, 0.0)
                .zip(circle(segments, phi))
                .map(|((bc, bs), (tc, ts))| {
                    let bottom = [radius * bc, radius * bs, 0.0];
                    let top = [radius * tc, radius * ts, height];
                    [0, 1, 2].map(|k| (1.0 - s) * bottom[k] + s * top[k])
                })
                .collect()
        })
        .collect();
    let mut b = Builder::default();
    b.stack(&rows, Cap::Flat, Cap::Flat);
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::stats;
    use crate::repair::{silent, status, tritri};
    use serde_json::json;

    fn make(id: &str, params: Value) -> Mesh {
        generate(id, params.as_object().unwrap()).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    /// 1 − (polygon of n sides inscribed in a circle) ÷ (circle): the area a coarse round loses.
    fn area_loss(n: f64) -> f64 {
        1.0 - n / TAU * (TAU / n).sin()
    }

    /// 1 − (its perimeter) ÷ (the circle's).
    fn length_loss(n: f64) -> f64 {
        1.0 - n / PI * (PI / n).sin()
    }

    /// Asserts the mesh is closed, oriented, outward and crossing-free, and returns its volume
    /// and area.
    fn sound(id: &str, mesh: &Mesh) -> (f64, f64) {
        let s = silent(|c| status::analyze(mesh, c)).unwrap().status;
        assert!(s.closed && s.oriented, "{id}: {s:?}");
        assert_eq!(s.shells, 1, "{id}");
        assert_eq!(s.degenerate_triangles, 0, "{id}");
        let crossing = silent(|c| tritri::intersecting_pairs(mesh, c)).unwrap();
        assert!(
            crossing.is_empty(),
            "{id}: {} crossing pairs",
            crossing.len()
        );
        let st = stats(mesh);
        assert!(st.volume > 0.0, "{id}: faces inward");
        let b = crate::mesh::bounds(mesh).unwrap();
        assert!(
            b[0][2].abs() < 1e-5
                && (b[0][0] + b[1][0]).abs() < 1e-4
                && (b[0][1] + b[1][1]).abs() < 1e-4,
            "{id}: {b:?}"
        );
        (st.volume, st.area)
    }

    fn close(id: &str, what: &str, got: f64, want: f64, tolerance: f64) {
        let error = (got - want).abs() / want;
        assert!(
            error <= tolerance,
            "{id} {what}: {got} vs {want}, off by {error:e}, allowed {tolerance:e}"
        );
    }

    #[test]
    fn the_catalog_has_the_thirteen_primitives_and_every_default_builds() {
        let ids: Vec<&str> = catalog().iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 13);
        for p in catalog() {
            let mesh = make(p.id, json!({}));
            sound(p.id, &mesh);
            for param in &p.params {
                assert!(
                    param.min <= param.default && param.default <= param.max,
                    "{} {}",
                    p.id,
                    param.key
                );
            }
        }
    }

    #[test]
    fn flat_solids_are_exact() {
        let (v, a) = sound(
            "box",
            &make("box", json!({"length": 10, "width": 20, "height": 30})),
        );
        assert_eq!((v, a), (6000.0, 2200.0));
        let (v, a) = sound(
            "pyramid",
            &make("pyramid", json!({"length": 20, "width": 20, "height": 30})),
        );
        let slant = (30.0f64 * 30.0 + 100.0).sqrt();
        close("pyramid", "volume", v, 400.0 * 30.0 / 3.0, 1e-6);
        close("pyramid", "area", a, 400.0 + 4.0 * 20.0 * slant / 2.0, 1e-6);
        // A six-sided base fits its 30 × 20 box: the hexagon's area is 3/4 of the box's.
        let (v, _) = sound(
            "pyramid",
            &make(
                "pyramid",
                json!({"length": 30, "width": 20, "height": 12, "sides": 6}),
            ),
        );
        close("pyramid 6", "volume", v, 0.75 * 600.0 * 12.0 / 3.0, 1e-6);
        // Square ends: (h / 3)(A1 + A2 + √(A1 A2)).
        let (v, a) = sound(
            "pyramid_frustum",
            &make(
                "pyramid_frustum",
                json!({"length": 20, "width": 20, "top_length": 10, "top_width": 10, "height": 12}),
            ),
        );
        close(
            "frustum",
            "volume",
            v,
            12.0 / 3.0 * (400.0 + 100.0 + 200.0),
            1e-6,
        );
        let side = (12.0f64 * 12.0 + 25.0).sqrt();
        close(
            "frustum",
            "area",
            a,
            400.0 + 100.0 + 4.0 * (20.0 + 10.0) / 2.0 * side,
            1e-6,
        );
        // Rectangular ends that are not alike: the prismatoid formula, exact for straight sides.
        let (v, _) = sound(
            "pyramid_frustum",
            &make(
                "pyramid_frustum",
                json!({"length": 40, "width": 10, "top_length": 10, "top_width": 30, "height": 9}),
            ),
        );
        close(
            "frustum 2",
            "volume",
            v,
            9.0 / 6.0 * (400.0 + 4.0 * 25.0 * 20.0 + 300.0),
            1e-6,
        );
        // Edge a: V = (12 + 10√2) a³ / 3, A = (18 + 2√3) a².
        let (v, a) = sound(
            "rhombicuboctahedron",
            &make("rhombicuboctahedron", json!({"edge": 10})),
        );
        close(
            "rhombicuboctahedron",
            "volume",
            v,
            (12.0 + 10.0 * 2f64.sqrt()) * 1000.0 / 3.0,
            1e-6,
        );
        close(
            "rhombicuboctahedron",
            "area",
            a,
            (18.0 + 2.0 * 3f64.sqrt()) * 100.0,
            1e-6,
        );
        assert_eq!(
            make("rhombicuboctahedron", json!({})).triangle_count(),
            8 + 18 * 2
        );
    }

    #[test]
    fn round_solids_come_within_their_resolution_of_the_formula() {
        for n in [16.0, 64.0, 256.0] {
            let (r, h) = (10.0, 20.0);
            let (v, a) = sound(
                "cylinder",
                &make("cylinder", json!({"radius": r, "height": h, "segments": n})),
            );
            close("cylinder", "volume", v, PI * r * r * h, area_loss(n) + 1e-6);
            close(
                "cylinder",
                "area",
                a,
                TAU * r * h + TAU * r * r,
                area_loss(n) + 1e-6,
            );
            let (v, a) = sound(
                "cone",
                &make("cone", json!({"radius": r, "height": h, "segments": n})),
            );
            close(
                "cone",
                "volume",
                v,
                PI * r * r * h / 3.0,
                area_loss(n) + 1e-6,
            );
            close(
                "cone",
                "area",
                a,
                PI * r * (r + (r * r + h * h).sqrt()),
                2.0 * area_loss(n) + 1e-6,
            );
            let (t, hh) = (4.0, 15.0);
            let (v, a) = sound(
                "cone_frustum",
                &make(
                    "cone_frustum",
                    json!({"radius": r, "top_radius": t, "height": hh, "segments": n}),
                ),
            );
            close(
                "cone_frustum",
                "volume",
                v,
                PI * hh / 3.0 * (r * r + r * t + t * t),
                area_loss(n) + 1e-6,
            );
            let slant = ((r - t) * (r - t) + hh * hh).sqrt();
            close(
                "cone_frustum",
                "area",
                a,
                PI * (r + t) * slant + PI * (r * r + t * t),
                2.0 * area_loss(n) + 1e-6,
            );
        }
    }

    #[test]
    fn spheres_ellipsoids_and_tori_converge_to_their_formulas() {
        let mut last = f64::INFINITY;
        for (n, m) in [(16.0, 8.0), (64.0, 32.0), (256.0, 128.0)] {
            let r = 10.0;
            let (v, a) = sound(
                "sphere",
                &make("sphere", json!({"radius": r, "segments": n, "rings": m})),
            );
            let tol = area_loss(n) + area_loss(2.0 * m) + 2.0 * length_loss(2.0 * m);
            close("sphere", "volume", v, 4.0 / 3.0 * PI * r * r * r, 2.0 * tol);
            close("sphere", "area", a, 4.0 * PI * r * r, 2.0 * tol);
            let error = (v - 4.0 / 3.0 * PI * r * r * r).abs();
            assert!(
                error < last,
                "the sphere's error must shrink as the mesh gets finer"
            );
            last = error;

            let abc = [15.0, 10.0, 8.0];
            let (v, a) = sound(
                "ellipsoid",
                &make(
                    "ellipsoid",
                    json!({"radius_x": abc[0], "radius_y": abc[1], "radius_z": abc[2], "segments": n, "rings": m}),
                ),
            );
            close(
                "ellipsoid",
                "volume",
                v,
                4.0 / 3.0 * PI * abc[0] * abc[1] * abc[2],
                2.0 * tol,
            );
            close("ellipsoid", "area", a, ellipsoid_area(abc), 3.0 * tol);

            let (big, small) = (15.0, 5.0);
            let (v, a) = sound(
                "torus",
                &make(
                    "torus",
                    json!({"radius": big, "tube_radius": small, "segments": n, "tube_segments": m}),
                ),
            );
            let tol = area_loss(n) + area_loss(m) + length_loss(n);
            close(
                "torus",
                "volume",
                v,
                TAU * big * PI * small * small,
                2.0 * tol,
            );
            close("torus", "area", a, TAU * big * TAU * small, 2.0 * tol);

            let (across, up) = (5.0, 3.0);
            let (v, a) = sound(
                "elliptical_torus",
                &make(
                    "elliptical_torus",
                    json!({"radius": big, "tube_width": across, "tube_height": up, "segments": n, "tube_segments": m}),
                ),
            );
            close(
                "elliptical_torus",
                "volume",
                v,
                TAU * big * PI * across * up,
                2.0 * tol,
            );
            close(
                "elliptical_torus",
                "area",
                a,
                elliptical_torus_area(big, across, up),
                3.0 * tol,
            );
        }
    }

    #[test]
    fn a_helix_holds_a_wire_of_its_length() {
        for (n, m) in [(32.0, 12.0), (128.0, 48.0)] {
            let (radius, wire, turns, lead) = (10.0, 1.5, 3.0, 8.0);
            let mesh = make(
                "helix",
                json!({"radius": radius, "tube_radius": wire, "turns": turns, "lead": lead, "segments": n, "tube_segments": m}),
            );
            let (v, a) = sound("helix", &mesh);
            let length = turns * (TAU * TAU * radius * radius + lead * lead).sqrt();
            let tol = area_loss(m) + 2.0 * area_loss(n) + 1e-4;
            close("helix", "volume", v, PI * wire * wire * length, 2.0 * tol);
            close(
                "helix",
                "area",
                a,
                TAU * wire * length + TAU * wire * wire,
                2.0 * tol,
            );
            let b = crate::mesh::bounds(&mesh).unwrap();
            close(
                "helix",
                "height",
                (b[1][2] - b[0][2]) as f64,
                lead * turns + 2.0 * wire,
                0.02,
            );
        }
        let error = generate("helix", json!({"lead": 2.0}).as_object().unwrap()).unwrap_err();
        assert!(error.starts_with("The turns would touch"), "{error}");
    }

    #[test]
    fn a_rotation_hyperboloid_matches_its_volume_and_surface() {
        for (n, m) in [(32.0, 8.0), (256.0, 64.0)] {
            let (r, h, twist) = (10.0, 20.0, 90.0f64);
            let (v, a) = sound(
                "hyperboloid",
                &make(
                    "hyperboloid",
                    json!({"radius": r, "height": h, "twist": twist, "segments": n, "rings": m}),
                ),
            );
            let phi = twist.to_radians();
            // |P(s)|² = R²(1 − 2s(1 − s)(1 − cos φ)) along every line; integrate π |P|² over z.
            close(
                "hyperboloid",
                "volume",
                v,
                PI * r * r * h * (1.0 - (1.0 - phi.cos()) / 3.0),
                area_loss(n) + 2.0 / (m * m) + 1e-6,
            );
            let side = surface_of_revolution(
                |s| r * (1.0 - 2.0 * s * (1.0 - s) * (1.0 - phi.cos())).sqrt(),
                h,
            );
            close(
                "hyperboloid",
                "area",
                a,
                side + TAU * r * r,
                2.0 * area_loss(n) + 2.0 / (m * m) + 1e-6,
            );
        }
        // No twist: a cylinder.
        let (v, _) = sound(
            "hyperboloid",
            &make("hyperboloid", json!({"twist": 0, "segments": 512})),
        );
        close(
            "hyperboloid",
            "untwisted",
            v,
            PI * 100.0 * 20.0,
            area_loss(512.0) + 1e-6,
        );
    }

    #[test]
    fn values_outside_the_catalog_ranges_are_refused_with_a_sentence() {
        let err = |id: &str, p: Value| generate(id, p.as_object().unwrap()).unwrap_err();
        assert_eq!(
            err("box", json!({"length": -1})),
            "Length (X) must be from 0.001 mm to 10000 mm."
        );
        assert_eq!(
            err("sphere", json!({"segments": 7.5})),
            "Segments must be a whole number from 3 to 2048."
        );
        assert_eq!(
            err("cylinder", json!({"radius": "big"})),
            "Radius must be a number."
        );
        assert_eq!(
            err("teapot", json!({})),
            "The part library has no primitive \"teapot\"."
        );
        assert!(
            err("torus", json!({"tube_radius": 20})).starts_with("The tube radius must be smaller")
        );
        assert!(
            err("sphere", json!({"segments": 2048, "rings": 1024}))
                .starts_with("That would make 4,190,208 triangles")
        );
    }

    /// Surface area of the ellipsoid with semi-axes `abc`, by integrating over a fine grid in the
    /// two angles (midpoint rule).
    fn ellipsoid_area([a, b, c]: [f64; 3]) -> f64 {
        let (nu, nv) = (2000, 1000);
        let mut sum = 0.0;
        for i in 0..nu {
            let u = TAU * (i as f64 + 0.5) / nu as f64;
            for j in 0..nv {
                let v = PI * (j as f64 + 0.5) / nv as f64;
                let (su, cu) = u.sin_cos();
                let (sv, cv) = v.sin_cos();
                let n = [b * c * sv * sv * cu, a * c * sv * sv * su, a * b * sv * cv];
                sum += dot(n, n).sqrt();
            }
        }
        sum * (TAU / nu as f64) * (PI / nv as f64)
    }

    /// Pappus: the tube's perimeter (an ellipse, by the midpoint rule) times the length of the
    /// circle its centroid runs round.
    fn elliptical_torus_area(radius: f64, a: f64, b: f64) -> f64 {
        let n = 100_000;
        let perimeter: f64 = (0..n)
            .map(|i| {
                let t = TAU * (i as f64 + 0.5) / n as f64;
                (a * a * t.sin().powi(2) + b * b * t.cos().powi(2)).sqrt()
            })
            .sum::<f64>()
            * TAU
            / n as f64;
        // The outline is symmetric about the tube's centre, so its centroid runs round at `radius`.
        perimeter * TAU * radius
    }

    /// Area of the surface swept by r(s), s = z / h from 0 to 1, round the Z axis.
    fn surface_of_revolution(r: impl Fn(f64) -> f64, h: f64) -> f64 {
        let n = 200_000;
        let ds = 1.0 / n as f64;
        (0..n)
            .map(|i| {
                let (s0, s1) = (i as f64 * ds, (i + 1) as f64 * ds);
                let (r0, r1) = (r(s0), r(s1));
                PI * (r0 + r1) * ((r1 - r0).powi(2) + (h * ds).powi(2)).sqrt()
            })
            .sum()
    }
}
