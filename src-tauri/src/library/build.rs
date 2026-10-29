//! The ways the library builds closed, outward-facing meshes without self-intersections, shared by
//! every category:
//! - `Builder`: points and triangles, ring stacks (solids of revolution, sweeps, threads), fans and
//!   strips, and `finish`, which turns the result outwards and stands it centred on the Z axis on
//!   z = 0.
//! - `extrude`: 2D polygons with holes (earcut) between two heights.
//! - `heightfield`: a slab between two height functions over a grid.
//! - `terrace`: a relief whose heights are bands of a sampled value (QR codes, pictures, base
//!   plates), from the bands' outlines on a grid of triangles, with vertical walls.
//! - `field`: the surface where a function changes sign, on the grid Generate shell samples
//!   (gyroid, fractal, Klein bottle).
//! - `voxels`: the outer faces of a set of cubes (Menger sponge).
//! - `union`: overlapping closed solids joined into one, by Wrap part surface.

use std::f64::consts::TAU;

use crate::mesh::{Mesh, cross, dot};

pub(crate) type P = [f64; 3];
pub(crate) type P2 = [f64; 2];

/// Collects points and triangles, then makes the result face outward and rest on z = 0.
#[derive(Default)]
pub(crate) struct Builder {
    pub(crate) points: Vec<P>,
    pub(crate) triangles: Vec<[u32; 3]>,
}

/// How a stack of rings is closed at one end.
pub(crate) enum Cap {
    /// A flat lid: a fan from the ring's centre.
    Flat,
    /// A fan to a single point (a cone's tip, a sphere's pole).
    Point(P),
}

impl Builder {
    pub(crate) fn point(&mut self, p: P) -> u32 {
        self.points.push(p);
        (self.points.len() - 1) as u32
    }

    pub(crate) fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.triangles.push([a, b, c]);
    }

    pub(crate) fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }

    /// Quads between two loops of equal length, `lo[k], lo[k+1], hi[k+1], hi[k]`; closed loops join
    /// the last point to the first.
    pub(crate) fn strip(&mut self, lo: &[u32], hi: &[u32], closed: bool) {
        let n = lo.len();
        let end = if closed { n } else { n - 1 };
        for k in 0..end {
            let k1 = (k + 1) % n;
            self.quad(lo[k], lo[k1], hi[k1], hi[k]);
        }
    }

    /// The points of a loop added, their ids in order.
    pub(crate) fn ring(&mut self, points: &[P]) -> Vec<u32> {
        points.iter().map(|&p| self.point(p)).collect()
    }

    /// Closed loops of points (each the same length, running anticlockwise seen from the top of
    /// the stack) joined one to the next, and closed at both ends. The winding is consistent; the
    /// final sign is fixed in `finish`.
    pub(crate) fn stack(&mut self, rings: &[Vec<P>], bottom: Cap, top: Cap) {
        let ids: Vec<Vec<u32>> = rings.iter().map(|ring| self.ring(ring)).collect();
        for pair in ids.windows(2) {
            self.strip(&pair[0], &pair[1], true);
        }
        self.cap(&ids[0], &rings[0], bottom, false);
        let last = ids.len() - 1;
        self.cap(&ids[last], &rings[last], top, true);
    }

    /// Closes a ring with a fan; `top` turns it to face the other way from a bottom cap.
    pub(crate) fn cap(&mut self, ids: &[u32], ring: &[P], cap: Cap, top: bool) {
        let c = match cap {
            Cap::Flat => self.point(centroid(ring)),
            Cap::Point(p) => self.point(p),
        };
        let n = ids.len();
        for k in 0..n {
            if top {
                self.triangle(c, ids[k], ids[(k + 1) % n]);
            } else {
                self.triangle(c, ids[(k + 1) % n], ids[k]);
            }
        }
    }

    /// A tube of rings round a closed path: ring k joined to ring k + 1, and the last ring to the
    /// first with its points shifted by `shift` (a Möbius band's half turn lands each corner on
    /// another one).
    pub(crate) fn closed_stack(&mut self, rings: &[Vec<P>], shift: usize) {
        let ids: Vec<Vec<u32>> = rings.iter().map(|ring| self.ring(ring)).collect();
        for pair in ids.windows(2) {
            self.strip(&pair[0], &pair[1], true);
        }
        let n = ids[0].len();
        let first: Vec<u32> = (0..n).map(|k| ids[0][(k + shift) % n]).collect();
        self.strip(&ids[ids.len() - 1], &first, true);
    }

    /// Another mesh's triangles added as they are (a separate shell, or one that shares no points).
    pub(crate) fn append(&mut self, mesh: &Mesh) {
        let base = self.points.len() as u32;
        self.points.extend(
            mesh.positions
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| p.map(f64::from)),
        );
        self.triangles.extend(
            mesh.indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| t.map(|i| i + base)),
        );
    }

    /// The signed volume × 6 of everything added.
    fn signed6(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                dot(
                    self.points[t[0] as usize],
                    cross(self.points[t[1] as usize], self.points[t[2] as usize]),
                )
            })
            .sum()
    }

    /// Turned inside out if the winding came out facing in.
    fn outward(&mut self) {
        if self.signed6() < 0.0 {
            for t in &mut self.triangles {
                t.swap(1, 2);
            }
        }
    }

    /// The mesh, turned inside out if its winding came out facing in, centred on the Z axis and
    /// lifted so its lowest point is at z = 0.
    pub(crate) fn finish(mut self) -> Result<Mesh, String> {
        self.outward();
        let (lo, hi) = extent(&self.points);
        let shift = [-(lo[0] + hi[0]) / 2.0, -(lo[1] + hi[1]) / 2.0, -lo[2]];
        self.into_mesh(shift)
    }

    /// The mesh where it was built (world space), turned outwards.
    pub(crate) fn finish_in_place(mut self) -> Result<Mesh, String> {
        self.outward();
        self.into_mesh([0.0; 3])
    }

    /// The mesh as built, unturned and unmoved.
    pub(crate) fn raw(self) -> Result<Mesh, String> {
        self.into_mesh([0.0; 3])
    }

    fn into_mesh(self, shift: P) -> Result<Mesh, String> {
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
            .map_err(|reason| format!("The part could not be built: {reason}."))?;
        Ok(mesh)
    }
}

/// The lowest and highest corner of the points' box.
pub(crate) fn extent(points: &[P]) -> (P, P) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

pub(crate) fn centroid(ring: &[P]) -> P {
    let mut c = [0.0; 3];
    for p in ring {
        for (a, v) in c.iter_mut().zip(p) {
            *a += v / ring.len() as f64;
        }
    }
    c
}

/// `n` points anticlockwise round the unit circle, the first at `start` radians.
pub(crate) fn circle(n: usize, start: f64) -> impl Iterator<Item = (f64, f64)> {
    (0..n).map(move |k| {
        let a = start + TAU * k as f64 / n as f64;
        (a.cos(), a.sin())
    })
}

/// A circle of radius `r` at height `z`, `n` points anticlockwise from +X.
pub(crate) fn ring_at(r: f64, z: f64, n: usize) -> Vec<P> {
    circle(n, 0.0).map(|(c, s)| [r * c, r * s, z]).collect()
}

/// A solid of revolution round Z: `profile` gives (radius, z) from the bottom centre's side round
/// to the top; the ends are closed with flat fans (or a point where the radius is 0).
pub(crate) fn revolve(profile: &[(f64, f64)], segments: usize) -> Result<Mesh, String> {
    let (first, last) = (profile[0], profile[profile.len() - 1]);
    let inner: Vec<Vec<P>> = profile
        .iter()
        .filter(|(r, _)| *r > 0.0)
        .map(|&(r, z)| ring_at(r, z, segments))
        .collect();
    let cap = |(r, z): (f64, f64)| {
        if r > 0.0 {
            Cap::Flat
        } else {
            Cap::Point([0.0, 0.0, z])
        }
    };
    let mut b = Builder::default();
    b.stack(&inner, cap(first), cap(last));
    b.finish()
}

// ---------------------------------------------------------------------------------------------
// Polygons

/// A polygon with holes in the XY plane. The outline runs anticlockwise and the holes clockwise
/// (`Polygon::new` makes them so).
#[derive(Clone, Debug)]
pub(crate) struct Polygon {
    pub(crate) outer: Vec<P2>,
    pub(crate) holes: Vec<Vec<P2>>,
}

/// Twice the signed area of a loop (positive anticlockwise).
pub(crate) fn area2(lp: &[P2]) -> f64 {
    let n = lp.len();
    (0..n)
        .map(|k| {
            let (a, b) = (lp[k], lp[(k + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// The loop without points that repeat the one before (or the first), as earcut wants.
fn cleaned(mut lp: Vec<P2>) -> Vec<P2> {
    lp.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-12 && (a[1] - b[1]).abs() < 1e-12);
    while lp.len() > 1 {
        let (f, l) = (lp[0], lp[lp.len() - 1]);
        if (f[0] - l[0]).abs() < 1e-12 && (f[1] - l[1]).abs() < 1e-12 {
            lp.pop();
        } else {
            break;
        }
    }
    lp
}

impl Polygon {
    pub(crate) fn new(outer: Vec<P2>, holes: Vec<Vec<P2>>) -> Polygon {
        let mut outer = cleaned(outer);
        if area2(&outer) < 0.0 {
            outer.reverse();
        }
        let holes = holes
            .into_iter()
            .map(|h| {
                let mut h = cleaned(h);
                if area2(&h) > 0.0 {
                    h.reverse();
                }
                h
            })
            .collect();
        Polygon { outer, holes }
    }

    /// Every loop, outline first.
    pub(crate) fn loops(&self) -> impl Iterator<Item = &Vec<P2>> {
        std::iter::once(&self.outer).chain(&self.holes)
    }

    /// Whether `p` lies inside the outline and outside every hole (even-odd over all loops).
    pub(crate) fn contains(&self, p: P2) -> bool {
        let mut inside = false;
        for lp in self.loops() {
            let n = lp.len();
            for k in 0..n {
                let (a, b) = (lp[k], lp[(k + 1) % n]);
                if (a[1] > p[1]) != (b[1] > p[1])
                    && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
                {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// Triangles over the polygon's points (numbered outline first, then each hole), each
    /// anticlockwise seen from +Z: the constrained Delaunay triangulation of the loops, without
    /// new points, so no triangle is a sliver of zero area. Fails with a sentence when the loops
    /// cross or touch each other.
    pub(crate) fn triangulate(&self) -> Result<Vec<[usize; 3]>, String> {
        use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
        let bad = || "The outline crosses itself, so it cannot be filled.".to_string();
        let mut points = Vec::new();
        let mut edges = Vec::new();
        for lp in self.loops() {
            let start = points.len();
            let n = lp.len();
            for (k, p) in lp.iter().enumerate() {
                points.push(Point2::new(p[0], p[1]));
                edges.push([start + k, start + (k + 1) % n]);
            }
        }
        let count = points.len();
        let mut conflict = false;
        let cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
            ConstrainedDelaunayTriangulation::try_bulk_load_cdt(points, edges, |_| conflict = true)
                .map_err(|_| bad())?;
        if conflict || cdt.num_vertices() != count {
            return Err(bad());
        }
        let mut out = Vec::with_capacity(count);
        for face in cdt.inner_faces() {
            let v = face.vertices().map(|h| h.fix().index());
            let at = |i: usize| {
                let p = cdt
                    .vertex(spade::handles::FixedVertexHandle::from_index(i))
                    .position();
                [p.x, p.y]
            };
            let (a, b, c) = (at(v[0]), at(v[1]), at(v[2]));
            let centre = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            if self.contains(centre) {
                out.push(v);
            }
        }
        // A polygon of n points with h holes has n + 2h − 2 triangles.
        if out.len() != count + 2 * self.holes.len() - 2 {
            return Err(bad());
        }
        Ok(out)
    }
}

/// The polygons as solids between heights `z0 < z1`: the faces top and bottom and a wall round
/// every loop.
pub(crate) fn extrude(
    b: &mut Builder,
    polygons: &[Polygon],
    z0: f64,
    z1: f64,
) -> Result<(), String> {
    for poly in polygons {
        let loops: Vec<&Vec<P2>> = poly.loops().collect();
        let mut low = Vec::new();
        let mut high = Vec::new();
        for lp in &loops {
            let lo: Vec<u32> = lp.iter().map(|p| b.point([p[0], p[1], z0])).collect();
            let hi: Vec<u32> = lp.iter().map(|p| b.point([p[0], p[1], z1])).collect();
            // Outline anticlockwise and holes clockwise: the solid is on the left of every loop.
            b.strip(&lo, &hi, true);
            low.extend(lo);
            high.extend(hi);
        }
        for [a, c, d] in poly.triangulate()? {
            b.triangle(high[a], high[c], high[d]);
            b.triangle(low[a], low[d], low[c]);
        }
    }
    Ok(())
}

/// A circle of radius `r` round `centre` with `n` points, anticlockwise.
pub(crate) fn circle_2d(centre: P2, r: f64, n: usize, start: f64) -> Vec<P2> {
    circle(n, start)
        .map(|(c, s)| [centre[0] + r * c, centre[1] + r * s])
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Height fields

/// A closed slab over the grid `xs` × `ys` (increasing): its top at `top(i, j)` and its bottom at
/// `bottom(i, j)`, which must stay below the top, with walls round the four sides. Each grid cell
/// is split along the diagonal whose ends differ least in height.
pub(crate) fn heightfield(
    xs: &[f64],
    ys: &[f64],
    top: impl Fn(usize, usize) -> f64,
    bottom: impl Fn(usize, usize) -> f64,
) -> Builder {
    let (nx, ny) = (xs.len(), ys.len());
    let mut b = Builder::default();
    let mut layer = |z: &dyn Fn(usize, usize) -> f64| -> Vec<u32> {
        let mut ids = Vec::with_capacity(nx * ny);
        for (j, &y) in ys.iter().enumerate() {
            for (i, &x) in xs.iter().enumerate() {
                ids.push(b.point([x, y, z(i, j)]));
            }
        }
        ids
    };
    let t = layer(&top);
    let d = layer(&bottom);
    let at = |ids: &[u32], i: usize, j: usize| ids[j * nx + i];
    let z = |b: &Builder, id: u32| b.points[id as usize][2];
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            for (ids, up) in [(&t, true), (&d, false)] {
                let (a, bb, c, dd) = (
                    at(ids, i, j),
                    at(ids, i + 1, j),
                    at(ids, i + 1, j + 1),
                    at(ids, i, j + 1),
                );
                // Split along the flatter diagonal.
                let (p, q) = ((z(&b, a) - z(&b, c)).abs(), (z(&b, bb) - z(&b, dd)).abs());
                let tris = if p <= q {
                    [[a, bb, c], [a, c, dd]]
                } else {
                    [[a, bb, dd], [bb, c, dd]]
                };
                for [x, y, w] in tris {
                    if up {
                        b.triangle(x, y, w);
                    } else {
                        b.triangle(x, w, y);
                    }
                }
            }
        }
    }
    // The border, anticlockwise seen from above, and the walls under it.
    let mut border = Vec::new();
    border.extend((0..nx).map(|i| (i, 0)));
    border.extend((1..ny).map(|j| (nx - 1, j)));
    border.extend((0..nx - 1).rev().map(|i| (i, ny - 1)));
    border.extend((1..ny - 1).rev().map(|j| (0, j)));
    let lo: Vec<u32> = border.iter().map(|&(i, j)| at(&d, i, j)).collect();
    let hi: Vec<u32> = border.iter().map(|&(i, j)| at(&t, i, j)).collect();
    b.strip(&lo, &hi, true);
    b
}

/// `n + 1` coordinates from `a` to `b`.
pub(crate) fn steps(a: f64, b: f64, n: usize) -> Vec<f64> {
    (0..=n).map(|i| a + (b - a) * i as f64 / n as f64).collect()
}
